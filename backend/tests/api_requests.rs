//! Request-draft acceptance (P05-T03): owner-only drafts, exact validation.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, and draft routes over real
//! PostgreSQL with the deterministic fake provider. Proves:
//! - an owner's draft is saved, reloaded, and edited privately;
//! - strangers and anonymous actors cannot read or mutate it, and overposted
//!   author/state/contact keys are refused;
//! - draft saves consume no publication allowance and create no publication
//!   fact, while every invalid requirement is refused naming its field.
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
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::requests::{
    cycles_for_request, request as read_request, requests_for_author, revisions_for_request,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    let lookup = "p05t03-test-only-lookup-key".to_owned();
    let encryption = "p05t03-test-only-encryption-key".to_owned();
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

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

/// The draft receipt carries exactly the requirement keys plus state.
fn assert_receipt_shape(body: &Value) {
    let object = body.as_object().expect("receipt is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "budget",
            "category_code",
            "city_code",
            "condition",
            "id",
            "notes",
            "region_code",
            "state",
            "title"
        ]
    );
    let rendered = body.to_string();
    for absent in [
        "author",
        "phone",
        "cipher",
        "token",
        "session",
        "state_code",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in draft receipt");
    }
}

#[tokio::test]
async fn own_draft_is_saved_reloaded_and_edited_privately() {
    let db = TestDatabase::create("p05t03_owner")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t03_owner_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let owner = provision_active(&app, "+55 11 90000-0001", "Draft Owner").await;
    let cookie = login_cookie(app.clone(), "+55 11 90000-0001").await;
    // Account provisioning records its own facts; drafts must add none.
    let provisioned_events = table_count(db.pool(), "business_events").await;

    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(draft_body()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_receipt_shape(&created);
    assert_eq!(created["title"], "Refrigerator");
    assert_eq!(created["budget"], "520.00");
    assert_eq!(created["state"], "draft");
    let id = created["id"].as_str().expect("receipt carries id");

    let (status, reloaded) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/drafts/{id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reloaded, created, "private reload matches the receipt");

    let (status, edited) = call(
        app.clone(),
        "PUT",
        &format!("/api/v1/requests/drafts/{id}"),
        Some(json!({
            "title": "Double-door refrigerator",
            "category_code": "home_appliances",
            "budget": "800.00",
            "condition": "new",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": "Needs to fit a 70cm niche.",
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(edited["title"], "Double-door refrigerator");
    assert_eq!(edited["budget"], "800.00");
    assert_eq!(edited["condition"], "new");

    let (status, reloaded) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/drafts/{id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reloaded, edited, "edited requirements persist");

    // The stored row stays a private draft owned by the caller: no cycle, no
    // revision, no publication time, no publication fact.
    let stored = read_request(db.pool(), id.parse().expect("id parses"))
        .await
        .expect("request reads")
        .expect("draft reads");
    assert_eq!(stored.author_id, owner);
    assert_eq!(stored.state, "draft");
    assert!(stored.original_published_at.is_none());
    assert_eq!(stored.current_cycle_number, 0);
    assert_eq!(stored.current_revision_number, 0);
    assert_eq!(table_count(db.pool(), "request_cycles").await, 0);
    assert_eq!(table_count(db.pool(), "request_revisions").await, 0);
    assert_eq!(
        table_count(db.pool(), "business_events").await,
        provisioned_events,
        "draft writes record no business fact"
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn stranger_and_anonymous_cannot_read_or_mutate() {
    let db = TestDatabase::create("p05t03_stranger")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t03_stranger_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0002", "Owner Two").await;
    let owner_cookie = login_cookie(app.clone(), "+55 11 90000-0002").await;
    provision_active(&app, "+55 11 90000-0003", "Stranger Sue").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0003").await;

    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(draft_body()),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"]
        .as_str()
        .expect("receipt carries id")
        .to_owned();
    let path = format!("/api/v1/requests/drafts/{id}");

    // Anonymous actors share one refusal on every method.
    for method in ["POST", "GET", "PUT"] {
        let (status, body) = call(
            app.clone(),
            method,
            if method == "POST" {
                "/api/v1/requests/drafts"
            } else {
                &path
            },
            if method == "GET" {
                None
            } else {
                Some(draft_body())
            },
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} refuses anonymous"
        );
        assert_eq!(body["code"], "unauthenticated");
    }

    // Strangers share one privacy-safe refusal with missing rows: no oracle.
    let (status, body) = call(app.clone(), "GET", &path, None, Some(&stranger_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let (status, body) = call(
        app.clone(),
        "PUT",
        &path,
        Some(draft_body()),
        Some(&stranger_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let ghost = format!("/api/v1/requests/drafts/{}", uuid::Uuid::now_v7());
    let (status, body) = call(app.clone(), "GET", &ghost, None, Some(&stranger_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    // Overposted ownership, lifecycle, and contact keys are refused as unknown.
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(json!({
            "title": "Refrigerator",
            "category_code": "home_appliances",
            "budget": "520.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": "",
            "author_id": uuid::Uuid::now_v7(),
            "state": "active",
            "phone": "+55 11 90000-0003",
            "original_published_at": "2026-10-08T00:00:00Z",
        })),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        app.clone(),
        "PUT",
        &path,
        Some(json!({
            "title": "Refrigerator",
            "category_code": "home_appliances",
            "budget": "520.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
            "current_cycle_number": 3,
        })),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing moved: the owner's draft still reads back untouched.
    let (status, reloaded) = call(app.clone(), "GET", &path, None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reloaded, created);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn draft_save_creates_no_publication_fact_or_allowance() {
    let db = TestDatabase::create("p05t03_facts")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t03_facts_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let owner = provision_active(&app, "+55 11 90000-0004", "Fact Owner").await;
    let cookie = login_cookie(app.clone(), "+55 11 90000-0004").await;
    let provisioned_events = table_count(db.pool(), "business_events").await;

    // Two drafts plus an edit: still no event, no cycle, no revision.
    for _ in 0..2 {
        let (status, _) = call(
            app.clone(),
            "POST",
            "/api/v1/requests/drafts",
            Some(draft_body()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let drafts = requests_for_author(db.pool(), owner)
        .await
        .expect("owner reads");
    assert_eq!(drafts.len(), 2);
    let first = drafts[0].id.to_string();
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!("/api/v1/requests/drafts/{first}"),
        Some(draft_body()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        table_count(db.pool(), "business_events").await,
        provisioned_events,
        "draft writes record no business fact"
    );
    assert_eq!(table_count(db.pool(), "request_cycles").await, 0);
    assert_eq!(table_count(db.pool(), "request_revisions").await, 0);
    for draft in requests_for_author(db.pool(), owner)
        .await
        .expect("owner reads")
    {
        assert_eq!(draft.state, "draft");
        assert!(draft.original_published_at.is_none());
    }
    assert_eq!(
        cycles_for_request(db.pool(), drafts[0].id)
            .await
            .expect("cycles read")
            .len(),
        0
    );
    assert_eq!(
        revisions_for_request(db.pool(), drafts[0].id)
            .await
            .expect("revisions read")
            .len(),
        0
    );

    // Every invalid requirement names its field and stores nothing.
    let refusals: Vec<(Value, StatusCode, &str, &str)> = vec![
        (
            json!({
                "title": "Refrigerator", "category_code": "home_appliances",
                "budget": "520.00", "condition": "either",
                "region_code": "centro", "notes": "",
            }),
            StatusCode::BAD_REQUEST,
            "missing_field",
            "city_code",
        ),
        (
            json!({
                "title": "Refrigerator", "category_code": "home_appliances",
                "budget": "0", "condition": "either",
                "city_code": "campinas", "region_code": "centro", "notes": "",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "nonpositive_amount",
            "budget",
        ),
        (
            json!({
                "title": "Refrigerator", "category_code": "home_appliances",
                "budget": "10.123", "condition": "either",
                "city_code": "campinas", "region_code": "centro", "notes": "",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "overprecision",
            "budget",
        ),
        (
            json!({
                "title": "Refrigerator", "category_code": "home_appliances",
                "budget": "many", "condition": "either",
                "city_code": "campinas", "region_code": "centro", "notes": "",
            }),
            StatusCode::BAD_REQUEST,
            "malformed_amount",
            "budget",
        ),
        (
            json!({
                "title": "Refrigerator", "category_code": "home_appliances",
                "budget": "520.00", "condition": "refurbished",
                "city_code": "campinas", "region_code": "centro", "notes": "",
            }),
            StatusCode::BAD_REQUEST,
            "invalid_field",
            "condition",
        ),
        (
            json!({
                "title": "", "category_code": "home_appliances",
                "budget": "520.00", "condition": "either",
                "city_code": "campinas", "region_code": "centro", "notes": "",
            }),
            StatusCode::BAD_REQUEST,
            "invalid_field",
            "title",
        ),
        (
            json!({
                "title": "Call +55 11 98765-4321 now", "category_code": "home_appliances",
                "budget": "520.00", "condition": "either",
                "city_code": "campinas", "region_code": "centro", "notes": "",
            }),
            StatusCode::BAD_REQUEST,
            "invalid_field",
            "title",
        ),
        (
            json!({
                "title": "Refrigerator", "category_code": "motor_vehicles",
                "budget": "520.00", "condition": "either",
                "city_code": "campinas", "region_code": "centro", "notes": "",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_category",
            "",
        ),
        (
            json!({
                "title": "Refrigerator", "category_code": "home_appliances",
                "budget": "520.00", "condition": "either",
                "city_code": "nowhere", "region_code": "centro", "notes": "",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_city",
            "",
        ),
        (
            json!({
                "title": "Refrigerator", "category_code": "home_appliances",
                "budget": "520.00", "condition": "either",
                "city_code": "campinas", "region_code": "norte", "notes": "",
            }),
            StatusCode::BAD_REQUEST,
            "invalid_field",
            "region_code",
        ),
    ];
    for (body, status, code, field) in refusals {
        let (refused, refused_body) = call(
            app.clone(),
            "POST",
            "/api/v1/requests/drafts",
            Some(body),
            Some(&cookie),
        )
        .await;
        assert_eq!(refused, status, "refusal status for {code}");
        assert_eq!(refused_body["code"], code);
        if !field.is_empty() {
            assert_eq!(refused_body["fields"][field], code, "field names {field}");
        }
    }
    assert_eq!(
        requests_for_author(db.pool(), owner)
            .await
            .expect("owner reads")
            .len(),
        2,
        "refusals store nothing"
    );
    assert_eq!(
        table_count(db.pool(), "business_events").await,
        provisioned_events,
        "refusals record no business fact"
    );
    db.cleanup().await.expect("suite cleans up");
}
