//! Staff metrics HTTP boundary: aggregates for granted roles only.
//!
//! Route: `GET /api/v1/metrics/operational?purpose=...` answers 200 with
//! non-sensitive aggregates (incident, sharing, and free-professional
//! counts with exact ratios plus the effective policy version) for live
//! moderator-or-better grants; anonymous callers answer 401 and regular
//! users answer 403. The purpose query is required and audited through
//! the same inspection contract as the sibling staff boundary. Payloads
//! carry counts and ratios only; reporter identities, phones,
//! destinations, tokens, and secrets never serialize here.
//!
//! Authentication reuses current-state sessions with the shared cookie
//! name; staff standing is decided by the application layer against live
//! grants.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::collections::HashMap;

use super::errors::{ApiError, Code};
use crate::application::metric_corrections::effective_policy_version;
use crate::application::operational_metrics::{
    incident_metrics, professional_activity, sharing_counts, IncidentMetrics, ProfessionalActivity,
    SharingCounts, PROFESSIONAL_ACTIVITY_DAYS,
};
use crate::application::sessions::authenticate;
use crate::application::staff_permissions::require_moderator;
use crate::persistence::events::{record as record_event, NewEvent};

/// Shared state for the metrics routes: the pool only.
#[derive(Clone)]
pub struct MetricsState {
    pool: sqlx::PgPool,
}

impl MetricsState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the metrics routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: MetricsState) -> Router {
    Router::new()
        .route("/api/v1/metrics/operational", get(operational))
        .with_state(state)
}

/// Staff aggregate receipt: counts and ratios only.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct OperationalResponse {
    incidents: IncidentMetrics,
    sharing: SharingCounts,
    professionals: ProfessionalActivity,
    policy_version: String,
}

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

async fn authenticated_user(
    state: &MetricsState,
    headers: &HeaderMap,
) -> Result<uuid::Uuid, ApiError> {
    let token = match session_token(headers) {
        Some(token) => token,
        None => return Err(ApiError::new(Code::Unauthenticated, "no usable session")),
    };
    match authenticate(&state.pool, &token).await {
        Ok(Some(account)) => Ok(account.user.id),
        Ok(None) => Err(ApiError::new(Code::Unauthenticated, "no usable session")),
        Err(_) => Err(ApiError::internal()),
    }
}

/// Serve staff aggregates: staff grant plus audited purpose required.
async fn operational(
    State(state): State<MetricsState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let user_id = match authenticated_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    let purpose = match params.get("purpose") {
        Some(purpose) if !purpose.trim().is_empty() && purpose.chars().count() <= 500 => {
            purpose.clone()
        }
        _ => {
            return ApiError::new(Code::InvalidField, "operational purpose is required")
                .into_response();
        }
    };
    match require_moderator(&state.pool, user_id).await {
        Ok(_) => {}
        Err(error) => {
            return match error {
                crate::application::staff_permissions::StaffError::NotActive => {
                    ApiError::new(Code::Unauthenticated, "no usable session").into_response()
                }
                crate::application::staff_permissions::StaffError::NotPermitted => {
                    ApiError::new(Code::ForbiddenRole, "staff power required").into_response()
                }
                _ => ApiError::internal().into_response(),
            };
        }
    }
    let policy_version = match effective_policy_version(&state.pool, chrono::Utc::now()).await {
        Ok(version) => version,
        Err(_) => return ApiError::internal().into_response(),
    };
    // Audited purpose: one access fact per call carrying the stated
    // purpose under the effective policy version. No domain row moves.
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return ApiError::internal().into_response(),
    };
    if record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(user_id),
            resource_kind: "policy",
            resource_id: uuid::Uuid::now_v7(),
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "metric.accessed",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({"purpose": purpose, "policy_version": policy_version}),
        },
    )
    .await
    .is_err()
    {
        tx.rollback().await.ok();
        return ApiError::internal().into_response();
    }
    if tx.commit().await.is_err() {
        return ApiError::internal().into_response();
    }
    let incidents = match incident_metrics(&state.pool).await {
        Ok(incidents) => incidents,
        Err(_) => return ApiError::internal().into_response(),
    };
    let sharing = match sharing_counts(&state.pool).await {
        Ok(sharing) => sharing,
        Err(_) => return ApiError::internal().into_response(),
    };
    let since = chrono::Utc::now() - chrono::Duration::days(PROFESSIONAL_ACTIVITY_DAYS);
    let professionals = match professional_activity(&state.pool, since).await {
        Ok(professionals) => professionals,
        Err(_) => return ApiError::internal().into_response(),
    };
    (
        StatusCode::OK,
        Json(OperationalResponse {
            incidents,
            sharing,
            professionals,
            policy_version,
        }),
    )
        .into_response()
}
