//! Liveness and readiness probes.
//!
//! Liveness is process-local and never touches the database. Readiness reports
//! only dependency names (`["database"]`) and a status word: it never renders
//! connection strings, credentials, or any configuration value. Bodies below are
//! exact byte-stable JSON (keys sort alphabetically via `serde_json`).

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Shared dependency state observed by readiness.
/// Holds readiness flags only — never configuration values.
#[derive(Debug, Clone, Default)]
pub struct DependencyStatus {
    database_ready: Arc<AtomicBool>,
}

impl DependencyStatus {
    /// Mark the database reachable with migrations current.
    pub fn set_database_ready(&self, ready: bool) {
        self.database_ready.store(ready, Ordering::SeqCst);
    }

    fn database_ready(&self) -> bool {
        self.database_ready.load(Ordering::SeqCst)
    }
}

/// Liveness: the process is alive. No authentication, no database.
pub async fn liveness() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({"status": "alive", "service": "procurali-backend"})),
    )
}

/// Readiness: every prerequisite usable. Unavailable names only, no values.
pub async fn readiness(
    axum::extract::State(status): axum::extract::State<DependencyStatus>,
) -> impl IntoResponse {
    if status.database_ready() {
        (
            StatusCode::OK,
            Json(json!({"status": "ready", "unavailable": []})),
        )
            .into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(readiness_body())).into_response()
    }
}

fn readiness_body() -> Value {
    json!({"status": "not_ready", "unavailable": ["database"]})
}

/// Live pool-backed readiness: one `SELECT 1` through the pool.
///
/// Success renders the exact ready body; any failure renders the exact
/// not-ready body with the dependency name only — never a connection value.
/// This handler is wired to routes by a later card; tests exercise it through
/// a test-local router against real databases.
pub async fn probe_pool(
    axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
) -> impl IntoResponse {
    if crate::persistence::pool::probe(&pool).await.is_ok() {
        (
            StatusCode::OK,
            Json(json!({"status": "ready", "unavailable": []})),
        )
            .into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(readiness_body())).into_response()
    }
}
