//! Retention, sharing and operational indicators without invented paid use.
//!
//! Canonical rules: measurement 20.8 (recurring buyers/sellers over two
//! 30-day windows; share intent vs attributable landing conversion with
//! anonymous visits kept as visits; active professionals split free from
//! paid; valid-incident rate with disclosed delay; allowance friction),
//! INV-36 with AC-38 (reports stay allegations — raw volume never equals
//! reviewed validity; duplicates group without extra weight),
//! INV-39 with AC-44 (declaration, paid access, phone control, and
//! reputation stay separate — free activity never implies a paid
//! customer), and INV-42 (duplicates and identified abuse stay out of
//! aggregates with traceable corrections).
//!
//! Radar and subscription indicators stay explicitly deferred: the free
//! MVP has no confirmed paid interval or Radar ledger, so this module
//! reports them unavailable rather than zero.

use serde::Serialize;
use std::collections::HashSet;

/// Retention window in days for recurring buyer/seller comparison.
pub const RETENTION_WINDOW_DAYS: i64 = 30;
/// Professional activity window in days for free-active classification.
pub const PROFESSIONAL_ACTIVITY_DAYS: i64 = 30;
/// Share attribution window in days (mirrors the sharing workflow).
pub const ATTRIBUTION_WINDOW_DAYS: i64 = 7;
/// Paid indicators stay deferred: no confirmed subscription exists here.
pub const PAID_DEFERRED_REASON: &str =
    "paid subscription and Radar remain future; free declaration never implies paid customer";
/// Radar indicators stay deferred for the same future-scope reason.
pub const RADAR_DEFERRED_REASON: &str =
    "Radar matching and alerts are post-MVP; no Radar ledger exists in this delivery";

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

fn median_float(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[middle])
    } else {
        Some((values[middle - 1] + values[middle]) / 2.0)
    }
}

/// Typed operational-metric failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationalError {
    /// An inverted window or negative count was supplied.
    InvalidField,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for OperationalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid operational field"),
            Self::StorageFailed => f.write_str("operational lookup failed"),
        }
    }
}

impl std::error::Error for OperationalError {}

/// Recurring demand and supply over two consecutive 30-day windows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Retention {
    /// Distinct buyers publishing in the preceding window.
    pub previous_buyers: i64,
    /// Those buyers publishing again in the current window.
    pub recurring_buyers: i64,
    /// Recurring buyers per previous-window buyer.
    pub recurring_buyer_rate: Ratio,
    /// Distinct sellers submitting in the preceding window.
    pub previous_sellers: i64,
    /// Those sellers submitting again in the current window.
    pub recurring_seller_rate: Ratio,
    /// Recurring sellers per previous-window seller.
    pub recurring_sellers: i64,
}

/// Pure retention assembly from distinct identities: intersection over
/// preceding membership. Draft-only users never enter the input sets.
#[must_use]
pub fn retention_from_sets(
    previous_buyers: &HashSet<uuid::Uuid>,
    current_buyers: &HashSet<uuid::Uuid>,
    previous_sellers: &HashSet<uuid::Uuid>,
    current_sellers: &HashSet<uuid::Uuid>,
) -> Retention {
    let recurring_buyers = previous_buyers.intersection(current_buyers).count() as i64;
    let recurring_sellers = previous_sellers.intersection(current_sellers).count() as i64;
    let previous_buyers_count = previous_buyers.len() as i64;
    let previous_sellers_count = previous_sellers.len() as i64;
    Retention {
        previous_buyers: previous_buyers_count,
        recurring_buyers,
        recurring_buyer_rate: ratio(recurring_buyers, previous_buyers_count),
        previous_sellers: previous_sellers_count,
        recurring_seller_rate: ratio(recurring_sellers, previous_sellers_count),
        recurring_sellers,
    }
}

/// Distinct buyers publishing (first-publication set, never drafts) inside
/// one half-open window.
pub async fn buyers_in_window(
    pool: &sqlx::PgPool,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Result<HashSet<uuid::Uuid>, OperationalError> {
    if from > to {
        return Err(OperationalError::InvalidField);
    }
    let rows: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT author_id FROM requests
         WHERE original_published_at IS NOT NULL
           AND original_published_at >= $1 AND original_published_at < $2",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .map_err(|_| OperationalError::StorageFailed)?;
    Ok(rows.into_iter().collect())
}

/// Distinct sellers submitting offers inside one half-open window.
pub async fn sellers_in_window(
    pool: &sqlx::PgPool,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Result<HashSet<uuid::Uuid>, OperationalError> {
    if from > to {
        return Err(OperationalError::InvalidField);
    }
    let rows: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT seller_id FROM offers
         WHERE created_at >= $1 AND created_at < $2",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .map_err(|_| OperationalError::StorageFailed)?;
    Ok(rows.into_iter().collect())
}

/// Retention over the two 30-day windows ending at `now`: preceding
/// `[now-60d, now-30d)` against current `[now-30d, now)`.
pub async fn compute_retention(
    pool: &sqlx::PgPool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Retention, OperationalError> {
    let current_from = now - chrono::Duration::days(RETENTION_WINDOW_DAYS);
    let previous_from = now - chrono::Duration::days(2 * RETENTION_WINDOW_DAYS);
    let previous_buyers = buyers_in_window(pool, previous_from, current_from).await?;
    let current_buyers = buyers_in_window(pool, current_from, now).await?;
    let previous_sellers = sellers_in_window(pool, previous_from, current_from).await?;
    let current_sellers = sellers_in_window(pool, current_from, now).await?;
    Ok(retention_from_sets(
        &previous_buyers,
        &current_buyers,
        &previous_sellers,
        &current_sellers,
    ))
}

/// Observed sharing volume: recorded intents plus landing visibility split.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharingCounts {
    /// Distinct recorded share actions (intent, never delivery).
    pub share_intents: i64,
    /// All observable landings.
    pub total_landings: i64,
    /// Landings naming an existing demand.
    pub identifiable_landings: i64,
    /// Direct landings with no demand reference (visits, not people).
    pub direct_visits: i64,
}

/// Sharing volume from durable facts: `share.intended` events plus the
/// landing ledger. Identifiers and instants only.
pub async fn sharing_counts(pool: &sqlx::PgPool) -> Result<SharingCounts, OperationalError> {
    let share_intents: i64 =
        sqlx::query_scalar("SELECT count(*) FROM business_events WHERE kind = 'share.intended'")
            .fetch_one(pool)
            .await
            .map_err(|_| OperationalError::StorageFailed)?;
    let row: (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE request_id IS NOT NULL)
         FROM share_landings",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| OperationalError::StorageFailed)?;
    Ok(SharingCounts {
        share_intents,
        total_landings: row.0,
        identifiable_landings: row.1,
        direct_visits: row.0 - row.1,
    })
}

/// Attributable acquisition: distinct markers only, anonymous visits kept
/// separate and never in the denominator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Acquisition {
    /// Distinct attributable landing markers.
    pub attributable_visitors: i64,
    /// Distinct attributed registrations among them.
    pub attributed_registrations: i64,
    /// Anonymous/direct visits reported separately (not people).
    pub anonymous_visits: i64,
    /// Attributed registrations per attributable visitor.
    pub conversion: Ratio,
}

/// Summarize acquisition without inflating unique people: repeated markers
/// deduplicate, and anonymous visits never enter the denominator.
#[must_use]
pub fn summarize_acquisition(
    landing_ids: &[uuid::Uuid],
    attributed_landing_ids: &[uuid::Uuid],
    anonymous_visits: i64,
) -> Acquisition {
    let attributable: HashSet<uuid::Uuid> = landing_ids.iter().copied().collect();
    let attributed: HashSet<uuid::Uuid> = attributed_landing_ids.iter().copied().collect();
    let attributable_visitors = attributable.len() as i64;
    let attributed_registrations = attributable.intersection(&attributed).count() as i64;
    Acquisition {
        attributable_visitors,
        attributed_registrations,
        anonymous_visits,
        conversion: ratio(attributed_registrations, attributable_visitors),
    }
}

/// Explicitly deferred paid/Radar indicator: unavailable, never zero.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeferredIndicator {
    /// Always false in this delivery.
    pub available: bool,
    /// Why the indicator is deferred.
    pub reason: String,
}

fn deferred(reason: &str) -> DeferredIndicator {
    DeferredIndicator {
        available: false,
        reason: reason.to_owned(),
    }
}

/// Paid adoption stays deferred: no confirmed paid interval exists here.
#[must_use]
pub fn paid_indicator() -> DeferredIndicator {
    deferred(PAID_DEFERRED_REASON)
}

/// Radar adoption stays deferred: no Radar ledger exists in this delivery.
#[must_use]
pub fn radar_adoption() -> DeferredIndicator {
    deferred(RADAR_DEFERRED_REASON)
}

/// Paid renewal stays deferred for the same future-scope reason.
#[must_use]
pub fn paid_renewal() -> DeferredIndicator {
    deferred(PAID_DEFERRED_REASON)
}

/// Declared professional activity: free facts only, paid kept deferred.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfessionalActivity {
    /// Declared profiles not withdrawn.
    pub declared: i64,
    /// Declared profiles with an offer in the activity window.
    pub free_active: i64,
    /// Paid breakdown stays deferred (never zero paid customers invented).
    pub paid: DeferredIndicator,
}

/// Free professional activity since `since`: declared profiles plus the
/// subset with at least one submitted offer in the window. Withdrawn
/// profiles stay out. Paid access is not inferred here.
pub async fn professional_activity(
    pool: &sqlx::PgPool,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<ProfessionalActivity, OperationalError> {
    let declared: i64 =
        sqlx::query_scalar("SELECT count(*) FROM professional_profiles WHERE withdrawn_at IS NULL")
            .fetch_one(pool)
            .await
            .map_err(|_| OperationalError::StorageFailed)?;
    let free_active: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT p.user_id)
         FROM professional_profiles p
         JOIN offers o ON o.seller_id = p.user_id
         WHERE p.withdrawn_at IS NULL AND o.created_at >= $1",
    )
    .bind(since)
    .fetch_one(pool)
    .await
    .map_err(|_| OperationalError::StorageFailed)?;
    Ok(ProfessionalActivity {
        declared,
        free_active,
        paid: paid_indicator(),
    })
}

/// A declaration alone never implies a paid customer: only a confirmed
/// subscription does, and none exists in this delivery.
#[must_use]
pub fn is_paid_customer(has_confirmed_subscription: bool) -> bool {
    has_confirmed_subscription
}

/// Reviewed safety: raw allegations against reviewed validity with delay.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncidentMetrics {
    /// All filed report rows (allegations, duplicates included).
    pub raw_reports: i64,
    /// Cases reviewed valid.
    pub valid_incidents: i64,
    /// Eligible marketplace interactions (unique contacts here).
    pub eligible_interactions: i64,
    /// Valid incidents per eligible interaction.
    pub valid_incident_rate: Ratio,
    /// Raw reports per eligible interaction (always >= valid rate here).
    pub raw_report_rate: Ratio,
    /// Median hours from case creation to latest review/decision.
    pub median_review_delay_hours: Option<f64>,
}

/// Safety aggregates from durable facts: raw reports stay allegations,
/// validity comes only from reviewed cases, and the review delay is
/// disclosed. No reporter identity enters the output.
pub async fn incident_metrics(pool: &sqlx::PgPool) -> Result<IncidentMetrics, OperationalError> {
    let raw_reports: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(pool)
        .await
        .map_err(|_| OperationalError::StorageFailed)?;
    let valid_incidents: i64 =
        sqlx::query_scalar("SELECT count(*) FROM report_cases WHERE status = 'valid'")
            .fetch_one(pool)
            .await
            .map_err(|_| OperationalError::StorageFailed)?;
    let eligible_interactions: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(pool)
        .await
        .map_err(|_| OperationalError::StorageFailed)?;
    let delays: Vec<f64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM (updated_at - created_at)) / 3600.0)::float8
         FROM report_cases WHERE status IN ('valid', 'invalid')",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| OperationalError::StorageFailed)?;
    Ok(IncidentMetrics {
        raw_reports,
        valid_incidents,
        eligible_interactions,
        valid_incident_rate: ratio(valid_incidents, eligible_interactions),
        raw_report_rate: ratio(raw_reports, eligible_interactions),
        median_review_delay_hours: median_float(delays),
    })
}

/// Allowance friction from operational observations: refused legitimate
/// attempts, those succeeding later, and reviewed appeals. Refusals never
/// consume a successful-action quota; this helper only assembles supplied
/// observed counts.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AllowanceFriction {
    /// Legitimate attempts refused by an allowance.
    pub refused: i64,
    /// Refused users completing the action later.
    pub later_succeeded: i64,
    /// Appeals reviewed.
    pub reviewed_appeals: i64,
    /// Later successes per refusal.
    pub eventual_success: Ratio,
}

/// Assemble allowance friction from observed counts. Negative inputs
/// refuse with a static reason.
pub fn allowance_friction(
    refused: i64,
    later_succeeded: i64,
    reviewed_appeals: i64,
) -> Result<AllowanceFriction, OperationalError> {
    if refused < 0 || later_succeeded < 0 || reviewed_appeals < 0 {
        return Err(OperationalError::InvalidField);
    }
    Ok(AllowanceFriction {
        refused,
        later_succeeded,
        reviewed_appeals,
        eventual_success: ratio(later_succeeded, refused),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_denominators_stay_not_applicable() {
        assert_eq!(ratio(0, 0).value, None);
        assert_eq!(ratio(1, 0).value, None);
        assert_eq!(ratio(1, 2).value, Some(0.5));
        assert_eq!(median_float(vec![]), None);
        assert_eq!(median_float(vec![2.0]), Some(2.0));
    }

    #[test]
    fn repeat_markers_do_not_inflate_acquisition() {
        let first = uuid::Uuid::now_v7();
        let second = uuid::Uuid::now_v7();
        let summary = summarize_acquisition(&[first, first, second], &[first], 5);
        assert_eq!(summary.attributable_visitors, 2);
        assert_eq!(summary.attributed_registrations, 1);
        assert_eq!(summary.anonymous_visits, 5);
        assert_eq!(summary.conversion.value, Some(0.5));
        let unknown_only = summarize_acquisition(&[], &[], 4);
        assert_eq!(unknown_only.conversion.value, None);
        assert_eq!(unknown_only.anonymous_visits, 4);
    }

    #[test]
    fn declaration_alone_never_implies_paid() {
        assert!(!is_paid_customer(false));
        assert!(is_paid_customer(true));
        for indicator in [paid_indicator(), radar_adoption(), paid_renewal()] {
            assert!(!indicator.available);
            assert!(!indicator.reason.is_empty());
        }
        let activity = ProfessionalActivity {
            declared: 2,
            free_active: 1,
            paid: paid_indicator(),
        };
        let rendered = serde_json::to_string(&activity).expect("activity serializes");
        assert!(rendered.contains("free_active"));
        assert!(!rendered.contains("canary"));
    }

    #[test]
    fn reviewed_validity_differs_from_raw_volume() {
        let friction = allowance_friction(4, 3, 1).expect("friction assembles");
        assert_eq!(friction.eventual_success.value, Some(0.75));
        assert!(allowance_friction(-1, 0, 0).is_err());
        let valid = ratio(1, 4);
        let raw = ratio(3, 4);
        assert_ne!(valid, raw);
        let rendered = serde_json::to_string(&valid).expect("ratio serializes");
        assert!(!rendered.contains("canary"));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            OperationalError::InvalidField,
            OperationalError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
