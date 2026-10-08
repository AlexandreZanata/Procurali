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
use procurali_backend::application::publish_request::{publish_request, PublishError};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{
    set_category_status, upsert_city, upsert_region, CategoryStatus,
};
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

/// Publication facts recorded for one request: kinds in row order.
async fn published_facts(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Vec<(String, Option<i32>)> {
    sqlx::query_as(
        "SELECT kind, cycle FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1
         ORDER BY occurred_at, id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await
    .expect("events read")
}

/// Create one draft through the real draft route; return its id.
async fn create_draft(app: &axum::Router, cookie: &str) -> uuid::Uuid {
    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(draft_body()),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    created["id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses")
}

#[tokio::test]
async fn valid_minimal_request_becomes_active_with_seven_day_deadline() {
    let db = TestDatabase::create("p05t04_publish")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t04_publish_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let owner = provision_active(&app, "+55 11 90000-0011", "Publish Owner").await;
    let cookie = login_cookie(app.clone(), "+55 11 90000-0011").await;
    let id = create_draft(&app, &cookie).await;

    let published = publish_request(db.pool(), owner, id)
        .await
        .expect("valid draft publishes");
    assert_eq!(published.id, id);
    assert_eq!(published.state, "active");
    assert_eq!(published.cycle_number, 1);
    assert_eq!(published.revision_number, 1);
    assert_eq!(published.budget, "520.00");
    assert_eq!(
        published
            .deadline
            .signed_duration_since(published.original_published_at),
        chrono::Duration::days(7),
        "the exclusive deadline is exactly seven days out"
    );

    // Stored truth matches: active and public with one cycle, one revision,
    // the original publication time, and exactly one publication fact.
    let stored = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(stored.author_id, owner);
    assert_eq!(stored.state, "active");
    assert_eq!(stored.current_cycle_number, 1);
    assert_eq!(stored.current_revision_number, 1);
    assert_eq!(
        stored.original_published_at,
        Some(published.original_published_at)
    );
    let cycles = cycles_for_request(db.pool(), id)
        .await
        .expect("cycles read");
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].deadline, published.deadline);
    let revisions = revisions_for_request(db.pool(), id)
        .await
        .expect("revisions read");
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].title, "Refrigerator");
    assert_eq!(revisions[0].budget_cents, 52_000);
    assert_eq!(
        published_facts(db.pool(), id).await,
        [("request.published".to_owned(), Some(1))]
    );

    // The row moved past drafting: the draft endpoint no longer serves it.
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/drafts/{id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn each_invalid_requirement_is_refused_without_event() {
    let db = TestDatabase::create("p05t04_refusals")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t04_refusals_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let owner = provision_active(&app, "+55 11 90000-0012", "Refusal Owner").await;
    let cookie = login_cookie(app.clone(), "+55 11 90000-0012").await;

    async fn set_status(pool: &sqlx::PgPool, status: CategoryStatus) {
        let mut tx = pool.begin().await.expect("transaction begins");
        assert!(set_category_status(&mut tx, "home_appliances", status)
            .await
            .expect("status transitions"));
        tx.commit().await.expect("transition commits");
    }

    // A retired category finishes existing cycles only: new use is refused.
    let retired = create_draft(&app, &cookie).await;
    set_status(db.pool(), CategoryStatus::Retired).await;
    assert_eq!(
        publish_request(db.pool(), owner, retired).await,
        Err(PublishError::RetiredCategory)
    );
    assert!(published_facts(db.pool(), retired).await.is_empty());
    // A prohibited category is never usable.
    set_status(db.pool(), CategoryStatus::Prohibited).await;
    assert_eq!(
        publish_request(db.pool(), owner, retired).await,
        Err(PublishError::ProhibitedCategory)
    );
    assert!(published_facts(db.pool(), retired).await.is_empty());
    set_status(db.pool(), CategoryStatus::Allowed).await;

    // A disabled city cannot back a new publication.
    let unserved = create_draft(&app, &cookie).await;
    let mut tx = db.pool().begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", false)
        .await
        .expect("city disables");
    tx.commit().await.expect("disable commits");
    assert_eq!(
        publish_request(db.pool(), owner, unserved).await,
        Err(PublishError::CityNotEnabled)
    );
    assert!(published_facts(db.pool(), unserved).await.is_empty());
    let mut tx = db.pool().begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", true)
        .await
        .expect("city re-enables");
    tx.commit().await.expect("enable commits");

    // Restricted accounts cannot publish, without distinguishing states.
    let suspended = create_draft(&app, &cookie).await;
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(owner)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        publish_request(db.pool(), owner, suspended).await,
        Err(PublishError::NotActive)
    );
    assert!(published_facts(db.pool(), suspended).await.is_empty());
    assert_eq!(
        publish_request(db.pool(), uuid::Uuid::now_v7(), suspended).await,
        Err(PublishError::NotActive)
    );
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(owner)
        .execute(db.pool())
        .await
        .expect("synthetic restoration applies");

    // Missing rows and strangers share precise refusals once eligible again.
    assert_eq!(
        publish_request(db.pool(), owner, uuid::Uuid::now_v7()).await,
        Err(PublishError::NotFound)
    );
    provision_active(&app, "+55 11 90000-0013", "Publish Stranger").await;
    let stranger: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Publish Stranger'")
            .fetch_one(db.pool())
            .await
            .expect("stranger reads");
    assert_eq!(
        publish_request(db.pool(), stranger, suspended).await,
        Err(PublishError::NotFound)
    );
    // A terminal row is never republished (synthetic closure fixture: the
    // closure writers arrive in a later card; the guard is proven here).
    sqlx::query("UPDATE requests SET state = 'cancelled' WHERE id = $1")
        .bind(suspended)
        .execute(db.pool())
        .await
        .expect("synthetic closure applies");
    assert_eq!(
        publish_request(db.pool(), owner, suspended).await,
        Err(PublishError::ForbiddenState)
    );
    assert!(published_facts(db.pool(), suspended).await.is_empty());

    // Refused drafts stay drafts with no history and no facts.
    for id in [retired, unserved] {
        let stored = read_request(db.pool(), id)
            .await
            .expect("request reads")
            .expect("draft reads");
        assert_eq!(stored.state, "draft");
        assert!(stored.original_published_at.is_none());
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeated_publication_returns_the_existing_result() {
    let db = TestDatabase::create("p05t04_replay")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t04_replay_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let owner = provision_active(&app, "+55 11 90000-0014", "Replay Owner").await;
    let cookie = login_cookie(app.clone(), "+55 11 90000-0014").await;
    let id = create_draft(&app, &cookie).await;

    let first = publish_request(db.pool(), owner, id)
        .await
        .expect("valid draft publishes");
    assert_eq!(
        published_facts(db.pool(), id).await,
        [("request.published".to_owned(), Some(1))]
    );
    // The repeat returns the same receipt: no second request, no second
    // cycle, no second revision, no second event.
    let second = publish_request(db.pool(), owner, id)
        .await
        .expect("repeat publication replays");
    assert_eq!(second, first, "replay returns the existing result");
    assert_eq!(
        published_facts(db.pool(), id).await,
        [("request.published".to_owned(), Some(1))]
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    assert_eq!(
        revisions_for_request(db.pool(), id)
            .await
            .expect("revisions read")
            .len(),
        1
    );
    assert_eq!(
        requests_for_author(db.pool(), owner)
            .await
            .expect("owner reads")
            .len(),
        1
    );
    db.cleanup().await.expect("suite cleans up");
}
