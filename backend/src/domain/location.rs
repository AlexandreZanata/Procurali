//! Locality values: stable identities, never labels or guessed geography.
//!
//! Canonical rules: INV-11 (one item need — the type holds exactly one
//! category, never a list), INV-16 (a locality change never silently moves a
//! published request — locality snapshots travel with the resource, not with
//! the account), INV-22 (offers satisfy the selected city scope), EC-35 (label
//! renames preserve the stable conceptual identity; substantive scope changes
//! are versioned, never silently remapped), AC-13 (deterministic discovery
//! over explicit filters — ordering lives with the query, identity here).
//!
//! Identities are opaque trimmed codes: equality is by value, renames do not
//! affect it, and no code embeds geography. Catalog data and scope-extension
//! mechanics arrive in their owning cards; this module fixes comparison only.

/// Maximum code length. Codes are short identifiers, not prose.
pub const MAX_CODE_LEN: usize = 64;

/// Opaque stable city identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CityId(String);

/// Opaque stable region identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegionId(String);

/// Opaque stable category identity. Exactly one per need (INV-11): the type
/// holds a single value, so a bundled list is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CategoryId(String);

/// Locality refusal: empty or overlong codes. Existing codes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationError {
    /// Missing code.
    Empty,
    /// Overlong or whitespace-only code.
    Invalid,
}

impl LocationError {
    /// Stable wire code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => "missing_field",
            Self::Invalid => "invalid_field",
        }
    }
}

impl std::fmt::Display for LocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for LocationError {}

/// Trim and validate a raw code into its owned form.
fn clean_code(input: &str) -> Result<String, LocationError> {
    if input.is_empty() {
        return Err(LocationError::Empty);
    }
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_CODE_LEN {
        return Err(LocationError::Invalid);
    }
    Ok(trimmed.to_owned())
}

impl CityId {
    /// Parse an opaque city code.
    ///
    /// # Errors
    ///
    /// Returns [`LocationError`] for empty or overlong input.
    pub fn parse(input: &str) -> Result<Self, LocationError> {
        clean_code(input).map(Self)
    }

    /// The code text (for persistence and deterministic ordering).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RegionId {
    /// Parse an opaque region code.
    ///
    /// # Errors
    ///
    /// Returns [`LocationError`] for empty or overlong input.
    pub fn parse(input: &str) -> Result<Self, LocationError> {
        clean_code(input).map(Self)
    }

    /// The code text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CategoryId {
    /// Parse an opaque category code.
    ///
    /// # Errors
    ///
    /// Returns [`LocationError`] for empty or overlong input.
    pub fn parse(input: &str) -> Result<Self, LocationError> {
        clean_code(input).map(Self)
    }

    /// The code text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A request's locality snapshot: city plus region label, compared by value.
/// The same region label under different cities is a different locality.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Locality {
    /// Stable city identity.
    pub city: CityId,
    /// Region label valid within the city scope.
    pub region: RegionId,
}

impl Locality {
    /// Snapshot a locality. Snapshots travel with the resource (INV-16).
    #[must_use]
    pub const fn new(city: CityId, region: RegionId) -> Self {
        Self { city, region }
    }

    /// City-scope satisfaction (INV-22): the offer locality must equal the
    /// request locality. Extensions beyond equality are a future explicit
    /// mechanism, not a guess made here.
    #[must_use]
    pub fn satisfies_scope(request: &Self, offer: &Self) -> bool {
        request == offer
    }
}
