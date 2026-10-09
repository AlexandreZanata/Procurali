//! Block acceptance (P10-T02): bilateral restriction with history intact.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, offer,
//! contact, and block routes over real PostgreSQL with the deterministic
//! fake provider. Proves:
//! - blocking after contact stops future handoffs while the prior
//!   initiation stands untouched;
//! - unblocking lifts only the restriction, never reviving offers, while
//!   fresh eligible activity proceeds normally;
//! - self, foreign, malformed, and anonymous mutations refuse, and repeat
//!   actions record no duplicate facts.
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
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p10t02-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p10t02-test-only-encryption-key";

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

async fn start_handoff(app: &axum::Router, demand: &str, offer: &str, cookie: &str) -> Value {
    let (status, receipt) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    receipt
}

async fn relationship_facts(pool: &sqlx::PgPool, kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'relationship' AND kind = $1",
    )
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("facts read")
}

/// Refusals carry no destination key.
fn assert_no_destination(body: &Value) {
    let rendered = body.to_string();
    assert!(
        !rendered.contains("destination"),
        "no destination on refusal"
    );
    assert!(!rendered.contains("cipher"), "no ciphertext on refusal");
}

#[tokio::test]
async fn blocking_after_contact_stops_handoffs_preserves_prior() {
    let db = TestDatabase::create("p10t02_block")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t02_block_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0401", "Block Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0401").await;
    provision_active(&app, "+55 11 90000-0402", "Block Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0402").await;
    let (demand, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let receipt = start_handoff(&app, &demand, &offer, &buyer_cookie).await;
    assert_eq!(receipt["repeat"], false);

    // Blocking invalidates the live pair offer atomically with the row and
    // one fact; the prior initiation stands byte-identical.
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Block Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    let (status, blocked) = call(
        app.clone(),
        "POST",
        "/api/v1/blocks",
        Some(json!({"blocked_user_id": seller})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(blocked["created"], true);
    assert_eq!(blocked["invalidated_offers"], 1);
    assert_eq!(
        relationship_facts(db.pool(), "relationship.blocked").await,
        1
    );
    let state: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer.parse::<uuid::Uuid>().expect("id parses"))
        .fetch_one(db.pool())
        .await
        .expect("offer reads");
    assert_eq!(state, "invalidated");

    // Future handoffs refuse with no destination while the prior contact
    // row and its frozen snapshot stand untouched. The refusal arrives
    // through the invalidated offer state — the block caused it — while
    // the direct block path below proves the relationship refusal itself.
    let (status, body) = call(
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
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);
    // Direct block refusal on a fresh eligible demand with no offers yet.
    let (status, fresh) = call(
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
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let fresh = fresh["id"].as_str().expect("receipt carries id").to_owned();
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{fresh}/publication"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{fresh}/offers"),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Washer 10kg", "price": "700.00",
            "condition": "new", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&seller_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden_role");
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT offer_price_cents, request_budget_cents FROM contacts")
            .fetch_all(db.pool())
            .await
            .expect("contacts read");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], (52_000, 60_000));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn unblocking_cannot_revive_previous_offers() {
    let db = TestDatabase::create("p10t02_unblock")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t02_unblock_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    provision_active(&app, "+55 11 90000-0403", "Unblock Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0403").await;
    provision_active(&app, "+55 11 90000-0404", "Unblock Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0404").await;
    let (demand, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Unblock Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/v1/blocks",
        Some(json!({"blocked_user_id": seller})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Lifting removes the restriction only: the invalidated offer stays
    // terminal, and its handoff still refuses.
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
    assert_eq!(
        relationship_facts(db.pool(), "relationship.unblocked").await,
        1
    );
    let state: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer.parse::<uuid::Uuid>().expect("id parses"))
        .fetch_one(db.pool())
        .await
        .expect("offer reads");
    assert_eq!(state, "invalidated");
    let (status, body) = call(
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
    assert_eq!(status, StatusCode::CONFLICT);
    assert_no_destination(&body);

    // Repeat unblocking is a quiet no-op, while a fresh demand flows
    // normally under the same pair.
    let (status, lifted) = call(
        app.clone(),
        "DELETE",
        &format!("/api/v1/blocks/{seller}"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lifted["removed"], false);
    assert_eq!(
        relationship_facts(db.pool(), "relationship.unblocked").await,
        1
    );
    let (fresh_demand, fresh_offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let receipt = start_handoff(&app, &fresh_demand, &fresh_offer, &buyer_cookie).await;
    assert_eq!(receipt["repeat"], false);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn wrong_owner_self_block_refuse_repeat_facts_deduped() {
    let db = TestDatabase::create("p10t02_misuse")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t02_misuse_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer: uuid::Uuid = provision_active(&app, "+55 11 90000-0405", "Misuse Owner").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0405").await;
    provision_active(&app, "+55 11 90000-0406", "Misuse Seller").await;

    // Self-blocks refuse with the field named; foreign keys (blocker
    // claims, reasons, states) are unknown fields and refuse as such.
    for (body, code, field) in [
        (
            json!({"blocked_user_id": buyer}),
            "invalid_field",
            "blocked_user_id",
        ),
        (
            json!({"blocked_user_id": buyer, "blocker_id": buyer}),
            "invalid_field",
            "",
        ),
        (
            json!({"blocked_user_id": buyer, "reason": "spam"}),
            "invalid_field",
            "",
        ),
        (json!({}), "missing_field", "blocked_user_id"),
        (
            json!({"blocked_user_id": "not-a-uuid"}),
            "invalid_field",
            "blocked_user_id",
        ),
    ] {
        let (status, refused) = call(
            app.clone(),
            "POST",
            "/api/v1/blocks",
            Some(body),
            Some(&buyer_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(refused["code"], code);
        if !field.is_empty() {
            assert_eq!(refused["fields"][field], code);
        }
    }
    // Ghost accounts refuse without an oracle beyond not-found.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/v1/blocks",
        Some(json!({"blocked_user_id": uuid::Uuid::now_v7()})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    // Anonymous callers share one refusal on both methods.
    for (method, path) in [
        ("POST", "/api/v1/blocks".to_owned()),
        ("DELETE", format!("/api/v1/blocks/{buyer}")),
    ] {
        let (status, body) = call(
            app.clone(),
            method,
            &path,
            (method == "POST").then(|| json!({"blocked_user_id": buyer})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthenticated");
    }
    // Repeat blocks converge with a single fact; unblocking a stranger's
    // pair is a quiet no-op with no oracle.
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Misuse Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    let (status, first) = call(
        app.clone(),
        "POST",
        "/api/v1/blocks",
        Some(json!({"blocked_user_id": seller})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, second) = call(
        app.clone(),
        "POST",
        "/api/v1/blocks",
        Some(json!({"blocked_user_id": seller})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["created"], false);
    assert_eq!(
        relationship_facts(db.pool(), "relationship.blocked").await,
        1
    );
    assert_eq!(first["invalidated_offers"], 0, "no pair offers existed");
    let stranger = uuid::Uuid::now_v7();
    let (status, lifted) = call(
        app.clone(),
        "DELETE",
        &format!("/api/v1/blocks/{stranger}"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lifted["removed"], false);
    db.cleanup().await.expect("suite cleans up");
}
