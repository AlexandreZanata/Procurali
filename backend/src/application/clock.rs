//! Effective-time port: one explicit clock per operation.
//!
//! Production code takes [`SystemClock`], which reads the real system time and
//! accepts no caller input — callers cannot spoof it with headers, parameters,
//! or configuration. Tests take [`FixedClock`], which moves only when the test
//! advances it, so seven-day waits never happen. The validated configuration
//! (tested separately) additionally refuses test-only clock switches in
//! production, so a fixed clock cannot leak into a live path through settings.

use crate::domain::time::Instant;

/// Source of the effective instant for one operation.
pub trait Clock: Send + Sync {
    /// The instant eligibility is evaluated at (the guarded mutation's time).
    fn now(&self) -> Instant;
}

/// Production clock: the real system time. Takes no input.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::from_system_time(std::time::SystemTime::now())
    }
}

/// Test clock: frozen until advanced. Never used outside tests.
#[derive(Debug)]
pub struct FixedClock {
    current: std::sync::Mutex<Instant>,
}

impl FixedClock {
    /// Freeze at `at`.
    #[must_use]
    pub fn new(at: Instant) -> Self {
        Self {
            current: std::sync::Mutex::new(at),
        }
    }

    /// Move forward by `delta`. Never backward (`Duration` is unsigned) and
    /// never panicking: an overflowing advance keeps the current instant.
    pub fn advance_by(&self, delta: std::time::Duration) {
        let mut current = self.current.lock().expect("test clock lock");
        let seconds = i64::try_from(delta.as_secs()).unwrap_or(i64::MAX);
        if let Ok(advanced) = current.add_seconds(seconds) {
            *current = advanced;
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        *self.current.lock().expect("test clock lock")
    }
}
