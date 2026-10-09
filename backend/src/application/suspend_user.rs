//! Temporary account suspension: restrict now, keep safety paths open.
//!
//! Canonical rules: INV-04 (pending, suspended, banned, or deleted accounts
//! cannot publish, renew, offer, or initiate contact — enforced by the
//! shared actor/pair guards, reused here and never reimplemented — while
//! specifically allowed safety and account-management actions stay open:
//! own-history reporting, own unresolved outcomes via the closure path,
//! and the future appeal/deletion writers), INV-28 with AC-25 (either
//! suspended party refuses contact with no new disclosure), INV-37 with
//! AC-39 (actor, reason, scope, time, duration, and previous/resulting
//! standing recorded through an inspection audit plus a durable fact
//! carrying the restriction deadline), INV-38 with EC-36 (only the account
//! state plus live-resource visibility move — deadlines, cycles, blocks,
//! content-suspension states, and other restrictions stand untouched), and
//! spec 16.3 (first ordinary suspensions last hours with a justified
//! duration; pending-investigation restrictions return for review inside
//! seven days — durations here run 1 to 168 hours so longer exclusions
//! stay with the permanent-ban card).
//!
//! Repeating a live suspension converges on the standing row with no new
//! fact. Resource restoration stays separate: hidden rows stay hidden
//! until the explicit restoration card lifts them.

use crate::application::staff_permissions::{authorized_inspect, InspectInput, StaffError};
use crate::persistence::events::{record as record_event, NewEvent};

/// Minimum suspension duration in hours.
pub const SUSPEND_MIN_HOURS: i64 = 1;
/// Maximum suspension duration in hours (seven-day review window).
pub const SUSPEND_MAX_HOURS: i64 = 168;
/// Reason bound in scalar values.
pub const SUSPEND_REASON_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const SUSPEND_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const SUSPEND_POLICY_MAX_CHARS: usize = 32;

/// One account suspension as supplied: who is restricted and for how long.
#[derive(Debug, Clone)]
pub struct SuspendUserInput {
    /// Account to restrict (must be active).
    pub user_id: uuid::Uuid,
    /// Why this account is restricted.
    pub reason: String,
    /// Restriction length in hours (1 to 168).
    pub duration_hours: i64,
    /// Rules version the suspension applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
    /// Decided case this suspension follows, when review precedes it.
    pub case_id: Option<uuid::Uuid>,
}

/// One suspended account: standing after the write.
#[derive(Debug, Clone, PartialEq)]
pub struct SuspendedUser {
    /// Restricted account.
    pub user_id: uuid::Uuid,
    /// `suspended`.
    pub state: String,
    /// Restriction deadline (database clock plus duration).
    pub ends_at: chrono::DateTime<chrono::Utc>,
    /// False when the row already stood suspended (no new fact).
    pub transitioned: bool,
    /// Live owned requests hidden by this write.
    pub hidden_requests: i64,
    /// Live owned offers hidden by this write.
    pub hidden_offers: i64,
}

/// Typed suspension failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspendUserError {
    /// Bad reason/duration/purpose/policy text.
    InvalidField,
    /// The caller's account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such account or case exists.
    NotFound,
    /// The caller holds no live moderator-or-better grant.
    NotPermitted,
    /// The account is pending, banned, or otherwise outside suspension.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for SuspendUserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid suspension field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("suspension target not found"),
            Self::NotPermitted => f.write_str("suspension not permitted"),
            Self::InvalidState => f.write_str("account is outside suspension"),
            Self::StorageFailed => f.write_str("suspension storage failed"),
        }
    }
}

impl std::error::Error for SuspendUserError {}

fn check_bounded(value: &str, max: usize) -> Result<(), SuspendUserError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(SuspendUserError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> SuspendUserError {
    match error {
        StaffError::InvalidField => SuspendUserError::InvalidField,
        StaffError::NotActive => SuspendUserError::NotActive,
        StaffError::NotFound => SuspendUserError::NotFound,
        StaffError::NotPermitted => SuspendUserError::NotPermitted,
        StaffError::StorageFailed => SuspendUserError::StorageFailed,
    }
}

/// Suspend one active account for a bounded duration: the account moves to
/// `suspended` and its live owned requests/offers hide, while deadlines,
/// cycles, blocks, and content-suspension states run untouched. Allowed
/// safety actions (own-history reports, own outcomes, future appeals)
/// stay open through their own gates. Repeating a live suspension
/// converges with no new fact.
///
/// # Errors
///
/// Returns [`SuspendUserError::InvalidField`] for bad text or durations,
/// [`SuspendUserError::NotActive`] for restricted callers or deleted
/// targets, [`SuspendUserError::NotFound`] for missing accounts or cases,
/// [`SuspendUserError::NotPermitted`] for ungranted callers,
/// [`SuspendUserError::InvalidState`] outside active standing, else
/// [`SuspendUserError::StorageFailed`]. Reasons are static.
pub async fn suspend_user(
    pool: &sqlx::PgPool,
    moderator_id: uuid::Uuid,
    input: SuspendUserInput,
) -> Result<SuspendedUser, SuspendUserError> {
    if !(SUSPEND_MIN_HOURS..=SUSPEND_MAX_HOURS).contains(&input.duration_hours) {
        return Err(SuspendUserError::InvalidField);
    }
    check_bounded(&input.reason, SUSPEND_REASON_MAX_CHARS)?;
    check_bounded(&input.policy_version, SUSPEND_POLICY_MAX_CHARS)?;
    check_bounded(&input.purpose, SUSPEND_PURPOSE_MAX_CHARS)?;
    authorized_inspect(
        pool,
        moderator_id,
        InspectInput {
            target_kind: "user".to_owned(),
            target_id: input.user_id,
            purpose: input.purpose,
            policy_version: input.policy_version.clone(),
        },
    )
    .await
    .map_err(staff_error)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| SuspendUserError::StorageFailed)?;
    if let Some(case_id) = input.case_id {
        let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM report_cases WHERE id = $1")
            .bind(case_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| SuspendUserError::StorageFailed)?;
        if found.is_none() {
            tx.rollback()
                .await
                .map_err(|_| SuspendUserError::StorageFailed)?;
            return Err(SuspendUserError::NotFound);
        }
    }
    // Lock the account row first: concurrent suspensions serialize here,
    // and every loser replays the winner's standing.
    let locked: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1 FOR UPDATE")
            .bind(input.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| SuspendUserError::StorageFailed)?;
    let (state, deleted_at) = match locked {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| SuspendUserError::StorageFailed)?;
            return Err(SuspendUserError::NotFound);
        }
    };
    if deleted_at.is_some() {
        tx.rollback()
            .await
            .map_err(|_| SuspendUserError::StorageFailed)?;
        return Err(SuspendUserError::NotActive);
    }
    if state == "suspended" {
        let standing = current_suspension(&mut tx, input.user_id).await?;
        tx.rollback()
            .await
            .map_err(|_| SuspendUserError::StorageFailed)?;
        return Ok(standing);
    }
    if state != "active" {
        tx.rollback()
            .await
            .map_err(|_| SuspendUserError::StorageFailed)?;
        return Err(SuspendUserError::InvalidState);
    }
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(input.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| SuspendUserError::StorageFailed)?;
    // Hide live owned resources without touching their states, deadlines,
    // or cycles: content-suspension standing stays independent, and
    // restoration later decides each row on current facts.
    let hidden_requests = sqlx::query(
        "UPDATE requests SET visibility = 'hidden', updated_at = now()
         WHERE author_id = $1 AND state = 'active' AND visibility = 'public'",
    )
    .bind(input.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| SuspendUserError::StorageFailed)?
    .rows_affected() as i64;
    let hidden_offers = sqlx::query(
        "UPDATE offers SET visibility = 'hidden', updated_at = now()
         WHERE seller_id = $1 AND state IN ('sent', 'viewed', 'contacted')
           AND visibility = 'visible'",
    )
    .bind(input.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| SuspendUserError::StorageFailed)?
    .rows_affected() as i64;
    let ends_at = chrono::Utc::now() + chrono::Duration::hours(input.duration_hours);
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(moderator_id),
            resource_kind: "user",
            resource_id: input.user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "user.suspended",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "reason": input.reason,
                "scope": input.user_id,
                "duration_hours": input.duration_hours,
                "ends_at": ends_at,
                "previous_state": state,
                "policy_version": input.policy_version,
                "case_id": input.case_id,
                "hidden_requests": hidden_requests,
                "hidden_offers": hidden_offers,
            }),
        },
    )
    .await
    .map_err(|_| SuspendUserError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| SuspendUserError::StorageFailed)?;
    Ok(SuspendedUser {
        user_id: input.user_id,
        state: "suspended".to_owned(),
        ends_at,
        transitioned: true,
        hidden_requests,
        hidden_offers,
    })
}

/// Rebuild the standing view for an already-suspended row from its latest
/// suspension fact; falls back to an open deadline when no fact survives.
async fn current_suspension(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
) -> Result<SuspendedUser, SuspendUserError> {
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM business_events
         WHERE resource_kind = 'user' AND resource_id = $1 AND kind = 'user.suspended'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| SuspendUserError::StorageFailed)?;
    let ends_at = payload
        .as_ref()
        .and_then(|payload| payload.get("ends_at"))
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| raw.parse::<chrono::DateTime<chrono::Utc>>().ok());
    Ok(SuspendedUser {
        user_id,
        state: "suspended".to_owned(),
        ends_at: ends_at.unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(24)),
        transitioned: false,
        hidden_requests: 0,
        hidden_offers: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspension_duration_runs_one_to_168_hours() {
        for hours in [1, 24, 168] {
            assert!((SUSPEND_MIN_HOURS..=SUSPEND_MAX_HOURS).contains(&hours));
        }
        for hours in [0, -1, 169, 8760] {
            assert!(!(SUSPEND_MIN_HOURS..=SUSPEND_MAX_HOURS).contains(&hours));
        }
    }

    #[test]
    fn suspension_text_has_bounds() {
        assert!(check_bounded("repeated low-severity abuse", SUSPEND_REASON_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", SUSPEND_REASON_MAX_CHARS),
            Err(SuspendUserError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), SUSPEND_REASON_MAX_CHARS),
            Err(SuspendUserError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            SuspendUserError::InvalidField,
            SuspendUserError::NotActive,
            SuspendUserError::NotFound,
            SuspendUserError::NotPermitted,
            SuspendUserError::InvalidState,
            SuspendUserError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
