//! Production composition root: one router over every business boundary.
//!
//! Every HTTP boundary in this crate was written to be mounted on its own so
//! its suite could prove one concern at a time. This module is the single
//! place where those boundaries become one served application:
//!
//! - one router carries the account, session, request, offer, contact,
//!   report, block, notice, profile, moderation, metrics, sharing, public
//!   projection, and catalog boundaries, plus liveness and readiness;
//! - the production request bounds (body size, request timeout) and the
//!   privacy headers apply to all of them, exactly as the per-boundary suites
//!   expect;
//! - the built web bundle is served from the configured directory with its
//!   own explicit caching, so public assets keep their immutable caching while
//!   every private or API response stays `no-store`;
//! - the unknown route keeps the stable `not_found` refusal.
//!
//! The composition adds no business rule and no fallback identity: the
//! boundaries keep deciding ownership, authorization, quotas, and lifecycle,
//! and the database remains the only authority.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{response::IntoResponse, routing::get, Router};

use super::router::{self, MAX_BODY_BYTES, REQUEST_TIMEOUT_SECS};
use super::{
    accounts, assets, auth, blocks, catalogs, contacts, health, metrics, moderation, notices,
    offers, profiles, public_html, public_requests, reports, requests, security_headers, shares,
};
use crate::application::phone_verification::VerificationProvider;

/// Pool-backed readiness for the composed router.
///
/// `health::probe_pool` is written as an extractor handler so its own suite
/// can mount it on a pool state. The composition captures the pool directly
/// and keeps the same response contract: ready, or not ready with the
/// dependency name only.
async fn readiness(pool: sqlx::PgPool) -> axum::response::Response {
    health::probe_pool(axum::extract::State(pool))
        .await
        .into_response()
}

/// Everything the served application needs, assembled explicitly.
///
/// Nothing here is read from the environment by this module: the caller
/// validates the configuration and hands the values over, so the composition
/// stays a pure function of its inputs.
pub struct Application<S> {
    pool: sqlx::PgPool,
    provider: Arc<S>,
    phone_lookup_key: String,
    phone_encryption_key: String,
    public_origin: Option<String>,
    assets_dir: PathBuf,
}

impl<S> Application<S> {
    /// Assemble the application parts.
    #[must_use]
    pub fn new(
        pool: sqlx::PgPool,
        provider: Arc<S>,
        phone_lookup_key: String,
        phone_encryption_key: String,
        public_origin: Option<String>,
        assets_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            provider,
            phone_lookup_key,
            phone_encryption_key,
            public_origin,
            assets_dir,
        }
    }

    /// Build the served router.
    pub fn router(&self) -> Router
    where
        S: VerificationProvider + Send + Sync + 'static,
    {
        let pool = self.pool.clone();
        let provider = Arc::clone(&self.provider);
        let readiness_pool = pool.clone();

        // Private and public API boundaries, health probes, and the
        // unknown-route refusal: one router, one privacy-header layer.
        let api = Router::new()
            .route("/health/live", get(health::liveness))
            .route(
                "/health/ready",
                get(move || readiness(readiness_pool.clone())),
            )
            .merge(accounts::routes(accounts::AccountsState::new(
                pool.clone(),
                Arc::clone(&provider),
                self.phone_lookup_key.clone(),
                self.phone_encryption_key.clone(),
            )))
            .merge(auth::routes(auth::AuthState::new(
                pool.clone(),
                provider,
                self.phone_lookup_key.clone(),
                self.phone_encryption_key.clone(),
                self.public_origin.clone(),
            )))
            .merge(requests::routes(requests::RequestsState::new(pool.clone())))
            .merge(offers::routes(offers::OffersState::new(pool.clone())))
            .merge(contacts::routes(contacts::ContactsState::new(
                pool.clone(),
                self.phone_lookup_key.clone(),
                self.phone_encryption_key.clone(),
            )))
            .merge(blocks::routes(blocks::BlocksState::new(pool.clone())))
            .merge(reports::routes(reports::ReportsState::new(pool.clone())))
            .merge(notices::routes(notices::NoticesState::new(pool.clone())))
            .merge(profiles::routes(profiles::ProfilesState::new(pool.clone())))
            .merge(moderation::routes(moderation::ModerationState::new(
                pool.clone(),
            )))
            .merge(metrics::routes(metrics::MetricsState::new(pool.clone())))
            .merge(shares::routes(shares::SharesState::new(pool.clone())))
            .merge(catalogs::routes(catalogs::CatalogsState::new(pool.clone())))
            .merge(public_requests::routes(
                public_requests::PublicRequestsState::new(pool.clone()),
            ))
            .merge(public_html::routes(public_html::PublicHtmlState::new(
                pool.clone(),
            )))
            .fallback(router::unknown_route)
            .layer(security_headers::SecurityHeadersLayer);

        // Public fingerprinted assets keep their own explicit caching, so they
        // are merged beside the private router instead of inside the
        // `no-store` layer.
        let assets = assets::routes(assets::AssetsState::new(self.assets_dir.clone()));

        router::with_bounds(
            Router::new().merge(api).merge(assets),
            MAX_BODY_BYTES,
            std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )
    }
}
