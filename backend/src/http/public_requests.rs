//! Public request boundary: lifecycle projection without sessions.
//!
//! Routes: `GET /api/v1/public/requests/{id}` projects one demand for
//! whoever opens the link. Authentication is deliberately optional: a
//! usable session resolves its viewer for the block check, while missing
//! or stale sessions read as anonymous instead of refusing — anonymous
//! browsing cannot enforce a personal block, and both land on the same
//! generic unavailable shape. This is the one boundary where a missing
//! session is not a 401, by canonical design.
//!
//! Responses carry business fields and declared names only: full details
//! with the offer action for eligible demand, standing without
//! interaction for terminal demand, and one indistinguishable 404 for
//! missing, draft, private, hidden, suspended, prohibited, or blocked
//! rows. The cookie name is shared from the session boundary; the small
//! parsing helper repeats here because cross-file sharing belongs to the
//! production merge that unifies all route modules.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::Serialize;

use super::errors::{ApiError, Code};
use crate::application::public_request::{project_request, PublicProjection};
use crate::application::sessions::authenticate;

/// Shared state for the public routes: the pool only.
#[derive(Clone)]
pub struct PublicRequestsState {
    pool: sqlx::PgPool,
}

impl PublicRequestsState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the public routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: PublicRequestsState) -> Router {
    Router::new()
        .route("/api/v1/public/requests/{id}", get(detail))
        .with_state(state)
}

/// Limited terminal receipt: standing with no interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LimitedResponse {
    id: uuid::Uuid,
    status: String,
    availability: String,
    offer_action: bool,
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
async fn optional_viewer(state: &PublicRequestsState, headers: &HeaderMap) -> Option<uuid::Uuid> {
    let token = session_token(headers)?;
    match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => Some(account.user.id),
        _ => None,
    }
}

/// Project one demand for whoever opens the link: 200 with full or
/// limited standing, or one generic 404 for everything unavailable.
async fn detail(
    State(state): State<PublicRequestsState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let viewer_id = optional_viewer(&state, &headers).await;
    match project_request(&state.pool, viewer_id, id).await {
        Ok(PublicProjection::Full(full)) => (StatusCode::OK, axum::Json(full)).into_response(),
        Ok(PublicProjection::Limited(limited)) => (
            StatusCode::OK,
            axum::Json(LimitedResponse {
                id: limited.id,
                status: limited.status,
                availability: limited.availability,
                offer_action: limited.offer_action,
            }),
        )
            .into_response(),
        Ok(PublicProjection::Unavailable) | Err(_) => {
            ApiError::new(Code::NotFound, "request not found").into_response()
        }
    }
}
