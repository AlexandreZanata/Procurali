//! Offer-submission acceptance (P07-T02): guarded creation on demand.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, and
//! offer routes over real PostgreSQL with the deterministic fake provider.
//! Proves:
//! - a valid used offer matches used demand with an exact receipt and a
//!   buyer notice, and no phone anywhere near the seller;
//! - self-offers, wrong conditions, over-budget prices, stale revisions,
//!   expired demand, missing keys, overposts, and blocks refuse without
//!   creating offers;
//! - the seller response never carries the buyer's phone destination.
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
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    let lookup = "p07t02-test-only-lookup-key".to_owned();
    let encryption = "p07t02-test-only-encryption-key".to_owned();
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

/// Publish one used-only BRL 600 demand through the real routes; return id.
async fn publish_demand(app: &axum::Router, cookie: &str) -> String {
    let (status, draft) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(json!({
            "title": "Refrigerator",
            "category_code": "home_appliances",
            "budget": "600.00",
            "condition": "used",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": "Frost-free preferred.",
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

fn offer_body() -> Value {
    json!({
        "revision_number": 1,
        "cycle_number": 1,
        "description": "Frost-free 300L",
        "price": "520.00",
        "condition": "used",
        "city_code": "campinas",
        "region_code": "centro",
        "notes": "Pickup only.",
        "available": true,
        "available_in_city": true,
    })
}

/// The submission receipt carries exactly the terms plus slot facts.
fn assert_receipt_shape(body: &Value) {
    let object = body.as_object().expect("receipt is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "city_code",
            "condition",
            "cycle_number",
            "description",
            "id",
            "notes",
            "price",
            "region_code",
            "request_id",
            "revision_number",
            "state"
        ]
    );
    let rendered = body.to_string();
    for absent in [
        "author",
        "phone",
        "cipher",
        "lookup",
        "destination",
        "token",
        "session",
        "reporter",
        "secret",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in offer receipt");
    }
}

async fn offer_count(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM offers WHERE request_id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .expect("count reads")
}

#[tokio::test]
async fn valid_used_offer_matches_used_demand() {
    let db = TestDatabase::create("p07t02_valid")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t02_valid_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0121", "Demand Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0121").await;
    provision_active(&app, "+55 11 90000-0122", "Offer Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0122").await;
    let demand = publish_demand(&app, &buyer_cookie).await;

    let (status, submitted) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(offer_body()),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_receipt_shape(&submitted);
    assert_eq!(submitted["request_id"], demand);
    assert_eq!(submitted["price"], "520.00");
    assert_eq!(submitted["condition"], "used");
    assert_eq!(submitted["state"], "sent");
    assert_eq!(submitted["cycle_number"], 1);
    assert_eq!(submitted["revision_number"], 1);
    assert_eq!(
        offer_count(db.pool(), demand.parse().expect("id parses")).await,
        1
    );

    // The buyer is notified without learning anything new about the seller
    // beyond the offer itself, and the seller learns nothing about the buyer.
    let offer_id: uuid::Uuid = submitted["id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses");
    let notices: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, body FROM notices WHERE resource_id = $1")
            .bind(offer_id)
            .fetch_all(db.pool())
            .await
            .expect("notices read");
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].0, "offer.received");
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'offer' AND kind = 'offer.submitted'",
    )
    .fetch_one(db.pool())
    .await
    .expect("events read");
    assert_eq!(events, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn incompatible_submissions_refuse_without_new_offer() {
    let db = TestDatabase::create("p07t02_refusals")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t02_refusals_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0123", "Refusal Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0123").await;
    provision_active(&app, "+55 11 90000-0124", "Refusal Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0124").await;
    let demand = publish_demand(&app, &buyer_cookie).await;
    let demand_id: uuid::Uuid = demand.parse().expect("id parses");

    // Self-offers never take a slot, in any context.
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(offer_body()),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden_role");
    // New against used-only, and a cent over the ceiling, refuse exactly.
    for (mutated, status, code, field) in [
        (
            "condition",
            StatusCode::BAD_REQUEST,
            "invalid_field",
            "condition",
        ),
        (
            "price",
            StatusCode::UNPROCESSABLE_ENTITY,
            "amount_too_large",
            "price",
        ),
    ] {
        let mut body = offer_body();
        body[mutated] = if mutated == "condition" {
            json!("new")
        } else {
            json!("600.01")
        };
        let (refused, refused_body) = call(
            app.clone(),
            "POST",
            &format!("/api/v1/requests/{demand}/offers"),
            Some(body),
            Some(&seller_cookie),
        )
        .await;
        assert_eq!(refused, status, "{mutated} refusal status");
        assert_eq!(refused_body["code"], code);
        assert_eq!(refused_body["fields"][field], code);
    }
    // A missing key, an overposted author, and a ghost request refuse.
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "condition": "used",
            "city_code": "campinas", "region_code": "centro",
            "available": true, "available_in_city": true,
        })),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "missing_field");
    assert_eq!(body["fields"]["price"], "missing_field");
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "price": "520.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "author_id": uuid::Uuid::now_v7(),
            "phone": "+55 11 90000-0124",
            "available": true, "available_in_city": true,
        })),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let ghost = uuid::Uuid::now_v7();
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{ghost}/offers"),
        Some(offer_body()),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    // A revised demand makes observed terms stale; an elapsed demand is
    // expired; a blocked pair is refused — all without new offers.
    let owner: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Refusal Owner'")
            .fetch_one(db.pool())
            .await
            .expect("owner reads");
    procurali_backend::application::revise_request::revise_request(
        db.pool(),
        owner,
        demand_id,
        procurali_backend::application::revise_request::ReviseInput {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "used".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Needs ice maker.".to_owned(),
        },
    )
    .await
    .expect("fixture revision revises");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(offer_body()),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict_revision");
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(demand_id)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic expiry applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(offer_body()),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "expired");
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)")
        .bind(
            sqlx::query_scalar::<_, uuid::Uuid>(
                "SELECT id FROM users WHERE display_name = 'Refusal Owner'",
            )
            .fetch_one(db.pool())
            .await
            .expect("buyer reads"),
        )
        .bind(
            sqlx::query_scalar::<_, uuid::Uuid>(
                "SELECT id FROM users WHERE display_name = 'Refusal Seller'",
            )
            .fetch_one(db.pool())
            .await
            .expect("seller reads"),
        )
        .execute(db.pool())
        .await
        .expect("synthetic block applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(offer_body()),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden_role");
    assert_eq!(offer_count(db.pool(), demand_id).await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn buyer_phone_is_absent_from_seller_submission_response() {
    let db = TestDatabase::create("p07t02_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t02_privacy_"),
        "known suite identity in the database name"
    );
    // Unmistakable synthetic canary: no test value may survive serialization.
    const CANARY_DIGITS: &str = "551190000125";
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0125", "Private Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0125").await;
    provision_active(&app, "+55 11 90000-0126", "Private Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0126").await;
    let demand = publish_demand(&app, &buyer_cookie).await;

    let (status, submitted) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(offer_body()),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let rendered = submitted.to_string();
    for absent in [
        "author",
        CANARY_DIGITS,
        "90000-0125",
        "90000",
        "phone",
        "ciphertext",
        "lookup",
        "destination",
        "address",
        "token",
        "session",
        "reporter",
        "secret",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in seller receipt");
    }
    // The buyer notice carries the offer, never the phone either.
    let bodies: Vec<String> = sqlx::query_scalar("SELECT body FROM notices")
        .fetch_all(db.pool())
        .await
        .expect("notices read");
    assert_eq!(bodies.len(), 1);
    assert!(!bodies[0].contains(CANARY_DIGITS));
    assert!(!bodies[0].contains("90000"));
    db.cleanup().await.expect("suite cleans up");
}
