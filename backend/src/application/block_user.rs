//! Bilateral block operations: restrict futures, preserve history.
//!
//! Canonical rules: INV-25 with AC-35 and EC-23 (either direction stops
//! mutual discovery and new offers/contact — live pair offers invalidate
//! here while history and reporting stay available), INV-35 (active blocks
//! prevent new interactions both ways without rewriting evidence),
//! INV-38 with EC-24 and AC-36 (unblock removes only the relationship
//! restriction — previously invalidated offers stay terminal, and future
//! eligible activity follows normal cycle and allowance rules).
//!
//! Only the blocker acts, on themselves, with no reason exchanged: the
//! body names the blocked account and nothing else. Blocking invalidates
//! every live offer between the pair in either direction and records one
//! `relationship.blocked` fact; repeats return the standing block with no
//! new fact. Unblocking lifts exactly the named direction with one
//! `relationship.unblocked` fact and revives nothing.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::persistence::blocks::{block, unblock, Block};
use crate::persistence::events::{record as record_event, NewEvent};
use serde_json::json;

/// One block outcome: the standing row plus what this call changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockResult {
    /// The standing block row (new or pre-existing).
    pub block: Block,
    /// True when this call created the row.
    pub created: bool,
    /// Live pair offers invalidated by this call (zero on repeats).
    pub invalidated_offers: u64,
}

/// One unblock outcome: whether a row was lifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnblockResult {
    /// True when a block row existed and was removed.
    pub removed: bool,
}

/// Typed block-action failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockActionError {
    /// Blocker and blocked are the same account.
    SelfBlock,
    /// The caller account is missing, deleted, or not `active`.
    NotActive,
    /// No such blocked account exists.
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for BlockActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SelfBlock => f.write_str("cannot block self"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("account not found"),
            Self::StorageFailed => f.write_str("block storage failed"),
        }
    }
}

impl std::error::Error for BlockActionError {}

/// Block one account for the authenticated caller: record the directional
/// row (idempotently), invalidate every live offer between the pair in
/// either direction, and record one `relationship.blocked` fact — all
/// atomically. History, contacts, facts, and other pairs stand untouched.
///
/// # Errors
///
/// Returns [`BlockActionError::SelfBlock`] for identical accounts,
/// [`BlockActionError::NotActive`] for restricted callers,
/// [`BlockActionError::NotFound`] for missing blocked accounts, else
/// [`BlockActionError::StorageFailed`].
pub async fn block_user(
    pool: &sqlx::PgPool,
    blocker_id: uuid::Uuid,
    blocked_id: uuid::Uuid,
) -> Result<BlockResult, BlockActionError> {
    if blocker_id == blocked_id {
        return Err(BlockActionError::SelfBlock);
    }
    match check_actor(pool, blocker_id)
        .await
        .map_err(|_| BlockActionError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(BlockActionError::NotActive),
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| BlockActionError::StorageFailed)?;
    let target: Option<i32> = sqlx::query_scalar("SELECT 1 FROM users WHERE id = $1")
        .bind(blocked_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| BlockActionError::StorageFailed)?;
    if target.is_none() {
        tx.rollback()
            .await
            .map_err(|_| BlockActionError::StorageFailed)?;
        return Err(BlockActionError::NotFound);
    }
    let (row, created) = block(&mut tx, blocker_id, blocked_id)
        .await
        .map_err(|_| BlockActionError::StorageFailed)?;
    let mut invalidated_offers = 0;
    if created {
        invalidated_offers = sqlx::query(
            "UPDATE offers SET state = 'invalidated', terminal_reason = 'blocked',
                    updated_at = now()
             FROM requests
             WHERE offers.request_id = requests.id
               AND ((offers.seller_id = $1 AND requests.author_id = $2)
                 OR (offers.seller_id = $2 AND requests.author_id = $1))
               AND offers.state IN ('sent', 'viewed', 'contacted', 'suspended')",
        )
        .bind(blocker_id)
        .bind(blocked_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| BlockActionError::StorageFailed)?
        .rows_affected();
        record_event(
            &mut *tx,
            NewEvent {
                actor_id: Some(blocker_id),
                resource_kind: "relationship",
                resource_id: row.id,
                cycle: None,
                revision: None,
                effective_at: chrono::Utc::now(),
                kind: "relationship.blocked",
                policy: "mvp-free",
                source: "api",
                payload: json!({"state": "blocked"}),
            },
        )
        .await
        .map_err(|_| BlockActionError::StorageFailed)?;
    }
    tx.commit()
        .await
        .map_err(|_| BlockActionError::StorageFailed)?;
    Ok(BlockResult {
        block: row,
        created,
        invalidated_offers,
    })
}

/// Lift one directional block for the authenticated caller: remove exactly
/// the named row with one `relationship.unblocked` fact. Nothing else
/// moves — invalidated offers stay terminal, and future eligible activity
/// follows normal cycle and allowance rules.
///
/// # Errors
///
/// Returns [`BlockActionError::NotActive`] for restricted callers, else
/// [`BlockActionError::StorageFailed`]. Lifting a non-existent row
/// succeeds with `removed` false and no fact.
pub async fn unblock_user(
    pool: &sqlx::PgPool,
    blocker_id: uuid::Uuid,
    blocked_id: uuid::Uuid,
) -> Result<UnblockResult, BlockActionError> {
    match check_actor(pool, blocker_id)
        .await
        .map_err(|_| BlockActionError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(BlockActionError::NotActive),
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| BlockActionError::StorageFailed)?;
    if !unblock(&mut tx, blocker_id, blocked_id)
        .await
        .map_err(|_| BlockActionError::StorageFailed)?
    {
        tx.rollback()
            .await
            .map_err(|_| BlockActionError::StorageFailed)?;
        return Ok(UnblockResult { removed: false });
    }
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(blocker_id),
            resource_kind: "relationship",
            resource_id: blocked_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "relationship.unblocked",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "unblocked"}),
        },
    )
    .await
    .map_err(|_| BlockActionError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| BlockActionError::StorageFailed)?;
    Ok(UnblockResult { removed: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        for error in [
            BlockActionError::SelfBlock,
            BlockActionError::NotActive,
            BlockActionError::NotFound,
            BlockActionError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
