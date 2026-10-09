//! Account deletion cascades: end interaction, keep only eligible history.
//!
//! Canonical rules: INV-04 (deleted accounts cannot publish, renew, offer,
//! initiate contact, or complete outcomes — enforced by the shared guards,
//! reused here and never reimplemented), INV-25 with INV-28 (terminal
//! offers and restricted parties refuse contact with no new disclosure),
//! INV-44 with AC-40 (deletion stops current interaction immediately:
//! public identity and new contact disappear, affected resources become
//! unavailable, and only purpose-limited retained evidence remains),
//! EC-01 (buyers: cancel unresolved requests with an account-deletion
//! reason, hide identity and content, invalidate live offers, stop
//! contact; sellers get an unavailable status),
//! EC-02 (sellers: invalidate offers with destination access removed at
//! once; buyers keep limited historical contact context and reporting
//! rights; externally learned numbers are never claimed recalled —
//! contact rows are never touched here),
//! EC-38 (deletion during an open case preserves the incident rows as-is
//! for policy review; nothing is hard-deleted on this path), and INV-37
//! with AC-39 (actor, reason, scope, time, and previous/resulting
//! standing recorded in a durable fact carrying the retention marker).
//!
//! Deletion is self-service for any non-deleted account (suspended owners
//! explicitly keep this path) or administrator-operated for others. Phone
//! material is destroyed and the lookup rotated so the number recycles
//! without exposing the previous owner's history; display names scrub to
//! a static label. Reports, contacts, blocks, and audit rows stay exactly
//! as they were.

use crate::application::staff_permissions::StaffError;
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::notices::{record as record_notice, NewNotice};

/// Reason bound in scalar values.
pub const DELETE_REASON_MAX_CHARS: usize = 1000;
/// Scrubbed public identity: no name, no number, no history hint.
pub const DELETED_DISPLAY_NAME: &str = "Deleted user";

/// One account deletion as supplied: whose interaction ends and why.
#[derive(Debug, Clone)]
pub struct DeleteAccountInput {
    /// Account to delete (self, or any account for administrators).
    pub user_id: uuid::Uuid,
    /// Why this account is deleted (recorded in the fact only).
    pub reason: String,
}

/// One deleted account: standing after the write.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedAccount {
    /// Deleted account.
    pub user_id: uuid::Uuid,
    /// `deleted`.
    pub state: String,
    /// False when the row already stood deleted (no new fact).
    pub deleted: bool,
    /// Unresolved owned requests cancelled by this write.
    pub cancelled_requests: i64,
    /// Live offers invalidated by this write (own plus related).
    pub invalidated_offers: i64,
    /// Retention marker: `incident-hold` with open cases, else `standard`.
    pub retention: String,
}

/// Typed deletion failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteAccountError {
    /// Bad reason text.
    InvalidField,
    /// The requester is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such account exists.
    NotFound,
    /// The caller deletes a foreign account without a live administrator grant.
    NotPermitted,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for DeleteAccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid deletion field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("deletion target not found"),
            Self::NotPermitted => f.write_str("deletion not permitted"),
            Self::StorageFailed => f.write_str("deletion storage failed"),
        }
    }
}

impl std::error::Error for DeleteAccountError {}

fn check_bounded(value: &str, max: usize) -> Result<(), DeleteAccountError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(DeleteAccountError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> DeleteAccountError {
    match error {
        StaffError::InvalidField => DeleteAccountError::InvalidField,
        StaffError::NotActive => DeleteAccountError::NotActive,
        StaffError::NotFound => DeleteAccountError::NotFound,
        StaffError::NotPermitted => DeleteAccountError::NotPermitted,
        StaffError::StorageFailed => DeleteAccountError::StorageFailed,
    }
}

/// Delete one account with its cascades: unresolved owned requests cancel
/// hidden with open cycles ended, live own and related offers invalidate,
/// sessions revoke, in-flight challenges consume, phone material is
/// destroyed with the lookup rotated, and the public identity scrubs to a
/// static label. Counterparties get an unavailable status; reports,
/// contacts, blocks, and audit rows stand byte-identical for policy
/// review. Repeating a live deletion converges with no new fact.
///
/// # Errors
///
/// Returns [`DeleteAccountError::InvalidField`] for bad reason text,
/// [`DeleteAccountError::NotActive`] for restricted requesters or deleted
/// targets, [`DeleteAccountError::NotFound`] for missing accounts,
/// [`DeleteAccountError::NotPermitted`] for foreign accounts without a
/// live administrator grant, else [`DeleteAccountError::StorageFailed`].
/// Reasons are static.
pub async fn delete_account(
    pool: &sqlx::PgPool,
    requester_id: uuid::Uuid,
    input: DeleteAccountInput,
) -> Result<DeletedAccount, DeleteAccountError> {
    check_bounded(&input.reason, DELETE_REASON_MAX_CHARS)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| DeleteAccountError::StorageFailed)?;
    // Requester standing: self-deletion stays open to every non-deleted
    // account (suspended owners explicitly keep it); foreign deletions
    // need a live administrator grant.
    let requester: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(requester_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| DeleteAccountError::StorageFailed)?;
    match requester {
        Some((_, None)) => {}
        _ => {
            tx.rollback()
                .await
                .map_err(|_| DeleteAccountError::StorageFailed)?;
            return Err(DeleteAccountError::NotActive);
        }
    }
    if requester_id != input.user_id {
        crate::application::staff_permissions::require_admin(pool, requester_id)
            .await
            .map_err(staff_error)?;
    }
    // Lock the target row first: concurrent deletions serialize here, and
    // every loser replays the winner's standing.
    let locked: Option<(String, Option<chrono::DateTime<chrono::Utc>>, String)> = sqlx::query_as(
        "SELECT state, deleted_at, phone_lookup FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(input.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    let (state, deleted_at, phone_lookup) = match locked {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| DeleteAccountError::StorageFailed)?;
            return Err(DeleteAccountError::NotFound);
        }
    };
    if deleted_at.is_some() {
        tx.rollback()
            .await
            .map_err(|_| DeleteAccountError::StorageFailed)?;
        return Ok(DeletedAccount {
            user_id: input.user_id,
            state,
            deleted: false,
            cancelled_requests: 0,
            invalidated_offers: 0,
            retention: "standard".to_owned(),
        });
    }
    // Buyer cascade: unresolved owned requests cancel hidden with open
    // cycles ended; related live offers on them invalidate.
    let cancelled: Vec<uuid::Uuid> = sqlx::query_scalar(
        "UPDATE requests SET state = 'cancelled', visibility = 'hidden', updated_at = now()
         WHERE author_id = $1 AND state IN ('active', 'suspended')
         RETURNING id",
    )
    .bind(input.user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    for request_id in &cancelled {
        sqlx::query(
            "UPDATE request_cycles SET ended_at = now()
             WHERE request_id = $1 AND ended_at IS NULL",
        )
        .bind(request_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| DeleteAccountError::StorageFailed)?;
    }
    let mut invalidated = 0i64;
    if !cancelled.is_empty() {
        invalidated += sqlx::query(
            "UPDATE offers SET state = 'invalidated', terminal_reason = 'deleted',
                    updated_at = now()
             WHERE request_id = ANY($1)
               AND state IN ('sent', 'viewed', 'contacted', 'suspended')",
        )
        .bind(&cancelled)
        .execute(&mut *tx)
        .await
        .map_err(|_| DeleteAccountError::StorageFailed)?
        .rows_affected() as i64;
    }
    // Seller cascade: own remaining live offers invalidate hidden.
    invalidated += sqlx::query(
        "UPDATE offers SET state = 'invalidated', terminal_reason = 'deleted',
                visibility = 'hidden', updated_at = now()
         WHERE seller_id = $1
           AND state IN ('sent', 'viewed', 'contacted', 'suspended')",
    )
    .bind(input.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?
    .rows_affected() as i64;
    // Sessions revoke and in-flight challenges consume: old cookies and
    // pending verifications die with the account.
    sqlx::query(
        "UPDATE sessions SET revoked_at = now()
         WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(input.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    sqlx::query(
        "UPDATE phone_challenges SET consumed_at = now()
         WHERE phone_lookup = $1 AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(&phone_lookup)
    .execute(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    // Identity scrub: static label, destroyed ciphertext, rotated lookup.
    // The partial unique index already excludes deleted rows, and the
    // rotation additionally separates any later recycled number from this
    // history.
    sqlx::query(
        "UPDATE users SET state = 'deleted', deleted_at = now(),
                display_name = $2, phone_ciphertext = '\\x',
                phone_lookup = 'deleted:' || id::text
         WHERE id = $1",
    )
    .bind(input.user_id)
    .bind(DELETED_DISPLAY_NAME)
    .execute(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    // Retention marker: open incident involvement holds the row for policy
    // review; everything else is standard. Nothing is hard-deleted here.
    let open_cases: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT reports.case_id) FROM reports
         JOIN report_cases ON report_cases.id = reports.case_id
         WHERE report_cases.status IN ('open', 'review')
           AND (reports.reporter_id = $1
             OR EXISTS (SELECT 1 FROM requests
                        WHERE requests.id = reports.target_id
                          AND reports.target_kind = 'request'
                          AND requests.author_id = $1)
             OR EXISTS (SELECT 1 FROM offers
                        WHERE offers.id = reports.target_id
                          AND reports.target_kind = 'offer'
                          AND offers.seller_id = $1))",
    )
    .bind(input.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    let retention = if open_cases > 0 {
        "incident-hold"
    } else {
        "standard"
    };
    let fact = record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(requester_id),
            resource_kind: "user",
            resource_id: input.user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "user.deleted",
            policy: "mvp-free",
            source: if requester_id == input.user_id {
                "api"
            } else {
                "staff"
            },
            payload: serde_json::json!({
                "reason": input.reason,
                "scope": input.user_id,
                "previous_state": state,
                "retention": retention,
                "open_cases": open_cases,
                "cancelled_requests": cancelled.len() as i64,
                "invalidated_offers": invalidated,
            }),
        },
    )
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    // Counterparties learn unavailability only: affected sellers and buyers
    // get one static notice each, with no reason, reporter, or phone.
    let counterparties: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT offers.seller_id FROM offers WHERE request_id = ANY($1)
         UNION
         SELECT DISTINCT requests.author_id FROM offers
         JOIN requests ON requests.id = offers.request_id
         WHERE offers.seller_id = $2 AND offers.terminal_reason = 'deleted'",
    )
    .bind(&cancelled)
    .bind(input.user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| DeleteAccountError::StorageFailed)?;
    for counterparty in counterparties
        .into_iter()
        .filter(|counterparty| *counterparty != input.user_id)
    {
        record_notice(
            &mut *tx,
            NewNotice {
                account_id: counterparty,
                kind: "offer.unavailable",
                resource_kind: "user",
                resource_id: input.user_id,
                event_id: fact.id,
                body: "An offer is unavailable; the other account is closed.".to_owned(),
            },
        )
        .await
        .map_err(|_| DeleteAccountError::StorageFailed)?;
    }
    tx.commit()
        .await
        .map_err(|_| DeleteAccountError::StorageFailed)?;
    Ok(DeletedAccount {
        user_id: input.user_id,
        state: "deleted".to_owned(),
        deleted: true,
        cancelled_requests: cancelled.len() as i64,
        invalidated_offers: invalidated,
        retention: retention.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_reason_has_bounds() {
        assert!(check_bounded("leaving the marketplace", DELETE_REASON_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", DELETE_REASON_MAX_CHARS),
            Err(DeleteAccountError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), DELETE_REASON_MAX_CHARS),
            Err(DeleteAccountError::InvalidField)
        );
        assert_eq!(DELETED_DISPLAY_NAME, "Deleted user");
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            DeleteAccountError::InvalidField,
            DeleteAccountError::NotActive,
            DeleteAccountError::NotFound,
            DeleteAccountError::NotPermitted,
            DeleteAccountError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
