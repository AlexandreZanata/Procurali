//! UTC instants, exclusive deadlines, and rolling windows.
//!
//! Canonical rules: INV-08 (active demand needs a future deadline), INV-09
//! (terminal or restricted states take no new offers or contact — enforced by
//! state owners, with time expiry decided here), AC-10 (deadlines are
//! exclusive), EC-07 (the business effect must occur before the deadline),
//! principles §2.2/§2.3 (seven consecutive 24-hour periods, actual elapsed
//! time, no midnight resets).
//!
//! This module is pure: it never reads a system clock. The current instant
//! enters through the [`Clock`](crate::application::clock::Clock) port, and
//! eligibility is evaluated at the guarded mutation from that instant — never
//! from request arrival or caller-supplied time.

use chrono::{DateTime, SecondsFormat, Utc};

/// Seven consecutive 24-hour periods per publication cycle, in seconds.
pub const CYCLE_SECONDS: i64 = 7 * 24 * 60 * 60;

/// A UTC instant with second precision on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(DateTime<Utc>);

/// Typed time refusal, mapped to existing stable codes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeError {
    /// Empty input.
    Empty,
    /// Wrong shape (date-only, naive, separator, garbage) or out of range.
    Malformed,
}

impl TimeError {
    /// Stable wire code (existing registry entries only).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => "missing_field",
            Self::Malformed => "invalid_field",
        }
    }
}

impl std::fmt::Display for TimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for TimeError {}

impl Instant {
    /// Parse RFC3339 with an explicit offset, normalized to UTC.
    /// Naive dates, date-only values, and garbage are refused, never guessed.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError`] without panicking.
    pub fn parse_rfc3339(input: &str) -> Result<Self, TimeError> {
        if input.is_empty() {
            return Err(TimeError::Empty);
        }
        DateTime::parse_from_rfc3339(input)
            .map(|fixed| Self(fixed.with_timezone(&Utc)))
            .map_err(|_| TimeError::Malformed)
    }

    /// Map a system timestamp to an instant. Pure function of its input (no
    /// clock is read here): pre-epoch times saturate to the minimum instead of
    /// panicking.
    #[must_use]
    pub fn from_system_time(value: std::time::SystemTime) -> Self {
        let seconds = value
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0);
        Self(DateTime::<Utc>::from_timestamp(seconds, 0).unwrap_or(DateTime::<Utc>::MIN_UTC))
    }

    /// Canonical rendering: RFC3339 UTC with `Z`.
    #[must_use]
    pub fn format(self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// Deadline of a cycle published (or renewed) at this instant:
    /// exactly seven 24-hour periods later. Checked: overflow is an error.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::Malformed`] when the deadline overflows the
    /// representable range.
    pub fn cycle_deadline(self) -> Result<Self, TimeError> {
        self.0
            .checked_add_signed(chrono::Duration::seconds(CYCLE_SECONDS))
            .map(Self)
            .ok_or(TimeError::Malformed)
    }

    /// Shift forward by whole seconds. Checked: overflow is an error, never a
    /// wrap or panic. Powers test-clock advancement without sleeps.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::Malformed`] when the result leaves the
    /// representable range.
    pub fn add_seconds(self, delta_seconds: i64) -> Result<Self, TimeError> {
        self.0
            .checked_add_signed(chrono::Duration::seconds(delta_seconds))
            .map(Self)
            .ok_or(TimeError::Malformed)
    }

    /// Elapsed whole seconds from an earlier instant. Never negative:
    /// a later `earlier` saturates to zero instead of panicking.
    #[must_use]
    pub fn elapsed_seconds_since(self, earlier: Self) -> i64 {
        self.0.signed_duration_since(earlier.0).num_seconds().max(0)
    }

    /// Exclusive eligibility: the action is allowed strictly before the
    /// deadline. Equality denies — the effect did not occur before expiry.
    #[must_use]
    pub fn is_eligible_at(now: Self, deadline: Self) -> bool {
        now < deadline
    }

    /// Rolling-window membership over actual elapsed time: the event counts
    /// when it is after `now - length` and not in the future. No midnight
    /// resets, no calendar days.
    #[must_use]
    pub fn is_within_window(event_at: Self, now: Self, length: std::time::Duration) -> bool {
        let length = chrono::Duration::from_std(length).unwrap_or(chrono::Duration::MAX);
        let start = now
            .0
            .checked_sub_signed(length)
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        event_at.0 > start && event_at.0 <= now.0
    }
}
