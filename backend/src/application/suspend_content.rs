//! Scoped content suspension: hide now, correct privately, restore later.
//!
//! Canonical rules: INV-09 (suspended rows take no offers or contact —
//! enforced by the shared eligibility helper, reused here and never
//! reimplemented), INV-37 with AC-39 (actor, reason, scope, time, and
//! previous/resulting standing recorded through an inspection audit plus a
//! durable business fact carrying the prior state for later restoration),
//! INV-38 with EC-36 (only state and visibility move — blocks, account
//! standing, deadlines, and cycles stand untouched, so independent
//! restrictions stay effective), AC-34 with EC-15 (hidden rows read as
//! generic unavailability to strangers — enforced by the reads),
//! AC-49 (deadlines keep running under suspension; expiry moves state
//! only and keeps hidden visibility), EC-28 (owner corrections store
//! private revisions, keep the restriction, and never republish — enforced
//! by the revision operation, proven by this card's tests).
//!
//! Restoration is explicitly out of scope: the later restoration card owns
//! the authorized lift, reading the prior standing back out of the
//! suspension fact recorded here.

use crate::application::staff_permissions::{authorized_inspect, InspectInput, StaffError};
use crate::persistence::events::{record as record_event, NewEvent};

/// Suspendable target families.
pub const SUSPENDABLE_KINDS: [&str; 2] = ["request", "offer"];
/// Reason bound in scalar values.
pub const SUSPEND_REASON_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const SUSPEND_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const SUSPEND_POLICY_MAX_CHARS: usize = 32;

/// One content suspension as supplied: what to hide and why.
#[derive(Debug, Clone)]
pub struct SuspendInput {
    /// `request` or `offer`.
    pub target_kind: String,
    /// Content identifier (must already exist).
    pub target_id: uuid::Uuid,
    /// Why this content hides (recorded verbatim in the fact).
    pub reason: String,
    /// Rules version the suspension applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
    /// Decided case this suspension follows, when review precedes it.
    pub case_id: Option<uuid::Uuid>,
}

/// One suspended row: standing after the write.
#[derive(Debug, Clone, PartialEq)]
pub struct SuspendedContent {
    /// Suspended family.
    pub target_kind: String,
    /// Suspended identifier.
    pub target_id: uuid::Uuid,
    /// `suspended`.
    pub state: String,
    /// `hidden`.
    pub visibility: String,
    /// False when the row already stood suspended (no new fact).
    pub transitioned: bool,
}

/// Typed suspension failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspendError {
    /// Unknown kind or bad reason/purpose/policy text.
    InvalidField,
    /// The account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such content or case exists.
    NotFound,
    /// The caller holds no live moderator-or-better grant.
    NotPermitted,
    /// The row is a draft, terminal, or otherwise outside suspension.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for SuspendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid suspension field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("suspension target not found"),
            Self::NotPermitted => f.write_str("suspension not permitted"),
            Self::InvalidState => f.write_str("content is outside suspension"),
            Self::StorageFailed => f.write_str("suspension storage failed"),
        }
    }
}

impl std::error::Error for SuspendError {}

fn check_bounded(value: &str, max: usize) -> Result<(), SuspendError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(SuspendError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> SuspendError {
    match error {
        StaffError::InvalidField => SuspendError::InvalidField,
        StaffError::NotActive => SuspendError::NotActive,
        StaffError::NotFound => SuspendError::NotFound,
        StaffError::NotPermitted => SuspendError::NotPermitted,
        StaffError::StorageFailed => SuspendError::StorageFailed,
    }
}

/// Suspend one request or offer: hide it with its prior standing recorded
/// for later explicit restoration. Repeating a live suspension converges
/// on the standing row with no new fact. Nothing else moves — deadlines,
/// cycles, blocks, and account standing stand untouched.
///
/// # Errors
///
/// Returns [`SuspendError::InvalidField`] for unknown kinds or bad text,
/// [`SuspendError::NotActive`] for restricted callers,
/// [`SuspendError::NotFound`] for missing content or cases,
/// [`SuspendError::NotPermitted`] for ungranted callers,
/// [`SuspendError::InvalidState`] outside suspendable standings, else
/// [`SuspendError::StorageFailed`]. Reasons are static.
pub async fn suspend_content(
    pool: &sqlx::PgPool,
    moderator_id: uuid::Uuid,
    input: SuspendInput,
) -> Result<SuspendedContent, SuspendError> {
    if !SUSPENDABLE_KINDS.contains(&input.target_kind.as_str()) {
        return Err(SuspendError::InvalidField);
    }
    check_bounded(&input.reason, SUSPEND_REASON_MAX_CHARS)?;
    check_bounded(&input.policy_version, SUSPEND_POLICY_MAX_CHARS)?;
    check_bounded(&input.purpose, SUSPEND_PURPOSE_MAX_CHARS)?;
    authorized_inspect(
        pool,
        moderator_id,
        InspectInput {
            target_kind: input.target_kind.clone(),
            target_id: input.target_id,
            purpose: input.purpose,
            policy_version: input.policy_version.clone(),
        },
    )
    .await
    .map_err(staff_error)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| SuspendError::StorageFailed)?;
    if let Some(case_id) = input.case_id {
        let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM report_cases WHERE id = $1")
            .bind(case_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| SuspendError::StorageFailed)?;
        if found.is_none() {
            tx.rollback()
                .await
                .map_err(|_| SuspendError::StorageFailed)?;
            return Err(SuspendError::NotFound);
        }
    }
    let table = match input.target_kind.as_str() {
        "request" => "requests",
        "offer" => "offers",
        _ => {
            tx.rollback()
                .await
                .map_err(|_| SuspendError::StorageFailed)?;
            return Err(SuspendError::InvalidField);
        }
    };
    // Lock the row first: concurrent suspensions serialize here, and every
    // loser replays the winner's hidden row instead of duplicating facts.
    let locked: Option<(String, String)> = sqlx::query_as(&format!(
        "SELECT state, visibility FROM {table} WHERE id = $1 FOR UPDATE"
    ))
    .bind(input.target_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| SuspendError::StorageFailed)?;
    let (state, visibility) = match locked {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| SuspendError::StorageFailed)?;
            return Err(SuspendError::NotFound);
        }
    };
    if state == "suspended" {
        tx.rollback()
            .await
            .map_err(|_| SuspendError::StorageFailed)?;
        return Ok(SuspendedContent {
            target_kind: input.target_kind,
            target_id: input.target_id,
            state,
            visibility,
            transitioned: false,
        });
    }
    let suspendable = match input.target_kind.as_str() {
        "request" => state == "active",
        "offer" => matches!(state.as_str(), "sent" | "viewed" | "contacted"),
        _ => false,
    };
    if !suspendable {
        tx.rollback()
            .await
            .map_err(|_| SuspendError::StorageFailed)?;
        return Err(SuspendError::InvalidState);
    }
    sqlx::query(&format!(
        "UPDATE {table} SET state = 'suspended', visibility = 'hidden', updated_at = now()
         WHERE id = $1"
    ))
    .bind(input.target_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| SuspendError::StorageFailed)?;
    let kind = match input.target_kind.as_str() {
        "request" => "request.suspended",
        _ => "offer.suspended",
    };
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(moderator_id),
            resource_kind: match input.target_kind.as_str() {
                "request" => "request",
                _ => "offer",
            },
            resource_id: input.target_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind,
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "reason": input.reason,
                "scope": input.target_id,
                "previous_state": state,
                "previous_visibility": visibility,
                "policy_version": input.policy_version,
                "case_id": input.case_id,
            }),
        },
    )
    .await
    .map_err(|_| SuspendError::StorageFailed)?;
    tx.commit().await.map_err(|_| SuspendError::StorageFailed)?;
    Ok(SuspendedContent {
        target_kind: input.target_kind,
        target_id: input.target_id,
        state: "suspended".to_owned(),
        visibility: "hidden".to_owned(),
        transitioned: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspension_text_has_bounds() {
        assert!(check_bounded("credible fraud pattern", SUSPEND_REASON_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", SUSPEND_REASON_MAX_CHARS),
            Err(SuspendError::InvalidField)
        );
        assert_eq!(
            check_bounded("", SUSPEND_REASON_MAX_CHARS),
            Err(SuspendError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), SUSPEND_REASON_MAX_CHARS),
            Err(SuspendError::InvalidField)
        );
        assert!(!SUSPENDABLE_KINDS.contains(&"user"));
        assert!(!SUSPENDABLE_KINDS.contains(&"case"));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            SuspendError::InvalidField,
            SuspendError::NotActive,
            SuspendError::NotFound,
            SuspendError::NotPermitted,
            SuspendError::InvalidState,
            SuspendError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
