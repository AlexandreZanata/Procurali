//! Block HTTP boundary: own restrictions, nothing disclosed.
//!
//! Routes: `POST /api/v1/blocks` records the caller's block of one account
//! (201 created, or 200 replaying the standing row);
//! `DELETE /api/v1/blocks/:id` lifts the caller's block of one account
//! (200, whether or not a row stood). Bodies are closed
//! (`deny_unknown_fields`): the blocked account identifier exists —
//! blocker claims, reasons, states, and every other key do not, and are
//! refused as unknown. Receipts name the blocked account plus what changed
//! only; no reason travels anywhere because none is collected.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401)
//! with the same-host origin contract as the sibling boundaries (403
//! `forbidden_origin`). The cookie name is shared from the session
//! boundary; the small parsing/origin helpers repeat here because
//! cross-file sharing belongs to the production merge that unifies all route
//! modules.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::errors::{ApiError, Code};
use crate::application::block_user::{block_user, unblock_user, BlockActionError};
use crate::application::sessions::authenticate;

/// Shared state for the block routes: the pool only.
#[derive(Clone)]
pub struct BlocksState {
    pool: sqlx::PgPool,
}

impl BlocksState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the block routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: BlocksState) -> Router {
    Router::new()
        .route("/api/v1/blocks", post(block))
        .route("/api/v1/blocks/{id}", delete(unblock))
        .with_state(state)
}

/// Block body: exactly the blocked account. No reason exists to supply.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockBody {
    blocked_user_id: Option<String>,
}

/// Block receipt: who is blocked plus what this call changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BlockResponse {
    blocked_user_id: uuid::Uuid,
    created: bool,
    invalidated_offers: u64,
}

/// Unblock receipt: who is unblocked plus whether a row stood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct UnblockResponse {
    blocked_user_id: uuid::Uuid,
    removed: bool,
}

fn action_error(error: BlockActionError) -> Response {
    match error {
        BlockActionError::SelfBlock => ApiError::new(Code::InvalidField, "cannot block self")
            .with_field("blocked_user_id", Code::InvalidField)
            .into_response(),
        BlockActionError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        BlockActionError::NotFound => {
            ApiError::new(Code::NotFound, "account not found").into_response()
        }
        BlockActionError::StorageFailed => ApiError::internal().into_response(),
    }
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
/// with the sibling boundaries; unification lands with the production
/// merge).
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
    state: &BlocksState,
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

/// Block one account for the authenticated caller: 201 on creation, 200 on
/// repeat; live pair offers invalidate atomically with the row.
async fn block(State(state): State<BlocksState>, headers: HeaderMap, body: Bytes) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let input: BlockBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    let blocked_id = match input.blocked_user_id.as_deref() {
        Some(raw) => match raw.parse::<uuid::Uuid>() {
            Ok(id) => id,
            Err(_) => {
                return ApiError::new(Code::InvalidField, "blocked account is invalid")
                    .with_field("blocked_user_id", Code::InvalidField)
                    .into_response();
            }
        },
        None => {
            return ApiError::new(Code::MissingField, "required field is missing")
                .with_field("blocked_user_id", Code::MissingField)
                .into_response();
        }
    };
    match block_user(&state.pool, user_id, blocked_id).await {
        Ok(outcome) => {
            let body = Json(BlockResponse {
                blocked_user_id: blocked_id,
                created: outcome.created,
                invalidated_offers: outcome.invalidated_offers,
            });
            if outcome.created {
                (StatusCode::CREATED, body).into_response()
            } else {
                (StatusCode::OK, body).into_response()
            }
        }
        Err(error) => action_error(error),
    }
}

/// Lift the caller's block of one account, idempotently.
async fn unblock(
    State(state): State<BlocksState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    match unblock_user(&state.pool, user_id, id).await {
        Ok(outcome) => (
            StatusCode::OK,
            Json(UnblockResponse {
                blocked_user_id: id,
                removed: outcome.removed,
            }),
        )
            .into_response(),
        Err(error) => action_error(error),
    }
}
