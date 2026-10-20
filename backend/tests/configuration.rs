//! Configuration acceptance: synthetic development config succeeds; missing
//! secrets, bad URLs, production fakes, and test-only switches fail clearly
//! without printing values.

use procurali_backend::operations::config::{Environment, PhoneProvider, Settings};
use std::collections::BTreeMap;

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}

/// Minimal synthetic development configuration. Values are obviously fake and
/// carry no real credential.
fn development_values() -> BTreeMap<String, String> {
    BTreeMap::from([
        pair("PROCURALI_ENV", "development"),
        pair(
            "DATABASE_URL",
            "postgres://dev-user@localhost:5432/procurali_dev",
        ),
        pair(
            "TEST_DATABASE_URL",
            "postgres://dev-user@localhost:5432/procurali_test_scaffold",
        ),
        pair("SESSION_KEY", "dev-only-synthetic-not-a-secret"),
        pair("PHONE_LOOKUP_KEY", "dev-only-synthetic-lookup-key"),
        pair("PHONE_ENCRYPTION_KEY", "dev-only-synthetic-encryption-key"),
        pair("FAKE_VERIFY_CODE", "135790"),
    ])
}

/// Synthetic production configuration with the live provider selected.
fn production_values() -> BTreeMap<String, String> {
    BTreeMap::from([
        pair("PROCURALI_ENV", "production"),
        pair(
            "DATABASE_URL",
            "postgres://app-user@db.internal:5432/procurali",
        ),
        pair("PHONE_PROVIDER", "twilio"),
        pair("TWILIO_ACCOUNT_SID", "synthetic-sid"),
        pair("TWILIO_AUTH_TOKEN", "synthetic-token"),
        pair("TWILIO_VERIFY_SERVICE_SID", "synthetic-service"),
        pair(
            "SESSION_KEY",
            "production-synthetic-key-of-sufficient-length-0123456789",
        ),
        pair("PUBLIC_ORIGIN", "https://example.invalid"),
        pair("PHONE_LOOKUP_KEY", "production-synthetic-lookup-key"),
        pair(
            "PHONE_ENCRYPTION_KEY",
            "production-synthetic-encryption-key",
        ),
    ])
}

#[test]
fn synthetic_development_configuration_succeeds() {
    let settings = Settings::from_map(&development_values()).expect("dev config must load");
    assert_eq!(settings.environment(), Environment::Development);
    assert_eq!(settings.phone_provider(), PhoneProvider::Fake);
    assert!(settings.allows_test_doubles());
}

#[test]
fn missing_session_key_fails_without_printing_values() {
    let mut values = development_values();
    values.insert(
        "SESSION_KEY".to_owned(),
        "canary-session-value-must-never-appear".to_owned(),
    );
    values.remove("SESSION_KEY");
    let error = Settings::from_map(&values).expect_err("missing secret must fail startup");
    let rendered = format!("{error:?} {error}");
    assert!(rendered.contains("SESSION_KEY"));
    assert!(!rendered.contains("canary-session-value-must-never-appear"));
}

#[test]
fn invalid_database_url_fails_clearly() {
    let mut values = development_values();
    values.insert(
        "DATABASE_URL".to_owned(),
        "mysql://user@localhost:3306/procurali_dev".to_owned(),
    );
    let error = Settings::from_map(&values).expect_err("non-postgres URL must be refused");
    assert!(format!("{error}").contains("DATABASE_URL"));
}

#[test]
fn test_database_equal_to_application_database_is_refused() {
    let mut values = development_values();
    values.insert(
        "TEST_DATABASE_URL".to_owned(),
        "postgres://dev-user@localhost:5432/procurali_dev".to_owned(),
    );
    let error =
        Settings::from_map(&values).expect_err("test database must differ from app database");
    assert!(format!("{error}").contains("TEST_DATABASE_URL"));
}

#[test]
fn production_fake_provider_is_refused_even_when_requested() {
    let mut values = production_values();
    values.insert(pair("PHONE_PROVIDER", "fake").0, "fake".to_owned());
    values.insert(pair("FAKE_VERIFY_CODE", "135790").0, "135790".to_owned());
    let error = Settings::from_map(&values).expect_err("production must refuse the fake");
    assert!(format!("{error}").contains("PHONE_PROVIDER"));
    // A second enabling flag must not override the refusal: no such flag exists,
    // so even an unknown extra variable changes nothing.
    values.insert(
        pair("PROCURALI_ALLOW_FAKE_IDENTITY", "true").0,
        "true".to_owned(),
    );
    let error = Settings::from_map(&values).expect_err("production still refuses the fake");
    assert!(format!("{error}").contains("PHONE_PROVIDER"));
}

#[test]
fn production_requires_https_origin_and_long_session_key() {
    let mut values = production_values();
    values.insert(pair("SESSION_KEY", "too-short").0, "too-short".to_owned());
    let error = Settings::from_map(&values).expect_err("short production key must fail");
    assert!(format!("{error}").contains("SESSION_KEY"));

    values.insert(
        "SESSION_KEY".to_owned(),
        "production-synthetic-key-of-sufficient-length-0123456789".to_owned(),
    );
    values.insert(
        pair("PUBLIC_ORIGIN", "http://example.invalid").0,
        "http://example.invalid".to_owned(),
    );
    let error = Settings::from_map(&values).expect_err("plain-http origin must fail");
    assert!(format!("{error}").contains("PUBLIC_ORIGIN"));

    values.insert(
        "PUBLIC_ORIGIN".to_owned(),
        "https://example.invalid".to_owned(),
    );
    let settings = Settings::from_map(&values).expect("valid production config must load");
    assert_eq!(settings.environment(), Environment::Production);
    assert_eq!(settings.phone_provider(), PhoneProvider::Twilio);
    assert_eq!(settings.public_origin(), Some("https://example.invalid"));
}

#[test]
fn phone_keys_have_no_default_and_are_never_printed() {
    let mut values = development_values();
    values.insert(
        pair("PHONE_LOOKUP_KEY", "canary-lookup-key-must-never-appear").0,
        "canary-lookup-key-must-never-appear".to_owned(),
    );
    values.remove("PHONE_LOOKUP_KEY");
    let error =
        Settings::from_map(&values).expect_err("a missing phone lookup key must fail startup");
    assert!(format!("{error}").contains("PHONE_LOOKUP_KEY"));
    assert!(!format!("{error:?} {error}").contains("canary"));

    let mut values = development_values();
    values.remove("PHONE_ENCRYPTION_KEY");
    let error = Settings::from_map(&values).expect_err("a missing encryption key must fail");
    assert!(format!("{error}").contains("PHONE_ENCRYPTION_KEY"));
}

#[test]
fn fake_proof_code_belongs_to_the_fake_provider_only() {
    let mut values = production_values();
    values.insert(pair("FAKE_VERIFY_CODE", "135790").0, "135790".to_owned());
    let error = Settings::from_map(&values).expect_err("the live provider takes no fake code");
    assert!(format!("{error}").contains("FAKE_VERIFY_CODE"));

    let mut values = development_values();
    values.remove("FAKE_VERIFY_CODE");
    let error = Settings::from_map(&values).expect_err("the fake provider needs its proof code");
    assert!(format!("{error}").contains("FAKE_VERIFY_CODE"));
}

#[test]
fn bind_address_and_asset_directory_are_validated() {
    let settings = Settings::from_map(&development_values()).expect("dev config must load");
    assert_eq!(settings.bind_address(), "127.0.0.1:8080");
    assert_eq!(settings.assets_dir().to_string_lossy(), "web/dist");

    let mut values = development_values();
    values.insert(
        pair("APP_BIND_ADDRESS", "not-an-address").0,
        "not-an-address".to_owned(),
    );
    let error = Settings::from_map(&values).expect_err("a malformed bind address must fail");
    assert!(format!("{error}").contains("APP_BIND_ADDRESS"));

    values.insert("APP_BIND_ADDRESS".to_owned(), "127.0.0.1:99999".to_owned());
    let error = Settings::from_map(&values).expect_err("an out-of-range port must fail");
    assert!(format!("{error}").contains("APP_BIND_ADDRESS"));

    values.insert("APP_BIND_ADDRESS".to_owned(), "0.0.0.0:8080".to_owned());
    values.insert(
        pair("APP_ASSETS_DIR", "/srv/procurali/web-dist").0,
        "/srv/procurali/web-dist".to_owned(),
    );
    let settings = Settings::from_map(&values).expect("explicit serving values must load");
    assert_eq!(settings.bind_address(), "0.0.0.0:8080");
    assert_eq!(
        settings.assets_dir().to_string_lossy(),
        "/srv/procurali/web-dist"
    );
}

#[test]
fn test_clock_switch_is_rejected_outside_test_doubles() {
    let mut values = production_values();
    values.insert(
        pair("TEST_FIXED_CLOCK", "2026-10-15T12:00:00Z").0,
        "2026-10-15T12:00:00Z".to_owned(),
    );
    let error = Settings::from_map(&values).expect_err("test clock refused in production");
    assert!(format!("{error}").contains("TEST_FIXED_CLOCK"));

    let mut dev = development_values();
    dev.insert(
        "TEST_FIXED_CLOCK".to_owned(),
        "2026-10-15T12:00:00Z".to_owned(),
    );
    let settings = Settings::from_map(&dev).expect("test clock allowed in development");
    assert_eq!(settings.test_fixed_clock(), Some("2026-10-15T12:00:00Z"));

    dev.insert("TEST_FIXED_CLOCK".to_owned(), "not-an-instant".to_owned());
    let error = Settings::from_map(&dev).expect_err("malformed test clock refused");
    assert!(format!("{error}").contains("TEST_FIXED_CLOCK"));
}

#[test]
fn debug_output_never_contains_secret_values() {
    let settings = Settings::from_map(&development_values()).expect("dev config must load");
    let rendered = format!("{settings:?}");
    assert!(!rendered.contains("dev-user"));
    assert!(!rendered.contains("dev-only-synthetic-not-a-secret"));
    assert!(!rendered.contains("procurali_dev"));
    assert!(!rendered.contains("dev-only-synthetic-lookup-key"));
    assert!(!rendered.contains("dev-only-synthetic-encryption-key"));
    assert!(!rendered.contains("135790"));
    assert!(rendered.contains("Development"));
}
