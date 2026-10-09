//! Timed account restoration: lift the account, never the resources.
//!
//! Canonical rules: INV-38 with EC-36 (lifting the account restriction
//! overrides nothing independent — hidden resources, expired rows, blocks,
//! content suspensions, and bans all stand exactly as they were;
//! resource restoration stays separate and current under its own card),
//! EC-19 (restoring after expiry keeps the offer expired and never extends
//! a request — this writer touches the account row only), AC-49 (elapsed
//! deadlines stay elapsed), and INV-37 with AC-39 (actor, reason, scope,
//! time, and previous/resulting standing recorded through an inspection
//! audit plus a durable fact noting whether the restriction ended on time
//! or by explicit reversal).
//!
//! Both timed ends and explicit reversals flow through the one operation:
//! moderators restore inside granted scope, and banned rows refuse here
//! for the permanent-ban card to own.

use crate::application::staff_permissions::{authorized_inspect, InspectInput, StaffError};
use crate::persistence::events::{record as record_event, NewEvent};

/// Reason bound in scalar values.
pub const RESTORE_REASON_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const RESTORE_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const RESTORE_POLICY_MAX_CHARS: usize = 32;

/// One account restoration as supplied: whose restriction ends and why.
#[derive(Debug, Clone)]
pub struct RestoreUserInput {
    /// Account to restore (must be suspended).
    pub user_id: uuid::Uuid,
    /// Why the restriction ends (deadline reached or explicit reversal).
    pub reason: String,
    /// Rules version the restoration applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
}

/// One restored account: standing after the write.
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredUser {
    /// Restored account.
    pub user_id: uuid::Uuid,
    /// `active`.
    pub state: String,
    /// False when the row already stood active (no new fact).
    pub restored: bool,
    /// True when the suspension deadline had passed.
    pub timed: bool,
}

/// Typed restoration failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreUserError {
    /// Bad reason/purpose/policy text.
    InvalidField,
    /// The caller's account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such account exists.
    NotFound,
    /// The caller holds no live moderator-or-better grant.
    NotPermitted,
    /// The account is pending, banned, deleted, or otherwise outside restore.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RestoreUserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid restoration field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("restoration target not found"),
            Self::NotPermitted => f.write_str("restoration not permitted"),
            Self::InvalidState => f.write_str("account is outside restore"),
            Self::StorageFailed => f.write_str("restoration storage failed"),
        }
    }
}

impl std::error::Error for RestoreUserError {}

fn check_bounded(value: &str, max: usize) -> Result<(), RestoreUserError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(RestoreUserError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> RestoreUserError {
    match error {
        StaffError::InvalidField => RestoreUserError::InvalidField,
        StaffError::NotActive => RestoreUserError::NotActive,
        StaffError::NotFound => RestoreUserError::NotFound,
        StaffError::NotPermitted => RestoreUserError::NotPermitted,
        StaffError::StorageFailed => RestoreUserError::StorageFailed,
    }
}

/// Restore one suspended account to active: the account row alone moves.
/// Hidden resources stay hidden, expired rows stay expired, and every other
/// restriction stands — resource restoration is separate and current.
/// Banned rows refuse here for the permanent-ban card to own.
///
/// # Errors
///
/// Returns [`RestoreUserError::InvalidField`] for bad text,
/// [`RestoreUserError::NotActive`] for restricted callers or deleted
/// targets, [`RestoreUserError::NotFound`] for missing accounts,
/// [`RestoreUserError::NotPermitted`] for ungranted callers,
/// [`RestoreUserError::InvalidState`] outside suspended standing, else
/// [`RestoreUserError::StorageFailed`]. Reasons are static.
pub async fn restore_user(
    pool: &sqlx::PgPool,
    moderator_id: uuid::Uuid,
    input: RestoreUserInput,
) -> Result<RestoredUser, RestoreUserError> {
    check_bounded(&input.reason, RESTORE_REASON_MAX_CHARS)?;
    check_bounded(&input.policy_version, RESTORE_POLICY_MAX_CHARS)?;
    check_bounded(&input.purpose, RESTORE_PURPOSE_MAX_CHARS)?;
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
        .map_err(|_| RestoreUserError::StorageFailed)?;
    // Lock the account row first: concurrent restorations serialize here.
    let locked: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1 FOR UPDATE")
            .bind(input.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| RestoreUserError::StorageFailed)?;
    let (state, deleted_at) = match locked {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| RestoreUserError::StorageFailed)?;
            return Err(RestoreUserError::NotFound);
        }
    };
    if deleted_at.is_some() {
        tx.rollback()
            .await
            .map_err(|_| RestoreUserError::StorageFailed)?;
        return Err(RestoreUserError::NotActive);
    }
    if state == "active" {
        tx.rollback()
            .await
            .map_err(|_| RestoreUserError::StorageFailed)?;
        return Ok(RestoredUser {
            user_id: input.user_id,
            state,
            restored: false,
            timed: false,
        });
    }
    if state != "suspended" {
        tx.rollback()
            .await
            .map_err(|_| RestoreUserError::StorageFailed)?;
        return Err(RestoreUserError::InvalidState);
    }
    let suspension_ends: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT (payload->>'ends_at')::timestamptz FROM business_events
         WHERE resource_kind = 'user' AND resource_id = $1 AND kind = 'user.suspended'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(input.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RestoreUserError::StorageFailed)?;
    let now = chrono::Utc::now();
    let timed = suspension_ends.is_some_and(|ends_at| now >= ends_at);
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(input.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| RestoreUserError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(moderator_id),
            resource_kind: "user",
            resource_id: input.user_id,
            cycle: None,
            revision: None,
            effective_at: now,
            kind: "user.restored",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "reason": input.reason,
                "scope": input.user_id,
                "previous_state": state,
                "policy_version": input.policy_version,
                "timed": timed,
            }),
        },
    )
    .await
    .map_err(|_| RestoreUserError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RestoreUserError::StorageFailed)?;
    Ok(RestoredUser {
        user_id: input.user_id,
        state: "active".to_owned(),
        restored: true,
        timed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_text_has_bounds() {
        assert!(check_bounded("deadline reached", RESTORE_REASON_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", RESTORE_REASON_MAX_CHARS),
            Err(RestoreUserError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), RESTORE_REASON_MAX_CHARS),
            Err(RestoreUserError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RestoreUserError::InvalidField,
            RestoreUserError::NotActive,
            RestoreUserError::NotFound,
            RestoreUserError::NotPermitted,
            RestoreUserError::InvalidState,
            RestoreUserError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
