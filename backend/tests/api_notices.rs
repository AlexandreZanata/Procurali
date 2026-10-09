//! Notice acceptance (P09-T03): owner-only lists and acknowledgments.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, offer,
//! and notice routes over real PostgreSQL with the deterministic fake
//! provider. Proves:
//! - strangers read and acknowledge nothing foreign, with anonymous
//!   callers refused everywhere;
//! - repeat acknowledgments return the same timestamp with zero new facts
//!   of any kind;
//! - public surfaces and every stored body stay free of private notice
//!   contents.
//!
//! Notice-producing operations run through their real application paths.
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::catalogs::{routes as catalog_routes, CatalogsState};
use procurali_backend::http::notices::{routes as notice_routes, NoticesState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    let lookup = "p09t03-test-only-lookup-key".to_owned();
    let encryption = "p09t03-test-only-encryption-key".to_owned();
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
    .merge(request_routes(RequestsState::new(db.pool().clone())))
    .merge(offer_routes(OffersState::new(db.pool().clone())))
    .merge(notice_routes(NoticesState::new(db.pool().clone())))
    .merge(catalog_routes(CatalogsState::new(db.pool().clone())))
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

/// Provision one active account through the real routes; return its id.
async fn provision_active(app: &axum::Router, phone: &str, name: &str) -> uuid::Uuid {
    let (status, body) = call(
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

async fn seed_catalog(pool: &sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", true)
        .await
        .expect("fixture city stores");
    upsert_region(&mut tx, "campinas", "centro", "Centro")
        .await
        .expect("fixture region stores");
    tx.commit().await.expect("fixtures commit");
}

/// Publish one either/600 demand through the real routes; return id.
async fn publish_demand(app: &axum::Router, cookie: &str) -> String {
    let (status, draft) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(json!({
            "title": "Refrigerator",
            "category_code": "home_appliances",
            "budget": "600.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": "",
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = draft["id"].as_str().expect("receipt carries id").to_owned();
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{id}/publication"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    id
}

/// Submit one used/520 offer through the real route; return id.
async fn submit_offer(app: &axum::Router, demand: &str, cookie: &str) -> String {
    let (status, submitted) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(json!({
            "revision_number": 1,
            "cycle_number": 1,
            "description": "Frost-free 300L",
            "price": "520.00",
            "condition": "used",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": "",
            "available": true,
            "available_in_city": true,
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    submitted["id"]
        .as_str()
        .expect("receipt carries id")
        .to_owned()
}

async fn event_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM business_events")
        .fetch_one(pool)
        .await
        .expect("events read")
}

#[tokio::test]
async fn strangers_cannot_read_or_acknowledge_foreign_notices() {
    let db = TestDatabase::create("p09t03_stranger")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t03_stranger_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0601", "Notice Owner").await;
    let owner_cookie = login_cookie(app.clone(), "+55 11 90000-0601").await;
    provision_active(&app, "+55 11 90000-0602", "Notice Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0602").await;
    provision_active(&app, "+55 11 90000-0603", "Notice Stranger").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0603").await;
    let demand = publish_demand(&app, &owner_cookie).await;
    submit_offer(&app, &demand, &seller_cookie).await;

    // The owner lists their own notice; the stranger's own list is empty.
    let (status, list) = call(
        app.clone(),
        "GET",
        "/api/v1/notices",
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["total"], 1);
    assert_eq!(list["notices"].as_array().expect("list").len(), 1);
    assert_eq!(list["notices"][0]["kind"], "offer.received");
    let notice_id = list["notices"][0]["id"]
        .as_str()
        .expect("row carries id")
        .to_owned();
    let (status, stranger_list) = call(
        app.clone(),
        "GET",
        "/api/v1/notices",
        None,
        Some(&stranger_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stranger_list["total"], 0);

    // Foreign acknowledgment shares the missing-row refusal; anonymous
    // callers share one refusal on both methods.
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/notices/{notice_id}/acknowledgment"),
        None,
        Some(&stranger_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    for (method, path) in [
        ("GET", "/api/v1/notices".to_owned()),
        (
            "POST",
            format!("/api/v1/notices/{notice_id}/acknowledgment"),
        ),
    ] {
        let (status, body) = call(app.clone(), method, &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthenticated");
    }
    // Nothing moved for the stranger: the notice stands unacknowledged.
    let (status, list) = call(
        app.clone(),
        "GET",
        "/api/v1/notices",
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["notices"][0]["acknowledged_at"].is_null());
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeated_acknowledgment_emits_no_fake_engagement() {
    let db = TestDatabase::create("p09t03_repeat")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t03_repeat_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0604", "Repeat Owner").await;
    let owner_cookie = login_cookie(app.clone(), "+55 11 90000-0604").await;
    provision_active(&app, "+55 11 90000-0605", "Repeat Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0605").await;
    let demand = publish_demand(&app, &owner_cookie).await;
    let offer = submit_offer(&app, &demand, &seller_cookie).await;
    let (status, list) = call(
        app.clone(),
        "GET",
        "/api/v1/notices",
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let notice_id = list["notices"][0]["id"]
        .as_str()
        .expect("row carries id")
        .to_owned();
    let events_before = event_count(db.pool()).await;

    // First acknowledgment stamps once; repeats return the identical stamp
    // with zero new facts of any kind — no view, no outcome, no contact.
    let (status, first) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/notices/{notice_id}/acknowledgment"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(first["acknowledged_at"].is_string());
    for _ in 0..2 {
        let (status, repeated) = call(
            app.clone(),
            "POST",
            &format!("/api/v1/notices/{notice_id}/acknowledgment"),
            None,
            Some(&owner_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(repeated["acknowledged_at"], first["acknowledged_at"]);
    }
    assert_eq!(event_count(db.pool()).await, events_before);
    let viewed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'offer' AND kind = 'offer.viewed'",
    )
    .fetch_one(db.pool())
    .await
    .expect("view facts read");
    assert_eq!(viewed, 0);
    let offer_id: uuid::Uuid = offer.parse().expect("id parses");
    let state: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer_id)
        .fetch_one(db.pool())
        .await
        .expect("offer reads");
    assert_eq!(state, "sent");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn public_surfaces_carry_no_notice_contents() {
    let db = TestDatabase::create("p09t03_public")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t03_public_"),
        "known suite identity in the database name"
    );
    // Unmistakable synthetic canary: no test value may survive in public.
    const CANARY_DIGITS: &str = "551190000606";
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0606", "Public Owner").await;
    let owner_cookie = login_cookie(app.clone(), "+55 11 90000-0606").await;
    provision_active(&app, "+55 11 90000-0607", "Public Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0607").await;
    let demand = publish_demand(&app, &owner_cookie).await;
    submit_offer(&app, &demand, &seller_cookie).await;

    // Every stored body is a static template: no digits, no secrets.
    let bodies: Vec<String> = sqlx::query_scalar("SELECT body FROM notices")
        .fetch_all(db.pool())
        .await
        .expect("bodies read");
    assert!(!bodies.is_empty());
    for body in &bodies {
        assert!(!body.contains(CANARY_DIGITS));
        assert!(!body.contains("90000"));
        for absent in ["phone", "cipher", "token", "session", "address", "reporter"] {
            assert!(!body.contains(absent), "no {absent} in notice bodies");
        }
    }
    // Public discovery carries none of it either, authenticated or not.
    for cookie in [None, Some(owner_cookie.as_str())] {
        let (status, catalog) =
            call(app.clone(), "GET", "/api/v1/catalogs/cities", None, cookie).await;
        assert_eq!(status, StatusCode::OK);
        let rendered = catalog.to_string();
        assert!(!rendered.contains(CANARY_DIGITS));
        for body in &bodies {
            assert!(
                !rendered.contains(body.as_str()),
                "no notice text in public"
            );
        }
    }
    // Owner receipts stay scoped: exact keys with no sensitive material.
    let (status, list) = call(
        app.clone(),
        "GET",
        "/api/v1/notices",
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut keys: Vec<&str> = list["notices"][0]
        .as_object()
        .expect("row is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "acknowledged_at",
            "body",
            "created_at",
            "event_id",
            "id",
            "kind",
            "resource_id",
            "resource_kind"
        ]
    );
    assert!(!list.to_string().contains(CANARY_DIGITS));
    db.cleanup().await.expect("suite cleans up");
}
