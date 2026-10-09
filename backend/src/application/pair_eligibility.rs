//! Pair interaction guard: live eligibility plus active blocks.
//!
//! Canonical rules: INV-05 (no role input — two account identifiers in,
//! one verdict out), INV-28 with INV-35 and AC-35 (both parties eligible
//! with no either-direction block, evaluated live — an ineligible party
//! never learns block records, and a blocked pair refuses without naming
//! a direction).
//!
//! Every check runs in the caller's transaction under the DEC-0003
//! [`AttemptError`] contract, so discovery, offer, and contact writers
//! serialize check-then-act instead of racing a fresh block. Cached or
//! client-supplied relationship state has no parameter here by design: the
//! database truth is the only input besides the two identifiers.

use crate::persistence::transaction::AttemptError;

/// Typed pair-guard failure. Static reasons only; wire mapping belongs to
/// the route-owning card. Refusals name no direction and no third party.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairBlockError {
    /// Either account is missing, deleted, or not `active` (undistinguished).
    NotActive,
    /// An either-direction block relates the pair.
    Blocked,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for PairBlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::Blocked => f.write_str("relationship is blocked"),
            Self::StorageFailed => f.write_str("pair lookup failed"),
        }
    }
}

impl std::error::Error for PairBlockError {}

/// Refuse a new pair interaction unless both accounts are live and no
/// either-direction block relates them, evaluated live in the caller's
/// transaction. Account state precedes relationships, so a restricted
/// party never probes block records.
///
/// # Errors
///
/// Returns [`AttemptError::Abort`] with [`PairBlockError::NotActive`] or
/// [`PairBlockError::Blocked`] as business refusals (never retried), and
/// [`AttemptError::Db`] on database failure (retried only for genuine
/// `40001`/`40P01` conflicts).
pub async fn check_pair_interaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    first_id: uuid::Uuid,
    second_id: uuid::Uuid,
) -> Result<(), AttemptError<PairBlockError>> {
    for account_id in [first_id, second_id] {
        let standing: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
            sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
                .bind(account_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(AttemptError::Db)?;
        match standing {
            Some((state, None)) if state == "active" => {}
            _ => return Err(AttemptError::Abort(PairBlockError::NotActive)),
        }
    }
    // Queried inline (rather than through the shared reader) so genuine
    // storage conflicts keep their retryable identity instead of
    // flattening into a refusal.
    let blocked: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM user_blocks
         WHERE (blocker_id = $1 AND blocked_id = $2)
            OR (blocker_id = $2 AND blocked_id = $1)",
    )
    .bind(first_id)
    .bind(second_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    if blocked.is_some() {
        return Err(AttemptError::Abort(PairBlockError::Blocked));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        for error in [
            PairBlockError::NotActive,
            PairBlockError::Blocked,
            PairBlockError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
