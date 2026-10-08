//! Offer submission quotas: daily allowance and slot permanence.
//!
//! Canonical rules: INV-15 (nothing resets rolling counts — both checks
//! derive from immutable facts and live rows), INV-18 with AC-17 (one slot
//! per seller and request cycle — withdrawal or rejection neither frees
//! another slot nor resets the daily allowance; the slot row stays, so the
//! UNIQUE key keeps refusing), EC-33 (resubmission stays inside the
//! existing slot subject to the same daily allowance — it records a new
//! `offer.submitted` fact with new terms, never a new row).
//!
//! Ten successful submissions or resubmissions per rolling 24 hours,
//! measured as elapsed time. Ordinary term edits consume nothing and refund
//! nothing: they append terms history without recording a fact (a rule for
//! the edit writers, which simply call no counter here). Failed submissions
//! record nothing by construction, so they never consume allowance either.
//! Every read runs in the caller's transaction with the DEC-0003
//! [`AttemptError`] contract, so the submission writer serializes
//! check-then-insert instead of overcounting.

use crate::persistence::transaction::AttemptError;

/// Successful submissions or resubmissions per rolling 24 hours per seller.
pub const MAX_OFFER_SUBMISSIONS_PER_DAY: u32 = 10;

/// Allowance window length in hours: elapsed time, not calendar days.
pub const SUBMISSION_WINDOW_HOURS: i64 = 24;

/// Start of the rolling submission window ending at `now`.
#[must_use]
pub fn submission_window_start(
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    now - chrono::Duration::hours(SUBMISSION_WINDOW_HOURS)
}

/// Typed offer-quota failure. Static reasons only; wire mapping belongs to
/// the route-owning card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferLimitError {
    /// Ten successful submissions already fall inside the rolling window.
    QuotaExhausted,
    /// This seller already holds the slot for this request cycle — held
    /// through withdrawal and rejection alike.
    SlotOccupied,
    /// The seller account is missing, deleted, or not `active`.
    NotActive,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for OfferLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuotaExhausted => f.write_str("offer allowance exhausted"),
            Self::SlotOccupied => f.write_str("seller slot already taken"),
            Self::NotActive => f.write_str("account is not active"),
            Self::StorageFailed => f.write_str("quota lookup failed"),
        }
    }
}

impl std::error::Error for OfferLimitError {}

/// Successful submissions and resubmissions for one seller inside the
/// rolling window ending at `now`: `offer.submitted` facts with an
/// in-window effective time. Every success records exactly one such fact;
/// edits and failures record none, so the count is exact by construction.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure (retryable only for
/// genuine conflicts) and [`AttemptError::Abort`] wrapping
/// [`OfferLimitError::StorageFailed`] for an impossible negative count.
pub async fn submission_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    seller_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u32, AttemptError<OfferLimitError>> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM business_events
         WHERE actor_id = $1
           AND resource_kind = 'offer'
           AND kind = 'offer.submitted'
           AND effective_at > $2",
    )
    .bind(seller_id)
    .bind(submission_window_start(now))
    .fetch_one(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    u32::try_from(count).map_err(|_| AttemptError::Abort(OfferLimitError::StorageFailed))
}

/// True when this seller already holds an offer slot for this request
/// cycle, in any lifecycle state — live, withdrawn, rejected, expired, or
/// otherwise terminal. Slots are never deleted, so `true` is permanent for
/// the cycle.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure only.
pub async fn slot_occupied(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    cycle_number: i32,
    seller_id: uuid::Uuid,
) -> Result<bool, AttemptError<OfferLimitError>> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM offers
         WHERE request_id = $1 AND cycle_number = $2 AND seller_id = $3",
    )
    .bind(request_id)
    .bind(cycle_number)
    .bind(seller_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    Ok(found.is_some())
}

/// Refuse a submission that would exceed the daily allowance or retake a
/// held slot, evaluated live in the caller's transaction. The allowance is
/// checked before the slot; restricted sellers are refused without
/// distinguishing states.
///
/// # Errors
///
/// Returns [`AttemptError::Abort`] with [`OfferLimitError::QuotaExhausted`],
/// [`OfferLimitError::SlotOccupied`], or [`OfferLimitError::NotActive`] as
/// business refusals (never retried), and [`AttemptError::Db`] on database
/// failure (retried only for genuine `40001`/`40P01` conflicts).
pub async fn check_submission_limits(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    cycle_number: i32,
    seller_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), AttemptError<OfferLimitError>> {
    let standing: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(seller_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AttemptError::Db)?;
    match standing {
        Some((state, None)) if state == "active" => {}
        _ => return Err(AttemptError::Abort(OfferLimitError::NotActive)),
    }
    if submission_count(tx, seller_id, now).await? >= MAX_OFFER_SUBMISSIONS_PER_DAY {
        return Err(AttemptError::Abort(OfferLimitError::QuotaExhausted));
    }
    if slot_occupied(tx, request_id, cycle_number, seller_id).await? {
        return Err(AttemptError::Abort(OfferLimitError::SlotOccupied));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotas_match_the_policy_defaults() {
        assert_eq!(MAX_OFFER_SUBMISSIONS_PER_DAY, 10);
        assert_eq!(SUBMISSION_WINDOW_HOURS, 24);
    }

    #[test]
    fn window_is_elapsed_time_not_calendar_days() {
        let now =
            chrono::DateTime::from_timestamp(1_791_000_000, 0).expect("fixture instant builds");
        assert_eq!(
            submission_window_start(now).signed_duration_since(now),
            chrono::Duration::hours(-24)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            OfferLimitError::QuotaExhausted,
            OfferLimitError::SlotOccupied,
            OfferLimitError::NotActive,
            OfferLimitError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
