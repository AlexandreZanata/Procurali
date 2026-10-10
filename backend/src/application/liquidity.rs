//! Operational liquidity classification: history and live supply separate.
//!
//! Canonical rules: liquidity 20.9 (awaiting/no-offer-gap/low/healthy/
//! high-competition with highest-applicable precedence; 24-hour early
//! target; three live distinct sellers for competition; late first offers
//! never erase the missed target; renewed cycles carry a separate
//! indicator while the original record stands), INV-12 (renewal starts a
//! new cycle without rewriting initial publication), INV-25 (terminal
//! offers take no new contact — liveness comes from current rows only),
//! and INV-42 (duplicates excluded; known false/prohibited offers leave
//! through traceable corrections, never silent deletion).
//!
//! Historical relevance keeps honest later-withdrawn offers as response
//! evidence; current health uses only live rows. False or prohibited
//! offers are excluded by the caller supplying a corrected first-offer
//! instant (traceable corrections land in `metric_corrections`); this
//! module never rewrites history itself.

use serde::Serialize;

/// Early response target in hours after publication or renewal.
pub const EARLY_TARGET_HOURS: i64 = 24;
/// Distinct live sellers signalling high competition.
pub const COMPETITION_SELLERS: i64 = 3;

/// One liquidity state, highest-applicable wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Liquidity {
    /// Younger than 24 hours with no relevant offer yet.
    AwaitingSupply,
    /// At least 24 hours old with no relevant offer received.
    NoOfferGap,
    /// First offer arrived late, or no live offer remains.
    Low,
    /// Timely first offer with live supply remaining.
    Healthy,
    /// Healthy with at least three distinct live sellers.
    HighCompetition,
}

/// Typed liquidity failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidityError {
    /// Unknown request or cycle.
    NotFound,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for LiquidityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("liquidity target not found"),
            Self::StorageFailed => f.write_str("liquidity lookup failed"),
        }
    }
}

impl std::error::Error for LiquidityError {}

/// Pure classification from one clock: the publication (or renewal)
/// instant, the observation instant, the first relevant offer instant
/// (honest withdrawals kept, false/prohibited already corrected out),
/// and the current live distinct-seller count.
#[must_use]
pub fn classify(
    published_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    first_offer_at: Option<chrono::DateTime<chrono::Utc>>,
    live_sellers: i64,
) -> Liquidity {
    let target = published_at + chrono::Duration::hours(EARLY_TARGET_HOURS);
    match first_offer_at {
        None => {
            if now >= target {
                Liquidity::NoOfferGap
            } else {
                Liquidity::AwaitingSupply
            }
        }
        Some(first) => {
            let timely = first <= target;
            if live_sellers <= 0 || !timely {
                Liquidity::Low
            } else if live_sellers >= COMPETITION_SELLERS {
                Liquidity::HighCompetition
            } else {
                Liquidity::Healthy
            }
        }
    }
}

/// One liquidity observation: the state plus the facts behind it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiquidityReport {
    /// Observed demand.
    pub request_id: uuid::Uuid,
    /// Observation instant.
    pub observed_at: chrono::DateTime<chrono::Utc>,
    /// Original first-publication instant (renewals never move it).
    pub published_at: chrono::DateTime<chrono::Utc>,
    /// First relevant offer instant, if any received.
    pub first_offer_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Current live distinct sellers.
    pub live_sellers: i64,
    /// Classified state.
    pub state: Liquidity,
    /// Renewed-cycle indicator, when the request has a later cycle.
    pub cycle: Option<CycleLiquidity>,
}

/// Renewed-cycle indicator: the same precedence from the renewal clock,
/// kept separate from the original first-response record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CycleLiquidity {
    /// Renewed cycle number.
    pub cycle_number: i32,
    /// Cycle start instant.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// First relevant offer in this cycle, if any.
    pub first_offer_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Live distinct sellers in this cycle.
    pub live_sellers: i64,
    /// Cycle state.
    pub state: Liquidity,
}

/// First relevant offer instant for one demand: earliest submission that
/// is not an excluded correction (`invalidated`/`suspended` rows stay out;
/// honest withdrawals stay in as response evidence).
pub async fn first_offer_at(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, LiquidityError> {
    sqlx::query_scalar(
        "SELECT min(created_at) FROM offers
         WHERE request_id = $1 AND state NOT IN ('invalidated', 'suspended')",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .map_err(|_| LiquidityError::StorageFailed)
}

/// Current live distinct sellers for one demand: rows buyers can act on
/// now (`sent`/`viewed`/`contacted` and visible).
pub async fn live_seller_count(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Result<i64, LiquidityError> {
    sqlx::query_scalar(
        "SELECT count(DISTINCT seller_id) FROM offers
         WHERE request_id = $1
           AND state IN ('sent', 'viewed', 'contacted')
           AND visibility = 'visible'",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .map_err(|_| LiquidityError::StorageFailed)
}

/// Classify one demand from its original publication clock plus an
/// optional renewed-cycle indicator (latest cycle beyond the first).
pub async fn classify_request(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<LiquidityReport, LiquidityError> {
    let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT original_published_at FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| LiquidityError::StorageFailed)?;
    let published_at = match row {
        Some((Some(published_at),)) => published_at,
        _ => return Err(LiquidityError::NotFound),
    };
    let first = first_offer_at(pool, request_id).await?;
    let live = live_seller_count(pool, request_id).await?;
    let state = classify(published_at, now, first, live);
    let cycle = latest_cycle_liquidity(pool, request_id, now).await?;
    Ok(LiquidityReport {
        request_id,
        observed_at: now,
        published_at,
        first_offer_at: first,
        live_sellers: live,
        state,
        cycle,
    })
}

async fn latest_cycle_liquidity(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<CycleLiquidity>, LiquidityError> {
    let row: Option<(i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT cycle_number, started_at FROM request_cycles
         WHERE request_id = $1 ORDER BY cycle_number DESC LIMIT 1",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| LiquidityError::StorageFailed)?;
    let (cycle_number, started_at) = match row {
        Some(row) => row,
        None => return Ok(None),
    };
    if cycle_number <= 1 {
        return Ok(None);
    }
    let first: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT min(created_at) FROM offers
         WHERE request_id = $1 AND cycle_number = $2
           AND state NOT IN ('invalidated', 'suspended')",
    )
    .bind(request_id)
    .bind(cycle_number)
    .fetch_one(pool)
    .await
    .map_err(|_| LiquidityError::StorageFailed)?;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT seller_id) FROM offers
         WHERE request_id = $1 AND cycle_number = $2
           AND state IN ('sent', 'viewed', 'contacted')
           AND visibility = 'visible'",
    )
    .bind(request_id)
    .bind(cycle_number)
    .fetch_one(pool)
    .await
    .map_err(|_| LiquidityError::StorageFailed)?;
    Ok(Some(CycleLiquidity {
        cycle_number,
        started_at,
        first_offer_at: first,
        live_sellers: live,
        state: classify(started_at, now, first, live),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hour(base: chrono::DateTime<chrono::Utc>, hours: i64) -> chrono::DateTime<chrono::Utc> {
        base + chrono::Duration::hours(hours)
    }

    #[test]
    fn precedence_and_boundaries_hold() {
        let published = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed time parses")
            .with_timezone(&chrono::Utc);
        assert_eq!(
            classify(published, hour(published, 1), None, 0),
            Liquidity::AwaitingSupply
        );
        assert_eq!(
            classify(published, hour(published, 24), None, 0),
            Liquidity::NoOfferGap
        );
        assert_eq!(
            classify(published, hour(published, 1), Some(hour(published, 1)), 1),
            Liquidity::Healthy
        );
        assert_eq!(
            classify(published, hour(published, 2), Some(hour(published, 2)), 3),
            Liquidity::HighCompetition
        );
        assert_eq!(
            classify(published, hour(published, 2), Some(hour(published, 25)), 2),
            Liquidity::Low
        );
        assert_eq!(
            classify(published, hour(published, 2), Some(hour(published, 2)), 0),
            Liquidity::Low
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [LiquidityError::NotFound, LiquidityError::StorageFailed] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
