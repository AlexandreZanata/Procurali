//! Eligible local discovery: validated filters over indexed reads.
//!
//! Canonical rules: discovery 9.1 (selected city mandatory with explicit
//! filters; inclusive positive bounds with inverted ranges refused;
//! region preference ranks without filtering; original recency with a
//! stable tie-break; no silent national fallback), INV-08 with INV-09
//! and INV-16 (only actionable demand with current standing lists),
//! INV-22 (city scope selected explicitly), INV-35 (pair-blocked authors
//! stay out), and EC-17 with EC-35 (localities and category codes resolve
//! through stable identities — unknown codes refuse instead of falling
//! back).
//!
//! Unknown filter vocabulary refuses as invalid: an unscoped, mistyped,
//! or inverted filter never degrades into a nationwide or unfiltered
//! listing. Empty results are an honest empty page, not an error.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::persistence::discovery::{
    discover, DiscoveredRequest, DISCOVERY_LIMIT_DEFAULT, DISCOVERY_LIMIT_MAX,
};

/// Category conditions known to discovery filtering.
pub const DISCOVERY_CONDITIONS: [&str; 3] = ["new", "used", "either"];

/// One discovery query as supplied: mandatory city plus explicit filters.
#[derive(Debug, Clone)]
pub struct DiscoveryFilters {
    /// Selected city scope (required — never inferred, never national).
    pub city_code: Option<String>,
    /// Category code filter, if narrowing.
    pub category_code: Option<String>,
    /// Region code filter within the city, if narrowing.
    pub region_code: Option<String>,
    /// Region preference for ranking without filtering, if any.
    pub preferred_region: Option<String>,
    /// Inclusive minimum budget in cents, if bounding.
    pub min_budget_cents: Option<i64>,
    /// Inclusive maximum budget in cents, if bounding.
    pub max_budget_cents: Option<i64>,
    /// Condition filter, if narrowing.
    pub condition: Option<String>,
    /// Page size (defaulted and bounded).
    pub limit: Option<i64>,
    /// Keyset position from a previous page, if continuing.
    pub cursor: Option<String>,
}

/// One bounded discovery page: items in stated order with the position
/// for the next page, if any. Pages carry no totals by design.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryPage {
    /// Listed demands in rank order.
    pub items: Vec<DiscoveredRequest>,
    /// Keyset position for the next page, if rows remain.
    pub next_cursor: Option<String>,
}

/// Typed discovery failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryActionError {
    /// Missing/unknown city, region, category, or condition, inverted or
    /// non-positive bounds, bad paging or cursor.
    InvalidField,
    /// The viewing account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for DiscoveryActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid discovery field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::StorageFailed => f.write_str("discovery failed"),
        }
    }
}

impl std::error::Error for DiscoveryActionError {}

async fn known_city(pool: &sqlx::PgPool, city_code: &str) -> Result<bool, DiscoveryActionError> {
    let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM catalog_cities WHERE code = $1")
        .bind(city_code)
        .fetch_optional(pool)
        .await
        .map_err(|_| DiscoveryActionError::StorageFailed)?;
    Ok(found.is_some())
}

async fn known_region(
    pool: &sqlx::PgPool,
    city_code: &str,
    region_code: &str,
) -> Result<bool, DiscoveryActionError> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM catalog_regions WHERE city_code = $1 AND code = $2")
            .bind(city_code)
            .bind(region_code)
            .fetch_optional(pool)
            .await
            .map_err(|_| DiscoveryActionError::StorageFailed)?;
    Ok(found.is_some())
}

async fn known_category(
    pool: &sqlx::PgPool,
    category_code: &str,
) -> Result<bool, DiscoveryActionError> {
    let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM catalog_categories WHERE code = $1")
        .bind(category_code)
        .fetch_optional(pool)
        .await
        .map_err(|_| DiscoveryActionError::StorageFailed)?;
    Ok(found.is_some())
}

/// List eligible demand for one signed-in viewer under validated filters:
/// the city must resolve, every supplied vocabulary must exist, bounds
/// must be positive with min inside max, and paging must stay bounded.
///
/// # Errors
///
/// Returns [`DiscoveryActionError::InvalidField`] for missing or unknown
/// filter vocabulary, inverted or non-positive bounds, or bad paging and
/// cursors, [`DiscoveryActionError::NotActive`] for restricted viewers,
/// else [`DiscoveryActionError::StorageFailed`]. Reasons are static.
pub async fn discover_demands(
    pool: &sqlx::PgPool,
    viewer_id: uuid::Uuid,
    filters: DiscoveryFilters,
) -> Result<DiscoveryPage, DiscoveryActionError> {
    match check_actor(pool, viewer_id)
        .await
        .map_err(|_| DiscoveryActionError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(DiscoveryActionError::NotActive),
    }
    let city_code = match filters.city_code {
        Some(city_code) if !city_code.trim().is_empty() => city_code,
        _ => return Err(DiscoveryActionError::InvalidField),
    };
    if !known_city(pool, &city_code).await? {
        return Err(DiscoveryActionError::InvalidField);
    }
    if let Some(ref category_code) = filters.category_code {
        if !known_category(pool, category_code).await? {
            return Err(DiscoveryActionError::InvalidField);
        }
    }
    if let Some(ref region_code) = filters.region_code {
        if !known_region(pool, &city_code, region_code).await? {
            return Err(DiscoveryActionError::InvalidField);
        }
    }
    if let Some(ref preferred) = filters.preferred_region {
        if !known_region(pool, &city_code, preferred).await? {
            return Err(DiscoveryActionError::InvalidField);
        }
    }
    if let Some(ref condition) = filters.condition {
        if !DISCOVERY_CONDITIONS.contains(&condition.as_str()) {
            return Err(DiscoveryActionError::InvalidField);
        }
    }
    match (filters.min_budget_cents, filters.max_budget_cents) {
        (Some(min), _) if min < 1 => return Err(DiscoveryActionError::InvalidField),
        (_, Some(max)) if max < 1 => return Err(DiscoveryActionError::InvalidField),
        (Some(min), Some(max)) if min > max => return Err(DiscoveryActionError::InvalidField),
        _ => {}
    }
    let limit = filters.limit.unwrap_or(DISCOVERY_LIMIT_DEFAULT);
    if !(1..=DISCOVERY_LIMIT_MAX).contains(&limit) {
        return Err(DiscoveryActionError::InvalidField);
    }
    let (items, next_cursor) = discover(
        pool,
        crate::persistence::discovery::DiscoveryQuery {
            viewer_id: Some(viewer_id),
            city_code: &city_code,
            category_code: filters.category_code.as_deref(),
            region_code: filters.region_code.as_deref(),
            preferred_region: filters.preferred_region.as_deref(),
            min_budget_cents: filters.min_budget_cents,
            max_budget_cents: filters.max_budget_cents,
            condition: filters.condition.as_deref(),
            limit,
            cursor: filters.cursor.as_deref(),
        },
    )
    .await
    .map_err(|error| match error {
        crate::persistence::discovery::DiscoveryError::InvalidField => {
            DiscoveryActionError::InvalidField
        }
        _ => DiscoveryActionError::StorageFailed,
    })?;
    Ok(DiscoveryPage { items, next_cursor })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_conditions_are_closed() {
        for condition in ["new", "used", "either"] {
            assert!(DISCOVERY_CONDITIONS.contains(&condition));
        }
        assert!(!DISCOVERY_CONDITIONS.contains(&"refurbished"));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            DiscoveryActionError::InvalidField,
            DiscoveryActionError::NotActive,
            DiscoveryActionError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
