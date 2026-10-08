//! Protected phone-change history: keyed lookups, never destinations.
//!
//! Canonical rules: INV-07 (current numbers stay unique — history never
//! reassigns anything) and EC-21/EC-22 (future handoffs use the new verified
//! destination while historical context keeps the destination valid at that
//! time; a recycled number's new subscriber never inspects a previous
//! owner's records).
//!
//! History lives in `business_events`, not in a new table: every reassignment
//! appends one `account.phone_changed` fact carrying the old and new keyed
//! HMAC-SHA256 lookups. Lookups are opaque without the server-side lookup key,
//! so the ledger is safe to retain for review and recovery while destinations
//! themselves stay encrypted in exactly one live row. Readers here serve
//! recovery review and audit; they change nothing.

/// One recorded reassignment: lookup digests plus the instant they committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneHistoryEntry {
    /// When the reassignment committed (database clock).
    pub changed_at: chrono::DateTime<chrono::Utc>,
    /// Keyed lookup of the previous destination.
    pub old_lookup: String,
    /// Keyed lookup of the new destination.
    pub new_lookup: String,
}

/// Typed history-read failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryError {
    /// The lookup failed or a fact is malformed.
    StorageFailed,
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StorageFailed => f.write_str("phone history unavailable"),
        }
    }
}

impl std::error::Error for HistoryError {}

/// Read one account's reassignment history, oldest first.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`HistoryError::StorageFailed`] on database failure or a malformed
/// fact. Reasons are static; lookups are digests, never destinations.
pub async fn history_for<'e, E>(
    executor: E,
    user_id: uuid::Uuid,
) -> Result<Vec<PhoneHistoryEntry>, HistoryError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT occurred_at, payload FROM business_events
          WHERE resource_kind = 'account' AND resource_id = $1
            AND kind = 'account.phone_changed'
          ORDER BY occurred_at",
    )
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(|_| HistoryError::StorageFailed)?;
    use sqlx::Row;
    rows.iter()
        .map(|row| {
            let changed_at: chrono::DateTime<chrono::Utc> = row
                .try_get("occurred_at")
                .map_err(|_| HistoryError::StorageFailed)?;
            let payload: serde_json::Value = row
                .try_get("payload")
                .map_err(|_| HistoryError::StorageFailed)?;
            let old_lookup = payload
                .get("old_lookup")
                .and_then(serde_json::Value::as_str)
                .ok_or(HistoryError::StorageFailed)?
                .to_owned();
            let new_lookup = payload
                .get("new_lookup")
                .and_then(serde_json::Value::as_str)
                .ok_or(HistoryError::StorageFailed)?
                .to_owned();
            Ok(PhoneHistoryEntry {
                changed_at,
                old_lookup,
                new_lookup,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        let rendered = format!(
            "{:?} {}",
            HistoryError::StorageFailed,
            HistoryError::StorageFailed
        );
        assert!(!rendered.contains("canary"));
        assert!(!rendered.contains("+55"));
    }
}
