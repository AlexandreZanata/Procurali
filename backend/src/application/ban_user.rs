//! Permanent bans: admin-only indefinite exclusion with distinct cascades.
//!
//! Canonical rules: INV-04 (banned accounts cannot publish, renew, offer,
//! initiate contact, or complete outcomes — enforced by the shared guards,
//! reused here and never reimplemented), INV-25 (terminal offers take no
//! new contact, now including ban-invalidated ones), INV-28 with AC-25
//! (either banned party refuses contact with no new disclosure), INV-36
//! (volume alone never reaches this writer — only an administrator's
//! documented decision does), INV-37 with AC-39 (actor, evidence, reason,
//! scope, time, and previous/resulting standing recorded through an
//! inspection audit plus a durable fact), INV-38 with EC-36 (independent
//! restrictions stay effective; past legitimate actions remain historical
//! facts), EC-05 (banned sellers lose live offers and hidden content while
//! affected buyers get an offers-unavailable notice), EC-06 (banned buyers
//! lose unresolved requests to cancellation with related live offers
//! invalidated), and spec 21.6 with 21.7 (bans and reversals are
//! administrator-only; no timer reverses a ban — this writer records no
//! deadline, and the restoration writer refuses banned rows).
//!
//! Repeating a live ban converges on the standing row with no new fact.

use crate::application::staff_permissions::{authorized_inspect, InspectInput, StaffError};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::notices::{record as record_notice, NewNotice};

/// Reason bound in scalar values.
pub const BAN_REASON_MAX_CHARS: usize = 1000;
/// Evidence-reference bound in scalar values.
pub const BAN_EVIDENCE_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const BAN_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const BAN_POLICY_MAX_CHARS: usize = 32;

/// One permanent ban as supplied: who is excluded, on what documented basis.
#[derive(Debug, Clone)]
pub struct BanUserInput {
    /// Account to ban (must be active or suspended).
    pub user_id: uuid::Uuid,
    /// Why this account is banned.
    pub reason: String,
    /// Documented evidence references (required: bans are indefinite).
    pub evidence: String,
    /// Rules version the ban applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
    /// Decided case this ban follows, when review precedes it.
    pub case_id: Option<uuid::Uuid>,
}

/// One banned account: standing after the write.
#[derive(Debug, Clone, PartialEq)]
pub struct BannedUser {
    /// Excluded account.
    pub user_id: uuid::Uuid,
    /// `banned`.
    pub state: String,
    /// False when the row already stood banned (no new fact).
    pub transitioned: bool,
    /// Unresolved owned requests cancelled by this write.
    pub cancelled_requests: i64,
    /// Live offers invalidated by this write (own plus related).
    pub invalidated_offers: i64,
}

/// Typed ban failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BanError {
    /// Bad reason/evidence/purpose/policy text.
    InvalidField,
    /// The caller's account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such account or case exists.
    NotFound,
    /// The caller holds no live administrator grant.
    NotPermitted,
    /// The account is pending, deleted, or otherwise outside banning.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for BanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid ban field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("ban target not found"),
            Self::NotPermitted => f.write_str("ban not permitted"),
            Self::InvalidState => f.write_str("account is outside banning"),
            Self::StorageFailed => f.write_str("ban storage failed"),
        }
    }
}

impl std::error::Error for BanError {}

fn check_bounded(value: &str, max: usize) -> Result<(), BanError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(BanError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> BanError {
    match error {
        StaffError::InvalidField => BanError::InvalidField,
        StaffError::NotActive => BanError::NotActive,
        StaffError::NotFound => BanError::NotFound,
        StaffError::NotPermitted => BanError::NotPermitted,
        StaffError::StorageFailed => BanError::StorageFailed,
    }
}

/// Ban one account indefinitely by administrator authority: the account
/// moves to `banned`, its unresolved owned requests cancel hidden with
/// open cycles ended, its live offers and related live offers on its
/// cancelled requests invalidate, affected buyers get an
/// offers-unavailable notice, and the banned account gets a restriction
/// notice with no reporter or phone material. Deadlines, expired rows,
/// blocks, and every other restriction stand untouched. No timer reverses
/// a ban: no deadline is recorded, and restoration refuses banned rows.
///
/// # Errors
///
/// Returns [`BanError::InvalidField`] for bad text, [`BanError::NotActive`]
/// for restricted callers or deleted targets, [`BanError::NotFound`] for
/// missing accounts or cases, [`BanError::NotPermitted`] for
/// non-administrator callers, [`BanError::InvalidState`] outside
/// active/suspended standing, else [`BanError::StorageFailed`]. Reasons
/// are static.
pub async fn ban_user(
    pool: &sqlx::PgPool,
    admin_id: uuid::Uuid,
    input: BanUserInput,
) -> Result<BannedUser, BanError> {
    check_bounded(&input.reason, BAN_REASON_MAX_CHARS)?;
    check_bounded(&input.evidence, BAN_EVIDENCE_MAX_CHARS)?;
    check_bounded(&input.policy_version, BAN_POLICY_MAX_CHARS)?;
    check_bounded(&input.purpose, BAN_PURPOSE_MAX_CHARS)?;
    // Administrator-only: moderators decide and suspend, but bans and
    // reversals need the explicit administrator grant.
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
    let mut tx = pool.begin().await.map_err(|_| BanError::StorageFailed)?;
    if let Some(case_id) = input.case_id {
        let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM report_cases WHERE id = $1")
            .bind(case_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| BanError::StorageFailed)?;
        if found.is_none() {
            tx.rollback().await.map_err(|_| BanError::StorageFailed)?;
            return Err(BanError::NotFound);
        }
    }
    // Lock the account row first: concurrent bans serialize here, and every
    // loser replays the winner's standing.
    let locked: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1 FOR UPDATE")
            .bind(input.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| BanError::StorageFailed)?;
    let (state, deleted_at) = match locked {
        Some(standing) => standing,
        None => {
            tx.rollback().await.map_err(|_| BanError::StorageFailed)?;
            return Err(BanError::NotFound);
        }
    };
    if deleted_at.is_some() {
        tx.rollback().await.map_err(|_| BanError::StorageFailed)?;
        return Err(BanError::NotActive);
    }
    if state == "banned" {
        tx.rollback().await.map_err(|_| BanError::StorageFailed)?;
        return Ok(BannedUser {
            user_id: input.user_id,
            state,
            transitioned: false,
            cancelled_requests: 0,
            invalidated_offers: 0,
        });
    }
    if state != "active" && state != "suspended" {
        tx.rollback().await.map_err(|_| BanError::StorageFailed)?;
        return Err(BanError::InvalidState);
    }
    sqlx::query("UPDATE users SET state = 'banned' WHERE id = $1")
        .bind(input.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| BanError::StorageFailed)?;
    // Buyer cascade: unresolved ACTIVE owned requests cancel hidden with
    // open cycles ended; related live offers on them invalidate.
    // Content-suspended rows keep their moderation standing (independent
    // restriction): still unusable while banned, restorable after reversal
    // and review — the ban steamrolls no open review track.
    let cancelled: Vec<uuid::Uuid> = sqlx::query_scalar(
        "UPDATE requests SET state = 'cancelled', visibility = 'hidden', updated_at = now()
         WHERE author_id = $1 AND state = 'active'
         RETURNING id",
    )
    .bind(input.user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| BanError::StorageFailed)?;
    for request_id in &cancelled {
        sqlx::query(
            "UPDATE request_cycles SET ended_at = now()
             WHERE request_id = $1 AND ended_at IS NULL",
        )
        .bind(request_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| BanError::StorageFailed)?;
    }
    let mut invalidated = 0i64;
    if !cancelled.is_empty() {
        invalidated += sqlx::query(
            "UPDATE offers SET state = 'invalidated', terminal_reason = 'banned',
                    updated_at = now()
             WHERE request_id = ANY($1)
               AND state IN ('sent', 'viewed', 'contacted')",
        )
        .bind(&cancelled)
        .execute(&mut *tx)
        .await
        .map_err(|_| BanError::StorageFailed)?
        .rows_affected() as i64;
    }
    // Seller cascade: own remaining live offers invalidate hidden.
    // Suspended offers keep their standing for the same independence
    // reason as above.
    invalidated += sqlx::query(
        "UPDATE offers SET state = 'invalidated', terminal_reason = 'banned',
                visibility = 'hidden', updated_at = now()
         WHERE seller_id = $1
           AND state IN ('sent', 'viewed', 'contacted')",
    )
    .bind(input.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| BanError::StorageFailed)?
    .rows_affected() as i64;
    let fact = record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(admin_id),
            resource_kind: "user",
            resource_id: input.user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "user.banned",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "reason": input.reason,
                "evidence": input.evidence,
                "scope": input.user_id,
                "previous_state": state,
                "policy_version": input.policy_version,
                "case_id": input.case_id,
                "cancelled_requests": cancelled.len() as i64,
                "invalidated_offers": invalidated,
            }),
        },
    )
    .await
    .map_err(|_| BanError::StorageFailed)?;
    // Affected buyers learn only that offers are unavailable; the banned
    // account learns its restriction. Both bodies are static templates
    // with no reporter, phone, or secret material.
    let affected: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT requests.author_id FROM offers
         JOIN requests ON requests.id = offers.request_id
         WHERE offers.seller_id = $1 AND offers.terminal_reason = 'banned'",
    )
    .bind(input.user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| BanError::StorageFailed)?;
    for author_id in affected
        .into_iter()
        .filter(|author| *author != input.user_id)
    {
        record_notice(
            &mut *tx,
            NewNotice {
                account_id: author_id,
                kind: "offer.unavailable",
                resource_kind: "user",
                resource_id: input.user_id,
                event_id: fact.id,
                body: "An offer on your request is unavailable following a safety review."
                    .to_owned(),
            },
        )
        .await
        .map_err(|_| BanError::StorageFailed)?;
    }
    record_notice(
        &mut *tx,
        NewNotice {
            account_id: input.user_id,
            kind: "account.restricted",
            resource_kind: "user",
            resource_id: input.user_id,
            event_id: fact.id,
            body: "Your account is restricted following a safety review. Appeal options will be provided."
                .to_owned(),
        },
    )
    .await
    .map_err(|_| BanError::StorageFailed)?;
    tx.commit().await.map_err(|_| BanError::StorageFailed)?;
    Ok(BannedUser {
        user_id: input.user_id,
        state: "banned".to_owned(),
        transitioned: true,
        cancelled_requests: cancelled.len() as i64,
        invalidated_offers: invalidated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ban_documents_reason_and_evidence() {
        assert!(check_bounded("trafficking pattern", BAN_REASON_MAX_CHARS).is_ok());
        assert!(check_bounded("report and contact pattern", BAN_EVIDENCE_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("", BAN_REASON_MAX_CHARS),
            Err(BanError::InvalidField)
        );
        assert_eq!(
            check_bounded("   ", BAN_EVIDENCE_MAX_CHARS),
            Err(BanError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), BAN_REASON_MAX_CHARS),
            Err(BanError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            BanError::InvalidField,
            BanError::NotActive,
            BanError::NotFound,
            BanError::NotPermitted,
            BanError::InvalidState,
            BanError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
