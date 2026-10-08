//! Shared current-account and relationship guards for writers.
//!
//! Canonical rules: INV-04 (restricted accounts cannot act — every check reads
//! current state, so suspension, bans, and deletion deny immediately, AC-25),
//! INV-05 (roles never authorize — these guards take account identifiers only;
//! no role, owner claim, or session-validity boolean exists anywhere here),
//! INV-28/INV-35 and AC-35 (both parties eligible with no either-direction
//! block). There is deliberately no generic `is_authenticated` override:
//! writers call the specific check for their action, and every denial names
//! its exact reason without revealing unrelated records.

use crate::persistence::eligibility::{
    account_standing, blocked_either_direction, EligibilityDbError,
};

/// Why one guarded action is refused. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// The account is missing, deleted, or not `active`.
    AccountNotActive,
    /// An either-direction block relates the pair.
    BlockedRelationship,
}

impl std::fmt::Display for RefusalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountNotActive => f.write_str("account is not eligible"),
            Self::BlockedRelationship => f.write_str("relationship is blocked"),
        }
    }
}

/// Outcome of one guard check: permitted, or refused for a named reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The action may proceed.
    Permitted,
    /// The action is refused.
    Refused(RefusalReason),
}

/// Typed guard failure, distinct from business refusal. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EligibilityError {
    /// The lookup failed (connection, transaction state).
    StorageFailed,
}

impl std::fmt::Display for EligibilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("eligibility check failed")
    }
}

impl std::error::Error for EligibilityError {}

impl From<EligibilityDbError> for EligibilityError {
    fn from(_: EligibilityDbError) -> Self {
        Self::StorageFailed
    }
}

/// Check one actor alone: live row, `active` state, not deleted.
///
/// Missing, pending, suspended, banned, and deleted accounts share one
/// indistinguishable refusal (anti-enumeration).
///
/// # Errors
///
/// Returns [`EligibilityError::StorageFailed`] on database failure only.
pub async fn check_actor(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> Result<CheckOutcome, EligibilityError> {
    let standing = account_standing(pool, user_id).await?;
    match standing {
        Some(standing) if standing.state == "active" && !standing.deleted => {
            Ok(CheckOutcome::Permitted)
        }
        _ => Ok(CheckOutcome::Refused(RefusalReason::AccountNotActive)),
    }
}

/// Check one pair for offer/contact eligibility: both actors eligible with no
/// either-direction block.
///
/// State is evaluated before relationships, so an ineligible requester never
/// learns block records. Sessions are irrelevant here by design: a live
/// session for a since-restricted account still refuses, because these reads
/// see current rows, not authentication history.
///
/// # Errors
///
/// Returns [`EligibilityError::StorageFailed`] on database failure only.
pub async fn check_pair(
    pool: &sqlx::PgPool,
    viewer_id: uuid::Uuid,
    other_id: uuid::Uuid,
) -> Result<CheckOutcome, EligibilityError> {
    if check_actor(pool, viewer_id).await? != CheckOutcome::Permitted {
        return Ok(CheckOutcome::Refused(RefusalReason::AccountNotActive));
    }
    if check_actor(pool, other_id).await? != CheckOutcome::Permitted {
        return Ok(CheckOutcome::Refused(RefusalReason::AccountNotActive));
    }
    if blocked_either_direction(pool, viewer_id, other_id).await? {
        return Ok(CheckOutcome::Refused(RefusalReason::BlockedRelationship));
    }
    Ok(CheckOutcome::Permitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_carry_no_values() {
        for reason in [
            RefusalReason::AccountNotActive,
            RefusalReason::BlockedRelationship,
        ] {
            let rendered = format!("{reason:?} {reason}");
            assert!(!rendered.contains("canary"));
        }
    }
}
