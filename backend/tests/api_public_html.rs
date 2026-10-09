//! Public-HTML acceptance (P13-T03): safe server-rendered pages.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real public HTML route with real cookie
//! sessions, with fixtures through the real operations. Proves:
//! - the HTML-only response identifies item, budget, and region for
//!   active demand, deterministically on repeat;
//! - a malicious title renders as escaped text in body and metadata,
//!   never as markup, with no private offer link anywhere;
//! - suspended and deleted links carry no title or phone in HTML or
//!   metadata, sharing one generic byte-identical page with missing
//!   rows — including for blocked signed-in viewers.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::delete_account::{delete_account, DeleteAccountInput};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::public_html::{routes as html_routes, PublicHtmlState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p13t03-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p13t03-test-only-encryption-key";

fn test_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    account_routes(AccountsState::new(
        db.pool().clone(),
        Arc::clone(&provider),
        LOOKUP_KEY.to_owned(),
        ENCRYPTION_KEY.to_owned(),
    ))
    .merge(auth_routes(AuthState::new(
        db.pool().clone(),
        provider,
        LOOKUP_KEY.to_owned(),
        ENCRYPTION_KEY.to_owned(),
        None,
    )))
    .merge(html_routes(PublicHtmlState::new(db.pool().clone())))
}

/// One HTML call: status, content type, cache directive, and raw text.
async fn fetch(
    app: axum::Router,
    demand: uuid::Uuid,
    cookie: Option<&str>,
) -> (StatusCode, String, String, String) {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/public/requests/{demand}/html"))
        .header("host", "app.test")
        .header("origin", "http://app.test");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).expect("test request builds"))
        .await
        .expect("route responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let cache = response
        .headers()
        .get("cache-control")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    let text = String::from_utf8(bytes.to_vec()).expect("html is UTF-8");
    (status, content_type, cache, text)
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

/// One published demand through the real operations.
async fn publish_demand(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("600.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("fixture draft validates");
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

/// HTML bodies identify business facts only: no phone digits, notes,
/// offers, reporter, token, or secret material — and no private link.
fn assert_safe_html(text: &str) {
    for absent in [
        "90000",
        "55119",
        "phone",
        "lookup",
        "cipher",
        "token",
        "notes",
        "offers",
        "reporter",
        "detail",
        "secret",
        "contact",
        "/api/v1/requests/",
    ] {
        assert!(!text.contains(absent), "no {absent} in page output");
    }
}

#[tokio::test]
async fn html_identifies_item_budget_region() {
    let db = TestDatabase::create("p13t03_open")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t03_open_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2201", "Open Buyer").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;

    // The HTML-only response identifies the demand before any script
    // runs: item, budget, region, seller line, and metadata — twice
    // byte-identical, with safe headers.
    let (status, content_type, cache, first) = fetch(app.clone(), demand, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert_eq!(cache, "no-store");
    for present in [
        "Refrigerator",
        "600.00",
        "centro",
        "campinas",
        "Open Buyer",
        "og:title",
        "og:description",
    ] {
        assert!(first.contains(present), "page names {present}");
    }
    assert_safe_html(&first);
    let (_, _, _, second) = fetch(app.clone(), demand, None).await;
    assert_eq!(first, second);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn malicious_title_renders_as_text() {
    let db = TestDatabase::create("p13t03_xss")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t03_xss_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2202", "Markup Buyer").await;
    let demand = publish_demand(db.pool(), buyer, "<script>alert(1)</script> Fridge").await;

    // Markup in the title renders escaped in body and metadata alike:
    // no script or image element survives, and no private offer link
    // exists anywhere on the page.
    let (status, _, _, page) = fetch(app.clone(), demand, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!page.contains("<script>"));
    assert!(!page.contains("<img"));
    assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt; Fridge"));
    assert_safe_html(&page);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_deleted_absent_from_html() {
    let db = TestDatabase::create("p13t03_hidden")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t03_hidden_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2203", "Hidden Buyer").await;
    let leaving = provision_active(&app, "+55 11 90000-2205", "Leaving Buyer").await;
    let viewer = provision_active(&app, "+55 11 90000-2204", "Hidden Viewer").await;
    let viewer_cookie = login_cookie(app.clone(), "+55 11 90000-2204").await;
    let suspended = publish_demand(db.pool(), buyer, "Refrigerator").await;
    let cancelled = publish_demand(db.pool(), leaving, "Freezer").await;
    sqlx::query("UPDATE requests SET state = 'suspended', visibility = 'hidden' WHERE id = $1")
        .bind(suspended)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    block_user(db.pool(), buyer, viewer)
        .await
        .expect("pair blocks");
    delete_account(
        db.pool(),
        leaving,
        DeleteAccountInput {
            user_id: leaving,
            reason: "leaving the marketplace".to_owned(),
        },
    )
    .await
    .expect("leaving deletion deletes");

    // The deleted owner's demand resolves as cancelled: standing without
    // interaction, title, or phone.
    let (status, _, _, page) = fetch(app.clone(), cancelled, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("cancelled"));
    assert!(!page.contains("Freezer"));
    assert_safe_html(&page);

    // Suspended, blocked, and missing links share one generic
    // byte-identical page: no title, no phone, no metadata beyond the
    // static unavailable wording.
    let ghost = uuid::Uuid::now_v7();
    let mut generics = Vec::new();
    for (id, cookie) in [
        (suspended, None),
        (ghost, None),
        (suspended, Some(viewer_cookie.as_str())),
    ] {
        let (status, content_type, _, page) = fetch(app.clone(), id, cookie).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type, "text/html; charset=utf-8");
        assert!(!page.contains("Refrigerator"));
        assert!(!page.contains("Hidden Buyer"));
        assert_safe_html(&page);
        assert!(page.contains("Unavailable request"));
        generics.push(page);
    }
    assert_eq!(generics[0], generics[1]);
    assert_eq!(generics[1], generics[2]);
    db.cleanup().await.expect("suite cleans up");
}
