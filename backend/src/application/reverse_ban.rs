//! Formal ban reversal: restore the account, never the terminal past.
//!
//! Canonical rules: INV-38 with EC-36 (lifting the ban overrides nothing
//! independent — cancelled requests stay cancelled, invalidated offers
//! stay invalidated, expired rows stay expired, hidden rows stay hidden,
//! and every other restriction stands; resource restoration stays separate
//! and current under its own card), INV-37 with AC-39 (actor, reason,
//! scope, time, and previous/resulting standing recorded through an
//! inspection audit plus a durable fact), and spec 21.6 with 21.7
//! (reversals are administrator-only and explicitly recorded — never a
//! timer, never silent).
//!
//! Reversing an active account converges with no new fact.

use crate::application::staff_permissions::{authorized_inspect, InspectInput, StaffError};
use crate::persistence::events::{record as record_event, NewEvent};

/// Reason bound in scalar values.
pub const REVERSE_REASON_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const REVERSE_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const REVERSE_POLICY_MAX_CHARS: usize = 32;

/// One ban reversal as supplied: whose exclusion ends and why.
#[derive(Debug, Clone)]
pub struct ReverseBanInput {
    /// Banned account to restore (must be banned).
    pub user_id: uuid::Uuid,
    /// Why the ban ends.
    pub reason: String,
    /// Rules version the reversal applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
}

/// One reversed ban: standing after the write.
#[derive(Debug, Clone, PartialEq)]
pub struct ReversedBan {
    /// Restored account.
    pub user_id: uuid::Uuid,
    /// `active`.
    pub state: String,
    /// False when the row already stood active (no new fact).
    pub reversed: bool,
}

/// Typed reversal failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverseBanError {
    /// Bad reason/purpose/policy text.
    InvalidField,
    /// The caller's account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such account exists.
    NotFound,
    /// The caller holds no live administrator grant.
    NotPermitted,
    /// The account is suspended, deleted, or otherwise outside reversal.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ReverseBanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid reversal field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("reversal target not found"),
            Self::NotPermitted => f.write_str("reversal not permitted"),
            Self::InvalidState => f.write_str("account is outside reversal"),
            Self::StorageFailed => f.write_str("reversal storage failed"),
        }
    }
}

impl std::error::Error for ReverseBanError {}

fn check_bounded(value: &str, max: usize) -> Result<(), ReverseBanError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(ReverseBanError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> ReverseBanError {
    match error {
        StaffError::InvalidField => ReverseBanError::InvalidField,
        StaffError::NotActive => ReverseBanError::NotActive,
        StaffError::NotFound => ReverseBanError::NotFound,
        StaffError::NotPermitted => ReverseBanError::NotPermitted,
        StaffError::StorageFailed => ReverseBanError::StorageFailed,
    }
}

/// Reverse one ban by administrator authority: the account row alone moves
/// to `active`. Cancelled requests, invalidated offers, expired rows,
/// hidden visibility, and every other restriction stand exactly as they
/// were — old terminal cycles never revive.
///
/// # Errors
///
/// Returns [`ReverseBanError::InvalidField`] for bad text,
/// [`ReverseBanError::NotActive`] for restricted callers or deleted
/// targets, [`ReverseBanError::NotFound`] for missing accounts,
/// [`ReverseBanError::NotPermitted`] for non-administrator callers,
/// [`ReverseBanError::InvalidState`] outside banned standing, else
/// [`ReverseBanError::StorageFailed`]. Reasons are static.
pub async fn reverse_ban(
    pool: &sqlx::PgPool,
    admin_id: uuid::Uuid,
    input: ReverseBanInput,
) -> Result<ReversedBan, ReverseBanError> {
    check_bounded(&input.reason, REVERSE_REASON_MAX_CHARS)?;
    check_bounded(&input.policy_version, REVERSE_POLICY_MAX_CHARS)?;
    check_bounded(&input.purpose, REVERSE_PURPOSE_MAX_CHARS)?;
    // Administrator-only: moderators never reverse bans, even their own
    // suspensions' escalations.
    crate::application::staff_permissions::require_admin(pool, admin_id)
        .await
        .map_err(staff_error)?;
    authorized_inspect(
        pool,
        admin_id,
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
        .map_err(|_| ReverseBanError::StorageFailed)?;
    // Lock the account row first: concurrent reversals serialize here.
    let locked: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1 FOR UPDATE")
            .bind(input.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ReverseBanError::StorageFailed)?;
    let (state, deleted_at) = match locked {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| ReverseBanError::StorageFailed)?;
            return Err(ReverseBanError::NotFound);
        }
    };
    if deleted_at.is_some() {
        tx.rollback()
            .await
            .map_err(|_| ReverseBanError::StorageFailed)?;
        return Err(ReverseBanError::NotActive);
    }
    if state == "active" {
        tx.rollback()
            .await
            .map_err(|_| ReverseBanError::StorageFailed)?;
        return Ok(ReversedBan {
            user_id: input.user_id,
            state,
            reversed: false,
        });
    }
    if state != "banned" {
        tx.rollback()
            .await
            .map_err(|_| ReverseBanError::StorageFailed)?;
        return Err(ReverseBanError::InvalidState);
    }
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(input.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ReverseBanError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(admin_id),
            resource_kind: "user",
            resource_id: input.user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "user.ban_reversed",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "reason": input.reason,
                "scope": input.user_id,
                "previous_state": state,
                "policy_version": input.policy_version,
            }),
        },
    )
    .await
    .map_err(|_| ReverseBanError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| ReverseBanError::StorageFailed)?;
    Ok(ReversedBan {
        user_id: input.user_id,
        state: "active".to_owned(),
        reversed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_text_has_bounds() {
        assert!(check_bounded("new exonerating evidence", REVERSE_REASON_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", REVERSE_REASON_MAX_CHARS),
            Err(ReverseBanError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), REVERSE_REASON_MAX_CHARS),
            Err(ReverseBanError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ReverseBanError::InvalidField,
            ReverseBanError::NotActive,
            ReverseBanError::NotFound,
            ReverseBanError::NotPermitted,
            ReverseBanError::InvalidState,
            ReverseBanError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
