//! Recovery-route acceptance (P04-T08): generic initiation, owner access,
//! recycled-number review.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account and session routes over real
//! PostgreSQL 18.6 with the deterministic fake provider. Proves:
//! - a correct existing owner recovers access with a working session;
//! - unverified requesters get byte-identical generic refusals and zero rows;
//! - a recycled number yields review bookkeeping, never the former owner's
//!   history, while the new subscriber onboards distinctly.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    let lookup = "p04t08-test-only-lookup-key".to_owned();
    let encryption = "p04t08-test-only-encryption-key".to_owned();
    account_routes(AccountsState::new(
        db.pool().clone(),
        Arc::clone(&provider),
        lookup.clone(),
        encryption.clone(),
    ))
    .merge(auth_routes(AuthState::new(
        db.pool().clone(),
        provider,
        lookup,
        encryption,
        None,
    )))
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

async fn post_raw(
    app: axum::Router,
    path: &str,
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = app
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .header("host", "app.test")
                .header("origin", "http://app.test")
                .body(Body::from(body.to_string()))
                .expect("test request builds"),
        )
        .await
        .expect("route responds");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    let parsed: Value = serde_json::from_slice(&bytes).expect("response JSON parses");
    (status, headers, parsed)
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

/// Provision one active account through the real routes; return its id.
async fn provision_active(app: &axum::Router, phone: &str, name: &str) -> uuid::Uuid {
    let (status, body) = post_json(
        app.clone(),
        "/api/v1/accounts",
        json!({
            "display_name": name,
            "phone": phone,
            "city": "Campinas",
            "region": "SP",
            "policy_version": "v1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = post_json(
        app.clone(),
        "/api/v1/accounts/challenges",
        json!({"phone": phone}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, active) = post_json(
        app.clone(),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": phone, "code": FAKE_CODE}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["account_state"], "active");
    body["account_id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses")
}

#[tokio::test]
async fn correct_existing_owner_recovers_access() {
    let db = TestDatabase::create("p04t08_owner")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t08_owner_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let owner = provision_active(&app, "+5511911111111", "Owner").await;

    // Generic initiation, then confirmation restores access with a session.
    let (status, requested) = post_json(
        app.clone(),
        "/api/v1/accounts/recoveries",
        json!({"phone": "+5511911111111"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(requested["status"], "recovery_requested");
    let (status, headers, body) = post_raw(
        app.clone(),
        "/api/v1/accounts/recoveries/confirmations",
        json!({"phone": "+5511911111111", "code": FAKE_CODE}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["account_state"], "active");
    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("recovery issues a cookie");
    assert!(cookie.starts_with("session="));

    // The recovered session authenticates as the same owner, nothing else.
    let response = app
        .oneshot(
            Request::get("/api/v1/sessions/current")
                .header("cookie", cookie.split(';').next().expect("pair"))
                .body(Body::empty())
                .expect("test request builds"),
        )
        .await
        .expect("route responds");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    let current: Value = serde_json::from_slice(&bytes).expect("JSON parses");
    assert_eq!(current["account_id"], owner.to_string().as_str());
    assert_eq!(table_count(db.pool(), "users").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn unverified_requester_gets_no_historical_data() {
    let db = TestDatabase::create("p04t08_generic")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t08_generic_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    provision_active(&app, "+5511922222222", "Owner").await;

    // Known and unknown numbers share one byte-identical initiation body.
    let (_, known) = post_json(
        app.clone(),
        "/api/v1/accounts/recoveries",
        json!({"phone": "+5511922222222"}),
    )
    .await;
    let (status, unknown) = post_json(
        app.clone(),
        "/api/v1/accounts/recoveries",
        json!({"phone": "+5511999999999"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(unknown, known, "no existence oracle");
    assert_eq!(table_count(db.pool(), "phone_challenges").await, 1);

    // Wrong codes, unknown numbers, and malformed bodies share one refusal.
    let mut first = None;
    for (phone, code) in [
        ("+5511922222222", "000000"),
        ("+5511999999999", FAKE_CODE),
        ("+5511999999999", "000000"),
    ] {
        let (status, body) = post_json(
            app.clone(),
            "/api/v1/accounts/recoveries/confirmations",
            json!({"phone": phone, "code": code}),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "verification_failed");
        match first {
            None => first = Some(body),
            Some(ref expected) => assert_eq!(&body, expected, "byte-identical refusal"),
        }
    }
    assert_eq!(table_count(db.pool(), "sessions").await, 0);
    assert_eq!(table_count(db.pool(), "users").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn recycled_number_fixture_gets_review_not_history() {
    let db = TestDatabase::create("p04t08_recycled")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t08_recycled_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let former = provision_active(&app, "+5511933333333", "Former Owner").await;
    sqlx::query("UPDATE users SET state = 'deleted', deleted_at = now() WHERE id = $1")
        .bind(former)
        .execute(db.pool())
        .await
        .expect("former owner deletes");
    // Age the registration anchor past the resend window (no sleeps).
    sqlx::query("UPDATE phone_challenges SET created_at = now() - interval '61 seconds'")
        .execute(db.pool())
        .await
        .expect("anchor ages");

    // The recycled number still initiates generically, then reviews.
    let (status, _) = post_json(
        app.clone(),
        "/api/v1/accounts/recoveries",
        json!({"phone": "+5511933333333"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, headers, body) = post_raw(
        app.clone(),
        "/api/v1/accounts/recoveries/confirmations",
        json!({"phone": "+5511933333333", "code": FAKE_CODE}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "review_pending");
    assert!(
        headers.get("set-cookie").is_none(),
        "review issues no session"
    );
    assert_eq!(table_count(db.pool(), "sessions").await, 0);

    // The review fact lands on the deleted account's stream, digests only.
    let review: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT resource_id, payload::text FROM business_events
          WHERE kind = 'account.recycled_review'",
    )
    .fetch_all(db.pool())
    .await
    .expect("review facts read");
    assert_eq!(review.len(), 1);
    assert_eq!(review[0].0, former);
    assert!(!review[0].1.contains("55119"));

    // The new subscriber onboards as a distinct account with empty history.
    let (status, body) = post_json(
        app.clone(),
        "/api/v1/accounts",
        json!({
            "display_name": "New Subscriber",
            "phone": "+5511933333333",
            "city": "Valinhos",
            "region": "SP",
            "policy_version": "v1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let fresh: uuid::Uuid = body["account_id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses");
    assert_ne!(fresh, former, "identifiers stay separated");
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM business_events WHERE resource_id = $1 AND kind = 'account.phone_changed'",
    )
    .bind(fresh)
    .fetch_all(db.pool())
    .await
    .expect("history reads");
    assert!(history.is_empty(), "no transferred history");
    db.cleanup().await.expect("suite cleans up");
}
