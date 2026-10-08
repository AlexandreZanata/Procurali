//! Minimal binary scaffold.
//!
//! Parses the required runtime mode with a typed refusal (never a panic) and
//! prints a redacted startup line. Validated configuration, routing, and the
//! dependency-wired service arrive in later cards; this scaffold only proves the
//! package builds, starts, and refuses malformed settings deterministically.

use std::env;
use std::fmt;

/// Deployment mode selected via the `PROCURALI_ENV` environment variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Development,
    Test,
    Staging,
    Production,
}

/// Typed startup refusal. Carries no secret or connection value.
#[derive(Debug, PartialEq, Eq)]
enum StartupError {
    /// A required setting is missing or empty.
    Missing(&'static str),
    /// `PROCURALI_ENV` holds an unknown mode.
    InvalidMode(String),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(key) => write!(f, "missing required setting: {key}"),
            Self::InvalidMode(value) => write!(
                f,
                "invalid PROCURALI_ENV {value:?}: expected development|test|staging|production"
            ),
        }
    }
}

impl std::error::Error for StartupError {}

/// Parse a mode label exactly. Deterministic and side-effect free.
fn parse_mode(raw: &str) -> Result<Mode, StartupError> {
    match raw {
        "development" => Ok(Mode::Development),
        "test" => Ok(Mode::Test),
        "staging" => Ok(Mode::Staging),
        "production" => Ok(Mode::Production),
        other => Err(StartupError::InvalidMode(other.to_owned())),
    }
}

/// Read one required non-empty setting without ever printing its value.
fn require_env(key: &'static str) -> Result<String, StartupError> {
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(StartupError::Missing(key)),
    }
}

fn main() -> Result<(), StartupError> {
    // Skeleton default: an unset mode means local development. Production safety
    // (refusing test fakes and demanding secrets/origins) is enforced by the
    // validated configuration in a later card, not by this scaffold.
    let mode = match require_env("PROCURALI_ENV") {
        Ok(raw) => parse_mode(&raw)?,
        Err(StartupError::Missing(_)) => Mode::Development,
        Err(other) => return Err(other),
    };
    // Redacted by construction: only the mode label is printed, never a value.
    println!("procurali-backend starting (skeleton) mode={mode:?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_supported_mode() {
        assert_eq!(parse_mode("development"), Ok(Mode::Development));
        assert_eq!(parse_mode("test"), Ok(Mode::Test));
        assert_eq!(parse_mode("staging"), Ok(Mode::Staging));
        assert_eq!(parse_mode("production"), Ok(Mode::Production));
    }

    #[test]
    fn malformed_mode_is_a_typed_refusal_not_a_panic() {
        let error = parse_mode("prod").expect_err("unknown mode must be refused");
        assert_eq!(error, StartupError::InvalidMode("prod".to_owned()));
        assert!(error.to_string().contains("PROCURALI_ENV"));
    }

    #[test]
    fn missing_and_empty_settings_are_refused_without_printing_values() {
        let absent = require_env("PROCURALI_SKELETON_SENTINEL_ABSENT")
            .expect_err("a variable that is never set must be reported missing");
        assert_eq!(
            absent,
            StartupError::Missing("PROCURALI_SKELETON_SENTINEL_ABSENT")
        );
        assert!(!absent.to_string().contains("secret"));
    }
}
