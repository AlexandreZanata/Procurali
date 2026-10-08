//! Physical-item condition: what the buyer accepts and what the seller offers.
//!
//! Canonical rules: INV-20 (offer condition is new or used, matching the
//! request unless the buyer accepts either), AC-16 (wrong-condition offers
//! refused with the specific reason). Parsing is strict lowercase: no
//! case-guessing, no synonyms, no semantic invention.

/// What the buyer accepts on a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestCondition {
    /// Only new items.
    NewOnly,
    /// Only used items.
    UsedOnly,
    /// Either condition.
    Either,
}

/// What a specific offer represents (always one concrete condition).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OfferCondition {
    /// A new item.
    New,
    /// A used item.
    Used,
}

/// Unknown condition label. Maps to the existing `invalid_field` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionError;

impl ConditionError {
    /// Stable wire code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        "invalid_field"
    }
}

impl std::fmt::Display for ConditionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for ConditionError {}

impl RequestCondition {
    /// Parse `new` | `used` | `either` exactly.
    ///
    /// # Errors
    ///
    /// Returns [`ConditionError`] for any other label, including case variants.
    pub fn parse(input: &str) -> Result<Self, ConditionError> {
        match input {
            "new" => Ok(Self::NewOnly),
            "used" => Ok(Self::UsedOnly),
            "either" => Ok(Self::Either),
            _ => Err(ConditionError),
        }
    }

    /// Compatibility (INV-20, AC-16): used-only accepts used and refuses new;
    /// either accepts both.
    #[must_use]
    pub const fn accepts(self, offer: OfferCondition) -> bool {
        matches!(
            (self, offer),
            (Self::Either, _)
                | (Self::NewOnly, OfferCondition::New)
                | (Self::UsedOnly, OfferCondition::Used)
        )
    }
}

impl OfferCondition {
    /// Parse `new` | `used` exactly.
    ///
    /// # Errors
    ///
    /// Returns [`ConditionError`] for any other label, including case variants.
    pub fn parse(input: &str) -> Result<Self, ConditionError> {
        match input {
            "new" => Ok(Self::New),
            "used" => Ok(Self::Used),
            _ => Err(ConditionError),
        }
    }
}
