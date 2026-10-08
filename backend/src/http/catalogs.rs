//! Public catalog boundary: cities/regions and categories as reference data.
//!
//! Routes follow the frozen inventory (`contracts/openapi.yaml`):
//! `GET /api/v1/catalogs/cities` serves explicitly enabled cities with their
//! regions, and `GET /api/v1/catalogs/categories` serves the category set.
//! Both are unauthenticated public reference data carrying minimal fields —
//! codes, names, labels, scopes, statuses — never coordinates (no such column
//! exists) and never account data.

use axum::{extract::State, response::IntoResponse, routing::get, Json, Router};
use serde::Serialize;

use super::errors::ApiError;
use crate::persistence::catalogs::{categories, cities, regions_for_city};

/// Shared state for the catalog routes: the pool only.
#[derive(Clone)]
pub struct CatalogsState {
    pool: sqlx::PgPool,
}

impl CatalogsState {
    /// Assemble route state from explicit parts.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

/// Mount the catalog routes under `/api/v1` for tests and, later, the
/// production router merge.
pub fn routes(state: CatalogsState) -> Router {
    Router::new()
        .route("/api/v1/catalogs/cities", get(list_cities))
        .route("/api/v1/catalogs/categories", get(list_categories))
        .with_state(state)
}

/// Public region entry: identity and name only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RegionEntry {
    code: String,
    name: String,
}

/// Public city entry: identity, name, and nested regions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CityEntry {
    code: String,
    name: String,
    regions: Vec<RegionEntry>,
}

/// Public category entry: identity, label, scope, and lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CategoryEntry {
    code: String,
    label: String,
    scope: String,
    status: &'static str,
}

/// Serve explicitly enabled cities with their regions.
async fn list_cities(State(state): State<CatalogsState>) -> Result<impl IntoResponse, ApiError> {
    let mut entries = Vec::new();
    for city in cities(&state.pool, true)
        .await
        .map_err(|_| ApiError::internal())?
    {
        let mut regions = Vec::new();
        for region in regions_for_city(&state.pool, &city.code)
            .await
            .map_err(|_| ApiError::internal())?
        {
            regions.push(RegionEntry {
                code: region.code,
                name: region.name,
            });
        }
        entries.push(CityEntry {
            code: city.code,
            name: city.name,
            regions,
        });
    }
    Ok(Json(serde_json::json!({"cities": entries})))
}

/// Serve the category set with lifecycle statuses.
async fn list_categories(
    State(state): State<CatalogsState>,
) -> Result<impl IntoResponse, ApiError> {
    let mut entries = Vec::new();
    for category in categories(&state.pool)
        .await
        .map_err(|_| ApiError::internal())?
    {
        entries.push(CategoryEntry {
            code: category.code,
            label: category.label,
            scope: category.scope,
            status: category.status.as_str(),
        });
    }
    Ok(Json(serde_json::json!({"categories": entries})))
}
