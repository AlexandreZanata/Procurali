//! Effective expiry: deadline eligibility and conditional expiration.
//!
//! Canonical rules: INV-08 (an active request needs a future deadline —
//! evaluated here at the effective time, not from a cached status),
//! INV-09 (completed, cancelled, expired, suspended, removed, or otherwise
//! restricted requests accept no new offers or contact — only a live public
//! cycle qualifies), AC-10 (at or past the deadline every submission and
//! contact is refused and discovery drops the row, even before any expiry
//! job runs), AC-49 with EC-19 (a suspended row whose deadline passes
//! becomes expired while remaining hidden; restoration never reactivates or
//! extends it — this helper moves state only, never visibility or timing),
//! EC-07 (an action counts only when its business effect lands strictly
//! before the exclusive deadline; expiry losers produce no successful
//! event).
//!
//! The deadline is exclusive: eligibility holds exactly while `now` is
//! strictly before it, so a fixture evaluated at the precise deadline is
//! already expired. Suspension never pauses or extends timing — the cycle
//! row is never touched here; only lifecycle state moves, and only toward
//! `expired`. Both operations run in the caller's transaction and surface
//! storage failures as [`AttemptError::Db`], so the DEC-0003 runner retries
//! genuine conflicts instead of misreporting them.

use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::transaction::AttemptError;
use serde_json::json;

/// One eligibility row: lifecycle plus the current deadline, if any.
#[derive(Debug, sqlx::FromRow)]
struct EligibilityRow {
    id: uuid::Uuid,
    state: String,
    visibility: String,
    current_cycle_number: i32,
    current_revision_number: i32,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
}

/// One request currently open for offers and contact, with its bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionableRequest {
    /// Request identifier.
    pub id: uuid::Uuid,
    /// Current cycle number scoping downstream facts.
    pub cycle_number: i32,
    /// Current revision number scoping downstream facts.
    pub revision_number: i32,
    /// Exclusive deadline the action must strictly precede.
    pub deadline: chrono::DateTime<chrono::Utc>,
}

/// Typed eligibility failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestEligibilityError {
    /// No such request exists.
    NotFound,
    /// The row is a draft, terminal, suspended, or hidden row.
    NotActionable,
    /// The exclusive deadline is reached or passed.
    Expired,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for RequestEligibilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("request not found"),
            Self::NotActionable => f.write_str("request is not actionable"),
            Self::Expired => f.write_str("request deadline passed"),
            Self::StorageFailed => f.write_str("eligibility lookup failed"),
        }
    }
}

impl std::error::Error for RequestEligibilityError {}

/// Conditional-expiration outcome for one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryOutcome {
    /// The row moved to `expired` with exactly one new fact.
    Transitioned,
    /// The row was already settled (terminal or expired): nothing written.
    AlreadySettled,
    /// The row has no elapsed deadline (drafts, live cycles): nothing written.
    NotDue,
}

/// Typed expiration failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryError {
    /// No such request exists.
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ExpiryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("request not found"),
            Self::StorageFailed => f.write_str("expiry transition failed"),
        }
    }
}

impl std::error::Error for ExpiryError {}

/// True once the exclusive deadline is reached: eligibility holds exactly
/// while `now` is strictly before `deadline` (EC-07).
#[must_use]
pub fn is_elapsed(
    deadline: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    now >= deadline
}

/// Check one request for new offers and contact at the effective time.
///
/// Only a live public cycle qualifies: drafts, terminal rows, suspended
/// rows, and hidden rows are refused without distinguishing them, and an
/// elapsed deadline refuses even when the recorded lifecycle still reads
/// `active` (AC-10: no job needs to have run).
///
/// # Errors
///
/// Returns [`AttemptError::Abort`] with the business refusal (never
/// retried) and [`AttemptError::Db`] on database failure (retried only for
/// genuine `40001`/`40P01` conflicts).
pub async fn check_request_actionable(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ActionableRequest, AttemptError<RequestEligibilityError>> {
    let row: Option<EligibilityRow> = sqlx::query_as(
        "SELECT request.id, request.state, request.visibility,
                request.current_cycle_number, request.current_revision_number,
                cycle.deadline
         FROM requests AS request
         LEFT JOIN request_cycles AS cycle
           ON cycle.request_id = request.id
          AND cycle.cycle_number = request.current_cycle_number
         WHERE request.id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    let row = row.ok_or(AttemptError::Abort(RequestEligibilityError::NotFound))?;
    // Refusal precedence follows state-transitions: terminal and restricted
    // states outrank the elapsed deadline in what the caller reports.
    if row.state != "active" || row.visibility != "public" {
        return Err(AttemptError::Abort(RequestEligibilityError::NotActionable));
    }
    let deadline = row
        .deadline
        .ok_or(AttemptError::Abort(RequestEligibilityError::NotActionable))?;
    if is_elapsed(deadline, now) {
        return Err(AttemptError::Abort(RequestEligibilityError::Expired));
    }
    Ok(ActionableRequest {
        id: row.id,
        cycle_number: row.current_cycle_number,
        revision_number: row.current_revision_number,
        deadline,
    })
}

/// Expire one elapsed row, recording exactly one `request.expired` fact.
///
/// Active and suspended rows whose current deadline passed move to
/// `expired`; visibility and timing are never touched, so a suspended row
/// stays hidden (AC-49) and restoration can never reactivate it (EC-19).
/// The fact's effective time is the deadline itself — the instant expiry
/// took effect — not the worker's arrival time. Already settled rows and
/// rows with no elapsed deadline report back with zero writes, so repeats
/// and concurrent racers produce exactly one fact: the row lock serializes
/// them and every loser observes the winner's `expired` row.
///
/// # Errors
///
/// Returns [`AttemptError::Abort`] with [`ExpiryError::NotFound`] for a
/// missing row, and [`AttemptError::Db`] on database failure (retried only
/// for genuine `40001`/`40P01` conflicts).
pub async fn expire_if_elapsed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ExpiryOutcome, AttemptError<ExpiryError>> {
    // Lock the request row alone: FOR UPDATE cannot ride an outer join, and
    // the deadline read below observes the same snapshot while holding it.
    let locked: Option<(String, i32)> =
        sqlx::query_as("SELECT state, current_cycle_number FROM requests WHERE id = $1 FOR UPDATE")
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AttemptError::Db)?;
    let (state, cycle_number) = locked.ok_or(AttemptError::Abort(ExpiryError::NotFound))?;
    if state == "completed" || state == "cancelled" || state == "expired" || state == "draft" {
        return Ok(ExpiryOutcome::AlreadySettled);
    }
    let deadline: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT deadline FROM request_cycles WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(request_id)
    .bind(cycle_number)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    let deadline = match deadline {
        Some(deadline) if is_elapsed(deadline, now) => deadline,
        _ => return Ok(ExpiryOutcome::NotDue),
    };
    sqlx::query("UPDATE requests SET state = 'expired', updated_at = now() WHERE id = $1")
        .bind(request_id)
        .execute(&mut **tx)
        .await
        .map_err(AttemptError::Db)?;
    // The validated event writer stays the single insertion path: its error
    // mapping is safe here because the row lock above serializes concurrent
    // expirers (lock waits block instead of aborting), leaving commit time —
    // retried by the runner — as the only conflict point.
    record_event(
        &mut **tx,
        NewEvent {
            actor_id: None,
            resource_kind: "request",
            resource_id: request_id,
            cycle: Some(cycle_number),
            revision: None,
            effective_at: deadline,
            kind: "request.expired",
            policy: "mvp-free",
            source: "worker",
            payload: json!({"state": "expired", "cycle_number": cycle_number}),
        },
    )
    .await
    .map_err(|_| AttemptError::Abort(ExpiryError::StorageFailed))?;
    Ok(ExpiryOutcome::Transitioned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(seconds, 0).expect("fixture instant builds")
    }

    #[test]
    fn exact_deadline_is_already_expired() {
        let deadline = instant(1_791_000_000);
        assert!(!is_elapsed(deadline, instant(1_790_999_999)));
        assert!(is_elapsed(deadline, deadline));
        assert!(is_elapsed(deadline, instant(1_791_000_001)));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RequestEligibilityError::NotFound,
            RequestEligibilityError::NotActionable,
            RequestEligibilityError::Expired,
            RequestEligibilityError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
        for error in [ExpiryError::NotFound, ExpiryError::StorageFailed] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
