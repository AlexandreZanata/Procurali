//! Durable business events: one atomic fact per mutation.
//!
//! Canonical rules: INV-24 (term/contact history is append-only — events are
//! inserted, never updated or deleted through this module), INV-34 (repeats
//! duplicate nothing — the same fact is never recorded twice for one action).
//!
//! [`record`] takes any SQLx executor, so application operations pass their
//! open transaction and the mutation, its events, and its notices commit as
//! one unit. Payloads are bounded non-sensitive JSON; error values are static.

/// Maximum payload size in bytes (mirrors the database backstop).
pub const PAYLOAD_MAX_BYTES: usize = 8192;

/// A new business fact: actor, resource, cycle/revision context, effective
/// time, kind, policy, source, and a bounded non-sensitive payload.
#[derive(Debug, Clone)]
pub struct NewEvent {
    /// Acting account, if any (system-originated events use `None`).
    pub actor_id: Option<uuid::Uuid>,
    /// Resource family (`request`, `offer`, `contact`, `outcome`, ...).
    pub resource_kind: &'static str,
    /// Affected resource identifier.
    pub resource_id: uuid::Uuid,
    /// Request cycle context, when the fact is cycle-scoped.
    pub cycle: Option<i32>,
    /// Request revision context, when the fact is revision-scoped.
    pub revision: Option<uuid::Uuid>,
    /// Business effective time (from the operation clock, not arrival time).
    pub effective_at: chrono::DateTime<chrono::Utc>,
    /// Event kind (`request.published`, `offer.withdrawn`, ...).
    pub kind: &'static str,
    /// Policy set applied (`mvp-free` unless stated otherwise).
    pub policy: &'static str,
    /// Origin (`api`, `worker`, `staff`).
    pub source: &'static str,
    /// Bounded non-sensitive detail. Never phones, addresses, or secrets.
    pub payload: serde_json::Value,
}

/// A persisted business fact.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Recording instant (database clock).
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    /// Acting account, if any.
    pub actor_id: Option<uuid::Uuid>,
    /// Resource family.
    pub resource_kind: String,
    /// Affected resource identifier.
    pub resource_id: uuid::Uuid,
    /// Request cycle context, if any.
    pub cycle: Option<i32>,
    /// Request revision context, if any.
    pub revision: Option<uuid::Uuid>,
    /// Business effective time.
    pub effective_at: chrono::DateTime<chrono::Utc>,
    /// Event kind.
    pub kind: String,
    /// Policy set applied.
    pub policy: String,
    /// Origin.
    pub source: String,
    /// Stored payload, exactly as supplied.
    pub payload: serde_json::Value,
}

/// Typed event-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    /// Payload exceeds the byte bound.
    PayloadTooLarge,
    /// The insert failed (constraint, connection, or transaction state).
    StorageFailed,
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::PayloadTooLarge => "event payload exceeds its bound",
            Self::StorageFailed => "event storage failed",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for EventError {}

/// Record one event through the caller's executor (transaction or pool).
///
/// Runtime-checked SQL keeps `cargo build` offline-capable: no database is
/// needed at compile time. Type shapes are asserted by integration tests.
///
/// # Errors
///
/// Returns [`EventError::PayloadTooLarge`] without touching the database when
/// the payload exceeds its bound, else [`EventError::StorageFailed`] on any
/// insert failure. Reasons are static; no input values are rendered.
pub async fn record<'e, E>(executor: E, event: NewEvent) -> Result<Event, EventError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    if event.payload.to_string().len() > PAYLOAD_MAX_BYTES {
        return Err(EventError::PayloadTooLarge);
    }
    let row = sqlx::query(
        r#"INSERT INTO business_events
            (actor_id, resource_kind, resource_id, cycle, revision,
             effective_at, kind, policy, source, payload)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
           RETURNING id, occurred_at, actor_id, resource_kind, resource_id,
                     cycle, revision, effective_at, kind, policy, source, payload"#,
    )
    .bind(event.actor_id)
    .bind(event.resource_kind)
    .bind(event.resource_id)
    .bind(event.cycle)
    .bind(event.revision)
    .bind(event.effective_at)
    .bind(event.kind)
    .bind(event.policy)
    .bind(event.source)
    .bind(event.payload)
    .fetch_one(executor)
    .await
    .map_err(|_| EventError::StorageFailed)?;
    use sqlx::Row;
    Ok(Event {
        id: row.try_get("id").map_err(|_| EventError::StorageFailed)?,
        occurred_at: row
            .try_get("occurred_at")
            .map_err(|_| EventError::StorageFailed)?,
        actor_id: row
            .try_get("actor_id")
            .map_err(|_| EventError::StorageFailed)?,
        resource_kind: row
            .try_get("resource_kind")
            .map_err(|_| EventError::StorageFailed)?,
        resource_id: row
            .try_get("resource_id")
            .map_err(|_| EventError::StorageFailed)?,
        cycle: row
            .try_get("cycle")
            .map_err(|_| EventError::StorageFailed)?,
        revision: row
            .try_get("revision")
            .map_err(|_| EventError::StorageFailed)?,
        effective_at: row
            .try_get("effective_at")
            .map_err(|_| EventError::StorageFailed)?,
        kind: row.try_get("kind").map_err(|_| EventError::StorageFailed)?,
        policy: row
            .try_get("policy")
            .map_err(|_| EventError::StorageFailed)?,
        source: row
            .try_get("source")
            .map_err(|_| EventError::StorageFailed)?,
        payload: row
            .try_get("payload")
            .map_err(|_| EventError::StorageFailed)?,
    })
}
