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
use std::path::{Path, PathBuf};

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

    /// The protected value, for the few places that must consume it (the
    /// database URL, the phone keys). Callers must never log, print, or
    /// render the result; [`Secret`]'s own `Debug` keeps it redacted.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
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
    phone_lookup_key: Secret,
    phone_encryption_key: Secret,
    fake_verify_code: Option<Secret>,
    assets_dir: PathBuf,
    bind_address: String,
}

/// Default directory holding the built web bundle, relative to the process
/// working directory.
pub const DEFAULT_ASSETS_DIR: &str = "web/dist";

/// Default bind address: loopback only, so an unconfigured process never
/// listens on a public interface.
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8080";

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

        // The deterministic proof code exists only with the fake provider, and
        // only where doubles are allowed. It is never a default: the server
        // must be told which code the local fake accepts.
        let fake_verify_code = match get("FAKE_VERIFY_CODE") {
            Some(code) => {
                if phone_provider != PhoneProvider::Fake {
                    return Err(ConfigError::invalid("FAKE_VERIFY_CODE"));
                }
                if !environment.allows_test_doubles() {
                    return Err(ConfigError::invalid("FAKE_VERIFY_CODE"));
                }
                Some(Secret(code))
            }
            None => {
                if phone_provider == PhoneProvider::Fake {
                    return Err(ConfigError::missing("FAKE_VERIFY_CODE"));
                }
                None
            }
        };

        let (twilio_account_sid, twilio_auth_token, twilio_verify_service_sid) =
            if phone_provider == PhoneProvider::Twilio {
                (
                    Some(Secret(required("TWILIO_ACCOUNT_SID")?)),
                    Some(Secret(required("TWILIO_AUTH_TOKEN")?)),
                    Some(Secret(required("TWILIO_VERIFY_SERVICE_SID")?)),
                )
            } else {
                (None, None, None)
            };

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

        // Phone identity keys: one deterministic lookup digest and one
        // authenticated-encryption passphrase. Both are secrets with no
        // default, and both must stay stable across restarts.
        let phone_lookup_key = Secret(required("PHONE_LOOKUP_KEY")?);
        let phone_encryption_key = Secret(required("PHONE_ENCRYPTION_KEY")?);

        let assets_dir =
            PathBuf::from(get("APP_ASSETS_DIR").unwrap_or_else(|| DEFAULT_ASSETS_DIR.to_owned()));

        let bind_address =
            get("APP_BIND_ADDRESS").unwrap_or_else(|| DEFAULT_BIND_ADDRESS.to_owned());
        if !is_bind_address(&bind_address) {
            return Err(ConfigError::invalid("APP_BIND_ADDRESS"));
        }

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
            phone_lookup_key,
            phone_encryption_key,
            fake_verify_code,
            assets_dir,
            bind_address,
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

    /// Application database URL. The value is a secret: callers must not log
    /// it, and the redacted [`fmt::Debug`] rendering never carries it.
    #[must_use]
    pub fn database_url(&self) -> &str {
        self.database_url.expose()
    }

    /// Key for the deterministic phone lookup digest.
    #[must_use]
    pub fn phone_lookup_key(&self) -> &str {
        self.phone_lookup_key.expose()
    }

    /// Passphrase for the authenticated phone ciphertext.
    #[must_use]
    pub fn phone_encryption_key(&self) -> &str {
        self.phone_encryption_key.expose()
    }

    /// Fixed proof code of the deterministic fake provider, when it is the
    /// selected provider. The value is a secret: callers must not log it.
    #[must_use]
    pub fn fake_verify_code(&self) -> Option<&str> {
        self.fake_verify_code.as_ref().map(Secret::expose)
    }

    /// Directory holding the built web bundle.
    #[must_use]
    pub fn assets_dir(&self) -> &Path {
        &self.assets_dir
    }

    /// Address the served application binds to.
    #[must_use]
    pub fn bind_address(&self) -> &str {
        &self.bind_address
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
            .field("phone_lookup_key", &self.phone_lookup_key)
            .field("phone_encryption_key", &self.phone_encryption_key)
            .field(
                "fake_verify_code",
                &self.fake_verify_code.as_ref().map(|_| "set"),
            )
            .field("assets_dir", &self.assets_dir)
            .field("bind_address", &self.bind_address)
            .finish()
    }
}

fn is_postgres_url(value: &str) -> bool {
    value.starts_with("postgres://") || value.starts_with("postgresql://")
}

/// Accept `host:port` with a bounded TCP port. The host may be a name, an
/// IPv4 literal, or a bracketed IPv6 literal; the port is always numeric.
fn is_bind_address(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return false;
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_graphic() && c != ' ') {
        return false;
    }
    let Ok(port) = port.parse::<u32>() else {
        return false;
    };
    (1..=65_535).contains(&port)
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
