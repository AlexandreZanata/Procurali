//! HTTP routing with explicit request bounds and stable errors.
//!
//! The production stack applies the size bound outside and the timeout mapping
//! inside: a 413 from the size bound never passes through the timeout error
//! mapper, and timeout failures surface as 408. Firing proofs live in
//! `backend/tests/api_scaffold.rs`, which mirrors this exact stack around
//! body-reading and slow handlers (the skeleton has no slow production route;
//! real JSON routes exercise the production stack directly from P04 onward).

use super::health::{liveness, readiness, DependencyStatus};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::time::Duration;
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;

/// Maximum accepted request body: 1 MiB.
pub const MAX_BODY_BYTES: usize = 1_048_576;
/// Maximum time for one request: 30 seconds.
pub const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Production router with production bounds.
pub fn build_router(status: DependencyStatus) -> Router {
    build_router_with_limits(
        status,
        MAX_BODY_BYTES,
        Duration::from_secs(REQUEST_TIMEOUT_SECS),
    )
}

/// Router with explicit bounds. Test seam: caller-chosen values prove the same
/// stack without waiting out production timeouts.
pub fn build_router_with_limits(
    status: DependencyStatus,
    body_limit_bytes: usize,
    timeout: Duration,
) -> Router {
    Router::new()
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .fallback(fallback_unknown_route)
        .layer(
            ServiceBuilder::new()
                .layer(RequestBodyLimitLayer::new(body_limit_bytes))
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |_: tower::BoxError| async { StatusCode::REQUEST_TIMEOUT },
                ))
                .layer(tower::timeout::TimeoutLayer::new(timeout)),
        )
        .with_state(status)
}

/// Stable unknown-route refusal.
///
/// Uses code `not_found`, recorded here as a pending contract-registry addition:
/// the error-code registry (`contracts/domain-vectors.json`) gains this entry
/// when the structured-error card lands; the string is chosen now so wire
/// behavior stays stable from the first scaffold onward.
async fn fallback_unknown_route(request: Request<Body>) -> impl IntoResponse {
    let _ = request;
    (
        StatusCode::NOT_FOUND,
        Json(json!({"code": "not_found", "message": "unknown route"})),
    )
}
