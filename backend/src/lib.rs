//! Procurali backend library.
//!
//! Skeleton stage: the five module boundaries exist so integration tests can
//! construct against them. There is no business behavior yet; file-backed
//! submodules land in later cards (configuration, telemetry, router, and the
//! domain/application/persistence operations).

/// Pure value types and rules (money, clock, text, locality). No I/O.
pub mod domain {
    /// New/used acceptance and offer terms without synonyms.
    pub mod condition;
    /// Opaque stable city/region/category identities compared by value.
    pub mod location;
    /// Exact BRL money: integer cents inside, decimal strings on the wire.
    pub mod money;
    /// Canonical bounds and mechanical duplicate normalization.
    pub mod text;
    /// UTC instants, exclusive deadlines, rolling windows. No clock reads.
    pub mod time;
}

/// Business operations owning eligibility and transaction boundaries.
pub mod application {
    /// Effective-time port: production system clock vs frozen test clocks.
    pub mod clock;
    /// Durable per-action deduplication (same key/body replays, changed body conflicts).
    pub mod idempotency;
}

/// SQLx repositories, migrations, pool, and transaction machinery.
pub mod persistence {
    /// Durable business events recorded with their mutation.
    pub mod events;
    /// Compile-time migration journal and append-only runner.
    pub mod migrations;
    /// In-product per-recipient notices with scoped acknowledgment.
    pub mod notices;
    /// Bounded pools, redacted handling, and liveness probes.
    pub mod pool;
    /// Bounded serializable transaction retry (DEC-0003 machinery).
    pub mod transaction;
}

/// Axum router, handlers, DTO allowlists, and structured errors.
pub mod http {
    /// Response allowlists and strict request bodies.
    pub mod dto;
    /// Structured errors: closed codes, exact statuses, secret-free messages.
    pub mod errors;
    /// Liveness/readiness probes over dependency flags (no values in output).
    pub mod health;
    /// Router with explicit size/time bounds and stable errors.
    pub mod router;
}

/// Validated configuration, redacted logging, lifecycle, and workers.
pub mod operations {
    /// Validated runtime configuration (no default secrets, redacted output).
    pub mod config;
    /// Graceful server lifecycle (drain on shutdown).
    pub mod lifecycle;
    /// Redacted structured observability (typed fields only, no secret channels).
    pub mod telemetry;
}
