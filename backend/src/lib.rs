//! Procurali backend library.
//!
//! Skeleton stage: the five module boundaries exist so integration tests can
//! construct against them. There is no business behavior yet; file-backed
//! submodules land in later cards (configuration, telemetry, router, and the
//! domain/application/persistence operations).

/// Pure value types and rules (money, clock, text, locality). No I/O.
pub mod domain {}

/// Business operations owning eligibility and transaction boundaries.
pub mod application {}

/// SQLx repositories, migrations, pool, and transaction machinery.
pub mod persistence {}

/// Axum router, handlers, DTO allowlists, and structured errors.
pub mod http {}

/// Validated configuration, redacted logging, lifecycle, and workers.
pub mod operations {}
