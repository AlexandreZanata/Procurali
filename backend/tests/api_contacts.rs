//! Contact-initiation acceptance (P08-T02): the author's guarded handoff.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, offer,
//! and contact routes over real PostgreSQL with the deterministic fake
//! provider. Proves:
//! - a valid buyer handoff succeeds with the verified destination while
//!   unrelated buyers, sellers acting as buyers, and anonymous callers are
//!   refused;
//! - wrong states, withdrawn/rejected offers, elapsed deadlines, and
//!   restricted or blocked parties refuse with no destination disclosed;
//! - stale acknowledged terms conflict instead of silently accepting.
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
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p08t02-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p08t02-test-only-encryption-key";

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
    .merge(request_routes(RequestsState::new(db.pool().clone())))
    .merge(offer_routes(OffersState::new(db.pool().clone())))
    .merge(contact_routes(ContactsState::new(
        db.pool().clone(),
        LOOKUP_KEY.to_owned(),
        ENCRYPTION_KEY.to_owned(),
    )))
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

fn contact_body(handoff: &str, terms: i32) -> Value {
    json!({
        "handoff_id": handoff,
        "expected_offer_terms": terms,
        "entry_source": "offer_detail",
    })
}

/// The handoff receipt carries identifiers plus the destination — and
/// nothing else sensitive.
fn assert_receipt_shape(body: &Value) {
    let object = body.as_object().expect("receipt is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "contact_id",
            "cycle_number",
            "destination",
            "offer_id",
            "repeat",
            "request_id"
        ]
    );
}

/// Refusals never name a destination key.
fn assert_no_destination(body: &Value) {
    let rendered = body.to_string();
    assert!(
        !rendered.contains("destination"),
        "no destination on refusal"
    );
    assert!(!rendered.contains("cipher"), "no ciphertext on refusal");
}

#[tokio::test]
async fn valid_buyer_contact_succeeds_while_others_refuse() {
    let db = TestDatabase::create("p08t02_valid")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t02_valid_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0211", "Contact Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0211").await;
    provision_active(&app, "+55 11 90000-0212", "Contact Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0212").await;
    provision_active(&app, "+55 11 90000-0213", "Outside Buyer").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0213").await;
    let demand = publish_demand(&app, &buyer_cookie).await;
    let offer = submit_offer(&app, &demand, &seller_cookie).await;

    // The owning buyer receives the verified seller destination with one
    // initiation recorded; the receipt shape is exact.
    let handoff = uuid::Uuid::now_v7().to_string();
    let (status, receipt) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(contact_body(&handoff, 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_receipt_shape(&receipt);
    assert_eq!(receipt["repeat"], false);
    assert!(
        receipt["destination"]
            .as_str()
            .expect("destination renders")
            .contains("5511900000212"),
        "the verified seller destination reaches its buyer"
    );
    let contact_id = receipt["contact_id"].as_str().expect("receipt carries id");

    // The identical handoff replays its existing result with no duplicate.
    let (status, replayed) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(contact_body(&handoff, 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["contact_id"], contact_id);
    assert_eq!(replayed["repeat"], true);

    // Unrelated buyers, sellers acting as buyers, and anonymous callers
    // share refusals with no destination and no oracle.
    for (cookie, path) in [
        (
            Some(stranger_cookie.as_str()),
            format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        ),
        (
            Some(seller_cookie.as_str()),
            format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        ),
    ] {
        let (status, body) = call(
            app.clone(),
            "POST",
            &path,
            Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
            cookie,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        assert_no_destination(&body);
    }
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthenticated");
    assert_no_destination(&body);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn refusals_carry_no_destination() {
    let db = TestDatabase::create("p08t02_refusals")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t02_refusals_"),
        "known suite identity in the database name"
    );
    // One demand per terminal condition keeps fixtures independent.
    async fn demand_with_offer(app: &axum::Router, buyer: &str, seller: &str) -> (String, String) {
        let demand = publish_demand(app, buyer).await;
        let offer = submit_offer(app, &demand, seller).await;
        (demand, offer)
    }
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0221", "Refusal Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0221").await;
    provision_active(&app, "+55 11 90000-0222", "Refusal Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0222").await;

    // Withdrawn and rejected offers refuse without destination.
    let (withdrawn_demand, withdrawn_offer) =
        demand_with_offer(&app, &buyer_cookie, &seller_cookie).await;
    sqlx::query("UPDATE offers SET state = 'withdrawn' WHERE id = $1")
        .bind(withdrawn_offer.parse::<uuid::Uuid>().expect("id parses"))
        .execute(db.pool())
        .await
        .expect("synthetic withdrawal applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{withdrawn_demand}/offers/{withdrawn_offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "forbidden_state");
    assert_no_destination(&body);

    let (rejected_demand, rejected_offer) =
        demand_with_offer(&app, &buyer_cookie, &seller_cookie).await;
    sqlx::query("UPDATE offers SET state = 'rejected' WHERE id = $1")
        .bind(rejected_offer.parse::<uuid::Uuid>().expect("id parses"))
        .execute(db.pool())
        .await
        .expect("synthetic rejection applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{rejected_demand}/offers/{rejected_offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);

    // Elapsed deadlines refuse as expired without destination.
    let (elapsed_demand, elapsed_offer) =
        demand_with_offer(&app, &buyer_cookie, &seller_cookie).await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(elapsed_demand.parse::<uuid::Uuid>().expect("id parses"))
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic expiry applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{elapsed_demand}/offers/{elapsed_offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "expired");
    assert_no_destination(&body);

    // Banned sellers, suspended buyers, and blocked pairs refuse alike.
    let (banned_demand, banned_offer) =
        demand_with_offer(&app, &buyer_cookie, &seller_cookie).await;
    sqlx::query("UPDATE users SET state = 'banned' WHERE display_name = 'Refusal Seller'")
        .execute(db.pool())
        .await
        .expect("synthetic ban applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{banned_demand}/offers/{banned_offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);
    sqlx::query("UPDATE users SET state = 'active' WHERE display_name = 'Refusal Seller'")
        .execute(db.pool())
        .await
        .expect("synthetic restoration applies");
    sqlx::query("UPDATE users SET state = 'suspended' WHERE display_name = 'Refusal Owner'")
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{banned_demand}/offers/{banned_offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_no_destination(&body);
    sqlx::query("UPDATE users SET state = 'active' WHERE display_name = 'Refusal Owner'")
        .execute(db.pool())
        .await
        .expect("synthetic restoration applies");
    let buyer: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Refusal Owner'")
            .fetch_one(db.pool())
            .await
            .expect("buyer reads");
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Refusal Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)")
        .bind(buyer)
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic block applies");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{banned_demand}/offers/{banned_offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden_role");
    assert_no_destination(&body);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn stale_terms_acknowledgment_is_conflict() {
    let db = TestDatabase::create("p08t02_stale")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t02_stale_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0231", "Stale Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0231").await;
    provision_active(&app, "+55 11 90000-0232", "Stale Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0232").await;
    let demand = publish_demand(&app, &buyer_cookie).await;
    let offer = submit_offer(&app, &demand, &seller_cookie).await;

    // Edited terms move the acknowledgment target: the old number
    // conflicts instead of silently accepting, and the current one passes.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let edited = procurali_backend::persistence::offers::TermsSnapshot {
        description: "Frost-free 350L".to_owned(),
        price_cents: 55_000,
        condition: "used".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: String::new(),
    };
    let offer_id: uuid::Uuid = offer.parse().expect("id parses");
    procurali_backend::persistence::offers::insert_terms(&mut tx, offer_id, 2, &edited)
        .await
        .expect("terms store");
    procurali_backend::persistence::offers::update_current_terms(&mut tx, offer_id, 2, &edited)
        .await
        .expect("current promotes");
    tx.commit().await.expect("edit commits");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 1)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict_revision");
    assert_no_destination(&body);
    let (status, receipt) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 2)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(receipt["repeat"], false);

    // A revised demand strands the offer the same way.
    let owner: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Stale Owner'")
            .fetch_one(db.pool())
            .await
            .expect("owner reads");
    procurali_backend::application::revise_request::revise_request(
        db.pool(),
        owner,
        demand.parse().expect("id parses"),
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
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(contact_body(&uuid::Uuid::now_v7().to_string(), 2)),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict_revision");
    assert_no_destination(&body);
    db.cleanup().await.expect("suite cleans up");
}
