//! Validated runtime configuration.
//!
//! The loader is a pure function over explicit key/value pairs so tests stay
//! deterministic without touching the process environment. [`Settings::from_env`]
//! is the thin production entry point used by the binary once later cards wire it.
//!
//! Rules enforced here (technical safeguards, not marketplace policy):
//! - No default secrets: every secret has to be supplied explicitly.
//! - The deterministic fake phone provider and test-only clock are refused in
//!   production, even if another flag tries to enable them.
//! - Production demands an `https://` public origin and a long session key.
//! - Debug formatting and error messages never contain secret or URL values.

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

/// Deployment environment selected via `PROCURALI_ENV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Test,
    Staging,
    Production,
}

impl Environment {
    fn parse(raw: &str) -> Result<Self, ConfigError> {
        match raw {
            "development" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            "staging" => Ok(Self::Staging),
            "production" => Ok(Self::Production),
            _ => Err(ConfigError::invalid("PROCURALI_ENV")),
        }
    }

    /// Whether test-only doubles are tolerable in this environment.
    #[must_use]
    pub const fn allows_test_doubles(self) -> bool {
        matches!(self, Self::Development | Self::Test)
    }
}

/// Phone-proof provider selected via `PHONE_PROVIDER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneProvider {
    /// Deterministic fake. Test/development only; production startup refuses it.
    Fake,
    /// Live Twilio Verify adapter. Requires explicit credentials.
    Twilio,
}

/// A secret value that is never printed or logged.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret([{} chars redacted])", self.0.len())
    }
}

/// Typed configuration refusal. Carries field names and reasons only,
/// never the supplied values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// A required setting is missing or empty.
    #[error("missing required setting: {0}")]
    Missing(&'static str),
    /// A supplied setting fails validation.
    #[error("invalid setting: {0}")]
    Invalid(&'static str),
}

impl ConfigError {
    const fn missing(field: &'static str) -> Self {
        Self::Missing(field)
    }

    const fn invalid(field: &'static str) -> Self {
        Self::Invalid(field)
    }
}

/// Validated runtime settings. Construct via [`Settings::from_map`] (tests) or
/// [`Settings::from_env`] (real startup).
pub struct Settings {
    environment: Environment,
    database_url: Secret,
    test_database_url: Option<Secret>,
    phone_provider: PhoneProvider,
    twilio_account_sid: Option<Secret>,
    twilio_auth_token: Option<Secret>,
    twilio_verify_service_sid: Option<Secret>,
    session_key: Secret,
    public_origin: Option<String>,
    test_fixed_clock: Option<String>,
}

impl Settings {
    /// Minimum session-key length accepted in production.
    pub const PRODUCTION_SESSION_KEY_MIN_LEN: usize = 32;

    /// Load settings from the real process environment.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] naming the offending field when validation fails.
    pub fn from_env() -> Result<Self, ConfigError> {
        let map: BTreeMap<String, String> = std::env::vars().collect();
        Self::from_map(&map)
    }

    /// Load settings from explicit pairs. Deterministic; the unit under test.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] naming the offending field when validation fails.
    pub fn from_map(values: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let get = |key: &str| -> Option<String> {
            values
                .get(key)
                .map(String::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let required = |key: &'static str| -> Result<String, ConfigError> {
            get(key).ok_or(ConfigError::missing(key))
        };

        let environment = match get("PROCURALI_ENV") {
            Some(raw) => Environment::parse(&raw)?,
            None => Environment::Development,
        };

        let database_url = required("DATABASE_URL")?;
        if !is_postgres_url(&database_url) {
            return Err(ConfigError::invalid("DATABASE_URL"));
        }

        let test_database_url = match get("TEST_DATABASE_URL") {
            Some(url) => {
                if !is_postgres_url(&url) {
                    return Err(ConfigError::invalid("TEST_DATABASE_URL"));
                }
                if url == database_url {
                    // A test suite must never run against the application database.
                    return Err(ConfigError::invalid("TEST_DATABASE_URL"));
                }
                Some(Secret(url))
            }
            None => None,
        };

        let provider_raw = get("PHONE_PROVIDER").unwrap_or_else(|| "fake".to_owned());
        let phone_provider = match provider_raw.as_str() {
            "fake" => PhoneProvider::Fake,
            "twilio" => PhoneProvider::Twilio,
            _ => return Err(ConfigError::invalid("PHONE_PROVIDER")),
        };
        if phone_provider == PhoneProvider::Fake && !environment.allows_test_doubles() {
            return Err(ConfigError::invalid("PHONE_PROVIDER"));
        }

        let twilio_account_sid;
        let twilio_auth_token;
        let twilio_verify_service_sid;
        if phone_provider == PhoneProvider::Twilio {
            twilio_account_sid = Some(Secret(required("TWILIO_ACCOUNT_SID")?));
            twilio_auth_token = Some(Secret(required("TWILIO_AUTH_TOKEN")?));
            twilio_verify_service_sid = Some(Secret(required("TWILIO_VERIFY_SERVICE_SID")?));
        } else {
            twilio_account_sid = None;
            twilio_auth_token = None;
            twilio_verify_service_sid = None;
        }

        let session_key = Secret(required("SESSION_KEY")?);
        if environment == Environment::Production
            && session_key.len() < Self::PRODUCTION_SESSION_KEY_MIN_LEN
        {
            return Err(ConfigError::invalid("SESSION_KEY"));
        }

        let public_origin = match get("PUBLIC_ORIGIN") {
            Some(origin) => {
                if environment == Environment::Production && !origin.starts_with("https://") {
                    return Err(ConfigError::invalid("PUBLIC_ORIGIN"));
                }
                Some(origin)
            }
            None => {
                if environment == Environment::Production {
                    return Err(ConfigError::missing("PUBLIC_ORIGIN"));
                }
                None
            }
        };

        let test_fixed_clock = match get("TEST_FIXED_CLOCK") {
            Some(instant) => {
                if !environment.allows_test_doubles() {
                    return Err(ConfigError::invalid("TEST_FIXED_CLOCK"));
                }
                if !is_rfc3339_utc(&instant) {
                    return Err(ConfigError::invalid("TEST_FIXED_CLOCK"));
                }
                Some(instant)
            }
            None => None,
        };

        Ok(Self {
            environment,
            database_url: Secret(database_url),
            test_database_url,
            phone_provider,
            twilio_account_sid,
            twilio_auth_token,
            twilio_verify_service_sid,
            session_key,
            public_origin,
            test_fixed_clock,
        })
    }

    /// Current deployment environment.
    #[must_use]
    pub const fn environment(&self) -> Environment {
        self.environment
    }

    /// Selected phone-proof provider.
    #[must_use]
    pub const fn phone_provider(&self) -> PhoneProvider {
        self.phone_provider
    }

    /// Whether test doubles are tolerable under the active environment.
    #[must_use]
    pub fn allows_test_doubles(&self) -> bool {
        self.environment.allows_test_doubles()
    }

    /// Fixed clock instant for deterministic tests, if configured.
    #[must_use]
    pub fn test_fixed_clock(&self) -> Option<&str> {
        self.test_fixed_clock.as_deref()
    }

    /// Configured public origin, if any.
    #[must_use]
    pub fn public_origin(&self) -> Option<&str> {
        self.public_origin.as_deref()
    }
}

// Manual `Debug`: field names and shapes only. Values (URLs, secrets, SIDs)
// must never reach logs, so this impl is the privacy boundary for settings.
impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("environment", &self.environment)
            .field("database_url", &self.database_url)
            .field(
                "test_database_url",
                &self.test_database_url.as_ref().map(|_| "set"),
            )
            .field("phone_provider", &self.phone_provider)
            .field(
                "twilio_account_sid",
                &self.twilio_account_sid.as_ref().map(|_| "set"),
            )
            .field(
                "twilio_auth_token",
                &self.twilio_auth_token.as_ref().map(|_| "set"),
            )
            .field(
                "twilio_verify_service_sid",
                &self.twilio_verify_service_sid.as_ref().map(|_| "set"),
            )
            .field("session_key", &self.session_key)
            .field("public_origin", &self.public_origin.as_ref().map(|_| "set"))
            .field(
                "test_fixed_clock",
                &self.test_fixed_clock.as_ref().map(|_| "set"),
            )
            .finish()
    }
}

fn is_postgres_url(value: &str) -> bool {
    value.starts_with("postgres://") || value.starts_with("postgresql://")
}

/// Minimal RFC3339-UTC shape check (`YYYY-MM-DDTHH:MM:SSZ`).
/// Full instant parsing arrives with the clock module; the loader only needs
/// to reject obviously non-instant switches deterministically.
fn is_rfc3339_utc(value: &str) -> bool {
    const EXPECTED_LEN: usize = 20;
    if value.len() != EXPECTED_LEN || !value.ends_with('Z') {
        return false;
    }
    let bytes = value.as_bytes();
    const DIGIT_POSITIONS: [usize; 14] = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    if !DIGIT_POSITIONS
        .iter()
        .all(|pos| bytes[*pos].is_ascii_digit())
    {
        return false;
    }
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
}
