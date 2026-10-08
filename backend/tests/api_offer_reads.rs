//! Offer-comparison acceptance (P07-T04): private buyer sets, seller self.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, offer,
//! and read routes over real PostgreSQL with the deterministic fake
//! provider. Proves:
//! - each seller sees only their own terms, never a competitor's;
//! - unrelated buyers and anonymous callers read nothing private;
//! - term edits never refresh comparison position, price sorts stay exact,
//!   and stale or elapsed rows lose liveness or contact truthfully.
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
use procurali_backend::persistence::offers::{insert_terms, update_current_terms, TermsSnapshot};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";

fn test_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    let lookup = "p07t04-test-only-lookup-key".to_owned();
    let encryption = "p07t04-test-only-encryption-key".to_owned();
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

/// Publish one either-condition demand through the real routes; return id.
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

fn offer_body(price: &str, notes: &str) -> Value {
    json!({
        "revision_number": 1,
        "cycle_number": 1,
        "description": "Frost-free 300L",
        "price": price,
        "condition": "used",
        "city_code": "campinas",
        "region_code": "centro",
        "notes": notes,
        "available": true,
        "available_in_city": true,
    })
}

async fn submit(app: &axum::Router, demand: &str, cookie: &str, price: &str, notes: &str) -> Value {
    let (status, submitted) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(offer_body(price, notes)),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    submitted
}

#[tokio::test]
async fn sellers_only_see_their_own_offers() {
    let db = TestDatabase::create("p07t04_sellers")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t04_sellers_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0141", "Compare Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0141").await;
    provision_active(&app, "+55 11 90000-0142", "Seller Alpha").await;
    let alpha_cookie = login_cookie(app.clone(), "+55 11 90000-0142").await;
    provision_active(&app, "+55 11 90000-0143", "Seller Beta").await;
    let beta_cookie = login_cookie(app.clone(), "+55 11 90000-0143").await;
    let demand = publish_demand(&app, &buyer_cookie).await;
    let alpha = submit(&app, &demand, &alpha_cookie, "520.00", "Alpha notes.").await;
    let beta = submit(&app, &demand, &beta_cookie, "480.00", "Beta notes.").await;
    let alpha_id = alpha["id"].as_str().expect("receipt carries id").to_owned();
    let beta_id = beta["id"].as_str().expect("receipt carries id").to_owned();

    // Each seller lists exactly their own row with no trace of the other.
    let (status, mine) = call(
        app.clone(),
        "GET",
        "/api/v1/offers/mine",
        None,
        Some(&alpha_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = mine.as_array().expect("own list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], alpha_id);
    assert_eq!(rows[0]["price"], "520.00");
    let rendered = mine.to_string();
    assert!(!rendered.contains("480.00"), "no competitor price leaks");
    assert!(!rendered.contains("Beta notes"), "no competitor notes leak");
    for absent in ["author", "phone", "cipher", "token", "session"] {
        assert!(!rendered.contains(absent), "no {absent} in seller view");
    }
    let mut keys: Vec<&str> = rows[0]
        .as_object()
        .expect("row is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "city_code",
            "condition",
            "contact_allowed",
            "cycle_number",
            "description",
            "id",
            "live",
            "notes",
            "price",
            "region_code",
            "request_id",
            "revision_number",
            "state"
        ]
    );

    // Cross-seller detail shares the missing-row refusal: no oracle.
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/offers/{beta_id}"),
        None,
        Some(&alpha_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let (status, detail) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/offers/{beta_id}"),
        None,
        Some(&beta_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["price"], "480.00");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn unrelated_buyers_and_anonymous_read_nothing_private() {
    let db = TestDatabase::create("p07t04_stranger")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t04_stranger_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0144", "Private Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0144").await;
    provision_active(&app, "+55 11 90000-0145", "Private Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0145").await;
    provision_active(&app, "+55 11 90000-0146", "Outside Buyer").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0146").await;
    let demand = publish_demand(&app, &buyer_cookie).await;
    let submitted = submit(&app, &demand, &seller_cookie, "520.00", "Notes.").await;
    let offer_id = submitted["id"].as_str().expect("receipt carries id");

    // The owning buyer compares live offers with seller cards and no phone.
    let (status, list) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["total"], 1);
    assert_eq!(list["live"].as_array().expect("live").len(), 1);
    assert!(list["history"].as_array().expect("history").is_empty());
    let row = &list["live"][0];
    assert_eq!(row["price"], "520.00");
    assert_eq!(row["seller_name"], "Private Seller");
    assert_eq!(row["city_label"], "Campinas");
    assert_eq!(row["contact_allowed"], true);
    let rendered = list.to_string();
    for absent in [
        "phone",
        "cipher",
        "lookup",
        "destination",
        "token",
        "session",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in buyer list");
    }

    // Unrelated buyers share the missing-row refusal; anonymous callers
    // share one refusal on every read.
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&stranger_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    for path in [
        format!("/api/v1/requests/{demand}/offers"),
        "/api/v1/offers/mine".to_owned(),
        format!("/api/v1/offers/{offer_id}"),
    ] {
        let (status, body) = call(app.clone(), "GET", &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthenticated");
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn edits_keep_position_and_stale_loses_contact() {
    let db = TestDatabase::create("p07t04_order")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t04_order_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0147", "Order Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0147").await;
    provision_active(&app, "+55 11 90000-0148", "Order Alpha").await;
    let alpha_cookie = login_cookie(app.clone(), "+55 11 90000-0148").await;
    provision_active(&app, "+55 11 90000-0149", "Order Beta").await;
    let beta_cookie = login_cookie(app.clone(), "+55 11 90000-0149").await;
    let demand = publish_demand(&app, &buyer_cookie).await;
    let alpha = submit(&app, &demand, &alpha_cookie, "520.00", "Alpha.").await;
    let beta = submit(&app, &demand, &beta_cookie, "480.00", "Beta.").await;
    let alpha_id: uuid::Uuid = alpha["id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses");

    // Newest submission orders first, and a later term edit on the older
    // row never refreshes its position.
    let (status, list) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = list["live"]
        .as_array()
        .expect("live")
        .iter()
        .map(|row| row["id"].as_str().expect("row carries id"))
        .collect();
    assert_eq!(
        ids,
        [
            beta["id"].as_str().expect("id"),
            alpha["id"].as_str().expect("id")
        ]
    );
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let edited = TermsSnapshot {
        description: "Frost-free 350L".to_owned(),
        price_cents: 52_000,
        condition: "used".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Alpha.".to_owned(),
    };
    insert_terms(&mut tx, alpha_id, 2, &edited)
        .await
        .expect("terms store");
    update_current_terms(&mut tx, alpha_id, 2, &edited)
        .await
        .expect("current promotes");
    tx.commit().await.expect("edit commits");
    let (status, list) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = list["live"]
        .as_array()
        .expect("live")
        .iter()
        .map(|row| row["id"].as_str().expect("row carries id"))
        .collect();
    assert_eq!(
        ids,
        [
            beta["id"].as_str().expect("id"),
            alpha["id"].as_str().expect("id")
        ],
        "edits never refresh comparison position"
    );

    // Exact price sorts stay stable, and unknown sorts refuse.
    let (status, ascending) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers?sort=price_asc"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ascending["live"][0]["price"], "480.00");
    assert_eq!(ascending["live"][1]["price"], "520.00");
    let (status, descending) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers?sort=price_desc"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(descending["live"][0]["price"], "520.00");
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers?sort=cheapest"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_field");

    // A material request revision stales both rows for contact while they
    // stay comparable; an elapsed cycle moves them to history instead.
    let owner: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Order Owner'")
            .fetch_one(db.pool())
            .await
            .expect("owner reads");
    let demand_id: uuid::Uuid = demand.parse().expect("id parses");
    procurali_backend::application::revise_request::revise_request(
        db.pool(),
        owner,
        demand_id,
        procurali_backend::application::revise_request::ReviseInput {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Needs ice maker.".to_owned(),
        },
    )
    .await
    .expect("fixture revision revises");
    let (status, list) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["live"].as_array().expect("live").len(), 2);
    for row in list["live"].as_array().expect("live") {
        assert_eq!(row["contact_allowed"], false, "stale rows lose contact");
    }
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
    let (status, list) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["live"].as_array().expect("live").is_empty());
    assert_eq!(list["history"].as_array().expect("history").len(), 2);
    db.cleanup().await.expect("suite cleans up");
}
