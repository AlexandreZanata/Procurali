//! Data-transfer allowlists: public summaries and strict request bodies.
//!
//! Canonical rules: INV-01 (exactly one author, no transfer — bodies carry no
//! author field and reject smuggled ones), INV-05 (roles never authorize —
//! bodies carry no role or state field), INV-31 (public surfaces expose no
//! phone, address, reporter, or private terms), AC-19 (private comparison —
//! summaries expose only the reader's own scope, never cross-account data).
//!
//! Response DTOs serialize fixed field sets constructed from explicit
//! arguments — never from internal persistent models (none exist yet, and the
//! constructors stay independent when they land). Request bodies use
//! `deny_unknown_fields`: protected fields (`owner_id`, `state`, `role`, ...)
//! are rejected instead of stripped.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Public request summary: the exact visitor-safe allowlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicRequest {
    id: Uuid,
    title: String,
    budget: String,
    condition: String,
    category: String,
    city: String,
    region_label: String,
    published_at: String,
    deadline: String,
    cycle: u32,
    revision: Uuid,
}

impl PublicRequest {
    /// Build from explicit validated parts. No phone, address, offer, or
    /// reporter parameter exists, so none can leak by construction.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        id: Uuid,
        title: String,
        budget: String,
        condition: String,
        category: String,
        city: String,
        region_label: String,
        published_at: String,
        deadline: String,
        cycle: u32,
        revision: Uuid,
    ) -> Self {
        Self {
            id,
            title,
            budget,
            condition,
            category,
            city,
            region_label,
            published_at,
            deadline,
            cycle,
            revision,
        }
    }
}

/// Seller-scoped own-offer summary: own terms only, never competing offers,
/// buyer phones, or cross-account data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicOfferSummary {
    id: Uuid,
    price: String,
    condition: String,
    request_revision: Uuid,
}

impl PublicOfferSummary {
    /// Build from explicit validated parts.
    #[must_use]
    pub const fn new(id: Uuid, price: String, condition: String, request_revision: Uuid) -> Self {
        Self {
            id,
            price,
            condition,
            request_revision,
        }
    }
}

/// Request draft body: validated fields only. Ownership, state, and role
/// derive server-side (INV-01, INV-05); smuggled protected fields are refused.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRequestBody {
    /// Desired item title.
    pub title: String,
    /// Exact decimal budget string.
    pub budget: String,
    /// `new` | `used` | `either`.
    pub condition: String,
    /// Opaque category code.
    pub category: String,
    /// Opaque city code.
    pub city: String,
    /// Region label within the city scope.
    pub region: String,
}

/// Parse a strict UUID path identifier.
///
/// # Errors
///
/// Returns a 400 `invalid_field` error for malformed input (nonexistent but
/// well-formed ids resolve to 404 in handlers, never here).
pub fn parse_id(input: &str) -> Result<Uuid, super::errors::ApiError> {
    input.parse::<Uuid>().map_err(|_| {
        super::errors::ApiError::new(
            super::errors::Code::InvalidField,
            "identifier must be a UUID",
        )
    })
}
