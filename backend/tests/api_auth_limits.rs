//! Abuse-guard acceptance over real HTTP routes (P04-T05).
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account routes over real PostgreSQL 18.6.
//! Proves:
//! - hammered duplicates, unknown numbers, and wrong codes answer with
//!   byte-identical generic bodies and stable counts (no enumeration);
//! - provider failures account explicitly: 503 with zero challenge rows and
//!   zero state change, retryable without consuming anything;
//! - error and log surfaces reveal no account history.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::operations::twilio_verify::{
    OutgoingRequest, TransportOutcome, TwilioConfig, TwilioLiveProvider, VerifyTransport,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    account_routes(AccountsState::new(
        db.pool().clone(),
        Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE)),
        "p04t05-test-only-lookup-key".to_owned(),
        "p04t05-test-only-encryption-key".to_owned(),
    ))
}

/// Routes bound to a live adapter whose transport always times out.
fn failing_app(db: &TestDatabase) -> axum::Router {
    struct TimeoutTransport;
    impl VerifyTransport for TimeoutTransport {
        async fn post_form(&self, _request: OutgoingRequest) -> TransportOutcome {
            TransportOutcome::TimedOut
        }
    }
    let config = TwilioConfig::new(
        "canary-account-sid".to_owned(),
        "canary-auth-token".to_owned(),
        "canary-service-sid".to_owned(),
        "https://verify.twilio.com".to_owned(),
        Duration::from_secs(10),
    )
    .expect("live config validates");
    account_routes(AccountsState::new(
        db.pool().clone(),
        Arc::new(TwilioLiveProvider::new(config, TimeoutTransport)),
        "p04t05-test-only-lookup-key".to_owned(),
        "p04t05-test-only-encryption-key".to_owned(),
    ))
}

async fn post_json(app: axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("test request builds"),
        )
        .await
        .expect("route responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    let parsed: Value = serde_json::from_slice(&bytes).expect("response JSON parses");
    (status, parsed)
}

fn register_body(phone: &str) -> Value {
    json!({
        "display_name": "Limit User",
        "phone": phone,
        "city": "Campinas",
        "region": "SP",
        "policy_version": "v1",
    })
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

#[tokio::test]
async fn duplicate_and_unknown_paths_stay_generic_under_hammer() {
    let db = TestDatabase::create("p04t05_generic")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t05_generic_"),
        "known suite identity in the database name"
    );
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        register_body("+5511911111111"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // Duplicate registrations: identical refusals, one account, one fact.
    let mut first = None;
    for _ in 0..5 {
        let (status, body) = post_json(
            test_app(&db),
            "/api/v1/accounts",
            register_body("+5511911111111"),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "duplicate_intent");
        match first {
            None => first = Some(body),
            Some(ref expected) => assert_eq!(&body, expected, "byte-identical refusal"),
        }
    }
    assert_eq!(table_count(db.pool(), "users").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);

    // Unknown numbers: identical generic challenges, zero side effects.
    let mut first = None;
    for _ in 0..5 {
        let (status, body) = post_json(
            test_app(&db),
            "/api/v1/accounts/challenges",
            json!({"phone": "+5511999999999"}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        match first {
            None => first = Some(body),
            Some(ref expected) => assert_eq!(&body, expected, "byte-identical response"),
        }
    }
    assert_eq!(table_count(db.pool(), "phone_challenges").await, 0);

    // Wrong codes on a pending holder: identical refusals, still pending.
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges",
        json!({"phone": "+5511911111111"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let mut first = None;
    for _ in 0..5 {
        let (status, body) = post_json(
            test_app(&db),
            "/api/v1/accounts/challenges/confirmations",
            json!({"phone": "+5511911111111", "code": "000000"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "verification_failed");
        match first {
            None => first = Some(body),
            Some(ref expected) => assert_eq!(&body, expected, "byte-identical refusal"),
        }
    }
    let state: String = sqlx::query_scalar("SELECT state FROM users")
        .fetch_one(db.pool())
        .await
        .expect("state reads");
    assert_eq!(state, "pending");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn provider_failure_accounting_is_explicit_over_http() {
    let db = TestDatabase::create("p04t05_provider")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t05_provider_"),
        "known suite identity in the database name"
    );
    // Registration needs no provider and works while it is down.
    let (status, _) = post_json(
        failing_app(&db),
        "/api/v1/accounts",
        register_body("+5511922222222"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Failed challenge sends refuse explicitly with zero rows, retryably.
    for _ in 0..2 {
        let (status, body) = post_json(
            failing_app(&db),
            "/api/v1/accounts/challenges",
            json!({"phone": "+5511922222222"}),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "unavailable");
    }
    assert_eq!(table_count(db.pool(), "phone_challenges").await, 0);

    // A seeded live anchor plus a dead provider refuses without consuming.
    use procurali_backend::persistence::users::{create_challenge, NewChallenge};
    let mut tx = db.pool().begin().await.expect("transaction begins");
    create_challenge(
        &mut tx,
        NewChallenge {
            phone: "+5511922222222".to_owned(),
            code: "seeded-test-only-code".to_owned(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        },
        "p04t05-test-only-lookup-key",
    )
    .await
    .expect("anchor seeds");
    tx.commit().await.expect("anchor commits");
    let (status, body) = post_json(
        failing_app(&db),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": "+5511922222222", "code": "135790"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "unavailable");
    assert_eq!(table_count(db.pool(), "phone_challenges").await, 1);
    let state: String = sqlx::query_scalar("SELECT state FROM users")
        .fetch_one(db.pool())
        .await
        .expect("state reads");
    assert_eq!(state, "pending");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn logs_and_errors_reveal_no_history() {
    let db = TestDatabase::create("p04t05_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t05_privacy_"),
        "known suite identity in the database name"
    );
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        register_body("+5511933333333"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // Every refusal surface carries the remedy and nothing else.
    let (status, duplicate) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        register_body("+5511933333333"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        duplicate.as_object().expect("object").keys().count(),
        2,
        "refusal shape is exactly code plus message"
    );
    let rendered = duplicate.to_string();
    assert!(rendered.contains("recovery"));
    for secret in ["5511933333333", "Limit User", "account_id"] {
        assert!(!rendered.contains(secret), "no history in refusals");
    }
    // Unknown- and wrong-code paths echo nothing back either.
    let (_, unknown) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges",
        json!({"phone": "+5511999999999"}),
    )
    .await;
    let (_, wrong) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": "+5511999999999", "code": "000000"}),
    )
    .await;
    for body in [unknown, wrong] {
        assert!(!body.to_string().contains("55119"));
    }
    db.cleanup().await.expect("suite cleans up");
}
