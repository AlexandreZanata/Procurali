//! Professional-profile acceptance (P04-T09): free declaration, no privilege.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real profile, account, and session routes over real
//! PostgreSQL 18.6 with the deterministic fake provider. Proves:
//! - a free declared professional keeps ordinary capabilities with no payment
//!   anywhere in the flow or schema;
//! - self-declaration grants no staff permission or verification badge;
//! - strangers cannot edit and restricted accounts cannot bypass state checks.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::professional_profile::profile_for;
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::profiles::{routes as profile_routes, ProfilesState};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    let lookup = "p04t09-test-only-lookup-key".to_owned();
    let encryption = "p04t09-test-only-encryption-key".to_owned();
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
    .merge(profile_routes(ProfilesState::new(db.pool().clone())))
}

async fn request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("host", "app.test")
        .header("origin", "http://app.test");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let body = body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
    let response = app
        .oneshot(builder.body(body).expect("test request builds"))
        .await
        .expect("route responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    if bytes.is_empty() {
        return (status, Value::Null);
    }
    let parsed: Value = serde_json::from_slice(&bytes).expect("response JSON parses");
    (status, parsed)
}

/// Provision one active account through the real routes; return its id.
async fn provision_active(app: &axum::Router, phone: &str, name: &str) -> uuid::Uuid {
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/v1/accounts",
        Some(json!({
            "display_name": name,
            "phone": phone,
            "city": "Campinas",
            "region": "SP",
            "policy_version": "v1",
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/v1/accounts/challenges",
        Some(json!({"phone": phone})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, active) = request(
        app.clone(),
        "POST",
        "/api/v1/accounts/challenges/confirmations",
        Some(json!({"phone": phone, "code": FAKE_CODE})),
        None,
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

/// Log one provisioned phone in; return its cookie pair.
async fn login_cookie(app: axum::Router, phone: &str) -> String {
    let response = app
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("content-type", "application/json")
                .header("host", "app.test")
                .header("origin", "http://app.test")
                .body(Body::from(
                    json!({"phone": phone, "code": FAKE_CODE}).to_string(),
                ))
                .expect("test request builds"),
        )
        .await
        .expect("login responds");
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("login sets a cookie");
    set_cookie
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

fn declare_body() -> Value {
    json!({
        "business_name": "Test Shop",
        "business_type": "shop",
        "city": "Campinas",
        "region": "SP",
    })
}

#[tokio::test]
async fn free_declared_professional_keeps_ordinary_capabilities() {
    let db = TestDatabase::create("p04t09_free")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t09_free_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let user = provision_active(&app, "+5511911111111", "Seller").await;
    let cookie = login_cookie(app.clone(), "+5511911111111").await;

    // Free declaration stores exactly the factual label, nothing payable.
    let (status, body) = request(
        app.clone(),
        "PUT",
        "/api/v1/profiles/professional",
        Some(declare_body()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.as_object().expect("object").keys().count(),
        4,
        "label carries exactly the four public fields"
    );
    assert_eq!(body["business_name"], "Test Shop");
    let stored = profile_for(db.pool(), user)
        .await
        .expect("profile reads")
        .expect("declaration persists");
    assert_eq!(stored.business_type, "shop");

    // Payment, badge, role, and state fields do not exist on this boundary.
    for extra in [
        json!({"plan": "radar-pro"}),
        json!({"subscription": "paid"}),
        json!({"badge": "verified"}),
        json!({"verified": true}),
        json!({"role": "staff"}),
        json!({"state": "active"}),
    ] {
        let mut payload = declare_body();
        for (key, value) in extra.as_object().expect("object") {
            payload[key] = value.clone();
        }
        let (status, body) = request(
            app.clone(),
            "PUT",
            "/api/v1/profiles/professional",
            Some(payload),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_field");
    }
    // Off-vocabulary types and phone-shaped names are refused.
    for payload in [
        json!({
            "business_name": "Test Shop",
            "business_type": "certified",
            "city": "Campinas",
            "region": "SP",
        }),
        json!({
            "business_name": "Call +5511911111111",
            "business_type": "shop",
            "city": "Campinas",
            "region": "SP",
        }),
    ] {
        let (status, _) = request(
            app.clone(),
            "PUT",
            "/api/v1/profiles/professional",
            Some(payload),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Ordinary capabilities are untouched: the session still authenticates,
    // the schema holds no payment column, and the lifecycle round-trips free.
    let (status, _) = request(
        app.clone(),
        "GET",
        "/api/v1/sessions/current",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for table in ["users", "professional_profiles"] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns WHERE table_name = $1",
        )
        .bind(table)
        .fetch_all(db.pool())
        .await
        .expect("schema introspection");
        for forbidden in ["subscription", "plan", "badge", "verified", "volume"] {
            assert!(
                !columns.iter().any(|column| column.contains(forbidden)),
                "no {forbidden} column on {table}"
            );
        }
    }
    let (status, updated) = request(
        app.clone(),
        "PUT",
        "/api/v1/profiles/professional",
        Some(json!({
            "business_name": "Test Shop 2",
            "business_type": "merchant",
            "city": "Valinhos",
            "region": "SP",
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["business_name"], "Test Shop 2");
    let (status, _) = request(
        app.clone(),
        "DELETE",
        "/api/v1/profiles/me/professional",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(profile_for(db.pool(), user).await.expect("reads").is_none());
    let (status, _) = request(
        app.clone(),
        "PUT",
        "/api/v1/profiles/professional",
        Some(declare_body()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "re-declaration stays free");
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM business_events WHERE resource_id = $1 ORDER BY occurred_at",
    )
    .bind(user)
    .fetch_all(db.pool())
    .await
    .expect("facts read");
    for expected in [
        "account.registered",
        "account.activated",
        "account.professional_declared",
        "account.professional_updated",
        "account.professional_withdrawn",
        "account.professional_declared",
    ] {
        assert!(
            kinds.contains(&expected.to_owned()),
            "fact {expected} recorded"
        );
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn declaration_grants_no_staff_or_badge() {
    let db = TestDatabase::create("p04t09_privilege")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t09_privilege_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let plain = provision_active(&app, "+5511922222222", "Plain").await;
    let seller = provision_active(&app, "+5511933333333", "Seller").await;
    let cookie = login_cookie(app.clone(), "+5511933333333").await;
    let (status, _) = request(
        app.clone(),
        "PUT",
        "/api/v1/profiles/professional",
        Some(declare_body()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Declared and plain accounts authenticate identically; no role, staff,
    // badge, or verified marker appears anywhere.
    let plain_cookie = login_cookie(app.clone(), "+5511922222222").await;
    for cookie in [&cookie, &plain_cookie] {
        let (status, body) = request(
            app.clone(),
            "GET",
            "/api/v1/sessions/current",
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body.as_object().expect("object").keys().count(),
            2,
            "account receipt carries no privilege marker"
        );
    }
    assert!(profile_for(db.pool(), plain)
        .await
        .expect("reads")
        .is_none());
    let seller_profile = profile_for(db.pool(), seller)
        .await
        .expect("reads")
        .expect("declaration persists");
    let rendered = format!("{seller_profile:?}");
    for marker in ["staff", "badge", "verified", "admin", "role", "subscri"] {
        assert!(!rendered.to_lowercase().contains(marker));
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn stranger_and_restricted_cannot_bypass() {
    let db = TestDatabase::create("p04t09_guards")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t09_guards_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let owner = provision_active(&app, "+5511944444444", "Owner").await;
    let other = provision_active(&app, "+5511955555555", "Other").await;
    let cookie = login_cookie(app.clone(), "+5511944444444").await;

    // Missing and forged sessions answer 401 on both methods.
    for cookie in [
        None,
        Some("session=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
    ] {
        for method in ["PUT", "DELETE"] {
            let path = if method == "PUT" {
                "/api/v1/profiles/professional"
            } else {
                "/api/v1/profiles/me/professional"
            };
            let body = (method == "PUT").then(declare_body);
            let (status, payload) = request(app.clone(), method, path, body, cookie).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(payload["code"], "unauthenticated");
        }
    }
    // Only self routes exist: the stranger's account gains nothing.
    let (status, _) = request(
        app.clone(),
        "PUT",
        "/api/v1/profiles/professional",
        Some(declare_body()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(profile_for(db.pool(), other)
        .await
        .expect("reads")
        .is_none());
    assert!(profile_for(db.pool(), owner)
        .await
        .expect("reads")
        .is_some());

    // Restricted accounts keep no declaration privilege.
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(owner)
        .execute(db.pool())
        .await
        .expect("suspension applies");
    for method in ["PUT", "DELETE"] {
        let path = if method == "PUT" {
            "/api/v1/profiles/professional"
        } else {
            "/api/v1/profiles/me/professional"
        };
        let body = (method == "PUT").then(declare_body);
        let (status, _) = request(app.clone(), method, path, body, Some(&cookie)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    // Withdrawal drops the label while the row and history survive.
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(owner)
        .execute(db.pool())
        .await
        .expect("restoration applies");
    let (status, _) = request(
        app.clone(),
        "DELETE",
        "/api/v1/profiles/me/professional",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(profile_for(db.pool(), owner)
        .await
        .expect("reads")
        .is_none());
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM professional_profiles WHERE user_id = $1 AND withdrawn_at IS NOT NULL",
    )
    .bind(owner)
    .fetch_one(db.pool())
    .await
    .expect("rows read");
    assert_eq!(rows, 1, "withdrawal preserves the row");
    db.cleanup().await.expect("suite cleans up");
}
