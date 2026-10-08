//! Account HTTP boundary: registration, challenges, and confirmations.
//!
//! Routes follow the frozen inventory (`contracts/openapi.yaml`): `POST
//! /api/v1/accounts` creates the pending account (201, no session),
//! `POST /api/v1/accounts/challenges` sends challenges behind one generic 202
//! (no enumeration), and `POST
//! /api/v1/accounts/challenges/confirmations` activates on provider-confirmed
//! codes (200; the caller's own elapsed window reports 410 with code
//! `expired`, wrong or unknown proofs report 422 `verification_failed`).
//! Duplicate numbers are refused with 409 and a static recovery offer that
//! reveals no other owner's history (AC-03). `PATCH /api/v1/accounts/me`
//! lets the session owner update display name and default locality (200;
//! strangers answer 401, phone-shaped text answers 400): the user row only,
//! never resource snapshots (INV-16, AC-41, EC-17). Sessions and cookies
//! arrive with the session lifecycle card; merging these routes into the
//! production router (with its body/timeout bounds) lands with the card that
//! owns the merge.
//!
//! Bodies are strict (`deny_unknown_fields`): protected or unrelated fields
//! (CPF, documents, photos, birth dates, addresses, roles, states) are refused
//! with `invalid_field`, never stripped. Responses never carry phones.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::errors::{ApiError, Code};
use crate::application::phone_verification::VerificationProvider;
use crate::application::register_user::{
    confirm_challenge, register_account, start_challenge, ChallengeDispatch, ConfirmationOutcome,
    RegistrationError, CHALLENGE_WINDOW,
};
use crate::application::sessions::authenticate;
use crate::application::update_profile::{update_profile, ProfileError, ProfileUpdate};
use crate::persistence::users::{NewUser, PhoneKeys};

/// Shared state for the account routes: pool, provider, and phone keys.
///
/// Cloning shares the pool and provider handle; keys are small explicit
/// strings, never configuration values read from the environment here.
pub struct AccountsState<S> {
    pool: sqlx::PgPool,
    provider: Arc<S>,
    lookup_key: String,
    encryption_key: String,
}

impl<S> Clone for AccountsState<S> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            provider: Arc::clone(&self.provider),
            lookup_key: self.lookup_key.clone(),
            encryption_key: self.encryption_key.clone(),
        }
    }
}

impl<S> AccountsState<S> {
    /// Assemble route state from explicit parts. Keys stay server-side.
    #[must_use]
    pub fn new(
        pool: sqlx::PgPool,
        provider: Arc<S>,
        lookup_key: String,
        encryption_key: String,
    ) -> Self {
        Self {
            pool,
            provider,
            lookup_key,
            encryption_key,
        }
    }
}

/// Mount the account routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes<S>(state: AccountsState<S>) -> Router
where
    S: VerificationProvider + Send + Sync + 'static,
{
    Router::new()
        .route("/api/v1/accounts", post(register))
        .route("/api/v1/accounts/challenges", post(request_challenge))
        .route("/api/v1/accounts/challenges/confirmations", post(confirm))
        .route("/api/v1/accounts/me", patch(update_me))
        .with_state(state)
}

/// Minimal registration body: exactly the five accepted fields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterBody {
    display_name: String,
    phone: String,
    city: String,
    region: String,
    policy_version: String,
}

/// Challenge request body: the destination only.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChallengeBody {
    phone: String,
}

/// Confirmation body: the destination plus the submitted code.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmBody {
    phone: String,
    code: String,
}

/// Pending-account receipt: identifier and state only, never the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RegisterResponse {
    account_id: uuid::Uuid,
    account_state: &'static str,
}

/// Generic challenge response: identical for sent and unsent requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ChallengeResponse {
    status: &'static str,
}

/// Activation receipt: state only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ConfirmResponse {
    account_state: &'static str,
}

fn parse_body<T: serde::de::DeserializeOwned>(bytes: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|_| {
        ApiError::new(
            Code::InvalidField,
            "request body must match the route schema",
        )
    })
}

fn registration_error(error: RegistrationError) -> ApiError {
    match error {
        RegistrationError::InvalidInput => {
            ApiError::new(Code::InvalidField, "registration fields are invalid")
        }
        RegistrationError::DuplicatePhone => ApiError::new(
            Code::DuplicateIntent,
            "phone number already registered, recovery is available",
        ),
        RegistrationError::ProviderUnavailable => ApiError::unavailable(),
        RegistrationError::RateLimited => {
            ApiError::new(Code::RateLimited, "challenge rate limited")
        }
        RegistrationError::StorageFailed => ApiError::internal(),
    }
}

/// Register one minimal pending account.
async fn register<S>(
    State(state): State<AccountsState<S>>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError>
where
    S: VerificationProvider + Send + Sync,
{
    let input: RegisterBody = parse_body(&body)?;
    let user = register_account(
        &state.pool,
        NewUser {
            display_name: input.display_name,
            city: input.city,
            region: input.region,
            policy_version: input.policy_version,
            policy_accepted_at: chrono::Utc::now(),
            phone: input.phone,
        },
        PhoneKeys {
            lookup_key: &state.lookup_key,
            encryption_key: &state.encryption_key,
        },
    )
    .await
    .map_err(registration_error)?;
    Ok((
        StatusCode::CREATED,
        Json(RegisterResponse {
            account_id: user.id,
            account_state: "pending",
        }),
    ))
}

/// Request a verification challenge behind one generic response.
async fn request_challenge<S>(
    State(state): State<AccountsState<S>>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError>
where
    S: VerificationProvider + Send + Sync,
{
    let input: ChallengeBody = parse_body(&body)?;
    match start_challenge(
        &state.pool,
        &*state.provider,
        &input.phone,
        PhoneKeys {
            lookup_key: &state.lookup_key,
            encryption_key: &state.encryption_key,
        },
        CHALLENGE_WINDOW,
    )
    .await
    .map_err(registration_error)?
    {
        ChallengeDispatch::Sent | ChallengeDispatch::NotSent => Ok((
            StatusCode::ACCEPTED,
            Json(ChallengeResponse {
                status: "challenge_sent",
            }),
        )),
    }
}

/// Confirm phone control and activate the pending account.
///
/// The caller's own elapsed window reports 410 with code `expired` (frozen
/// route inventory); every other refusal uses the shared [`ApiError`] shape.
async fn confirm<S>(State(state): State<AccountsState<S>>, body: Bytes) -> Response
where
    S: VerificationProvider + Send + Sync,
{
    let input: ConfirmBody = match parse_body(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    match confirm_challenge(
        &state.pool,
        &*state.provider,
        &input.phone,
        &input.code,
        PhoneKeys {
            lookup_key: &state.lookup_key,
            encryption_key: &state.encryption_key,
        },
    )
    .await
    {
        Ok(ConfirmationOutcome::Activated | ConfirmationOutcome::AlreadyActive) => (
            StatusCode::OK,
            Json(ConfirmResponse {
                account_state: "active",
            }),
        )
            .into_response(),
        Ok(ConfirmationOutcome::Failed) => {
            ApiError::new(Code::VerificationFailed, "phone verification failed").into_response()
        }
        Ok(ConfirmationOutcome::Expired) => (
            StatusCode::GONE,
            Json(ApiError::new(Code::Expired, "challenge expired")),
        )
            .into_response(),
        Ok(ConfirmationOutcome::RateLimited) => {
            ApiError::new(Code::RateLimited, "challenge rate limited").into_response()
        }
        Err(error) => registration_error(error).into_response(),
    }
}

/// Partial profile body: display name, city, and region are each optional,
/// at least one is required, and nothing else is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateMeBody {
    display_name: Option<String>,
    city: Option<String>,
    region: Option<String>,
}

/// Updated profile receipt: public fields only, never phone material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct UpdateMeResponse {
    account_id: uuid::Uuid,
    display_name: String,
    city: String,
    region: String,
    account_state: String,
}

/// Extract the session token from a `Cookie` header value, if present.
fn session_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name.trim() == crate::http::auth::SESSION_COOKIE {
            let token = value.trim().to_owned();
            if token.is_empty() {
                None
            } else {
                Some(token)
            }
        } else {
            None
        }
    })
}

/// Same-host origin for this boundary's unsafe method: `Origin` (else
/// `Referer`) must match the request's own host. This is the configuration-free
/// subset of the session boundary's contract (`http::auth` adds the
/// configured public origin); unifying both lands with the production merge.
fn same_host_origin(headers: &HeaderMap) -> bool {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            let referer = headers
                .get(axum::http::header::REFERER)
                .and_then(|value| value.to_str().ok())?;
            let after_scheme = referer.split_once("://")?.1;
            let host = after_scheme.split('/').next()?;
            let scheme = referer.split_once("://")?.0;
            Some(format!("{scheme}://{host}"))
        });
    match (
        origin,
        headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    ) {
        (Some(origin), Some(host)) => {
            origin == format!("http://{host}") || origin == format!("https://{host}")
        }
        _ => false,
    }
}

/// Update the session owner's display name and default locality.
///
/// Authentication runs first (stale sessions answer 401), then the
/// same-host origin check (403 `forbidden_origin`), then the update: only
/// the user row changes plus one appended fact — never resource snapshots,
/// policy history, or phone material.
async fn update_me<S>(
    State(state): State<AccountsState<S>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response
where
    S: VerificationProvider + Send + Sync,
{
    let token = match session_token(&headers) {
        Some(token) => token,
        None => return ApiError::new(Code::Unauthenticated, "no usable session").into_response(),
    };
    let account = match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            return ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        Err(_) => return ApiError::internal().into_response(),
    };
    if !same_host_origin(&headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "code": "forbidden_origin",
                "message": "cross-origin request refused",
            })),
        )
            .into_response();
    }
    let input: UpdateMeBody = match parse_body(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    match update_profile(
        &state.pool,
        account.user.id,
        ProfileUpdate {
            display_name: input.display_name,
            city: input.city,
            region: input.region,
        },
    )
    .await
    {
        Ok(updated) => (
            StatusCode::OK,
            Json(UpdateMeResponse {
                account_id: updated.id,
                display_name: updated.display_name,
                city: updated.city,
                region: updated.region,
                account_state: updated.state,
            }),
        )
            .into_response(),
        Err(ProfileError::InvalidField) => {
            ApiError::new(Code::InvalidField, "profile fields are invalid").into_response()
        }
        Err(ProfileError::NotActive) => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        Err(ProfileError::StorageFailed) => ApiError::internal().into_response(),
    }
}
