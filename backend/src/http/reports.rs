//! Report HTTP boundary: relevant allegations in, reporter identity nowhere out.
//!
//! Routes: `POST /api/v1/reports` records the caller's allegation against a
//! relevant target (201 with the standing identifiers and status). Bodies
//! are closed (`deny_unknown_fields`): the target kind/identifier, reason,
//! and bounded detail exist — reporter claims, snapshots, statuses, and
//! every other key do not, and are refused as unknown. Snapshots resolve
//! server-side from the target row, so the caller cannot fabricate context.
//! Receipts name the allegation plus what was recorded only; the reporter
//! identifier and free-text detail never serialize here.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401)
//! with the same-host origin contract as the sibling boundaries (403
//! `forbidden_origin`). The cookie name is shared from the session
//! boundary; the small parsing/origin helpers repeat here because
//! cross-file sharing belongs to the production merge that unifies all route
//! modules. Fabricated targets and real-but-inaccessible private offers share
//! one indistinguishable 404 `not_found` (anti-enumeration); self-reports
//! refuse as 400 `invalid_field` on the target, mirroring self-blocks.

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::errors::{ApiError, Code};
use crate::application::create_report::{submit_report, ReportInput, SubmitError};
use crate::application::sessions::authenticate;

/// Shared state for the report routes: the pool only.
#[derive(Clone)]
pub struct ReportsState {
    pool: sqlx::PgPool,
}

impl ReportsState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the report routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: ReportsState) -> Router {
    Router::new()
        .route("/api/v1/reports", post(create))
        .with_state(state)
}

/// Report body: target plus allegation category. Snapshots and grouping
/// resolve server-side; nothing else is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportBody {
    target_kind: Option<String>,
    target_id: Option<String>,
    reason: Option<String>,
    detail: Option<String>,
}

/// Report receipt: standing identifiers plus allegation category and status.
/// No reporter identifier, detail, phone, or secret material exists here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReportResponse {
    id: uuid::Uuid,
    case_id: uuid::Uuid,
    target_kind: String,
    target_id: uuid::Uuid,
    reason: String,
    status: String,
}

fn action_error(error: SubmitError) -> Response {
    match error {
        SubmitError::InvalidField => {
            ApiError::new(Code::InvalidField, "invalid report field").into_response()
        }
        SubmitError::OwnContent => ApiError::new(Code::InvalidField, "cannot report own content")
            .with_field("target_id", Code::InvalidField)
            .into_response(),
        SubmitError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        SubmitError::NotFound | SubmitError::NotRelevant => {
            ApiError::new(Code::NotFound, "report target not found").into_response()
        }
        SubmitError::StorageFailed => ApiError::internal().into_response(),
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
    state: &ReportsState,
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

fn missing(field: &'static str) -> Response {
    ApiError::new(Code::MissingField, "required field is missing")
        .with_field(field, Code::MissingField)
        .into_response()
}

/// Record one allegation for the authenticated caller: 201 with the
/// standing identifiers. Resolution, snapshots, and grouping stay
/// server-side; assessment, trust, bans, and lifecycles stand untouched.
async fn create(State(state): State<ReportsState>, headers: HeaderMap, body: Bytes) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let input: ReportBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    let target_kind = match input.target_kind {
        Some(kind) => kind,
        None => return missing("target_kind"),
    };
    let target_id = match input.target_id.as_deref() {
        Some(raw) => match raw.parse::<uuid::Uuid>() {
            Ok(id) => id,
            Err(_) => {
                return ApiError::new(Code::InvalidField, "report target is invalid")
                    .with_field("target_id", Code::InvalidField)
                    .into_response();
            }
        },
        None => return missing("target_id"),
    };
    let reason = match input.reason {
        Some(reason) => reason,
        None => return missing("reason"),
    };
    match submit_report(
        &state.pool,
        user_id,
        ReportInput {
            target_kind: target_kind.clone(),
            target_id,
            reason: reason.clone(),
            detail: input.detail.unwrap_or_default(),
        },
    )
    .await
    {
        Ok(stored) => (
            StatusCode::CREATED,
            Json(ReportResponse {
                id: stored.id,
                case_id: stored.case_id,
                target_kind: stored.target_kind,
                target_id: stored.target_id,
                reason: stored.reason,
                status: stored.status,
            }),
        )
            .into_response(),
        Err(error) => action_error(error),
    }
}
