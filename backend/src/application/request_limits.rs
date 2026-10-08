//! Request activation quotas: open slots and rolling successful activations.
//!
//! Canonical rules: INV-08 (only genuinely open demand occupies slots — an
//! open request is `active` or `suspended` with an unexpired current cycle),
//! INV-15 (editing, renewal, removal, or recreation never resets rolling
//! counts — both counters derive from immutable facts and live rows, so no
//! operation here or elsewhere can reset them), AC-06 (three concurrent open
//! requests; completed, cancelled, and expired rows never count), EC-32 (a
//! further publication is refused with an explanation; closing, expiry, or
//! owner removal frees an open slot without refunding rolling activations).
//!
//! Removed rows occupy no slot: removal hides content, so hidden rows are
//! excluded from the open count while their facts and revisions stay intact
//! (history is never erased, hence never refunded). Successful activations
//! are `request.published` facts inside the trailing 24 hours measured as
//! elapsed time — never calendar days, never midnight resets. Renewal facts
//! join this count in their owning card; no renewal kind exists yet.
//!
//! Every read runs in the caller's transaction: the publication writer
//! evaluates [`check_activation_limits`] and mutates inside one
//! `SERIALIZABLE` transaction (DEC-0003), so concurrent activations
//! serialize instead of overpublishing. Storage failures surface as
//! [`AttemptError::Db`] with the original `sqlx::Error` intact, so the
//! serializable runner can retry genuine `40001`/`40P01` conflicts —
//! mapping them to a flat refusal here would turn a retriable conflict into
//! a wrongful quota denial. The account itself is guarded in the same
//! transaction, so a concurrently restricted account cannot slip through on
//! a stale preflight read.

use crate::persistence::transaction::AttemptError;

/// Concurrent open requests per account on the free MVP.
pub const MAX_OPEN_REQUESTS: u32 = 3;

/// Successful activations per rolling 24 hours per account.
pub const MAX_ACTIVATIONS_PER_WINDOW: u32 = 6;

/// Activation window length in hours: elapsed time, not calendar days.
pub const ACTIVATION_WINDOW_HOURS: i64 = 24;

/// Start of the rolling activation window ending at `now`.
#[must_use]
pub fn activation_window_start(
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    now - chrono::Duration::hours(ACTIVATION_WINDOW_HOURS)
}

/// Typed quota failure. Static reasons only; wire mapping belongs to the
/// route-owning card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitError {
    /// Three genuinely open requests already exist.
    OpenSlotsExhausted,
    /// Six successful activations already fall inside the rolling window.
    ActivationQuotaExhausted,
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for LimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenSlotsExhausted => f.write_str("open request slots exhausted"),
            Self::ActivationQuotaExhausted => f.write_str("activation quota exhausted"),
            Self::NotActive => f.write_str("account is not active"),
            Self::StorageFailed => f.write_str("quota lookup failed"),
        }
    }
}

impl std::error::Error for LimitError {}

/// Current open requests for one account: `active` or `suspended` rows with
/// an unexpired current cycle that are not hidden (removed).
///
/// Drafts never match (no cycle row joins); terminal, expired, and hidden
/// rows never occupy slots.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure (retryable only for
/// genuine conflicts) and [`AttemptError::Abort`] wrapping
/// [`LimitError::StorageFailed`] for an impossible negative count.
pub async fn open_request_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u32, AttemptError<LimitError>> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM requests AS request
         JOIN request_cycles AS cycle
           ON cycle.request_id = request.id
          AND cycle.cycle_number = request.current_cycle_number
         WHERE request.author_id = $1
           AND request.state IN ('active', 'suspended')
           AND request.visibility <> 'hidden'
           AND cycle.deadline > $2",
    )
    .bind(author_id)
    .bind(now)
    .fetch_one(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    u32::try_from(count).map_err(|_| AttemptError::Abort(LimitError::StorageFailed))
}

/// Successful activations for one account inside the rolling window ending
/// at `now`: `request.published` facts with an in-window effective time.
/// Removal, closure, and expiry never delete facts, so they never refund.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure (retryable only for
/// genuine conflicts) and [`AttemptError::Abort`] wrapping
/// [`LimitError::StorageFailed`] for an impossible negative count.
pub async fn activation_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u32, AttemptError<LimitError>> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM business_events
         WHERE actor_id = $1
           AND resource_kind = 'request'
           AND kind = 'request.published'
           AND effective_at > $2",
    )
    .bind(author_id)
    .bind(activation_window_start(now))
    .fetch_one(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    u32::try_from(count).map_err(|_| AttemptError::Abort(LimitError::StorageFailed))
}

/// Refuse an activation that would exceed either quota, evaluated live in
/// the caller's transaction. Open slots are checked before the rolling
/// window; restricted accounts are refused without distinguishing states.
///
/// # Errors
///
/// Returns [`AttemptError::Abort`] with [`LimitError::OpenSlotsExhausted`],
/// [`LimitError::ActivationQuotaExhausted`], or [`LimitError::NotActive`] as
/// business refusals (never retried), and [`AttemptError::Db`] on database
/// failure (retried only for genuine `40001`/`40P01` conflicts).
pub async fn check_activation_limits(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), AttemptError<LimitError>> {
    let standing: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(author_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AttemptError::Db)?;
    match standing {
        Some((state, None)) if state == "active" => {}
        _ => return Err(AttemptError::Abort(LimitError::NotActive)),
    }
    if open_request_count(tx, author_id, now).await? >= MAX_OPEN_REQUESTS {
        return Err(AttemptError::Abort(LimitError::OpenSlotsExhausted));
    }
    if activation_count(tx, author_id, now).await? >= MAX_ACTIVATIONS_PER_WINDOW {
        return Err(AttemptError::Abort(LimitError::ActivationQuotaExhausted));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotas_match_the_policy_defaults() {
        assert_eq!(MAX_OPEN_REQUESTS, 3);
        assert_eq!(MAX_ACTIVATIONS_PER_WINDOW, 6);
        assert_eq!(ACTIVATION_WINDOW_HOURS, 24);
    }

    #[test]
    fn window_is_elapsed_time_not_calendar_days() {
        let now =
            chrono::DateTime::from_timestamp(1_791_000_000, 0).expect("fixture instant builds");
        assert_eq!(
            activation_window_start(now).signed_duration_since(now),
            chrono::Duration::hours(-24)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            LimitError::OpenSlotsExhausted,
            LimitError::ActivationQuotaExhausted,
            LimitError::NotActive,
            LimitError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
