//! Distinct contact allowance: useful comparison without bulk collection.
//!
//! Canonical rules: INV-23 with AC-27 and EC-12 (contacts never reserve
//! inventory or label a winning sale — this module counts distinct sellers
//! only, and nothing here writes outcomes, reservations, or attributions),
//! INV-28 (allowance applies to eligible demand — counts derive from live
//! rows in the caller's transaction), EC-13 (independent offers stay
//! independently contactable — per-request-cycle counts never interact).
//!
//! Ten distinct sellers per request cycle. Repeats to an already contacted
//! seller consume no second slot; failed handoffs record nothing and
//! consume nothing. Every read runs in the caller's transaction under the
//! DEC-0003 [`AttemptError`] contract, so the handoff writer serializes
//! check-then-record instead of overcounting.

use crate::persistence::transaction::AttemptError;

/// Distinct sellers contacted per request cycle on the free MVP.
pub const MAX_DISTINCT_CONTACTS_PER_CYCLE: u32 = 10;

/// Typed contact-allowance failure. Static reasons only; wire mapping
/// belongs to the route-owning card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactLimitError {
    /// Ten distinct sellers are already contacted on this cycle, and this
    /// seller is not among them.
    AllowanceExhausted,
    /// The buyer account is missing, deleted, or not `active`.
    NotActive,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for ContactLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllowanceExhausted => f.write_str("contact allowance exhausted"),
            Self::NotActive => f.write_str("account is not active"),
            Self::StorageFailed => f.write_str("allowance lookup failed"),
        }
    }
}

impl std::error::Error for ContactLimitError {}

/// Distinct sellers contacted on one request cycle: repeats collapse to
/// one, failures never land, so the count is exact by construction.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure (retryable only for
/// genuine conflicts) and [`AttemptError::Abort`] wrapping
/// [`ContactLimitError::StorageFailed`] for an impossible negative count.
pub async fn distinct_contact_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    cycle_number: i32,
) -> Result<u32, AttemptError<ContactLimitError>> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT seller_id) FROM contacts
         WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(request_id)
    .bind(cycle_number)
    .fetch_one(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    u32::try_from(count).map_err(|_| AttemptError::Abort(ContactLimitError::StorageFailed))
}

/// Whether this seller already stands contacted on this cycle: repeats
/// consume no second slot, whatever the allowance reads.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure only.
pub async fn seller_contacted(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    cycle_number: i32,
    seller_id: uuid::Uuid,
) -> Result<bool, AttemptError<ContactLimitError>> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM contacts
         WHERE request_id = $1 AND cycle_number = $2 AND seller_id = $3",
    )
    .bind(request_id)
    .bind(cycle_number)
    .bind(seller_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    Ok(found.is_some())
}

/// Refuse a handoff that would exceed the distinct-seller allowance,
/// evaluated live in the caller's transaction. Already-contacted sellers
/// always pass; restricted buyers are refused without distinguishing
/// states.
///
/// # Errors
///
/// Returns [`AttemptError::Abort`] with
/// [`ContactLimitError::AllowanceExhausted`] or
/// [`ContactLimitError::NotActive`] as business refusals (never retried),
/// and [`AttemptError::Db`] on database failure (retried only for genuine
/// `40001`/`40P01` conflicts).
pub async fn check_contact_limits(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    cycle_number: i32,
    buyer_id: uuid::Uuid,
    seller_id: uuid::Uuid,
) -> Result<(), AttemptError<ContactLimitError>> {
    let standing: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(buyer_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AttemptError::Db)?;
    match standing {
        Some((state, None)) if state == "active" => {}
        _ => return Err(AttemptError::Abort(ContactLimitError::NotActive)),
    }
    if seller_contacted(tx, request_id, cycle_number, seller_id).await? {
        return Ok(());
    }
    if distinct_contact_count(tx, request_id, cycle_number).await?
        >= MAX_DISTINCT_CONTACTS_PER_CYCLE
    {
        return Err(AttemptError::Abort(ContactLimitError::AllowanceExhausted));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotas_match_the_policy_defaults() {
        assert_eq!(MAX_DISTINCT_CONTACTS_PER_CYCLE, 10);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ContactLimitError::AllowanceExhausted,
            ContactLimitError::NotActive,
            ContactLimitError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
