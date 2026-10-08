//! Text values: canonical length bounds and duplicate normalization.
//!
//! Bounds (principles §2.3): request titles and offer descriptions 120,
//! optional notes 500, display names 80, report details 1000 — counted in
//! Unicode scalar values in both languages. Shorter valid text stays allowed;
//! meaningfulness remains a moderation judgment, never a parse rule.
//!
//! Duplicate normalization (AC-07) is mechanical only: trim, collapse
//! whitespace runs, Unicode lowercase. No stop-word removal, no stemming, no
//! semantic matching — meaningful words are never dropped. Canonical
//! equivalence beyond case/whitespace (e.g. composed vs decomposed accents) is
//! NOT performed: no Unicode normalization dependency is pinned, and inventing
//! one here would silently change matching policy. Such pairs count as
//! distinct needs until a vetted dependency lands.

/// Which bound applies to a text value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextKind {
    /// Request title: 120 scalar values.
    RequestTitle,
    /// Offer description: 120 scalar values.
    OfferDescription,
    /// Optional request or offer note: 500 scalar values.
    Note,
    /// Display name: 80 scalar values.
    DisplayName,
    /// Report details: 1000 scalar values.
    ReportDetails,
}

/// Over-limit refusal. Maps to the existing `text_too_long` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextError;

impl TextError {
    /// Stable wire code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        "text_too_long"
    }
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for TextError {}

impl TextKind {
    /// Canonical scalar-value bound for this kind.
    #[must_use]
    pub const fn limit(self) -> usize {
        match self {
            Self::RequestTitle | Self::OfferDescription => 120,
            Self::Note => 500,
            Self::DisplayName => 80,
            Self::ReportDetails => 1000,
        }
    }

    /// Contract key in `contracts/domain-vectors.json` for cross-checks.
    #[must_use]
    pub const fn contract_key(self) -> &'static str {
        match self {
            Self::RequestTitle => "request_title",
            Self::OfferDescription => "offer_description",
            Self::Note => "request_note",
            Self::DisplayName => "display_name",
            Self::ReportDetails => "report_details",
        }
    }

    /// Check the scalar-value count against the bound. Empty text passes here:
    /// required-field validation belongs to the owning operation, not to length.
    ///
    /// # Errors
    ///
    /// Returns [`TextError`] when the bound is exceeded.
    pub fn check_length(self, text: &str) -> Result<usize, TextError> {
        let count = text.chars().count();
        if count > self.limit() {
            return Err(TextError);
        }
        Ok(count)
    }
}

/// Mechanical duplicate normalization: trim, collapse every whitespace run to
/// one space, lowercase. Meaningful words survive untouched.
#[must_use]
pub fn normalize_for_comparison(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Exact-need equality for duplicate detection (AC-07): normalized forms match
/// and are non-empty. Empty normalizations never count as duplicates —
/// required-field validation owns emptiness.
#[must_use]
pub fn same_need(first: &str, second: &str) -> bool {
    let left = normalize_for_comparison(first);
    let right = normalize_for_comparison(second);
    !left.is_empty() && left == right
}
