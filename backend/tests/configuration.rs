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
    let mut values = BTreeMap::from([
        pair("PROCURALI_ENV", "production"),
        pair(
            "DATABASE_URL",
            "postgres://app-user@db.internal:5432/procurali",
        ),
        pair("PHONE_PROVIDER", "fake"),
        pair(
            "SESSION_KEY",
            "production-synthetic-key-of-sufficient-length-0123456789",
        ),
        pair("PUBLIC_ORIGIN", "https://example.invalid"),
    ]);
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
    let mut values = BTreeMap::from([
        pair("PROCURALI_ENV", "production"),
        pair(
            "DATABASE_URL",
            "postgres://app-user@db.internal:5432/procurali",
        ),
        pair("PHONE_PROVIDER", "twilio"),
        pair("TWILIO_ACCOUNT_SID", "synthetic-sid"),
        pair("TWILIO_AUTH_TOKEN", "synthetic-token"),
        pair("TWILIO_VERIFY_SERVICE_SID", "synthetic-service"),
        pair("SESSION_KEY", "too-short"),
        pair("PUBLIC_ORIGIN", "http://example.invalid"),
    ]);
    let error = Settings::from_map(&values).expect_err("short production key must fail");
    assert!(format!("{error}").contains("SESSION_KEY"));

    values.insert(
        "SESSION_KEY".to_owned(),
        "production-synthetic-key-of-sufficient-length-0123456789".to_owned(),
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
fn test_clock_switch_is_rejected_outside_test_doubles() {
    let values = BTreeMap::from([
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
        pair("TEST_FIXED_CLOCK", "2026-10-15T12:00:00Z"),
    ]);
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
    assert!(rendered.contains("Development"));
}
