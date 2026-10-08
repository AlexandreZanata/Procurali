//! Session HTTP boundary: login, current session, and logout.
//!
//! Routes: `POST /api/v1/sessions` logs in after phone proof and issues the
//! session cookie; `GET /api/v1/sessions/current` reports the current account;
//! `DELETE /api/v1/sessions/current` revokes it (frozen inventory). Every
//! authenticated route re-reads current account state, so suspension, bans,
//! and deletion take effect immediately (INV-04); callers supply no identity
//! or role — the cookie resolves the account server-side (INV-05).
//!
//! Unsafe methods (POST, DELETE) require a same-origin `Origin` (or `Referer`)
//! matching the configured public origin, else the request's own host. Wrong
//! or missing origins fail with 403 and the stable wire code
//! `forbidden_origin`, recorded here as a pending contract-registry addition
//! (same precedent as the scaffold's `not_found`): the string is chosen now
//! so wire behavior stays stable. Authentication runs before the origin check,
//! so missing sessions answer 401 regardless of origin.
//!
//! Cookies are `HttpOnly`, `SameSite=Lax`, `Path=/`, with `Secure` exactly
//! when the configured origin is `https://` and a `Max-Age` covering the idle
//! window. Responses never carry phones.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::errors::{ApiError, Code};
use crate::application::phone_verification::VerificationProvider;
use crate::application::register_user::RegistrationError;
use crate::application::sessions::{authenticate, login_after_proof, revoke, LoginOutcome};
use crate::persistence::users::PhoneKeys;

/// Session cookie name. Single cookie, fixed name, no prefixes to negotiate.
pub const SESSION_COOKIE: &str = "session";

/// Shared state for the session routes.
pub struct AuthState<S> {
    pool: sqlx::PgPool,
    provider: Arc<S>,
    lookup_key: String,
    encryption_key: String,
    public_origin: Option<String>,
}

impl<S> AuthState<S> {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub fn new(
        pool: sqlx::PgPool,
        provider: Arc<S>,
        lookup_key: String,
        encryption_key: String,
        public_origin: Option<String>,
    ) -> Self {
        Self {
            pool,
            provider,
            lookup_key,
            encryption_key,
            public_origin,
        }
    }
}

impl<S> Clone for AuthState<S> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            provider: Arc::clone(&self.provider),
            lookup_key: self.lookup_key.clone(),
            encryption_key: self.encryption_key.clone(),
            public_origin: self.public_origin.clone(),
        }
    }
}

/// Mount the session routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes<S>(state: AuthState<S>) -> Router
where
    S: VerificationProvider + Send + Sync + 'static,
{
    Router::new()
        .route("/api/v1/sessions", post(login))
        .route("/api/v1/sessions/current", get(current))
        .route("/api/v1/sessions/current", delete(logout))
        .with_state(state)
}

/// Login body: the destination plus the submitted code.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    phone: String,
    code: String,
}

/// Login receipt: state only. The token travels in the cookie, never the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LoginResponse {
    account_state: &'static str,
}

/// Current-account receipt: identifier and state only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CurrentResponse {
    account_id: uuid::Uuid,
    account_state: String,
}

/// Extract the session token from a `Cookie` header value, if present.
fn session_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name.trim() == SESSION_COOKIE {
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

/// Origin of a request from `Origin`, falling back to `Referer`.
fn request_origin(headers: &HeaderMap) -> Option<String> {
    if let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return Some(origin.to_owned());
    }
    let referer = headers
        .get(axum::http::header::REFERER)
        .and_then(|value| value.to_str().ok())?;
    let after_scheme = referer.split_once("://")?.1;
    let host = after_scheme.split('/').next()?;
    let scheme = referer.split_once("://")?.0;
    Some(format!("{scheme}://{host}"))
}

/// Whether `origin` matches the configured public origin, else the request's
/// own host. Missing origins never match.
fn origin_allowed(origin: Option<&str>, headers: &HeaderMap, public_origin: Option<&str>) -> bool {
    let origin = match origin {
        Some(origin) if !origin.is_empty() => origin,
        _ => return false,
    };
    if let Some(expected) = public_origin {
        return origin == expected;
    }
    match headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        Some(host) => origin == format!("http://{host}") || origin == format!("https://{host}"),
        None => false,
    }
}

/// Refuse cross-origin unsafe requests with a stable wire code.
fn forbidden_origin() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "code": "forbidden_origin",
            "message": "cross-origin request refused",
        })),
    )
        .into_response()
}

/// Render one `Set-Cookie` value: `HttpOnly`, `SameSite=Lax`, `Path=/`,
/// `Secure` exactly for `https://` origins, `Max-Age` covering the idle window.
fn set_cookie(token: &str, public_origin: Option<&str>, max_age_secs: u64) -> String {
    let secure = public_origin.is_some_and(|origin| origin.starts_with("https://"));
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{}",
        if secure { "; Secure" } else { "" }
    )
}

/// Clear the session cookie (logout response).
fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Log in after phone proof and issue the session cookie.
async fn login<S>(State(state): State<AuthState<S>>, headers: HeaderMap, body: Bytes) -> Response
where
    S: VerificationProvider + Send + Sync,
{
    if !origin_allowed(
        request_origin(&headers).as_deref(),
        &headers,
        state.public_origin.as_deref(),
    ) {
        return forbidden_origin();
    }
    let input: LoginBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    match login_after_proof(
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
        Ok(LoginOutcome::Authenticated { token, .. }) => {
            let cookie = set_cookie(
                &token,
                state.public_origin.as_deref(),
                crate::application::sessions::SESSION_IDLE_WINDOW.as_secs(),
            );
            (
                StatusCode::OK,
                [(axum::http::header::SET_COOKIE, cookie)],
                Json(LoginResponse {
                    account_state: "active",
                }),
            )
                .into_response()
        }
        Ok(LoginOutcome::Failed) => {
            ApiError::new(Code::VerificationFailed, "phone verification failed").into_response()
        }
        Ok(LoginOutcome::Expired) => (
            StatusCode::GONE,
            Json(ApiError::new(Code::Expired, "challenge expired")),
        )
            .into_response(),
        Ok(LoginOutcome::RateLimited) => {
            ApiError::new(Code::RateLimited, "challenge rate limited").into_response()
        }
        Err(RegistrationError::InvalidInput) => {
            ApiError::new(Code::InvalidField, "login fields are invalid").into_response()
        }
        Err(RegistrationError::ProviderUnavailable) => ApiError::unavailable().into_response(),
        Err(RegistrationError::DuplicatePhone | RegistrationError::StorageFailed) => {
            ApiError::internal().into_response()
        }
        Err(RegistrationError::RateLimited) => {
            ApiError::new(Code::RateLimited, "challenge rate limited").into_response()
        }
    }
}

/// Report the current account behind the session cookie.
async fn current<S>(State(state): State<AuthState<S>>, headers: HeaderMap) -> Response
where
    S: VerificationProvider + Send + Sync,
{
    let token = match session_token(&headers) {
        Some(token) => token,
        None => return ApiError::new(Code::Unauthenticated, "no usable session").into_response(),
    };
    match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => (
            StatusCode::OK,
            Json(CurrentResponse {
                account_id: account.user.id,
                account_state: account.user.state,
            }),
        )
            .into_response(),
        Ok(None) => ApiError::new(Code::Unauthenticated, "no usable session").into_response(),
        Err(_) => ApiError::internal().into_response(),
    }
}

/// Revoke the current session and clear its cookie.
async fn logout<S>(State(state): State<AuthState<S>>, headers: HeaderMap) -> Response
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
    if !origin_allowed(
        request_origin(&headers).as_deref(),
        &headers,
        state.public_origin.as_deref(),
    ) {
        return forbidden_origin();
    }
    match revoke(&state.pool, account.session_id).await {
        Ok(true) => (
            StatusCode::NO_CONTENT,
            [(axum::http::header::SET_COOKIE, clear_cookie())],
        )
            .into_response(),
        Ok(false) | Err(_) => ApiError::internal().into_response(),
    }
}
