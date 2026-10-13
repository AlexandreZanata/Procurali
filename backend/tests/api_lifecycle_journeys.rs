//! Lifecycle journeys (P09-T04): terminal effects reconciled end to end.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, offer,
//! contact, and notice routes over real PostgreSQL with the deterministic
//! fake provider, composing the application lifecycle operations
//! (revision, renewal, closure, removal, expiry, cascades) where no HTTP
//! route owns them yet. Proves per distinct closure path:
//! - exact lifecycle states with distinguishable events and outcome rows;
//! - no new offer or destination after the closure-winning mutation;
//! - terminal rows never renew while fresh needs start cleanly.
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
use procurali_backend::http::blocks::{routes as block_routes, BlocksState};
use procurali_backend::http::catalogs::{routes as catalog_routes, CatalogsState};
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::http::moderation::{routes as moderation_routes, ModerationState};
use procurali_backend::http::notices::{routes as notice_routes, NoticesState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::reports::{routes as report_routes, ReportsState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p09t04-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p09t04-test-only-encryption-key";

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
    .merge(notice_routes(NoticesState::new(db.pool().clone())))
    .merge(catalog_routes(CatalogsState::new(db.pool().clone())))
    .merge(block_routes(BlocksState::new(db.pool().clone())))
    .merge(report_routes(ReportsState::new(db.pool().clone())))
    .merge(moderation_routes(ModerationState::new(db.pool().clone())))
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

/// Full live demand with one offer and one handoff through real paths:
/// buyer and seller cookies plus demand, offer, and destination.
struct LiveDemand {
    buyer_cookie: String,
    seller_cookie: String,
    demand: String,
    offer: String,
}

async fn live_demand(
    app: &axum::Router,
    buyer_phone: &str,
    buyer_name: &str,
    seller_phone: &str,
    seller_name: &str,
) -> LiveDemand {
    provision_active(app, buyer_phone, buyer_name).await;
    let buyer_cookie = login_cookie(app.clone(), buyer_phone).await;
    provision_active(app, seller_phone, seller_name).await;
    let seller_cookie = login_cookie(app.clone(), seller_phone).await;
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
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let demand = draft["id"].as_str().expect("receipt carries id").to_owned();
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{demand}/publication"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
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
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let offer = submitted["id"]
        .as_str()
        .expect("receipt carries id")
        .to_owned();
    let (status, handoff) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(handoff["destination"]
        .as_str()
        .expect("destination renders")
        .contains("55119000"));
    LiveDemand {
        buyer_cookie,
        seller_cookie,
        demand,
        offer,
    }
}

async fn account_id(pool: &sqlx::PgPool, display: &str) -> uuid::Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE display_name = $1")
        .bind(display)
        .fetch_one(pool)
        .await
        .expect("account reads")
}

async fn request_state(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> (String, String, i32) {
    sqlx::query_as("SELECT state, visibility, current_cycle_number FROM requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .expect("row reads")
}

async fn request_kinds(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT kind FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 ORDER BY occurred_at, id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await
    .expect("events read")
}

async fn live_offer_count(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM offers
         WHERE request_id = $1 AND state IN ('sent', 'viewed', 'contacted')",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .expect("live rows read")
}

/// Refusals after closure carry no destination key anywhere.
fn assert_no_destination(body: &Value) {
    let rendered = body.to_string();
    assert!(
        !rendered.contains("destination"),
        "no destination on refusal"
    );
    assert!(!rendered.contains("cipher"), "no ciphertext on refusal");
}

#[tokio::test]
async fn completed_journey_reconciles_end_to_end() {
    let db = TestDatabase::create("p09t04_completed")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t04_completed_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let live = live_demand(
        &app,
        "+55 11 90000-0701",
        "Journey Owner",
        "+55 11 90000-0702",
        "Journey Seller",
    )
    .await;
    let demand_id: uuid::Uuid = live.demand.parse().expect("id parses");
    let offer_id: uuid::Uuid = live.offer.parse().expect("id parses");

    // Completion with platform attribution, then the cascade retires the
    // live offer: exact lifecycle, facts, outcome, and slot states.
    let recorded = procurali_backend::application::record_outcome::record_outcome(
        db.pool(),
        account_id(db.pool(), "Journey Owner").await,
        demand_id,
        procurali_backend::application::record_outcome::OutcomeAnswer::Completed(
            procurali_backend::application::record_outcome::CompletionSource::Platform { offer_id },
        ),
    )
    .await
    .expect("completion records");
    assert_eq!(recorded.outcome, "completed");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    procurali_backend::application::request_offer_cascades::cascade_request_offers(
        &mut tx,
        demand_id,
        procurali_backend::application::request_offer_cascades::CascadeCause::RequestClosed,
    )
    .await
    .expect("cascade moves");
    tx.commit().await.expect("cascade commits");
    assert_eq!(request_state(db.pool(), demand_id).await.0, "completed");
    assert_eq!(live_offer_count(db.pool(), demand_id).await, 0);
    let kinds = request_kinds(db.pool(), demand_id).await;
    assert!(kinds.contains(&"request.published".to_owned()));
    assert!(kinds.contains(&"request.completed".to_owned()));

    // Nothing new starts: offers, handoffs, revisions, and renewals all
    // refuse, with no destination anywhere near the refusals.
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{}/offers", live.demand),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "price": "520.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&live.seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "forbidden_state");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/requests/{}/offers/{}/contact",
            live.demand, live.offer
        ),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);
    assert!(
        procurali_backend::application::revise_request::revise_request(
            db.pool(),
            account_id(db.pool(), "Journey Owner").await,
            demand_id,
            procurali_backend::application::revise_request::ReviseInput {
                title: "Refrigerator".to_owned(),
                category_code: "home_appliances".to_owned(),
                budget: "600.00".to_owned(),
                condition: "either".to_owned(),
                city_code: "campinas".to_owned(),
                region_code: "centro".to_owned(),
                notes: "Changed.".to_owned(),
            },
        )
        .await
        .is_err()
    );
    assert!(
        procurali_backend::application::renew_request::renew_request(
            db.pool(),
            account_id(db.pool(), "Journey Owner").await,
            demand_id,
            procurali_backend::application::renew_request::RenewalConfirmation {
                title: "Refrigerator".to_owned(),
                category_code: "home_appliances".to_owned(),
                budget: "600.00".to_owned(),
                condition: "either".to_owned(),
                city_code: "campinas".to_owned(),
                region_code: "centro".to_owned(),
                notes: String::new(),
            },
        )
        .await
        .is_err()
    );

    // A fresh need starts cleanly through the same routes.
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(json!({
            "title": "Washing machine",
            "category_code": "home_appliances",
            "budget": "800.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
        })),
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn cancelled_journey_reconciles_end_to_end() {
    let db = TestDatabase::create("p09t04_cancelled")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t04_cancelled_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let live = live_demand(
        &app,
        "+55 11 90000-0703",
        "Cancel Owner",
        "+55 11 90000-0704",
        "Cancel Seller",
    )
    .await;
    let demand_id: uuid::Uuid = live.demand.parse().expect("id parses");

    // Cancellation records buyer abandonment (never expiry or resolution),
    // retires the live offer, and stops everything behind it.
    let recorded = procurali_backend::application::record_outcome::record_outcome(
        db.pool(),
        account_id(db.pool(), "Cancel Owner").await,
        demand_id,
        procurali_backend::application::record_outcome::OutcomeAnswer::Cancelled,
    )
    .await
    .expect("cancellation records");
    assert_eq!(recorded.outcome, "cancelled");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    procurali_backend::application::request_offer_cascades::cascade_request_offers(
        &mut tx,
        demand_id,
        procurali_backend::application::request_offer_cascades::CascadeCause::RequestClosed,
    )
    .await
    .expect("cascade moves");
    tx.commit().await.expect("cascade commits");
    assert_eq!(request_state(db.pool(), demand_id).await.0, "cancelled");
    assert_eq!(live_offer_count(db.pool(), demand_id).await, 0);
    let kinds = request_kinds(db.pool(), demand_id).await;
    assert!(kinds.contains(&"request.cancelled".to_owned()));
    assert!(!kinds.contains(&"request.completed".to_owned()));
    assert!(!kinds.contains(&"request.expired".to_owned()));

    let (status, body) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/requests/{}/offers/{}/contact",
            live.demand, live.offer
        ),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);
    assert!(
        procurali_backend::application::renew_request::renew_request(
            db.pool(),
            account_id(db.pool(), "Cancel Owner").await,
            demand_id,
            procurali_backend::application::renew_request::RenewalConfirmation {
                title: "Refrigerator".to_owned(),
                category_code: "home_appliances".to_owned(),
                budget: "600.00".to_owned(),
                condition: "either".to_owned(),
                city_code: "campinas".to_owned(),
                region_code: "centro".to_owned(),
                notes: String::new(),
            },
        )
        .await
        .is_err()
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn expired_journey_reconciles_end_to_end() {
    let db = TestDatabase::create("p09t04_expired")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t04_expired_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let live = live_demand(
        &app,
        "+55 11 90000-0705",
        "Lapsed Owner",
        "+55 11 90000-0706",
        "Lapsed Seller",
    )
    .await;
    let demand_id: uuid::Uuid = live.demand.parse().expect("id parses");

    // Expiry retires the live offer with its own fact; submissions and
    // handoffs refuse as expired; a material revision lands without
    // republishing; renewal opens a genuinely new cycle.
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
    // While the recorded state still lags, handoffs refuse as expired.
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/requests/{}/offers/{}/contact",
            live.demand, live.offer
        ),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "expired");
    assert_no_destination(&body);
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{}/offers", live.demand),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "price": "520.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&live.seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "expired");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        procurali_backend::application::request_eligibility::expire_if_elapsed(
            &mut tx, demand_id, now
        )
        .await
        .expect("row expires"),
        procurali_backend::application::request_eligibility::ExpiryOutcome::Transitioned
    );
    procurali_backend::application::request_offer_cascades::cascade_request_offers(
        &mut tx,
        demand_id,
        procurali_backend::application::request_offer_cascades::CascadeCause::CycleEnded,
    )
    .await
    .expect("cascade moves");
    tx.commit().await.expect("transition commits");
    assert_eq!(request_state(db.pool(), demand_id).await.0, "expired");
    assert_eq!(live_offer_count(db.pool(), demand_id).await, 0);
    let kinds = request_kinds(db.pool(), demand_id).await;
    assert!(kinds.contains(&"request.expired".to_owned()));

    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{}/offers", live.demand),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "price": "520.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&live.seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "forbidden_state");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/requests/{}/offers/{}/contact",
            live.demand, live.offer
        ),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "forbidden_state");
    assert_no_destination(&body);
    let revised = procurali_backend::application::revise_request::revise_request(
        db.pool(),
        account_id(db.pool(), "Lapsed Owner").await,
        demand_id,
        procurali_backend::application::revise_request::ReviseInput {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Still wanted.".to_owned(),
        },
    )
    .await
    .expect("expired revision revises");
    assert!(revised.material);
    assert_eq!(request_state(db.pool(), demand_id).await.2, 1);
    let renewed = procurali_backend::application::renew_request::renew_request(
        db.pool(),
        account_id(db.pool(), "Lapsed Owner").await,
        demand_id,
        procurali_backend::application::renew_request::RenewalConfirmation {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Still wanted.".to_owned(),
        },
    )
    .await
    .expect("eligible renewal renews");
    assert_eq!(renewed.cycle_number, 2);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn removed_journey_reconciles_end_to_end() {
    let db = TestDatabase::create("p09t04_removed")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t04_removed_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let live = live_demand(
        &app,
        "+55 11 90000-0707",
        "Removed Owner",
        "+55 11 90000-0708",
        "Removed Seller",
    )
    .await;
    let demand_id: uuid::Uuid = live.demand.parse().expect("id parses");
    let offer_id: uuid::Uuid = live.offer.parse().expect("id parses");
    let owner = account_id(db.pool(), "Removed Owner").await;

    // Owner removal cancels and hides with history intact: reads go
    // privacy-dark, submissions and handoffs refuse, and public catalog
    // carries none of the concealed content.
    let removed =
        procurali_backend::application::remove_request::remove_request(db.pool(), owner, demand_id)
            .await
            .expect("removal removes");
    assert_eq!(removed.state, "cancelled");
    assert_eq!(removed.visibility, "hidden");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    procurali_backend::application::request_offer_cascades::cascade_request_offers(
        &mut tx,
        demand_id,
        procurali_backend::application::request_offer_cascades::CascadeCause::RequestRemoved,
    )
    .await
    .expect("cascade moves");
    tx.commit().await.expect("cascade commits");
    assert_eq!(live_offer_count(db.pool(), demand_id).await, 0);
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{}/offers", live.demand),
        None,
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["total"], 1,
        "owners still read their own concealed row"
    );
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{}/offers", live.demand),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "price": "520.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&live.seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "forbidden_state");
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/requests/{}/offers/{}/contact",
            live.demand, live.offer
        ),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);
    let kinds = request_kinds(db.pool(), demand_id).await;
    assert!(kinds.contains(&"request.removed".to_owned()));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM request_revisions WHERE request_id = $1"
        )
        .bind(demand_id)
        .fetch_one(db.pool())
        .await
        .expect("revisions read"),
        1,
        "history survives concealment"
    );
    let _ = offer_id;
    db.cleanup().await.expect("suite cleans up");
}

/// Submit one offer through the route; return its id.
async fn submit_offer(app: &axum::Router, demand: &str, cookie: &str, price: &str) -> String {
    let (status, submitted) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "price": price,
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
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

/// Open one handoff through the route; return the destination on success.
async fn open_handoff(
    app: &axum::Router,
    demand: &str,
    offer: &str,
    cookie: &str,
    terms: i32,
) -> (StatusCode, Value) {
    call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": terms,
            "entry_source": "offer_detail",
        })),
        Some(cookie),
    )
    .await
}

#[tokio::test]
async fn two_seller_comparison_contact_and_declared_completion() {
    let db = TestDatabase::create("p19t01_comparison")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p19t01_comparison_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0711", "Compare Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0711").await;
    provision_active(&app, "+55 11 90000-0712", "First Seller").await;
    let seller_one_cookie = login_cookie(app.clone(), "+55 11 90000-0712").await;
    provision_active(&app, "+55 11 90000-0713", "Second Seller").await;
    let seller_two_cookie = login_cookie(app.clone(), "+55 11 90000-0713").await;
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
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let demand = draft["id"].as_str().expect("receipt carries id").to_owned();
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{demand}/publication"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let demand_id: uuid::Uuid = demand.parse().expect("id parses");
    let offer_one = submit_offer(&app, &demand, &seller_one_cookie, "520.00").await;
    let offer_two = submit_offer(&app, &demand, &seller_two_cookie, "499.99").await;

    // Private comparison: the buyer reads both offers with no destinations;
    // each seller reads exactly their own offer, and nothing of the other's.
    let (status, compared) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(compared["total"], 2);
    let rendered = compared.to_string();
    assert!(
        !rendered.contains("destination"),
        "no handoff in comparison"
    );
    assert!(!rendered.contains("+55"), "no phone in comparison");
    for cookie in [&seller_one_cookie, &seller_two_cookie] {
        let (status, mine) = call(
            app.clone(),
            "GET",
            "/api/v1/offers/mine",
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(mine.as_array().expect("own list renders").len(), 1);
        let (status, _) = call(
            app.clone(),
            "GET",
            &format!("/api/v1/requests/{demand}/offers"),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "sellers never compare");
    }
    let (status, _) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/offers/{offer_one}"),
        None,
        Some(&seller_two_cookie),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "sellers never open another offer"
    );

    // Two distinct contacts are both usable comparison evidence.
    for offer in [&offer_one, &offer_two] {
        let (status, handoff) = open_handoff(&app, &demand, offer, &buyer_cookie, 1).await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(handoff["destination"]
            .as_str()
            .expect("destination renders")
            .contains("55119000"));
    }

    // Declared completion with platform attribution is a declaration, never
    // a verified sale: the outcome row says completed/platform, and no sale,
    // purchase, payment, or transaction fact exists anywhere near it.
    let buyer = account_id(db.pool(), "Compare Buyer").await;
    let offer_one_id: uuid::Uuid = offer_one.parse().expect("id parses");
    let recorded = procurali_backend::application::record_outcome::record_outcome(
        db.pool(),
        buyer,
        demand_id,
        procurali_backend::application::record_outcome::OutcomeAnswer::Completed(
            procurali_backend::application::record_outcome::CompletionSource::Platform {
                offer_id: offer_one_id,
            },
        ),
    )
    .await
    .expect("completion records");
    assert_eq!(recorded.outcome, "completed");
    assert_eq!(recorded.source.as_deref(), Some("platform"));
    assert_eq!(recorded.attributed_offer_id, Some(offer_one_id));
    let sale_facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE kind LIKE '%sale%' OR kind LIKE '%transaction%' OR kind LIKE '%payment%' OR kind LIKE '%purchase%'",
    )
    .fetch_one(db.pool())
    .await
    .expect("events read");
    assert_eq!(sale_facts, 0, "declared completion mints no sale");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    procurali_backend::application::request_offer_cascades::cascade_request_offers(
        &mut tx,
        demand_id,
        procurali_backend::application::request_offer_cascades::CascadeCause::RequestClosed,
    )
    .await
    .expect("cascade moves");
    tx.commit().await.expect("cascade commits");
    assert_eq!(live_offer_count(db.pool(), demand_id).await, 0);
    let (status, body) = open_handoff(&app, &demand, &offer_two, &buyer_cookie, 1).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn withdrawal_and_terms_edit_close_the_loop() {
    let db = TestDatabase::create("p19t01_withdrawedit")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p19t01_withdrawedit_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let live = live_demand(
        &app,
        "+55 11 90000-0714",
        "Withdraw Buyer",
        "+55 11 90000-0715",
        "Withdraw Seller",
    )
    .await;
    let demand_id: uuid::Uuid = live.demand.parse().expect("id parses");
    let offer_id: uuid::Uuid = live.offer.parse().expect("id parses");
    let seller = account_id(db.pool(), "Withdraw Seller").await;

    // Withdrawal after contact stops futures while preserving the earned
    // history: the contact row stands, the next handoff refuses.
    let withdrawn = procurali_backend::application::withdraw_offer::withdraw_offer(
        db.pool(),
        seller,
        offer_id,
        procurali_backend::application::withdraw_offer::WithdrawReason::Withdrawn,
    )
    .await
    .expect("withdrawal withdraws");
    assert_eq!(withdrawn.state, "withdrawn");
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts WHERE offer_id = $1")
        .bind(offer_id)
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 1, "earned contact history stands");
    let (status, body) = open_handoff(&app, &live.demand, &live.offer, &live.buyer_cookie, 1).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "forbidden_state");
    assert_no_destination(&body);

    // A live offer's edited terms need a fresh buyer choice: the stale
    // acknowledgment refuses, the current one opens the handoff.
    provision_active(&app, "+55 11 90000-0716", "Edit Seller").await;
    let editor_cookie = login_cookie(app.clone(), "+55 11 90000-0716").await;
    let edited_offer = submit_offer(&app, &live.demand, &editor_cookie, "530.00").await;
    let edited_id: uuid::Uuid = edited_offer.parse().expect("id parses");
    let editor = account_id(db.pool(), "Edit Seller").await;
    let edited = procurali_backend::application::edit_offer::edit_offer(
        db.pool(),
        editor,
        edited_id,
        procurali_backend::application::edit_offer::EditTermsInput {
            expected_terms_number: Some(1),
            description: Some("Frost-free 300L".to_owned()),
            price: Some("540.00".to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("live terms edit");
    assert_eq!(edited.price, "540.00", "the edit promotes current terms");
    let (status, body) =
        open_handoff(&app, &live.demand, &edited_offer, &live.buyer_cookie, 1).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict_revision");
    let (status, handoff) =
        open_handoff(&app, &live.demand, &edited_offer, &live.buyer_cookie, 2).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(handoff["destination"]
        .as_str()
        .expect("destination renders")
        .contains("55119000"));
    let _ = demand_id;
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn expiry_then_renewal_starts_fresh_cycle() {
    let db = TestDatabase::create("p19t01_renewal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p19t01_renewal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let live = live_demand(
        &app,
        "+55 11 90000-0717",
        "Renew Buyer",
        "+55 11 90000-0718",
        "Renew Seller",
    )
    .await;
    let demand_id: uuid::Uuid = live.demand.parse().expect("id parses");
    let owner = account_id(db.pool(), "Renew Buyer").await;
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

    // The no-offer gap: expiry retires the live offer and refuses new
    // submissions until the buyer explicitly renews.
    let report = procurali_backend::operations::jobs::expire_requests::process_expiry(
        db.pool(),
        demand_id,
        now,
    )
    .await
    .expect("sweep processes");
    assert!(report.transitioned);
    let mut tx = db.pool().begin().await.expect("transaction begins");
    procurali_backend::application::request_offer_cascades::cascade_request_offers(
        &mut tx,
        demand_id,
        procurali_backend::application::request_offer_cascades::CascadeCause::CycleEnded,
    )
    .await
    .expect("cascade moves");
    tx.commit().await.expect("cascade commits");
    assert_eq!(request_state(db.pool(), demand_id).await.0, "expired");
    assert_eq!(live_offer_count(db.pool(), demand_id).await, 0);
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{}/offers", live.demand),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Frost-free 300L", "price": "520.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&live.seller_cookie),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "expired demand takes no offers"
    );

    // Explicit renewal starts a fresh second cycle: new offers submit and
    // contact against current terms while first-cycle history stands.
    let renewed = procurali_backend::application::renew_request::renew_request(
        db.pool(),
        owner,
        demand_id,
        procurali_backend::application::renew_request::RenewalConfirmation {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: String::new(),
        },
    )
    .await
    .expect("renewal renews");
    assert_eq!(renewed.cycle_number, 2);
    let state = request_state(db.pool(), demand_id).await;
    assert_eq!((state.0.as_str(), state.2), ("active", 2));
    let (status, submitted) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{}/offers", live.demand),
        Some(json!({
            "revision_number": 1, "cycle_number": 2,
            "description": "Frost-free 300L", "price": "520.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&live.seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let fresh_offer = submitted["id"]
        .as_str()
        .expect("receipt carries id")
        .to_owned();
    let (status, handoff) =
        open_handoff(&app, &live.demand, &fresh_offer, &live.buyer_cookie, 1).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(handoff["destination"]
        .as_str()
        .expect("destination renders")
        .contains("55119000"));
    let kinds = request_kinds(db.pool(), demand_id).await;
    assert!(kinds.contains(&"request.expired".to_owned()));
    assert!(kinds.contains(&"request.renewed".to_owned()));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn safety_report_block_suspend_delete_and_policy_change() {
    let db = TestDatabase::create("p19t01_safety")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p19t01_safety_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let live = live_demand(
        &app,
        "+55 11 90000-0719",
        "Safety Buyer",
        "+55 11 90000-0720",
        "Safety Seller",
    )
    .await;
    let demand_id: uuid::Uuid = live.demand.parse().expect("id parses");
    let offer_id: uuid::Uuid = live.offer.parse().expect("id parses");
    let buyer = account_id(db.pool(), "Safety Buyer").await;
    let seller = account_id(db.pool(), "Safety Seller").await;

    // Accountable staff: a launch administrator bootstraps, then grants a
    // safety moderator through the real audited operations.
    provision_active(&app, "+55 11 90000-0721", "Safety Admin").await;
    let admin = account_id(db.pool(), "Safety Admin").await;
    procurali_backend::application::staff_permissions::bootstrap_grant(
        db.pool(),
        procurali_backend::application::staff_permissions::BootstrapInput {
            user_id: admin,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "launch cover".to_owned(),
            operator_label: "launch-operator-1".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("launch bootstrap grants");
    provision_active(&app, "+55 11 90000-0722", "Safety Moderator").await;
    let moderator = account_id(db.pool(), "Safety Moderator").await;
    procurali_backend::application::staff_permissions::grant_role(
        db.pool(),
        admin,
        procurali_backend::application::staff_permissions::GrantInput {
            user_id: moderator,
            role: "moderator".to_owned(),
            scope: "safety".to_owned(),
            reason: "triage cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("moderator granted");

    // The buyer reports the offer; the allegation files without moving any
    // lifecycle, and the live handoff still stands on its own eligibility.
    let report = procurali_backend::application::create_report::submit_report(
        db.pool(),
        buyer,
        procurali_backend::application::create_report::ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer_id,
            reason: "spam".to_owned(),
            detail: "Identical bulk offers across demands.".to_owned(),
        },
    )
    .await
    .expect("report files");
    assert_eq!(report.status, "open");
    let (status, handoff) =
        open_handoff(&app, &live.demand, &live.offer, &live.buyer_cookie, 1).await;
    assert_eq!(status, StatusCode::OK, "a report alone moves no lifecycle");
    assert_eq!(
        handoff["repeat"], true,
        "the retry replays the earned contact"
    );

    // Suspension hides the offer: contact refuses on safety grounds while
    // the earned contact fact and the report case both stand.
    let suspended = procurali_backend::application::suspend_content::suspend_content(
        db.pool(),
        moderator,
        procurali_backend::application::suspend_content::SuspendInput {
            target_kind: "offer".to_owned(),
            target_id: offer_id,
            reason: "suspected bulk spam".to_owned(),
            policy_version: "v1".to_owned(),
            purpose: "safety-triage".to_owned(),
            case_id: None,
        },
    )
    .await
    .expect("suspension suspends");
    assert_eq!(suspended.state, "suspended");
    let (status, body) = open_handoff(&app, &live.demand, &live.offer, &live.buyer_cookie, 1).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "forbidden_state");
    assert_no_destination(&body);

    // The buyer's block is directional and recorded: it invalidates the
    // pair's live offers, so the suspended offer keeps refusing on safety
    // grounds (restriction precedence) while the block row stands.
    let blocked = procurali_backend::application::block_user::block_user(db.pool(), buyer, seller)
        .await
        .expect("block blocks");
    assert!(blocked.created);
    let (status, body) = open_handoff(&app, &live.demand, &live.offer, &live.buyer_cookie, 1).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);
    let blocks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2",
    )
    .bind(buyer)
    .bind(seller)
    .fetch_one(db.pool())
    .await
    .expect("blocks read");
    assert_eq!(blocks, 1, "the directional block stands recorded");

    // The direct relationship refusal surfaces on fresh work: the blocked
    // seller cannot submit on the buyer's new demand.
    let (status, second) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(json!({
            "title": "Washing machine",
            "category_code": "home_appliances",
            "budget": "800.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": "",
        })),
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let demand_two = second["id"]
        .as_str()
        .expect("receipt carries id")
        .to_owned();
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{demand_two}/publication"),
        None,
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand_two}/offers"),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Washer 10kg", "price": "700.00",
            "condition": "new", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&live.seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden_role");

    // Deletion ends the buyer's access without erasing history: sessions
    // refuse, private reads refuse, and rows/facts stand for audit.
    let deleted = procurali_backend::application::delete_account::delete_account(
        db.pool(),
        buyer,
        procurali_backend::application::delete_account::DeleteAccountInput {
            user_id: buyer,
            reason: "leaving the marketplace".to_owned(),
        },
    )
    .await
    .expect("self-deletion deletes");
    assert!(deleted.deleted);
    let (status, _) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{}/offers", live.demand),
        None,
        Some(&live.buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM requests WHERE id = $1")
        .bind(demand_id)
        .fetch_one(db.pool())
        .await
        .expect("history reads");
    assert_eq!(remaining, 1, "request history stands after deletion");
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts WHERE request_id = $1")
        .bind(demand_id)
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert!(contacts >= 1, "contact evidence stands after deletion");

    // Policy change applies prospectively: the new version becomes effective
    // for new evaluations while the in-flight demand stands untouched. The
    // launch administrator grants a dedicated policy administrator (separation
    // of duties) before the version is recorded.
    let before = procurali_backend::application::metric_corrections::effective_policy_version(
        db.pool(),
        chrono::Utc::now(),
    )
    .await
    .expect("effective version reads");
    assert_eq!(before, "v1");
    provision_active(&app, "+55 11 90000-0723", "Policy Admin").await;
    let policy_admin = account_id(db.pool(), "Policy Admin").await;
    procurali_backend::application::staff_permissions::grant_role(
        db.pool(),
        admin,
        procurali_backend::application::staff_permissions::GrantInput {
            user_id: policy_admin,
            role: "administrator".to_owned(),
            scope: "policy".to_owned(),
            reason: "policy cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("policy administrator granted");
    procurali_backend::application::policy_changes::create_policy_version(
        db.pool(),
        policy_admin,
        procurali_backend::application::policy_changes::VersionInput {
            version: "v2".to_owned(),
            effective_from: chrono::Utc::now(),
            description: "P19 journey fixture: tightened contact wording.".to_owned(),
        },
    )
    .await
    .expect("policy version creates");
    let after = procurali_backend::application::metric_corrections::effective_policy_version(
        db.pool(),
        chrono::Utc::now(),
    )
    .await
    .expect("effective version re-reads");
    assert_eq!(after, "v2");
    let stored: (String, String) =
        sqlx::query_as("SELECT version, description FROM policy_versions WHERE version = 'v2'")
            .fetch_one(db.pool())
            .await
            .expect("version row reads");
    assert_eq!(stored.1, "P19 journey fixture: tightened contact wording.");
    let demand_state = request_state(db.pool(), demand_id).await;
    assert_eq!(
        demand_state.0.as_str(),
        "cancelled",
        "deletion cancels the unresolved owned demand without erasing it"
    );
    db.cleanup().await.expect("suite cleans up");
}
