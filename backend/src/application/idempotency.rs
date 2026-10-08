//! Durable action idempotency: one committed result per action.
//!
//! Canonical rules: INV-34 (repeated execution of one action duplicates
//! nothing) and EC-25 (a retried duplicate returns the existing business
//! result; a genuinely later handoff counts only if still eligible).
//! Mechanism follows `docs/engineering/decisions/DEC-0003-transactions.md`
//! and `contracts/idempotency-cases.json`: the scope is
//! (actor, operation, key, input digest).
//!
//! [`claim`] runs inside the caller's open transaction, so the mutation, its
//! events, and its idempotency record commit as one unit. A rolled-back attempt reserves nothing: the record
//! vanishes with its transaction, leaving the key reusable. Same key with
//! the same digest replays the stored private result reference; same key with
//! a different digest is refused as [`IdempotencyError::Conflict`] (stable
//! code `idempotency_key_reuse`) and stores nothing.
//!
//! Sensitive handoffs store a private result reference (the committed business
//! event identifier), never a reusable phone destination. A replayed reference
//! is not permission: the caller must revalidate full current eligibility
//! (INV-28, INV-38) before responding, and must never re-serve a cached
//! destination after a block, suspension, deletion, withdrawal, or expiry.

/// Maximum operation name length in scalar values (mirrors the database backstop).
pub const IDEMPOTENCY_OPERATION_MAX_CHARS: usize = 64;

/// Maximum idempotency key length in scalar values (mirrors the database backstop).
pub const IDEMPOTENCY_KEY_MAX_CHARS: usize = 128;

/// Maximum input digest length in scalar values (mirrors the database backstop).
pub const IDEMPOTENCY_DIGEST_MAX_CHARS: usize = 128;

/// A new idempotency record for one action's committed result.
#[derive(Debug, Clone)]
pub struct NewClaim {
    /// Acting account scoping the key. Keys never cross actors.
    pub actor_id: uuid::Uuid,
    /// Business operation (`publish_request`, `start_contact`, ...).
    pub operation: &'static str,
    /// Caller-supplied key for this action (stable across retries).
    pub key: String,
    /// Caller-supplied digest of the action input (stable across retries).
    /// The module compares digests for equality only; it never interprets
    /// business input.
    pub input_digest: String,
    /// Committed business event for this action, recorded in the same
    /// transaction. A private reference, never a phone destination.
    pub result_event: uuid::Uuid,
}

/// A persisted idempotency record.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedClaim {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Acting account scoping the key.
    pub actor_id: uuid::Uuid,
    /// Business operation.
    pub operation: String,
    /// Action key.
    pub key: String,
    /// Input digest as supplied.
    pub input_digest: String,
    /// Committed business event referenced by this record.
    pub result_event: uuid::Uuid,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Outcome of [`claim`]: a fresh record or the pre-existing one.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimOutcome {
    /// This call committed the record.
    Created(CompletedClaim),
    /// The record already existed with the same digest.
    Replayed(CompletedClaim),
}

impl ClaimOutcome {
    /// The record behind either outcome.
    #[must_use]
    pub fn record(&self) -> &CompletedClaim {
        match self {
            Self::Created(record) | Self::Replayed(record) => record,
        }
    }
}

/// Typed idempotency failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyError {
    /// The operation name is empty or exceeds its bound.
    InvalidOperation,
    /// The key is empty or exceeds its bound.
    InvalidKey,
    /// The digest is empty or exceeds its bound.
    InvalidDigest,
    /// This key already exists with a different input digest.
    Conflict,
    /// The lookup or insert failed (missing event, connection, transaction state).
    StorageFailed,
}

impl IdempotencyError {
    /// Stable machine-readable code. Only [`Self::Conflict`] carries the
    /// contract refusal `idempotency_key_reuse`.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidOperation | Self::InvalidKey | Self::InvalidDigest => {
                "invalid_idempotency_key"
            }
            Self::Conflict => "idempotency_key_reuse",
            Self::StorageFailed => "idempotency_storage_failed",
        }
    }
}

impl std::fmt::Display for IdempotencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOperation => f.write_str("invalid idempotency operation"),
            Self::InvalidKey => f.write_str("invalid idempotency key"),
            Self::InvalidDigest => f.write_str("invalid idempotency digest"),
            Self::Conflict => f.write_str(
                "idempotency key already used with different input (idempotency_key_reuse)",
            ),
            Self::StorageFailed => f.write_str("idempotency storage failed"),
        }
    }
}

impl std::error::Error for IdempotencyError {}

fn read_record(row: &sqlx::postgres::PgRow) -> Result<CompletedClaim, IdempotencyError> {
    use sqlx::Row;
    Ok(CompletedClaim {
        id: row
            .try_get("id")
            .map_err(|_| IdempotencyError::StorageFailed)?,
        actor_id: row
            .try_get("actor_id")
            .map_err(|_| IdempotencyError::StorageFailed)?,
        operation: row
            .try_get("operation")
            .map_err(|_| IdempotencyError::StorageFailed)?,
        key: row
            .try_get("key")
            .map_err(|_| IdempotencyError::StorageFailed)?,
        input_digest: row
            .try_get("input_digest")
            .map_err(|_| IdempotencyError::StorageFailed)?,
        result_event: row
            .try_get("result_event")
            .map_err(|_| IdempotencyError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| IdempotencyError::StorageFailed)?,
    })
}

/// Find the record for one (actor, operation, key), if any.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`IdempotencyError::StorageFailed`] on database failure only.
/// Reasons are static.
pub async fn find<'e, E>(
    executor: E,
    actor_id: uuid::Uuid,
    operation: &str,
    key: &str,
) -> Result<Option<CompletedClaim>, IdempotencyError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        r#"SELECT id, actor_id, operation, "key" AS key, input_digest,
                  result_event, created_at
           FROM idempotency_records
           WHERE actor_id = $1 AND operation = $2 AND "key" = $3"#,
    )
    .bind(actor_id)
    .bind(operation)
    .bind(key)
    .fetch_optional(executor)
    .await
    .map_err(|_| IdempotencyError::StorageFailed)?;
    row.map(|row| read_record(&row)).transpose()
}

/// Claim one action key inside the caller's open transaction.
///
/// The record commits atomically with the caller's mutation and events: pass
/// the same transaction used for those writes. On the first call the record
/// inserts and [`ClaimOutcome::Created`] returns. A repeat with the same
/// digest returns [`ClaimOutcome::Replayed`] with the stored private result
/// reference. A repeat with a different digest returns
/// [`IdempotencyError::Conflict`] and stores nothing.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`IdempotencyError::InvalidOperation`], [`IdempotencyError::InvalidKey`],
/// or [`IdempotencyError::InvalidDigest`] before any insert,
/// [`IdempotencyError::Conflict`] when the key exists with a different digest,
/// else [`IdempotencyError::StorageFailed`]. Reasons are static.
pub async fn claim(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: NewClaim,
) -> Result<ClaimOutcome, IdempotencyError> {
    if claim.operation.is_empty()
        || claim.operation.chars().count() > IDEMPOTENCY_OPERATION_MAX_CHARS
    {
        return Err(IdempotencyError::InvalidOperation);
    }
    if claim.key.is_empty() || claim.key.chars().count() > IDEMPOTENCY_KEY_MAX_CHARS {
        return Err(IdempotencyError::InvalidKey);
    }
    if claim.input_digest.is_empty()
        || claim.input_digest.chars().count() > IDEMPOTENCY_DIGEST_MAX_CHARS
    {
        return Err(IdempotencyError::InvalidDigest);
    }
    // `ON CONFLICT DO NOTHING`: a plain failed INSERT would abort the whole
    // transaction (25P02), poisoning every later statement. The upsert keeps
    // the transaction healthy; the winning record resolves with a plain SELECT.
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        r#"INSERT INTO idempotency_records
            (actor_id, operation, "key", input_digest, result_event)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (actor_id, operation, "key") DO NOTHING
           RETURNING id, actor_id, operation, "key" AS key, input_digest,
                     result_event, created_at"#,
    )
    .bind(claim.actor_id)
    .bind(claim.operation)
    .bind(claim.key.clone())
    .bind(claim.input_digest.clone())
    .bind(claim.result_event)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| IdempotencyError::StorageFailed)?;
    match row {
        Some(row) => Ok(ClaimOutcome::Created(read_record(&row)?)),
        None => {
            match find(&mut **tx, claim.actor_id, claim.operation, &claim.key).await {
                Ok(Some(existing)) if existing.input_digest == claim.input_digest => {
                    Ok(ClaimOutcome::Replayed(existing))
                }
                Ok(Some(_)) => Err(IdempotencyError::Conflict),
                // The winner committed after this snapshot started (concurrent
                // transactions): its row is invisible here. The required
                // serializable caller pattern (DEC-0003) surfaces this race as
                // a 40001 serialization failure with automatic fresh retry,
                // which then replays. Under weaker isolation this surfaces as
                // a storage failure — never a false success or false conflict —
                // and the caller repeats the whole action in a fresh
                // transaction, which replays the winner.
                Ok(None) | Err(_) => Err(IdempotencyError::StorageFailed),
            }
        }
    }
}
