//! Production composition acceptance (P21-T01): the served router.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Builds the real production composition root — every business
//! boundary merged into one router under the production bounds — and drives
//! it over real PostgreSQL with the deterministic fake provider. Proves:
//! - the buyer chain works over the composed router (registration, proof,
//!   session, draft, publication, public projection);
//! - strangers and anonymous callers learn nothing, exactly as the
//!   per-boundary suites prove;
//! - private responses carry the privacy headers, public assets keep their
//!   explicit caching, and an unknown route keeps the stable refusal;
//! - liveness and readiness answer for the served process.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::app::Application;
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p21t01-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p21t01-test-only-encryption-key";

/// The production composition over one disposable database.
fn test_app(db: &TestDatabase) -> axum::Router {
    Application::new(
        db.pool().clone(),
        Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE)),
        LOOKUP_KEY.to_owned(),
        ENCRYPTION_KEY.to_owned(),
        None,
        PathBuf::from("web/dist"),
    )
    .router()
}

/// Seed the public catalogs the request publication needs.
async fn seed_catalog(pool: &sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", true)
        .await
        .expect("fixture city stores");
    upsert_region(&mut tx, "campinas", "centro", "Centro")
        .await
        .expect("fixture region stores");
    tx.commit().await.expect("fixture catalogs commit");
}

async fn call(
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

/// Register and activate one account; return its id.
async fn provision_active(app: &axum::Router, phone: &str, name: &str) -> uuid::Uuid {
    let (status, created) = call(
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
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/v1/accounts/challenges",
        Some(json!({"phone": phone})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, active) = call(
        app.clone(),
        "POST",
        "/api/v1/accounts/challenges/confirmations",
        Some(json!({"phone": phone, "code": FAKE_CODE})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["account_state"], "active");
    created["account_id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses")
}

/// Log one provisioned phone in; return its session cookie.
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
    response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("login sets a cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

fn draft_body() -> Value {
    json!({
        "title": "Refrigerator",
        "category_code": "home_appliances",
        "budget": "520.00",
        "condition": "either",
        "city_code": "campinas",
        "region_code": "centro",
        "notes": "Preferably frost-free.",
    })
}

#[tokio::test]
async fn composed_router_serves_the_buyer_chain() {
    let db = TestDatabase::create("p21t01_chain")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p21t01_chain_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);

    let owner = provision_active(&app, "+55 11 90000-0211", "Composition Owner").await;
    let cookie = login_cookie(app.clone(), "+55 11 90000-0211").await;

    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(draft_body()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id = created["id"].as_str().expect("draft id").to_owned();

    let (status, published) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{draft_id}/publication"),
        Some(json!({})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let request_id = published["id"].as_str().expect("request id").to_owned();

    // The owner's own list shows the published request.
    let (status, list) = call(app.clone(), "GET", "/api/v1/requests", None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["total"], 1);

    // The public projection answers without a session.
    let (status, public) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/public/requests/{request_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public["title"], "Refrigerator");
    assert!(
        public.get("author_id").is_none() && public.get("phone").is_none(),
        "the public projection carries no owner identity or phone material"
    );

    // The publication belongs to the account that made it.
    let owner_row: String =
        sqlx::query_scalar("SELECT author_id::text FROM requests WHERE id = $1")
            .bind(uuid::Uuid::parse_str(&request_id).expect("request id parses"))
            .fetch_one(db.pool())
            .await
            .expect("author reads");
    assert_eq!(owner_row, owner.to_string());
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn composed_router_refuses_strangers_and_anonymous_callers() {
    let db = TestDatabase::create("p21t01_refuse")
        .await
        .expect("disposable database allocates");
    seed_catalog(db.pool()).await;
    let app = test_app(&db);

    provision_active(&app, "+55 11 90000-0212", "Composition Owner").await;
    let owner_cookie = login_cookie(app.clone(), "+55 11 90000-0212").await;
    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(draft_body()),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id = created["id"].as_str().expect("draft id").to_owned();
    let (status, published) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{draft_id}/publication"),
        Some(json!({})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let request_id = published["id"].as_str().expect("request id").to_owned();

    // A stranger sees the same 404 as a missing request: nothing is disclosed.
    provision_active(&app, "+55 11 90000-0213", "Composition Stranger").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0213").await;
    let (status, stranger) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{request_id}"),
        None,
        Some(&stranger_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(stranger["code"], "not_found");

    // An anonymous caller is refused before any ownership question.
    let (status, anonymous) = call(app.clone(), "GET", "/api/v1/requests", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous["code"], "unauthenticated");

    // The staff boundary refuses a cookie-less read on the composed router.
    let (status, staff) = call(
        app.clone(),
        "GET",
        "/api/v1/metrics/operational",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(staff["code"], "unauthenticated");

    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn composed_router_keeps_privacy_headers_and_stable_refusals() {
    let db = TestDatabase::create("p21t01_privacy")
        .await
        .expect("disposable database allocates");
    seed_catalog(db.pool()).await;
    let app = test_app(&db);

    // A private response carries the privacy header set.
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/requests")
                .header("host", "app.test")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("route responds");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let headers = response.headers().clone();
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
            .expect("cache control"),
        "no-store"
    );
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok())
            .expect("content type options"),
        "nosniff"
    );
    assert_eq!(
        headers
            .get("referrer-policy")
            .and_then(|value| value.to_str().ok())
            .expect("referrer policy"),
        "same-origin"
    );

    // An unknown route keeps the stable refusal code.
    let (status, body) = call(app.clone(), "GET", "/api/v1/not-a-route", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    // The public catalog answers with no phone-shaped material.
    let (status, cities) = call(app.clone(), "GET", "/api/v1/catalogs/cities", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!cities.to_string().contains("+55"));

    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn composed_router_serves_assets_with_their_own_caching() {
    let db = TestDatabase::create("p21t01_assets")
        .await
        .expect("disposable database allocates");
    let dir = std::env::temp_dir().join(format!("procurali-assets-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir)
        .await
        .expect("asset directory creates");
    tokio::fs::write(dir.join("app.js"), "export const x = 1;\n")
        .await
        .expect("asset writes");
    let app = Application::new(
        db.pool().clone(),
        Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE)),
        LOOKUP_KEY.to_owned(),
        ENCRYPTION_KEY.to_owned(),
        None,
        dir.clone(),
    )
    .router();

    let response = app
        .oneshot(
            Request::get("/assets/app.js")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("route responds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
            .expect("cache control"),
        "public, max-age=31536000, immutable"
    );

    tokio::fs::remove_dir_all(&dir).await.ok();
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn composed_router_answers_liveness_and_readiness() {
    let db = TestDatabase::create("p21t01_health")
        .await
        .expect("disposable database allocates");
    let app = test_app(&db);

    let (status, live) = call(app.clone(), "GET", "/health/live", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(live["status"], "alive");

    let (status, ready) = call(app.clone(), "GET", "/health/ready", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["unavailable"], json!([]));

    db.cleanup().await.expect("suite cleans up");
}
