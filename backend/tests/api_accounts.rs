//! Account-route acceptance (P04-T03): registration, challenges, confirmations.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real axum routes over real PostgreSQL 18.6 with the
//! deterministic fake provider. Proves:
//! - a minimal valid payload registers without CPF, photo, address, or birth
//!   date, while unrelated or protected fields are refused;
//! - wrong or expired proof never activates;
//! - concurrent registrations cannot create duplicate current-phone accounts;
//! - challenge/confirmation round trip activates exactly once with one shared
//!   fact set, and every surface stays free of phone material.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::accounts::{routes, AccountsState};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    routes(AccountsState::new(
        db.pool().clone(),
        Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE)),
        "p04t03-test-only-lookup-key".to_owned(),
        "p04t03-test-only-encryption-key".to_owned(),
    ))
}

fn register_body(phone: &str) -> Value {
    json!({
        "display_name": "Test User",
        "phone": phone,
        "city": "Campinas",
        "region": "SP",
        "policy_version": "v1",
    })
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

async fn user_state(db: &TestDatabase, phone: &str) -> Option<String> {
    use procurali_backend::persistence::users::canonicalize_phone;
    let canonical = canonicalize_phone(phone).expect("test phones shape");
    sqlx::query_scalar(
        "SELECT state FROM users WHERE phone_lookup = encode(hmac(convert_to($1, 'UTF8'),
         convert_to('p04t03-test-only-lookup-key', 'UTF8'), 'sha256'), 'hex')
         AND deleted_at IS NULL",
    )
    .bind(canonical)
    .fetch_optional(db.pool())
    .await
    .expect("state reads")
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

#[tokio::test]
async fn minimal_valid_payload_registers_without_extra_documents() {
    let db = TestDatabase::create("p04t03_minimal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t03_minimal_"),
        "known suite identity in the database name"
    );
    // Exactly the five accepted fields: pending account, no session.
    let (status, body) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        register_body("+5511911111111"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["account_state"], "pending");
    assert!(
        body["account_id"].as_str().is_some_and(|id| id.len() == 36),
        "receipt carries the identifier"
    );
    assert_eq!(
        body.as_object().expect("object").keys().count(),
        2,
        "receipt carries nothing else, no phone"
    );
    assert_eq!(
        user_state(&db, "+5511911111111").await.as_deref(),
        Some("pending")
    );

    // CPF, documents, photos, birth dates, addresses, and roles are refused.
    for extra in [
        json!({"cpf": "12345678901"}),
        json!({"document": "RG-1"}),
        json!({"photo": "http://127.0.0.1/x.png"}),
        json!({"birth_date": "1990-01-01"}),
        json!({"address": "Rua X, 1"}),
        json!({"role": "admin"}),
        json!({"state": "active"}),
        json!({"owner_id": "123e4567-e89b-42d3-a456-426614174001"}),
    ] {
        let mut payload = register_body("+5511911111111");
        for (key, value) in extra.as_object().expect("object") {
            payload[key] = value.clone();
        }
        let (status, body) = post_json(test_app(&db), "/api/v1/accounts", payload).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_field");
    }
    // Missing and malformed fields are refused; nothing extra was stored.
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        json!({"display_name": "Test User"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        json!({
            "display_name": "Test User",
            "phone": "not-a-phone",
            "city": "Campinas",
            "region": "SP",
            "policy_version": "v1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(table_count(db.pool(), "users").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn wrong_or_expired_proof_never_activates() {
    let db = TestDatabase::create("p04t03_proof")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t03_proof_"),
        "known suite identity in the database name"
    );
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        register_body("+5511922222222"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges",
        json!({"phone": "+5511922222222"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // Wrong codes never activate, through the exact error shape.
    let (status, body) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": "+5511922222222", "code": "000000"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "verification_failed");
    assert_eq!(
        user_state(&db, "+5511922222222").await.as_deref(),
        Some("pending")
    );

    // Unknown numbers share the same generic refusal (no enumeration).
    let (status, body) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": "+5511999999999", "code": FAKE_CODE}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "verification_failed");

    // An elapsed window reports expiry and never activates.
    sqlx::query("UPDATE phone_challenges SET expires_at = now() - interval '1 minute'")
        .execute(db.pool())
        .await
        .expect("window elapses");
    let (status, body) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": "+5511922222222", "code": FAKE_CODE}),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(body["code"], "expired");
    assert_eq!(
        user_state(&db, "+5511922222222").await.as_deref(),
        Some("pending")
    );
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_registrations_create_no_duplicate_phone_accounts() {
    let db = TestDatabase::create("p04t03_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t03_race_"),
        "known suite identity in the database name"
    );
    let barrier = Arc::new(tokio::sync::Barrier::new(4));
    let attempt = |index: usize, barrier: Arc<tokio::sync::Barrier>| {
        let app = test_app(&db);
        let mut payload = register_body("+5511933333333");
        payload["display_name"] = json!(format!("Race User {index}"));
        async move {
            barrier.wait().await;
            post_json(app, "/api/v1/accounts", payload).await.0
        }
    };
    let outcomes = tokio::join!(
        attempt(0, Arc::clone(&barrier)),
        attempt(1, Arc::clone(&barrier)),
        attempt(2, Arc::clone(&barrier)),
        attempt(3, Arc::clone(&barrier)),
    );
    let created = [outcomes.0, outcomes.1, outcomes.2, outcomes.3]
        .into_iter()
        .filter(|status| *status == StatusCode::CREATED)
        .count();
    let refused = [outcomes.0, outcomes.1, outcomes.2, outcomes.3]
        .into_iter()
        .filter(|status| *status == StatusCode::CONFLICT)
        .count();
    assert_eq!(created, 1, "exactly one registration wins");
    assert_eq!(refused, 3, "losers take the recovery path");
    assert_eq!(table_count(db.pool(), "users").await, 1);

    // The recovery refusal names the remedy without another owner's history.
    let (_, body) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        register_body("+5511933333333"),
    )
    .await;
    assert!(body.to_string().contains("recovery"));
    assert!(body.get("account_id").is_none());
    assert!(!body.to_string().contains("Race User"));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn challenge_and_confirmation_round_trip() {
    let db = TestDatabase::create("p04t03_roundtrip")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t03_roundtrip_"),
        "known suite identity in the database name"
    );
    let (status, _) = post_json(
        test_app(&db),
        "/api/v1/accounts",
        register_body("+5511944444444"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // Unknown numbers get the same generic challenge response, side-effect free.
    let (status, unknown) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges",
        json!({"phone": "+5511999999999"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(unknown["status"], "challenge_sent");
    assert_eq!(table_count(db.pool(), "phone_challenges").await, 0);

    let (status, sent) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges",
        json!({"phone": "+5511944444444"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(sent["status"], "challenge_sent");
    assert_eq!(sent.as_object().expect("object").keys().count(), 1);

    // The confirmed code activates exactly once with one shared fact set.
    let (status, active) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": "+5511944444444", "code": FAKE_CODE}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["account_state"], "active");
    let (status, replayed) = post_json(
        test_app(&db),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": "+5511944444444", "code": FAKE_CODE}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["account_state"], "active");
    assert_eq!(
        user_state(&db, "+5511944444444").await.as_deref(),
        Some("active")
    );
    assert_eq!(table_count(db.pool(), "users").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 2);

    // No response anywhere carries phone material.
    for payload in [sent, active, replayed, unknown] {
        assert!(!payload.to_string().contains("55119"));
    }
    db.cleanup().await.expect("suite cleans up");
}
