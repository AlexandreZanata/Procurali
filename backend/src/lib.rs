//! Procurali backend library.
//!
//! Skeleton stage: the five module boundaries exist so integration tests can
//! construct against them. There is no business behavior yet; file-backed
//! submodules land in later cards (configuration, telemetry, router, and the
//! domain/application/persistence operations).

/// Pure value types and rules (money, clock, text, locality). No I/O.
pub mod domain {
    /// Exact BRL money: integer cents inside, decimal strings on the wire.
    pub mod money;
}

/// Business operations owning eligibility and transaction boundaries.
pub mod application {}

/// SQLx repositories, migrations, pool, and transaction machinery.
pub mod persistence {
    /// Compile-time migration journal and append-only runner.
    pub mod migrations;
    /// Bounded pools, redacted handling, and liveness probes.
    pub mod pool;
}

/// Axum router, handlers, DTO allowlists, and structured errors.
pub mod http {
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
