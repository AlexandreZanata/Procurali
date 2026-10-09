//! Pair-safety journeys (P10-T06): block writers close cross-flow obligations.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, offer,
//! contact, block, and report routes over real PostgreSQL with the
//! deterministic fake provider. Proves:
//! - unblocking leaves the invalidated offer dead (fresh and replayed
//!   handoffs refuse with no destination) while a later eligible
//!   new-cycle demand flows end to end (AC-36, EC-24);
//! - anonymous browsing stops at authentication with no disclosure,
//!   signed-in strangers see no foreign demand, the buyer comparison
//!   hides the blocked offer from the live set without phone material,
//!   and the historical report path stays open without reopening contact
//!   (INV-28, INV-31, INV-35, AC-35, EC-23).
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
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::reports::{routes as report_routes, ReportsState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p10t06-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p10t06-test-only-encryption-key";

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
    .merge(block_routes(BlocksState::new(db.pool().clone())))
    .merge(report_routes(ReportsState::new(db.pool().clone())))
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

/// Full live demand with one offer through the real routes; return ids.
async fn live_demand(
    app: &axum::Router,
    buyer_cookie: &str,
    seller_cookie: &str,
) -> (String, String) {
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
        Some(buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let demand = draft["id"].as_str().expect("receipt carries id").to_owned();
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{demand}/publication"),
        None,
        Some(buyer_cookie),
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
        Some(seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let offer = submitted["id"]
        .as_str()
        .expect("receipt carries id")
        .to_owned();
    (demand, offer)
}

async fn post_handoff(
    app: &axum::Router,
    demand: &str,
    offer: &str,
    handoff_id: &str,
    cookie: &str,
) -> (StatusCode, Value) {
    call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(json!({
            "handoff_id": handoff_id,
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(cookie),
    )
    .await
}

/// Refusals carry no destination key and no phone digits.
fn assert_no_destination(body: &Value) {
    let rendered = body.to_string();
    for absent in [
        "destination",
        "cipher",
        "phone",
        "lookup",
        "token",
        "address",
        "90000",
        "55119",
    ] {
        assert!(!rendered.contains(absent), "no {absent} on refusal");
    }
}

#[tokio::test]
async fn unblock_keeps_old_offer_dead_and_new_cycle_possible() {
    let db = TestDatabase::create("p10t06_unblock")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t06_unblock_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0703", "Journey Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0703").await;
    provision_active(&app, "+55 11 90000-0704", "Journey Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0704").await;
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Journey Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    let (demand, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let first_handoff = uuid::Uuid::now_v7().to_string();
    let (status, receipt) =
        post_handoff(&app, &demand, &offer, &first_handoff, &buyer_cookie).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(receipt["repeat"], false);
    assert!(
        receipt["destination"]
            .as_str()
            .expect("receipt carries destination")
            .contains("5511900000704"),
        "live destination reaches the buyer"
    );

    // Blocking invalidates the live pair offer; replaying the ORIGINAL
    // handoff identity afterwards revalidates instead of replaying any
    // cached destination.
    let (status, blocked) = call(
        app.clone(),
        "POST",
        "/api/v1/blocks",
        Some(json!({"blocked_user_id": seller})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(blocked["invalidated_offers"], 1);
    let (status, replayed) =
        post_handoff(&app, &demand, &offer, &first_handoff, &buyer_cookie).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(replayed["code"], "forbidden_state");
    assert_no_destination(&replayed);

    // Unblocking lifts only the restriction: the old offer stays terminal
    // and refuses fresh handoffs, while a later eligible new-cycle demand
    // flows end to end with a live destination.
    let (status, lifted) = call(
        app.clone(),
        "DELETE",
        &format!("/api/v1/blocks/{seller}"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lifted["removed"], true);
    let state: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer.parse::<uuid::Uuid>().expect("id parses"))
        .fetch_one(db.pool())
        .await
        .expect("offer reads");
    assert_eq!(state, "invalidated");
    let (status, refused) = post_handoff(
        &app,
        &demand,
        &offer,
        &uuid::Uuid::now_v7().to_string(),
        &buyer_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&refused);

    let (fresh_demand, fresh_offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let (status, fresh) = post_handoff(
        &app,
        &fresh_demand,
        &fresh_offer,
        &uuid::Uuid::now_v7().to_string(),
        &buyer_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(fresh["repeat"], false);
    assert!(
        fresh["destination"]
            .as_str()
            .expect("receipt carries destination")
            .contains("5511900000704"),
        "new-cycle contact decrypts live"
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn anonymous_limited_signed_in_hidden_report_stays_open() {
    let db = TestDatabase::create("p10t06_journey")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t06_journey_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0705", "Hidden Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0705").await;
    provision_active(&app, "+55 11 90000-0706", "Hidden Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0706").await;
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Hidden Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    provision_active(&app, "+55 11 90000-0707", "Hidden Stranger").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0707").await;
    let (demand, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let (status, _) = post_handoff(
        &app,
        &demand,
        &offer,
        &uuid::Uuid::now_v7().to_string(),
        &buyer_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/v1/blocks",
        Some(json!({"blocked_user_id": seller})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Anonymous browsing stops at authentication on every surface, with
    // static refusals that disclose nothing.
    for (method, path) in [
        ("GET", "/api/v1/requests".to_owned()),
        ("GET", format!("/api/v1/requests/{demand}")),
        ("GET", format!("/api/v1/requests/{demand}/offers")),
    ] {
        let (status, body) = call(app.clone(), method, &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthenticated");
        assert_no_destination(&body);
    }

    // Signed-in strangers see no foreign demand: missing and foreign are
    // indistinguishable, and carry nothing private.
    for path in [
        format!("/api/v1/requests/{demand}"),
        format!("/api/v1/requests/{demand}/offers"),
    ] {
        let (status, body) = call(app.clone(), "GET", &path, None, Some(&stranger_cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        assert_no_destination(&body);
    }

    // The buyer's own comparison drops the invalidated pair offer from the
    // live set without phone material anywhere in the projection.
    let (status, compared) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let live: Vec<String> = compared["live"]
        .as_array()
        .expect("live set reads")
        .iter()
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect();
    assert!(!live.contains(&offer), "blocked offer leaves the live set");
    assert_no_destination(&compared);

    // The limited historical report path stays available while blocked,
    // and filing it reopens no contact.
    let (status, filed) = call(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(json!({
            "target_kind": "offer",
            "target_id": offer,
            "reason": "spam",
            "detail": "history stays reportable",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(filed["status"], "open");
    let (status, refused) = post_handoff(
        &app,
        &demand,
        &offer,
        &uuid::Uuid::now_v7().to_string(),
        &buyer_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&refused);
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 1, "reporting opened no contact");
    let reports: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(db.pool())
        .await
        .expect("reports read");
    assert_eq!(reports, 1);
    db.cleanup().await.expect("suite cleans up");
}
