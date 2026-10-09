//! Pair-block persistence: writes with distinct-user guarantees.
//!
//! Canonical rules: INV-35 (active blocks prevent new interactions in both
//! directions while preserving legitimate historical evidence — rows here
//! gate lookups only; contacts, offers, and facts are never rewritten by
//! block changes), AC-35 (either direction stops mutual discovery and new
//! offers/contact while history and reporting stay available), EC-23
//! (blocking disables new handoffs without touching standing history).
//!
//! The `user_blocks` table itself (distinct users, one direction each) is
//! authoritative: writers validate distinctness before the database
//! backstop confirms it, repeats converge idempotently, and removal lifts
//! exactly the named direction. Either-direction reads live in
//! `persistence::eligibility` alongside eligibility; this module owns
//! writes plus the directional fact. Relationship history stays private by
//! omission: no listing, no enumeration, no third-party exposure — a pair
//! check answers blocked-or-not and nothing else.

/// One block row: who blocked whom, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Blocking account.
    pub blocker_id: uuid::Uuid,
    /// Blocked account.
    pub blocked_id: uuid::Uuid,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Typed block-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// Blocker and blocked are the same account.
    SelfBlock,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for BlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SelfBlock => f.write_str("cannot block self"),
            Self::StorageFailed => f.write_str("block storage failed"),
        }
    }
}

impl std::error::Error for BlockError {}

fn read_block(row: &sqlx::postgres::PgRow) -> Result<Block, BlockError> {
    use sqlx::Row;
    Ok(Block {
        id: row.try_get("id").map_err(|_| BlockError::StorageFailed)?,
        blocker_id: row
            .try_get("blocker_id")
            .map_err(|_| BlockError::StorageFailed)?,
        blocked_id: row
            .try_get("blocked_id")
            .map_err(|_| BlockError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| BlockError::StorageFailed)?,
    })
}

/// Record one directional block, idempotently: a repeated block returns
/// its standing row with `created` false and no duplicate.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`BlockError::SelfBlock`] for identical accounts, else
/// [`BlockError::StorageFailed`]. Reasons are static. Missing accounts
/// surface as storage failures through the foreign keys.
pub async fn block(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    blocker_id: uuid::Uuid,
    blocked_id: uuid::Uuid,
) -> Result<(Block, bool), BlockError> {
    if blocker_id == blocked_id {
        return Err(BlockError::SelfBlock);
    }
    // `ON CONFLICT DO NOTHING` keeps the transaction healthy where a
    // caught unique violation would abort it; the standing row below
    // resolves whichever twin lost without failing the repeat.
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        "INSERT INTO user_blocks (blocker_id, blocked_id)
         VALUES ($1, $2)
         ON CONFLICT (blocker_id, blocked_id) DO NOTHING
         RETURNING id, blocker_id, blocked_id, created_at",
    )
    .bind(blocker_id)
    .bind(blocked_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| BlockError::StorageFailed)?;
    match row {
        Some(row) => Ok((read_block(&row)?, true)),
        None => {
            let standing: Option<sqlx::postgres::PgRow> = sqlx::query(
                "SELECT id, blocker_id, blocked_id, created_at FROM user_blocks
                 WHERE blocker_id = $1 AND blocked_id = $2",
            )
            .bind(blocker_id)
            .bind(blocked_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| BlockError::StorageFailed)?;
            standing
                .map(|row| read_block(&row).map(|block| (block, false)))
                .transpose()?
                .ok_or(BlockError::StorageFailed)
        }
    }
}

/// Lift one directional block, idempotently: returns whether a row existed.
/// Lifting never resurrects offers, contacts, or facts — those keep
/// whatever terminal state they reached while blocked.
///
/// # Errors
///
/// Returns [`BlockError::StorageFailed`] on database failure only.
pub async fn unblock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    blocker_id: uuid::Uuid,
    blocked_id: uuid::Uuid,
) -> Result<bool, BlockError> {
    let removed = sqlx::query("DELETE FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
        .bind(blocker_id)
        .bind(blocked_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| BlockError::StorageFailed)?;
    Ok(removed.rows_affected() == 1)
}

/// One directional block, if it exists. The only per-pair read here: full
/// enumeration does not exist by design.
///
/// # Errors
///
/// Returns [`BlockError::StorageFailed`] on database failure only.
pub async fn is_blocked<'e, E>(
    executor: E,
    blocker_id: uuid::Uuid,
    blocked_id: uuid::Uuid,
) -> Result<bool, BlockError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
            .bind(blocker_id)
            .bind(blocked_id)
            .fetch_optional(executor)
            .await
            .map_err(|_| BlockError::StorageFailed)?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        for error in [BlockError::SelfBlock, BlockError::StorageFailed] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
