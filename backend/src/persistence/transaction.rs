//! Bounded serializable transaction retry.
//!
//! Canonical rules: INV-34 (repeats duplicate nothing — a retried action
//! commits one fact) and EC-25 (a retried duplicate returns the existing
//! result; a later handoff counts only if still eligible). Mechanism follows
//! `docs/engineering/decisions/DEC-0003-transactions.md`: serializable
//! transactions for state-changing multi-resource operations, bounded retry of
//! genuinely retryable errors only, fresh eligibility on every attempt.
//!
//! [`run_serializable`] runs the caller's closure inside a `SERIALIZABLE`
//! transaction and retries only PostgreSQL serialization (`40001`) and
//! deadlock (`40P01`) failures. The bound is [`MAX_TRANSACTION_ATTEMPTS`]
//! total attempts (initial attempt plus at most three retries, per DEC-0003).
//! Every retry starts a fresh transaction; the caller must re-read the clock,
//! resource state, and quotas inside the closure instead of reusing a stale
//! read. The caller must also generate its stable action identity once outside
//! the closure and reuse it, so a retry cannot mint a second fact.
//!
//! Provider and network calls must stay outside the retry closure: an external
//! send inside [`run_serializable`] would repeat on retry. Call the provider
//! once after the transaction commits. Exhaustion surfaces explicitly as
//! [`TransactionError::RetryExhausted`]; it never returns a phantom success.

use std::future::Future;
use std::pin::Pin;

/// Total attempts including the initial try: 1 + at most 3 retries (DEC-0003).
pub const MAX_TRANSACTION_ATTEMPTS: u32 = 4;

/// SQLSTATE for PostgreSQL serialization failures (genuinely retryable).
pub const SERIALIZATION_FAILURE_SQLSTATE: &str = "40001";

/// SQLSTATE for PostgreSQL deadlock detection (genuinely retryable).
pub const DEADLOCK_SQLSTATE: &str = "40P01";

/// Per-attempt outcome from the caller's closure.
///
/// `Abort` carries a business or validation refusal and is never retried.
/// `Db` carries a storage failure and is retried only when
/// [`is_retryable_db_error`] holds.
#[derive(Debug)]
pub enum AttemptError<E> {
    /// Business refusal: do not retry, roll back, surface as-is.
    Abort(E),
    /// Storage failure: retry only when the SQLSTATE is retryable.
    Db(sqlx::Error),
}

/// Final transaction outcome. Static reasons only — no URLs or secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionError<E> {
    /// The operation refused: validation, ownership, state, or quota.
    Aborted(E),
    /// Every attempt hit a retryable conflict; nothing committed.
    RetryExhausted {
        /// Total attempts performed ([`MAX_TRANSACTION_ATTEMPTS`]).
        attempts: u32,
    },
    /// A non-retryable database failure; nothing committed.
    StorageFailed,
}

impl<E: std::fmt::Display> std::fmt::Display for TransactionError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Aborted(reason) => write!(f, "transaction refused: {reason}"),
            Self::RetryExhausted { attempts } => {
                write!(
                    f,
                    "transaction conflicts exhausted after {attempts} attempts"
                )
            }
            Self::StorageFailed => f.write_str("transaction storage failed"),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for TransactionError<E> {}

/// True only for genuinely retryable PostgreSQL conflicts.
///
/// Retryable: SQLSTATE `40001` (serialization failure) and `40P01`
/// (deadlock detected). Everything else — unique violations (`23505`),
/// undefined tables (`42P01`), check failures (`23514`), connection loss —
/// is not retried by this helper.
#[must_use]
pub fn is_retryable_db_error(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|database| matches!(database.code().as_deref(), Some("40001" | "40P01")))
}

/// Run `operation` inside a serializable transaction with bounded retry.
///
/// Each attempt begins a fresh transaction, sets `SERIALIZABLE` isolation as
/// its first statement, calls `operation` with the open transaction and the
/// zero-based attempt index, and commits on success. The closure must re-read
/// all eligibility inside every call and reuse the caller's stable action
/// identifier; it must not call external providers.
///
/// # Errors
///
/// Returns [`TransactionError::Aborted`] for business refusals (never retried),
/// [`TransactionError::RetryExhausted`] when every attempt hit a retryable
/// conflict, and [`TransactionError::StorageFailed`] for any non-retryable
/// database failure (begin, isolation, operation, or commit).
pub async fn run_serializable<T, E>(
    pool: &sqlx::PgPool,
    mut operation: impl for<'a, 'c> FnMut(
        &'a mut sqlx::Transaction<'c, sqlx::Postgres>,
        u32,
    ) -> Pin<
        Box<dyn Future<Output = Result<T, AttemptError<E>>> + Send + 'a>,
    >,
) -> Result<T, TransactionError<E>>
where
    T: Send,
    E: Send,
{
    let mut attempt: u32 = 0;
    loop {
        let mut tx = pool
            .begin()
            .await
            .map_err(|_| TransactionError::StorageFailed)?;
        if let Err(error) = sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            .execute(&mut *tx)
            .await
        {
            let retryable = is_retryable_db_error(&error);
            let _ = tx.rollback().await;
            if retryable {
                attempt += 1;
                if attempt < MAX_TRANSACTION_ATTEMPTS {
                    continue;
                }
                return Err(TransactionError::RetryExhausted {
                    attempts: MAX_TRANSACTION_ATTEMPTS,
                });
            }
            return Err(TransactionError::StorageFailed);
        }
        match operation(&mut tx, attempt).await {
            Ok(value) => match tx.commit().await {
                Ok(()) => return Ok(value),
                Err(error) => {
                    if is_retryable_db_error(&error) {
                        attempt += 1;
                        if attempt < MAX_TRANSACTION_ATTEMPTS {
                            continue;
                        }
                        return Err(TransactionError::RetryExhausted {
                            attempts: MAX_TRANSACTION_ATTEMPTS,
                        });
                    }
                    return Err(TransactionError::StorageFailed);
                }
            },
            Err(AttemptError::Abort(reason)) => {
                let _ = tx.rollback().await;
                return Err(TransactionError::Aborted(reason));
            }
            Err(AttemptError::Db(error)) => {
                let retryable = is_retryable_db_error(&error);
                let _ = tx.rollback().await;
                if retryable {
                    attempt += 1;
                    if attempt < MAX_TRANSACTION_ATTEMPTS {
                        continue;
                    }
                    return Err(TransactionError::RetryExhausted {
                        attempts: MAX_TRANSACTION_ATTEMPTS,
                    });
                }
                return Err(TransactionError::StorageFailed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_bound_is_documented() {
        assert_eq!(
            MAX_TRANSACTION_ATTEMPTS, 4,
            "initial attempt plus 3 retries"
        );
        assert_eq!(SERIALIZATION_FAILURE_SQLSTATE, "40001");
        assert_eq!(DEADLOCK_SQLSTATE, "40P01");
    }

    #[test]
    fn error_display_carries_no_values() {
        let exhausted: TransactionError<String> = TransactionError::RetryExhausted { attempts: 4 };
        let failed: TransactionError<String> = TransactionError::StorageFailed;
        let rendered = format!("{exhausted:?} {exhausted} {failed:?} {failed}");
        assert!(!rendered.contains("postgres://"));
        assert!(!rendered.contains("canary"));
    }
}
