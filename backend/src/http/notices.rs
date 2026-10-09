//! Notice HTTP boundary: owner-only lists and acknowledgments.
//!
//! Routes: `GET /api/v1/notices` lists the owner's notices newest-first
//! with the owned total (200); `POST /api/v1/notices/:id/acknowledgment`
//! acknowledges one owned notice idempotently (200). Missing and foreign
//! rows share one 404 with no existence oracle, and anonymous callers
//! share one 401. Bodies project stored templates verbatim — every notice
//! kind is produced template-only upstream, so no phone, address, or
//! secret can appear here by construction.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401)
//! with the same-host origin contract as the sibling boundaries (403
//! `forbidden_origin`) on the unsafe method. The cookie name is shared from
//! the session boundary; the small parsing/origin helpers repeat here
//! because cross-file sharing belongs to the production merge that unifies
//! all route modules.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::collections::HashMap;

use super::errors::{ApiError, Code};
use crate::application::notice_reads::{acknowledge_notice, list_notices, NoticeReadError};
use crate::application::sessions::authenticate;

/// Shared state for the notice routes: the pool only.
#[derive(Clone)]
pub struct NoticesState {
    pool: sqlx::PgPool,
}

impl NoticesState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the notice routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: NoticesState) -> Router {
    Router::new()
        .route("/api/v1/notices", get(list))
        .route("/api/v1/notices/{id}/acknowledgment", post(acknowledge))
        .with_state(state)
}

/// One owner-visible notice: the stored template plus its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct NoticeResponse {
    id: uuid::Uuid,
    kind: String,
    resource_kind: String,
    resource_id: uuid::Uuid,
    event_id: uuid::Uuid,
    body: String,
    acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Bounded owner list with the total owned count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ListResponse {
    notices: Vec<NoticeResponse>,
    total: i64,
}

fn receipt(view: crate::application::notice_reads::NoticeView) -> NoticeResponse {
    NoticeResponse {
        id: view.id,
        kind: view.kind,
        resource_kind: view.resource_kind,
        resource_id: view.resource_id,
        event_id: view.event_id,
        body: view.body,
        acknowledged_at: view.acknowledged_at,
        created_at: view.created_at,
    }
}

fn read_error(error: NoticeReadError) -> Response {
    match error {
        NoticeReadError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        NoticeReadError::NotFound => {
            ApiError::new(Code::NotFound, "notice not found").into_response()
        }
        NoticeReadError::InvalidPage => {
            ApiError::new(Code::InvalidField, "pagination is invalid").into_response()
        }
        NoticeReadError::StorageFailed => ApiError::internal().into_response(),
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
    state: &NoticesState,
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

fn pagination(params: &HashMap<String, String>, key: &str) -> Result<Option<u32>, ApiError> {
    match params.get(key) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u32>()
            .map(Some)
            .map_err(|_| ApiError::new(Code::InvalidField, "pagination is invalid")),
    }
}

/// List the owner's notices newest-first with the owned total.
async fn list(
    State(state): State<NoticesState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    let limit = match pagination(&params, "limit") {
        Ok(limit) => limit,
        Err(error) => return error.into_response(),
    };
    let offset = match pagination(&params, "offset") {
        Ok(offset) => offset,
        Err(error) => return error.into_response(),
    };
    match list_notices(&state.pool, user_id, limit, offset).await {
        Ok((views, total)) => (
            StatusCode::OK,
            Json(ListResponse {
                notices: views.into_iter().map(receipt).collect(),
                total,
            }),
        )
            .into_response(),
        Err(error) => read_error(error),
    }
}

/// Acknowledge one owned notice idempotently.
async fn acknowledge(
    State(state): State<NoticesState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
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
    match acknowledge_notice(&state.pool, user_id, id).await {
        Ok(view) => (StatusCode::OK, Json(receipt(view))).into_response(),
        Err(error) => read_error(error),
    }
}
