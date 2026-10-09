//! Share-intent boundary: prepared messages without recipients.
//!
//! Routes: `POST /api/v1/public/requests/{id}/share` prepares one share
//! for the given intent channel (201 with the stable link and the safe
//! message). Bodies are closed (`deny_unknown_fields`): the channel
//! exists — recipient lists, group names, delivery claims, and every
//! other key do not, and are refused as unknown. Nothing here collects
//! where the message goes, and nothing claims it arrived.
//!
//! Authentication is deliberately optional, exactly like the public
//! boundaries: usable sessions resolve their viewer for the block check,
//! while missing or stale sessions read as anonymous, never 401. The
//! cookie name is shared from the session boundary; the small parsing
//! helper repeats here because cross-file sharing belongs to the
//! production merge that unifies all route modules.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::errors::{ApiError, Code};
use crate::application::sessions::authenticate;
use crate::application::share_request::{prepare_share, ShareError};

/// Shared state for the share routes: the pool only.
#[derive(Clone)]
pub struct SharesState {
    pool: sqlx::PgPool,
}

impl SharesState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the share routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: SharesState) -> Router {
    Router::new()
        .route("/api/v1/public/requests/{id}/share", post(prepare))
        .with_state(state)
}

/// Share-intent body: the channel only. Recipients do not exist here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShareBody {
    channel: Option<String>,
}

/// Prepared-share receipt: link, message, and channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ShareResponse {
    request_id: uuid::Uuid,
    link: String,
    message: String,
    channel: String,
}

fn action_error(error: ShareError) -> Response {
    match error {
        ShareError::InvalidField => {
            ApiError::new(Code::InvalidField, "invalid share field").into_response()
        }
        ShareError::NotFound => {
            ApiError::new(Code::NotFound, "share target not found").into_response()
        }
        ShareError::StorageFailed => ApiError::internal().into_response(),
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

/// Resolve the optional viewer: usable sessions identify, while missing,
/// stale, or broken sessions read as anonymous — never a refusal here.
async fn optional_viewer(state: &SharesState, headers: &HeaderMap) -> Option<uuid::Uuid> {
    let token = session_token(headers)?;
    match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => Some(account.user.id),
        _ => None,
    }
}

fn missing(field: &'static str) -> Response {
    ApiError::new(Code::MissingField, "required field is missing")
        .with_field(field, Code::MissingField)
        .into_response()
}

/// Prepare one share for the intent channel: 201 with the stable link
/// and the safe message. Same-host origin is not required: safe POST
/// responses carry no credentials and no private data.
async fn prepare(
    State(state): State<SharesState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    body: Bytes,
) -> Response {
    let viewer_id = optional_viewer(&state, &headers).await;
    let input: ShareBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    let channel = match input.channel {
        Some(channel) => channel,
        None => return missing("channel"),
    };
    match prepare_share(&state.pool, viewer_id, id, &channel).await {
        Ok(package) => (
            StatusCode::CREATED,
            Json(ShareResponse {
                request_id: package.request_id,
                link: package.link,
                message: package.message,
                channel: package.channel,
            }),
        )
            .into_response(),
        Err(error) => action_error(error),
    }
}
