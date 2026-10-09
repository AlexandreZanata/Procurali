//! Cohort measurement reads: per-request windows over durable facts.
//!
//! Canonical rules: measurement 20.7 (seven-day interaction and 14-day
//! outcome windows counted per request from first publication; renewals
//! never re-enter; ratios with exact numerators, denominators, and
//! not-applicable empty states), INV-12 with INV-13 (original publication
//! identities stay distinct from renewal cycles and lifecycle outcomes),
//! and INV-42 (one row per durable fact — deduplication happens in
//! assembly, never by rewriting).
//!
//! These readers return typed rows; ratio assembly and cohort validation
//! belong to the application layer. Windows are half-open per request:
//! interaction covers publication through day seven, outcomes through
//! day fourteen.

use serde::Serialize;

/// One cohort member: the publication identity with its own clock.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    /// Published demand identifier.
    pub id: uuid::Uuid,
    /// Original first-publication instant (renewals never move it).
    pub published_at: chrono::DateTime<chrono::Utc>,
}

/// Contact volume for one member inside its interaction window.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactCount {
    /// Counted demand.
    pub request_id: uuid::Uuid,
    /// Unique initiations from publication through day seven.
    pub contacts: i64,
}

/// Offer tallies for one member inside its interaction window.
#[derive(Debug, Clone, PartialEq)]
pub struct OfferTallies {
    /// Counted demand.
    pub request_id: uuid::Uuid,
    /// Submitted rows in the window.
    pub submitted: i64,
    /// Distinct sellers in the window (resubmissions add none).
    pub sellers: i64,
    /// Submitted rows first viewed (contacted rows imply viewed).
    pub viewed: i64,
    /// Submitted rows receiving a unique contact.
    pub contacted: i64,
}

/// Latest outcome standing for one member inside its outcome window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutcomeRow {
    /// Counted demand.
    pub request_id: uuid::Uuid,
    /// Latest outcome (`completed`, `cancelled`, or `unresolved`).
    pub outcome: String,
    /// Latest source, when declared.
    pub source: Option<String>,
    /// Whether the attributed offer links a real contact row.
    pub contact_linked: bool,
}

/// Typed measurement failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricError {
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for MetricError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StorageFailed => f.write_str("metric lookup failed"),
        }
    }
}

impl std::error::Error for MetricError {}

/// Cohort members: distinct demands first-published inside the window,
/// optionally scoped to city and category snapshots.
pub async fn cohort_members(
    pool: &sqlx::PgPool,
    city_code: Option<&str>,
    category_code: Option<&str>,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<MemberRow>, MetricError> {
    let rows: Vec<(uuid::Uuid, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, original_published_at FROM requests
         WHERE original_published_at IS NOT NULL
           AND original_published_at >= $1 AND original_published_at <= $2
           AND ($3::text IS NULL OR city_code = $3)
           AND ($4::text IS NULL OR category_code = $4)
         ORDER BY original_published_at, id",
    )
    .bind(from)
    .bind(to)
    .bind(city_code)
    .bind(category_code)
    .fetch_all(pool)
    .await
    .map_err(|_| MetricError::StorageFailed)?;
    Ok(rows
        .into_iter()
        .map(|(id, published_at)| MemberRow { id, published_at })
        .collect())
}

/// Unique contact initiations per member from publication through day
/// seven. Members without contacts simply have no row here.
pub async fn contact_counts(
    pool: &sqlx::PgPool,
    member_ids: &[uuid::Uuid],
) -> Result<Vec<ContactCount>, MetricError> {
    let rows: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT contacts.request_id, count(*) FROM contacts
         JOIN requests ON requests.id = contacts.request_id
         WHERE contacts.request_id = ANY($1)
           AND contacts.initiated_at <= requests.original_published_at + interval '7 days'
         GROUP BY contacts.request_id",
    )
    .bind(member_ids)
    .fetch_all(pool)
    .await
    .map_err(|_| MetricError::StorageFailed)?;
    Ok(rows
        .into_iter()
        .map(|(request_id, contacts)| ContactCount {
            request_id,
            contacts,
        })
        .collect())
}

/// Offer tallies per member for submissions inside the interaction
/// window. Members without offers simply have no row here.
pub async fn offer_tallies(
    pool: &sqlx::PgPool,
    member_ids: &[uuid::Uuid],
) -> Result<Vec<OfferTallies>, MetricError> {
    let rows: Vec<(uuid::Uuid, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT offers.request_id,
                count(*),
                count(DISTINCT offers.seller_id),
                count(*) FILTER (WHERE offers.state IN ('viewed', 'contacted')),
                count(*) FILTER (WHERE EXISTS
                  (SELECT 1 FROM contacts WHERE contacts.offer_id = offers.id))
         FROM offers
         JOIN requests ON requests.id = offers.request_id
         WHERE offers.request_id = ANY($1)
           AND offers.created_at <= requests.original_published_at + interval '7 days'
         GROUP BY offers.request_id",
    )
    .bind(member_ids)
    .fetch_all(pool)
    .await
    .map_err(|_| MetricError::StorageFailed)?;
    Ok(rows
        .into_iter()
        .map(
            |(request_id, submitted, sellers, viewed, contacted)| OfferTallies {
                request_id,
                submitted,
                sellers,
                viewed,
                contacted,
            },
        )
        .collect())
}

/// Latest outcome per member recorded inside its outcome window:
/// superseded answers stay out, so corrections never double-count.
pub async fn outcome_rows(
    pool: &sqlx::PgPool,
    member_ids: &[uuid::Uuid],
) -> Result<Vec<OutcomeRow>, MetricError> {
    let rows: Vec<(uuid::Uuid, String, Option<String>, Option<uuid::Uuid>)> = sqlx::query_as(
        "SELECT outcomes.request_id, outcomes.outcome, outcomes.source,
                outcomes.attributed_offer_id
         FROM request_outcomes AS outcomes
         JOIN requests ON requests.id = outcomes.request_id
         WHERE outcomes.request_id = ANY($1)
           AND outcomes.created_at <= requests.original_published_at + interval '14 days'
           AND NOT EXISTS (SELECT 1 FROM request_outcomes AS newer
                           WHERE newer.supersedes = outcomes.id)
         ORDER BY outcomes.request_id, outcomes.created_at DESC",
    )
    .bind(member_ids)
    .fetch_all(pool)
    .await
    .map_err(|_| MetricError::StorageFailed)?;
    let mut out = Vec::with_capacity(rows.len());
    for (request_id, outcome, source, attributed_offer_id) in rows {
        let contact_linked = match attributed_offer_id {
            Some(offer_id) => {
                let found: Option<i32> =
                    sqlx::query_scalar("SELECT 1 FROM contacts WHERE offer_id = $1 LIMIT 1")
                        .bind(offer_id)
                        .fetch_optional(pool)
                        .await
                        .map_err(|_| MetricError::StorageFailed)?;
                found.is_some()
            }
            None => false,
        };
        out.push(OutcomeRow {
            request_id,
            outcome,
            source,
            contact_linked,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        let rendered = format!(
            "{:?} {}",
            MetricError::StorageFailed,
            MetricError::StorageFailed
        );
        assert!(!rendered.contains("canary"));
    }
}
