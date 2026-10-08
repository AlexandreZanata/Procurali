//! API-contract acceptance: exact error codes/statuses, DTO allowlists with
//! canary sweeps, overposting refusal, and strict identifier parsing.

use procurali_backend::domain::{
    condition::ConditionError, location::LocationError, money::MoneyError, text::TextError,
    time::TimeError,
};
use procurali_backend::http::dto::{
    parse_id, CreateRequestBody, PublicOfferSummary, PublicRequest,
};
use procurali_backend::http::errors::{ApiError, Code};
use procurali_backend::operations::config::ConfigError;
use procurali_backend::persistence::pool::PoolError;
use serde_json::Value;
use uuid::Uuid;

fn registry_codes() -> Vec<String> {
    let text = std::fs::read_to_string("../contracts/domain-vectors.json")
        .expect("frozen error registry is readable");
    let registry: Value = serde_json::from_str(&text).expect("registry parses");
    registry["errors"]["codes"]
        .as_array()
        .expect("code list")
        .iter()
        .map(|code| code.as_str().expect("string code").to_owned())
        .collect()
}

#[tokio::test]
async fn error_codes_and_statuses_match_fixtures() {
    // Every code the implementation can emit exists in the frozen registry.
    let registry = registry_codes();
    let emitted = vec![
        ApiError::from(MoneyError::Empty),
        ApiError::from(MoneyError::Malformed),
        ApiError::from(MoneyError::NonPositive),
        ApiError::from(MoneyError::Overprecision),
        ApiError::from(MoneyError::Overflow),
        ApiError::from(ConditionError),
        ApiError::from(TextError),
        ApiError::from(LocationError::Empty),
        ApiError::from(LocationError::Invalid),
        ApiError::from(TimeError::Empty),
        ApiError::from(TimeError::Malformed),
        ApiError::from(ConfigError::Missing("SESSION_KEY")),
        ApiError::from(PoolError::ConnectionFailed),
        ApiError::from(PoolError::QueryFailed),
        ApiError::from(PoolError::InvalidUrl),
        ApiError::from(procurali_backend::operations::telemetry::ErrorCode::Unavailable),
        ApiError::new(Code::NotFound, "no such route"),
        ApiError::new(Code::ForbiddenOwner, "not the owner"),
        ApiError::new(Code::DuplicateIntent, "already recorded"),
    ];
    assert!(!registry.is_empty());
    for error in &emitted {
        assert!(
            registry.contains(&error.code().to_owned()),
            "code {} is registered",
            error.code()
        );
    }
    // Exact status per code family, rendered through the response path.
    let cases = [
        (
            ApiError::from(MoneyError::Malformed),
            400u16,
            "malformed_amount",
        ),
        (
            ApiError::from(MoneyError::Overprecision),
            422,
            "overprecision",
        ),
        (
            ApiError::from(MoneyError::Overflow),
            422,
            "amount_too_large",
        ),
        (
            ApiError::new(Code::NotFound, "no such route"),
            404,
            "not_found",
        ),
        (
            ApiError::from(PoolError::ConnectionFailed),
            503,
            "unavailable",
        ),
        (ApiError::from(ConfigError::Missing("X")), 500, "internal"),
        (
            ApiError::new(Code::ForbiddenOwner, "not the owner"),
            403,
            "forbidden_owner",
        ),
    ];
    for (error, status, code) in cases {
        let response = axum::response::IntoResponse::into_response(error);
        assert_eq!(response.status().as_u16(), status);
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("error body reads");
        let rendered: Value = serde_json::from_slice(&bytes).expect("error JSON parses");
        assert_eq!(rendered["code"], code);
        assert!(rendered["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()));
    }
    // The fields map renders exactly alongside code/message.
    let with_field =
        ApiError::from(MoneyError::Overprecision).with_field("budget", Code::Overprecision);
    let rendered = serde_json::to_value(&with_field).expect("error serializes");
    assert_eq!(rendered["code"], "overprecision");
    assert_eq!(rendered["fields"]["budget"], "overprecision");
    assert_eq!(
        rendered.as_object().expect("object").keys().count(),
        3,
        "no extra keys in the error shape"
    );
}

#[test]
fn public_json_contains_only_allowlisted_fields() {
    const SESSION_CANARY: &str = "sess-CANARY-material-never-logged";
    let request = PublicRequest::new(
        Uuid::nil(),
        "Geladeira Consul 340L".to_owned(),
        "600.00".to_owned(),
        "used".to_owned(),
        "refrigerators".to_owned(),
        "sp-sao-paulo".to_owned(),
        "Jardim Primavera".to_owned(),
        "2026-10-08T12:00:00Z".to_owned(),
        "2026-10-15T12:00:00Z".to_owned(),
        1,
        Uuid::nil(),
    );
    let rendered: Value = serde_json::to_value(&request).expect("request serializes");
    let keys: Vec<&str> = rendered
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "budget",
            "category",
            "city",
            "condition",
            "cycle",
            "deadline",
            "id",
            "published_at",
            "region_label",
            "revision",
            "title"
        ],
        "exact public allowlist, alphabetically stable"
    );
    let text = serde_json::to_string(&rendered).expect("text renders");
    assert!(!text.contains(SESSION_CANARY));
    assert!(!text.contains("phone"));
    assert!(!text.contains("session"));
    assert!(!text.contains("reporter"));

    let offer = PublicOfferSummary::new(
        Uuid::nil(),
        "520.00".to_owned(),
        "used".to_owned(),
        Uuid::nil(),
    );
    let rendered: Value = serde_json::to_value(&offer).expect("offer serializes");
    let keys: Vec<&str> = rendered
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["condition", "id", "price", "request_revision"]);
    let text = serde_json::to_string(&rendered).expect("text renders");
    assert!(!text.contains("phone"));
    assert!(!text.contains("buyer"));
}

#[test]
fn caller_cannot_smuggle_ownership_state_or_role() {
    // Protected fields are refused, not stripped.
    for smuggled in [
        r#"{"title":"T","budget":"1.00","condition":"used","category":"c","city":"s","region":"r","owner_id":"00000000-0000-0000-0000-000000000000"}"#,
        r#"{"title":"T","budget":"1.00","condition":"used","category":"c","city":"s","region":"r","state":"active"}"#,
        r#"{"title":"T","budget":"1.00","condition":"used","category":"c","city":"s","region":"r","role":"admin"}"#,
    ] {
        assert!(
            serde_json::from_str::<CreateRequestBody>(smuggled).is_err(),
            "protected field refused: {smuggled}"
        );
    }
    // The documented body parses with exactly its six fields.
    let body: CreateRequestBody = serde_json::from_str(
        r#"{"title":"Geladeira","budget":"600.00","condition":"used","category":"refrigerators","city":"sp-sao-paulo","region":"Jardim Primavera"}"#,
    )
    .expect("documented body parses");
    assert_eq!(body.title, "Geladeira");
    assert_eq!(body.region, "Jardim Primavera");
}

#[test]
fn malformed_identifiers_are_rejected() {
    assert!(parse_id("not-a-uuid").is_err());
    assert!(parse_id("").is_err());
    assert!(parse_id("00000000-0000-0000-0000-00000000000Z").is_err());
    let valid = parse_id("123e4567-e89b-12d3-a456-426614174000").expect("UUIDv1 parses");
    assert_eq!(valid.to_string(), "123e4567-e89b-12d3-a456-426614174000");
    let error = parse_id("nope").expect_err("malformed id fails");
    assert_eq!(error.code(), "invalid_field");
}
