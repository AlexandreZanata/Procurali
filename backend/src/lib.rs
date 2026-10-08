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
    /// Bounded verification abuse controls (guarded challenge flows).
    pub mod auth_limits;
    /// Proof-gated phone reassignment with protected history.
    pub mod change_phone;
    /// Effective-time port: production system clock vs frozen test clocks.
    pub mod clock;
    /// Buyer closure: completion and cancellation as declared outcomes.
    pub mod close_request;
    /// Exact duplicate demand: normalized same-owner open-need matching.
    pub mod duplicate_intent;
    /// Live terms editing: new revisions with preserved engagement.
    pub mod edit_offer;
    /// Shared current-account and relationship guards for writers.
    pub mod eligibility;
    /// Durable per-action deduplication (same key/body replays, changed body conflicts).
    pub mod idempotency;
    /// Offer submission quotas: daily allowance and slot permanence.
    pub mod offer_limits;
    /// Private offer comparison reads: buyer sets and seller self-views.
    pub mod offer_reads;
    /// Phone-verification provider boundary with a restricted deterministic fake.
    pub mod phone_verification;
    /// Declared professional classification, free and separate.
    pub mod professional_profile;
    /// Complete publication validation: drafts become seven-day active cycles.
    pub mod publish_request;
    /// Account recovery and recycled-number review boundary.
    pub mod recovery;
    /// Minimal registration and phone-proof activation flows.
    pub mod register_user;
    /// Buyer decline: rejection as a comparison choice, never misconduct.
    pub mod reject_offer;
    /// Owner removal: business concealment with immutable history.
    pub mod remove_request;
    /// Explicit renewal: fresh seven-day cycles from eligible demand.
    pub mod renew_request;
    /// Private request draft creation and editing for the owning author.
    pub mod request_drafts;
    /// Effective expiry: deadline eligibility and conditional expiration.
    pub mod request_eligibility;
    /// Request activation quotas: open slots and rolling successful counts.
    pub mod request_limits;
    /// Request-driven offer cascades: demand moves, live offers follow.
    pub mod request_offer_cascades;
    /// Owner request reads: bounded lists and details with truthful actions.
    pub mod request_reads;
    /// Request revisions: material classification with preserved history.
    pub mod revise_request;
    /// Session lifecycle: login after proof and current-state authentication.
    pub mod sessions;
    /// Guarded offer submission: matching availability on eligible demand.
    pub mod submit_offer;
    /// Owner profile editing: display name and default locality.
    pub mod update_profile;
    /// First buyer view: sent-to-viewed engagement with one conversion fact.
    pub mod view_offer;
    /// Seller withdrawal: terminal honesty without reservation inference.
    pub mod withdraw_offer;
}

/// SQLx repositories, migrations, pool, and transaction machinery.
pub mod persistence {
    /// Category and locality catalogs with stable identities.
    pub mod catalogs;
    /// Current-account and block-ledger reads for guards.
    pub mod eligibility;
    /// Durable business events recorded with their mutation.
    pub mod events;
    /// Compile-time migration journal and append-only runner.
    pub mod migrations;
    /// In-product per-recipient notices with scoped acknowledgment.
    pub mod notices;
    /// Offer slots and immutable terms over recorded facts.
    pub mod offers;
    /// Phone-change history readers over recorded facts.
    pub mod phone_history;
    /// Bounded pools, redacted handling, and liveness probes.
    pub mod pool;
    /// Request ownership, requirement revisions, and activation cycles.
    pub mod requests;
    /// Bounded serializable transaction retry (DEC-0003 machinery).
    pub mod transaction;
    /// Account identity: users, credential digests, challenges, blocks ledger.
    pub mod users;
}

/// Axum router, handlers, DTO allowlists, and structured errors.
pub mod http {
    /// Registration, challenge, and confirmation routes (pending to active).
    pub mod accounts;
    /// Session routes: login, current account, and logout with CSRF contract.
    pub mod auth;
    /// Public catalog routes: cities/regions and categories.
    pub mod catalogs;
    /// Response allowlists and strict request bodies.
    pub mod dto;
    /// Structured errors: closed codes, exact statuses, secret-free messages.
    pub mod errors;
    /// Liveness/readiness probes over dependency flags (no values in output).
    pub mod health;
    /// Offer submission routes: guarded creation on eligible demand.
    pub mod offers;
    /// Professional-profile routes: free declaration and withdrawal.
    pub mod profiles;
    /// Request-draft routes: owner-only creation, reads, and edits.
    pub mod requests;
    /// Router with explicit size/time bounds and stable errors.
    pub mod router;
}

/// Validated configuration, redacted logging, lifecycle, and workers.
pub mod operations {
    /// Validated runtime configuration (no default secrets, redacted output).
    pub mod config;
    /// Request lifecycle background jobs: expiry sweeps and owner notices.
    pub mod jobs;
    /// Graceful server lifecycle (drain on shutdown).
    pub mod lifecycle;
    /// Redacted structured observability (typed fields only, no secret channels).
    pub mod telemetry;
    /// Live Twilio Verify adapter over an injected transport.
    pub mod twilio_verify;
    /// Durable bounded background-job claims over PostgreSQL.
    pub mod worker;
}
