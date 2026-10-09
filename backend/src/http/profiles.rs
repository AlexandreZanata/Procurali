//! Professional-profile HTTP boundary: free declaration and withdrawal,
//! plus the modest public reputation projection.
//!
//! Routes: `PUT /api/v1/profiles/professional` stores the owner's declaration
//! (200, frozen inventory); `DELETE /api/v1/profiles/me/professional`
//! withdraws it (204, always idempotent); `GET /api/v1/profiles/{id}/reputation`
//! projects modest public evidence (200 with account age, recent activity,
//! and buyer-reported resolutions when supported — never a verified-sales
//! claim, complaint badge, phone, or reporter identity). No payment input
//! exists anywhere on these routes, and responses carry exactly their
//! factual fields — no badge, verification, staff, subscription, or volume
//! marker.
//!
//! Declaration and withdrawal authenticate with current-state sessions
//! (stale sessions answer 401); the reputation read is fully public by
//! design and takes no session at all.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401);
//! the unsafe methods share the same-host origin contract as the account
//! boundary (403 `forbidden_origin`). The cookie name is shared from the
//! session boundary; the small parsing/origin helpers repeat here because
//! cross-file sharing belongs to the production merge that unifies all three
//! route modules.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::errors::{ApiError, Code};
use crate::application::professional_profile::{
    declare_profile, withdraw_profile, NewProfessional, ProfessionalError,
};
use crate::application::reputation::{project_reputation, ReputationError};
use crate::application::sessions::authenticate;

/// Shared state for the profile routes: the pool only. No provider, keys, or
/// payment configuration exists on this boundary.
#[derive(Clone)]
pub struct ProfilesState {
    pool: sqlx::PgPool,
}

impl ProfilesState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the profile routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: ProfilesState) -> Router {
    Router::new()
        .route("/api/v1/profiles/professional", put(declare))
        .route("/api/v1/profiles/me/professional", delete(withdraw))
        .route("/api/v1/profiles/{id}/reputation", get(reputation))
        .with_state(state)
}

/// Declaration body: exactly the four factual fields. Payment, badge, role,
/// and state fields do not exist here and are refused as unknown.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclareBody {
    business_name: String,
    business_type: String,
    city: String,
    region: String,
}

/// Factual label receipt: exactly the four public fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProfileResponse {
    business_name: String,
    business_type: String,
    city: String,
    region: String,
}

/// Extract the session token from a `Cookie` header value, if present.
fn session_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name.trim() == super::auth::SESSION_COOKIE {
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

/// Same-host origin for this boundary's unsafe methods: `Origin` (else
/// `Referer`) must match the request's own host (config-free subset shared
/// with the account boundary; unification lands with the production merge).
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

async fn authenticated_user(
    state: &ProfilesState,
    headers: &HeaderMap,
) -> Result<uuid::Uuid, ApiError> {
    let token = match session_token(headers) {
        Some(token) => token,
        None => {
            return Err(ApiError::new(Code::Unauthenticated, "no usable session"));
        }
    };
    match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => Ok(account.user.id),
        Ok(None) => Err(ApiError::new(Code::Unauthenticated, "no usable session")),
        Err(_) => Err(ApiError::internal()),
    }
}

fn origin_refused(headers: &HeaderMap) -> Option<Response> {
    if same_host_origin(headers) {
        None
    } else {
        Some(
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "code": "forbidden_origin",
                    "message": "cross-origin request refused",
                })),
            )
                .into_response(),
        )
    }
}

/// Declare or update the owner's professional classification, free of charge.
async fn declare(State(state): State<ProfilesState>, headers: HeaderMap, body: Bytes) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let input: DeclareBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    match declare_profile(
        &state.pool,
        user_id,
        NewProfessional {
            business_name: input.business_name,
            business_type: input.business_type,
            city: input.city,
            region: input.region,
        },
    )
    .await
    {
        Ok(profile) => (
            StatusCode::OK,
            Json(ProfileResponse {
                business_name: profile.business_name,
                business_type: profile.business_type,
                city: profile.city,
                region: profile.region,
            }),
        )
            .into_response(),
        Err(ProfessionalError::InvalidField) => {
            ApiError::new(Code::InvalidField, "professional fields are invalid").into_response()
        }
        Err(ProfessionalError::NotActive) => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        Err(ProfessionalError::StorageFailed) => ApiError::internal().into_response(),
    }
}

/// Withdraw the owner's declaration, keeping row and history.
async fn withdraw(State(state): State<ProfilesState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    match withdraw_profile(&state.pool, user_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => ApiError::internal().into_response(),
    }
}

/// Project one account's modest public evidence: 200 with labeled facts,
/// or 404 for missing accounts. Fully public by design — no session is
/// read, since every field here is public evidence by construction.
async fn reputation(State(state): State<ProfilesState>, Path(id): Path<uuid::Uuid>) -> Response {
    match project_reputation(&state.pool, id).await {
        Ok(projection) => (StatusCode::OK, Json(projection)).into_response(),
        Err(ReputationError::NotFound) => {
            ApiError::new(Code::NotFound, "account not found").into_response()
        }
        Err(ReputationError::StorageFailed) => ApiError::internal().into_response(),
    }
}
