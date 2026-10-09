//! Moderation HTTP boundary: staff queues and decisions, reporter-free.
//!
//! Routes: `GET /api/v1/moderation/cases` pages the unresolved queue in
//! severity-then-age order (200 with bounded summaries);
//! `POST /api/v1/moderation/cases/{id}/review` opens review with triage
//! (200); `POST /api/v1/moderation/cases/{id}/decision` records a
//! valid/invalid/duplicate disposition (200). Bodies are closed
//! (`deny_unknown_fields`): severity, disposition, reason, evidence,
//! policy version, and purpose exist — reporter claims, snapshots,
//! assessments of other cases, and every other key do not, and are refused
//! as unknown. Queue rows and receipts carry counts, severities, reasons,
//! and business-context snapshots only; reporter identities, free-text
//! detail, phones, and secrets never serialize here.
//!
//! Authentication reuses current-state sessions (stale sessions answer 401)
//! with the same-host origin contract as the sibling boundaries (403
//! `forbidden_origin`). Staff standing itself is decided by the
//! application layer against live grants. The cookie name is shared from
//! the session boundary; the small parsing/origin helpers repeat here
//! because cross-file sharing belongs to the production merge that unifies
//! all route modules.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::errors::{ApiError, Code};
use crate::application::moderation_review::{
    decide_case, open_review, queue_cases, CaseSummary, DecideInput, OpenReviewInput, ReviewError,
};
use crate::application::sessions::authenticate;

/// Shared state for the moderation routes: the pool only.
#[derive(Clone)]
pub struct ModerationState {
    pool: sqlx::PgPool,
}

impl ModerationState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the moderation routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: ModerationState) -> Router {
    Router::new()
        .route("/api/v1/moderation/cases", get(queue))
        .route("/api/v1/moderation/cases/{id}/review", post(open))
        .route("/api/v1/moderation/cases/{id}/decision", post(decide))
        .with_state(state)
}

/// Queue page receipt: bounded summaries with page position.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct QueueResponse {
    cases: Vec<CaseSummary>,
    total: i64,
    limit: i64,
    offset: i64,
}

/// Review-opening body: triage plus audit references. Nothing else exists.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewBody {
    severity: Option<String>,
    policy_version: Option<String>,
    purpose: Option<String>,
}

/// Review-opening receipt: standing after triage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReviewResponse {
    case_id: uuid::Uuid,
    status: String,
    severity: String,
}

/// Decision body: the human decision with its references. Evidence is the
/// only optional key.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionBody {
    disposition: Option<String>,
    reason: Option<String>,
    evidence: Option<String>,
    policy_version: Option<String>,
    purpose: Option<String>,
}

/// Decision receipt: standing plus recorded rationale (staff-scoped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DecisionResponse {
    case_id: uuid::Uuid,
    status: String,
    reason: String,
}

fn action_error(error: ReviewError) -> Response {
    match error {
        ReviewError::InvalidField => {
            ApiError::new(Code::InvalidField, "invalid review field").into_response()
        }
        ReviewError::InvalidState => {
            ApiError::new(Code::ForbiddenState, "case is not in review standing").into_response()
        }
        ReviewError::NotActive => {
            ApiError::new(Code::Unauthenticated, "no usable session").into_response()
        }
        ReviewError::NotPermitted => {
            ApiError::new(Code::ForbiddenRole, "staff power required").into_response()
        }
        ReviewError::NotFound => {
            ApiError::new(Code::NotFound, "review case not found").into_response()
        }
        ReviewError::StorageFailed => ApiError::internal().into_response(),
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
    state: &ModerationState,
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

/// Page the unresolved staff queue in severity-then-age order.
async fn queue(
    State(state): State<ModerationState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    let limit = match params.get("limit") {
        Some(raw) => match raw.parse::<i64>() {
            Ok(limit) => limit,
            Err(_) => {
                return ApiError::new(Code::InvalidField, "queue limit is invalid").into_response();
            }
        },
        None => 20,
    };
    let offset = match params.get("offset") {
        Some(raw) => match raw.parse::<i64>() {
            Ok(offset) => offset,
            Err(_) => {
                return ApiError::new(Code::InvalidField, "queue offset is invalid")
                    .into_response();
            }
        },
        None => 0,
    };
    match queue_cases(&state.pool, user_id, limit, offset).await {
        Ok(page) => (
            StatusCode::OK,
            Json(QueueResponse {
                cases: page.cases,
                total: page.total,
                limit: page.limit,
                offset: page.offset,
            }),
        )
            .into_response(),
        Err(error) => action_error(error),
    }
}

/// Open review on one case with triage severity.
async fn open(
    State(state): State<ModerationState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    body: Bytes,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let input: ReviewBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    let severity = match input.severity {
        Some(severity) => severity,
        None => return missing("severity"),
    };
    let policy_version = match input.policy_version {
        Some(policy_version) => policy_version,
        None => return missing("policy_version"),
    };
    let purpose = match input.purpose {
        Some(purpose) => purpose,
        None => return missing("purpose"),
    };
    match open_review(
        &state.pool,
        user_id,
        id,
        OpenReviewInput {
            severity,
            policy_version,
            purpose,
        },
    )
    .await
    {
        Ok(opened) => (
            StatusCode::OK,
            Json(ReviewResponse {
                case_id: opened.case_id,
                status: opened.status,
                severity: opened.severity,
            }),
        )
            .into_response(),
        Err(error) => action_error(error),
    }
}

/// Record one disposition on a case under review.
async fn decide(
    State(state): State<ModerationState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    body: Bytes,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    if let Some(response) = origin_refused(&headers) {
        return response;
    }
    let input: DecisionBody = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return ApiError::new(
                Code::InvalidField,
                "request body must match the route schema",
            )
            .into_response();
        }
    };
    let disposition = match input.disposition {
        Some(disposition) => disposition,
        None => return missing("disposition"),
    };
    let reason = match input.reason {
        Some(reason) => reason,
        None => return missing("reason"),
    };
    let policy_version = match input.policy_version {
        Some(policy_version) => policy_version,
        None => return missing("policy_version"),
    };
    let purpose = match input.purpose {
        Some(purpose) => purpose,
        None => return missing("purpose"),
    };
    match decide_case(
        &state.pool,
        user_id,
        id,
        DecideInput {
            disposition,
            reason,
            evidence: input.evidence,
            policy_version,
            purpose,
        },
    )
    .await
    {
        Ok(decision) => (
            StatusCode::OK,
            Json(DecisionResponse {
                case_id: decision.case_id,
                status: decision.status,
                reason: decision.reason,
            }),
        )
            .into_response(),
        Err(error) => action_error(error),
    }
}
