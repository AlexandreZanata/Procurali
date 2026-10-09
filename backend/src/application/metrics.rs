//! Mature cohort denominators and primary metrics: exact ratios only.
//!
//! Canonical rules: measurement 20.7 (seven-day interaction and 14-day
//! outcome windows counted per request from first publication; renewals
//! never re-enter; not-applicable zero denominators; sample size with
//! city and category; unknown outcomes disclosed, never fabricated),
//! INV-12 with INV-13 (original publication identities stay distinct from
//! renewal cycles and lifecycle outcomes), INV-33 (contacts measure
//! usable connections, never proven conversations or sales),
//! INV-42 (duplicates excluded with traceable corrections — latest
//! outcomes only), and INV-43 with AC-31 and AC-48 (platform-attributed
//! resolutions stay separate from elsewhere and unknown-source ones;
//! unknown outcomes reduce coverage openly).
//!
//! Ratios carry numerator, denominator, and an optional value: empty or
//! immature cohorts report not-applicable, never a false zero.

use serde::Serialize;

use crate::persistence::metrics::{cohort_members, contact_counts, offer_tallies, outcome_rows};

/// One cohort query: the publication window with city/category snapshots.
#[derive(Debug, Clone)]
pub struct CohortScope {
    /// City snapshot filter, if scoping.
    pub city_code: Option<String>,
    /// Category snapshot filter, if scoping.
    pub category_code: Option<String>,
    /// Window start (inclusive, first-publication time).
    pub from: chrono::DateTime<chrono::Utc>,
    /// Window end (inclusive, first-publication time).
    pub to: chrono::DateTime<chrono::Utc>,
}

/// Cohort maturity: outcome windows closed for every member or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Maturity {
    /// Every member is past its 14-day outcome window.
    Mature,
    /// Some member may still resolve: computed, flagged, never zeroed.
    Immature,
}

/// One exact ratio: numerator and denominator with the value present only
/// for non-zero denominators.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ratio {
    /// Counted events.
    pub numerator: i64,
    /// Cohort basis.
    pub denominator: i64,
    /// Quotient, absent for empty cohorts.
    pub value: Option<f64>,
}

/// Cohort measurements: counts with exact ratios and maturity. No sale,
/// success-percentage, or trust vocabulary exists here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CohortMetrics {
    /// Distinct demands first-published in the window.
    pub published: i64,
    /// Whether every member passed its outcome window.
    pub maturity: Maturity,
    /// Unique contact initiations in seven days per published demand.
    pub north_star: Ratio,
    /// Demands with at least one unique contact in seven days.
    pub coverage: Ratio,
    /// Demands with at least one relevant offer in seven days.
    pub offer_coverage: Ratio,
    /// Distinct seller offers in the window per published demand.
    pub offers_per_request: Ratio,
    /// Median distinct sellers per published demand, when published.
    pub offers_per_request_median: Option<f64>,
    /// Relevant offers first viewed in the window per submitted offer.
    pub offer_view_rate: Ratio,
    /// Submitted offers receiving a unique contact per submitted offer.
    pub offer_to_contact: Ratio,
    /// Buyer-reported completed demands by day 14.
    pub completed: i64,
    /// Completed demands linked to a valid historical contact.
    pub completed_platform: i64,
    /// Completed demands from elsewhere or undisclosed sources.
    pub completed_elsewhere: i64,
    /// Completed demands per published demand.
    pub resolution_rate: Ratio,
    /// Platform-attributed completions per published demand.
    pub platform_resolution_rate: Ratio,
    /// Completed demands per demand with a final outcome.
    pub respondent_success: Ratio,
    /// Demands with any outcome answer per published demand.
    pub outcome_response: Ratio,
    /// Published demands with no final outcome by day 14.
    pub unknown_outcomes: i64,
}

/// Typed metric failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricActionError {
    /// Unknown city or category, or an inverted window.
    InvalidField,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for MetricActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid metric field"),
            Self::StorageFailed => f.write_str("metric computation failed"),
        }
    }
}

impl std::error::Error for MetricActionError {}

fn ratio(numerator: i64, denominator: i64) -> Ratio {
    Ratio {
        numerator,
        denominator,
        value: if denominator == 0 {
            None
        } else {
            Some(numerator as f64 / denominator as f64)
        },
    }
}

fn median(mut values: Vec<i64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[middle] as f64)
    } else {
        Some((values[middle - 1] + values[middle]) as f64 / 2.0)
    }
}

fn maturity_of(to: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> Maturity {
    if now >= to + chrono::Duration::days(14) {
        Maturity::Mature
    } else {
        Maturity::Immature
    }
}

/// Compute one cohort: distinct first-published demands in the window with
/// per-request interaction and outcome windows, exact ratios, unknown
/// outcomes disclosed, and maturity flagged.
///
/// # Errors
///
/// Returns [`MetricActionError::InvalidField`] for unknown city/category
/// vocabulary or inverted windows, else [`MetricActionError::StorageFailed`].
/// Reasons are static.
pub async fn compute_cohort(
    pool: &sqlx::PgPool,
    scope: CohortScope,
) -> Result<CohortMetrics, MetricActionError> {
    if scope.from > scope.to {
        return Err(MetricActionError::InvalidField);
    }
    if let Some(ref city) = scope.city_code {
        let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM catalog_cities WHERE code = $1")
            .bind(city)
            .fetch_optional(pool)
            .await
            .map_err(|_| MetricActionError::StorageFailed)?;
        if found.is_none() {
            return Err(MetricActionError::InvalidField);
        }
    }
    if let Some(ref category) = scope.category_code {
        let found: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM catalog_categories WHERE code = $1")
                .bind(category)
                .fetch_optional(pool)
                .await
                .map_err(|_| MetricActionError::StorageFailed)?;
        if found.is_none() {
            return Err(MetricActionError::InvalidField);
        }
    }
    let members = cohort_members(
        pool,
        scope.city_code.as_deref(),
        scope.category_code.as_deref(),
        scope.from,
        scope.to,
    )
    .await
    .map_err(|_| MetricActionError::StorageFailed)?;
    let published = members.len() as i64;
    let ids: Vec<uuid::Uuid> = members.iter().map(|member| member.id).collect();
    let mut contacts_by_request = std::collections::HashMap::new();
    for row in contact_counts(pool, &ids)
        .await
        .map_err(|_| MetricActionError::StorageFailed)?
    {
        contacts_by_request.insert(row.request_id, row.contacts);
    }
    let mut tallies_by_request = std::collections::HashMap::new();
    for row in offer_tallies(pool, &ids)
        .await
        .map_err(|_| MetricActionError::StorageFailed)?
    {
        tallies_by_request.insert(row.request_id, row);
    }
    let mut outcomes_by_request = std::collections::HashMap::new();
    for row in outcome_rows(pool, &ids)
        .await
        .map_err(|_| MetricActionError::StorageFailed)?
    {
        outcomes_by_request.entry(row.request_id).or_insert(row);
    }
    let total_contacts: i64 = members
        .iter()
        .map(|member| contacts_by_request.get(&member.id).copied().unwrap_or(0))
        .sum();
    let covered = members
        .iter()
        .filter(|member| contacts_by_request.get(&member.id).copied().unwrap_or(0) > 0)
        .count() as i64;
    let sellers_per_request: Vec<i64> = members
        .iter()
        .map(|member| {
            tallies_by_request
                .get(&member.id)
                .map(|tallies| tallies.sellers)
                .unwrap_or(0)
        })
        .collect();
    let total_sellers: i64 = sellers_per_request.iter().sum();
    let with_offers = sellers_per_request.iter().filter(|&&n| n > 0).count() as i64;
    let submitted: i64 = members
        .iter()
        .map(|member| {
            tallies_by_request
                .get(&member.id)
                .map(|tallies| tallies.submitted)
                .unwrap_or(0)
        })
        .sum();
    let viewed: i64 = members
        .iter()
        .map(|member| {
            tallies_by_request
                .get(&member.id)
                .map(|tallies| tallies.viewed)
                .unwrap_or(0)
        })
        .sum();
    let contacted_offers: i64 = members
        .iter()
        .map(|member| {
            tallies_by_request
                .get(&member.id)
                .map(|tallies| tallies.contacted)
                .unwrap_or(0)
        })
        .sum();
    let mut completed = 0i64;
    let mut completed_platform = 0i64;
    let mut with_final = 0i64;
    let mut with_answer = 0i64;
    for member in &members {
        let Some(outcome) = outcomes_by_request.get(&member.id) else {
            continue;
        };
        with_answer += 1;
        match outcome.outcome.as_str() {
            "completed" => {
                completed += 1;
                with_final += 1;
                if outcome.source.as_deref() == Some("platform") && outcome.contact_linked {
                    completed_platform += 1;
                }
            }
            "cancelled" => with_final += 1,
            _ => {}
        }
    }
    Ok(CohortMetrics {
        published,
        maturity: maturity_of(scope.to, chrono::Utc::now()),
        north_star: ratio(total_contacts, published),
        coverage: ratio(covered, published),
        offer_coverage: ratio(with_offers, published),
        offers_per_request: ratio(total_sellers, published),
        offers_per_request_median: median(sellers_per_request),
        offer_view_rate: ratio(viewed, submitted),
        offer_to_contact: ratio(contacted_offers, submitted),
        completed,
        completed_platform,
        completed_elsewhere: completed - completed_platform,
        resolution_rate: ratio(completed, published),
        platform_resolution_rate: ratio(completed_platform, published),
        respondent_success: ratio(completed, with_final),
        outcome_response: ratio(with_answer, published),
        unknown_outcomes: published - with_final,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_denominators_stay_not_applicable() {
        for metric in [
            ratio(0, 0),
            ratio(5, 0),
            Ratio {
                numerator: 0,
                denominator: 0,
                value: None,
            },
        ] {
            assert_eq!(metric.value, None);
        }
        assert_eq!(ratio(2, 1).value, Some(2.0));
        assert_eq!(ratio(1, 1).value, Some(1.0));
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![2]), Some(2.0));
        assert_eq!(median(vec![1, 3]), Some(2.0));
    }

    #[test]
    fn maturity_needs_closed_outcome_windows() {
        let end = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed time parses")
            .with_timezone(&chrono::Utc);
        assert_eq!(
            maturity_of(end, end + chrono::Duration::days(14)),
            Maturity::Mature
        );
        assert_eq!(
            maturity_of(
                end,
                end + chrono::Duration::days(14) - chrono::Duration::seconds(1)
            ),
            Maturity::Immature
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            MetricActionError::InvalidField,
            MetricActionError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
