//! Request lifecycle background jobs: expiry sweeps and owner notices.
//!
//! Each submodule owns one job family end to end: how its jobs are
//! produced, claimed through the bounded worker machinery, processed
//! idempotently, and settled. Families never share payload shapes.

/// Request-expiry sweeps: ended cycles with exactly one owner notice.
pub mod expire_requests;
/// Outcome prompts: ask once per cycle, never nag, never mutate.
pub mod outcome_prompts;
/// Retention cleanup sweeps: redact due ordinary data, keep the rest.
pub mod retention_cleanup;
