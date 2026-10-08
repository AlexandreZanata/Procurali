//! Exact BRL money: integer cents inside, decimal strings on the wire.
//!
//! Canonical rules: INV-10 (positive BRL, at most two decimals), INV-19 (offer
//! price positive, request currency, at most the current maximum budget), AC-05
//! (invalid publication refused with the requirement), AC-16 (above-ceiling
//! offers refused with the specific budget reason).
//!
//! Representation contract (`contracts/domain-vectors.json`): API money matches
//! `^-?\d+(\.\d{1,2})?$`. A leading minus is accepted only to refuse the value
//! with the specific non-positive reason (frozen vector `-5.00`); plus signs
//! stay malformed. Internally and persisted it is integer minor units.
//! Binary floating point appears nowhere: no `f32`/`f64` in this module.

/// Positive BRL amount in integer minor units (cents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money {
    cents: i64,
}

/// Typed money refusal. Each variant maps to one stable error `code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    /// Empty input: nothing to parse.
    Empty,
    /// Wrong shape: whitespace, signs, separators, exponents, letters,
    /// missing digits, or more than one point.
    Malformed,
    /// Zero value: budgets and prices are strictly positive.
    NonPositive,
    /// More than two fractional digits. Never rounded.
    Overprecision,
    /// Digits exceed the integer-cent range. Never wrapped or saturated.
    Overflow,
}

impl MoneyError {
    /// Stable wire code. `amount_too_large` is a pending registry addition for
    /// the structured-error card; the string is fixed now so behavior is stable.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => "missing_field",
            Self::Malformed => "malformed_amount",
            Self::NonPositive => "nonpositive_amount",
            Self::Overprecision => "overprecision",
            Self::Overflow => "amount_too_large",
        }
    }
}

impl std::fmt::Display for MoneyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for MoneyError {}

impl Money {
    /// Parse an exact decimal string into cents. Strict: no trimming, no
    /// rounding, no float anywhere on the path.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError`] for every non-conforming input, never panics.
    pub fn parse(input: &str) -> Result<Self, MoneyError> {
        if input.is_empty() {
            return Err(MoneyError::Empty);
        }
        // One leading minus is grammatical so negatives fail with the specific
        // non-positive reason; everything else non-digit stays malformed.
        let (negative, magnitude) = match input.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, input),
        };
        let (whole, fraction) = match magnitude.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (magnitude, None),
        };
        if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MoneyError::Malformed);
        }
        let fraction_cents: i64 = match fraction {
            None => 0,
            Some(one) if one.len() == 1 && one.bytes().all(|b| b.is_ascii_digit()) => {
                i64::from(one.as_bytes()[0] - b'0') * 10
            }
            Some(two) if two.len() == 2 && two.bytes().all(|b| b.is_ascii_digit()) => {
                let digits = two.as_bytes();
                i64::from(digits[0] - b'0') * 10 + i64::from(digits[1] - b'0')
            }
            Some(_) => {
                // Digits beyond two decimals are overprecision; anything else
                // (empty, letters, second point) is malformed.
                if fraction.is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                }) {
                    return Err(MoneyError::Overprecision);
                }
                return Err(MoneyError::Malformed);
            }
        };
        let mut units: i64 = 0;
        for digit in whole.bytes() {
            units = units
                .checked_mul(10)
                .and_then(|value| value.checked_add(i64::from(digit - b'0')))
                .ok_or(MoneyError::Overflow)?;
        }
        let cents = units
            .checked_mul(100)
            .and_then(|value| value.checked_add(fraction_cents))
            .ok_or(MoneyError::Overflow)?;
        if negative || cents <= 0 {
            return Err(MoneyError::NonPositive);
        }
        Ok(Self { cents })
    }

    /// Integer minor units for persistence and comparison.
    #[must_use]
    pub const fn cents(self) -> i64 {
        self.cents
    }

    /// Exact canonical rendering, always two fractional digits.
    #[must_use]
    pub fn format(self) -> String {
        format!("{}.{:02}", self.cents / 100, self.cents % 100)
    }

    /// Offer compatibility (INV-19, AC-16): a price fits when it does not
    /// exceed the current maximum budget. Equality fits.
    #[must_use]
    pub fn fits_budget(price: Self, budget: Self) -> bool {
        price.cents <= budget.cents
    }
}
