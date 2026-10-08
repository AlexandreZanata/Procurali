//! Guard reads over the real identity schema: account standing and blocks.
//!
//! These readers serve the request/offer/contact writers of later phases
//! (INV-28 contact needs both parties eligible with no block, INV-35 active
//! blocks stop new interactions in both directions, AC-25 suspension prevents
//! contact, AC-35 either-direction blocks stop discovery and new offers while
//! history stays available). Reads are current-state only: nothing here
//! caches, and nothing overrides a denial.
//!
//! Block rows themselves arrive through moderation commands in P10; until
//! then, tests seed synthetic rows directly (the ledger is real either way).

/// One account's current standing for guard decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStanding {
    /// Account identifier.
    pub user_id: uuid::Uuid,
    /// Lifecycle state (`pending`, `active`, `suspended`, `banned`, `deleted`).
    pub state: String,
    /// Whether the account is deleted.
    pub deleted: bool,
}

/// Typed guard-read failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EligibilityDbError {
    /// The lookup failed (connection, transaction state).
    StorageFailed,
}

impl std::fmt::Display for EligibilityDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("eligibility lookup failed")
    }
}

impl std::error::Error for EligibilityDbError {}

/// Read one account's standing, if the row exists.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`EligibilityDbError::StorageFailed`] on database failure only.
/// Missing accounts answer `None` (callers refuse without distinguishing).
pub async fn account_standing<'e, E>(
    executor: E,
    user_id: uuid::Uuid,
) -> Result<Option<AccountStanding>, EligibilityDbError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> =
        sqlx::query("SELECT id, state, deleted_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(executor)
            .await
            .map_err(|_| EligibilityDbError::StorageFailed)?;
    row.map(|row| {
        use sqlx::Row;
        let deleted_at: Option<chrono::DateTime<chrono::Utc>> = row
            .try_get("deleted_at")
            .map_err(|_| EligibilityDbError::StorageFailed)?;
        Ok(AccountStanding {
            user_id: row
                .try_get("id")
                .map_err(|_| EligibilityDbError::StorageFailed)?,
            state: row
                .try_get("state")
                .map_err(|_| EligibilityDbError::StorageFailed)?,
            deleted: deleted_at.is_some(),
        })
    })
    .transpose()
}

/// True when an active block relates `first` and `second` in either direction
/// (AC-35, INV-35). One directed row blocks the pair both ways.
///
/// # Errors
///
/// Returns [`EligibilityDbError::StorageFailed`] on database failure only.
pub async fn blocked_either_direction<'e, E>(
    executor: E,
    first: uuid::Uuid,
    second: uuid::Uuid,
) -> Result<bool, EligibilityDbError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM user_blocks
          WHERE (blocker_id = $1 AND blocked_id = $2)
             OR (blocker_id = $2 AND blocked_id = $1) LIMIT 1",
    )
    .bind(first)
    .bind(second)
    .fetch_optional(executor)
    .await
    .map_err(|_| EligibilityDbError::StorageFailed)?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        let rendered = format!(
            "{:?} {}",
            EligibilityDbError::StorageFailed,
            EligibilityDbError::StorageFailed
        );
        assert!(!rendered.contains("canary"));
    }
}
