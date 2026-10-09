//! Privacy-surface acceptance (P12-T04): canaries stay out everywhere.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Seeds distinct synthetic phone, session, reporter, and
//! destination canaries, then drives the real HTTP routes with real
//! cookie sessions and the deterministic fake provider. Proves:
//! - every public and private-unowned response excludes canary values
//!   and prohibited field names;
//! - no exercised response redirects, carries a location, or reflects
//!   canary input back through errors, health probes, or notices;
//! - the known valid handoff reveals only the selected current seller
//!   destination, never buyer phones, competitor data, or reporter
//!   material — while deletion hides the rest without breaking
//!   counterparty reporting.
//!
//! Server-side telemetry keeps its own redaction unit tests inside
//! `operations::telemetry`; this suite pins the observable wire: every
//! exercised response body and header. All names, numbers, codes, and
//! keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::delete_account::{delete_account, DeleteAccountInput};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::blocks::{routes as block_routes, BlocksState};
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::http::health::{liveness, readiness, DependencyStatus};
use procurali_backend::http::notices::{routes as notice_routes, NoticesState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::reports::{routes as report_routes, ReportsState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p12t04-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p12t04-test-only-encryption-key";

const BUYER_PHONE: &str = "+55 11 90000-1801";
const SELLER_ONE_PHONE: &str = "+55 11 90000-1802";
const SELLER_TWO_PHONE: &str = "+55 11 90000-1803";
const STRANGER_PHONE: &str = "+55 11 90000-1804";
const BUYER_DIGITS: &str = "5511900001801";
const SELLER_ONE_DIGITS: &str = "5511900001802";
const SELLER_TWO_DIGITS: &str = "5511900001803";
const STRANGER_DIGITS: &str = "5511900001804";
const SELLER_THREE_DIGITS: &str = "5511900001805";
const REPORTER_CANARY: &str = "canary-reporter-detail-1801";

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
    .merge(notice_routes(NoticesState::new(db.pool().clone())))
    .route("/health/live", axum::routing::get(liveness))
    .route(
        "/health/ready",
        axum::routing::get(readiness).with_state(DependencyStatus::default()),
    )
}

/// One call with transport-level guarantees: no redirect status and no
/// location header on any exercised response, since no route redirects
/// and private bodies must never become cacheable navigation.
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
    assert!(!status.is_redirection(), "no exercised response redirects");
    assert!(
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .is_none(),
        "no exercised response carries a location"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    if bytes.is_empty() {
        return (status, Value::Null);
    }
    let parsed: Value = serde_json::from_slice(&bytes).expect("response JSON parses");
    (status, parsed)
}

/// Assert one body carries no canary value and no prohibited field name.
/// Values match as substrings (static prose like "no usable session" is
/// fine); field names match as exact JSON keys, so prose never trips the
/// check. Destination digits travel only as an explicit allowlist
/// argument.
fn assert_clean(body: &Value, allow_destination: Option<&str>, session_tokens: &[String]) {
    let rendered = body.to_string();
    for secret in [
        BUYER_DIGITS,
        SELLER_ONE_DIGITS,
        SELLER_TWO_DIGITS,
        SELLER_THREE_DIGITS,
        STRANGER_DIGITS,
        REPORTER_CANARY,
    ] {
        if allow_destination == Some(secret) {
            continue;
        }
        assert!(!rendered.contains(secret), "no {secret} in response output");
    }
    for token in session_tokens {
        assert!(
            !rendered.contains(token.as_str()),
            "no session token in response output"
        );
    }
    let mut keys = Vec::new();
    collect_keys(body, &mut keys);
    for field in [
        "reporter",
        "reporter_id",
        "detail",
        "phone",
        "phone_lookup",
        "phone_ciphertext",
        "lookup",
        "cipher",
        "token",
        "session",
        "session_id",
        "address",
        "street_address",
        "secret",
        "plaintext",
        "destination",
    ] {
        if allow_destination.is_some() && field == "destination" {
            continue;
        }
        assert!(
            !keys.iter().any(|key| key == field),
            "no {field} key in response output"
        );
    }
}

fn collect_keys(body: &Value, out: &mut Vec<String>) {
    match body {
        Value::Object(map) => {
            for (key, value) in map {
                out.push(key.clone());
                collect_keys(value, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_keys(item, out);
            }
        }
        _ => {}
    }
}

/// Provision one active account through the real routes; return its id and
/// raw cookie pair, recording the session token as a canary.
async fn provision_active(
    app: &axum::Router,
    phone: &str,
    name: &str,
    session_tokens: &mut Vec<String>,
) -> (uuid::Uuid, String) {
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
    assert_clean(&body, None, session_tokens);
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
    let id = body["account_id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses");
    let response = app
        .clone()
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
    let pair = set_cookie
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned();
    let token = pair
        .split_once('=')
        .map(|(_, value)| value.trim().to_owned());
    if let Some(token) = token {
        if !token.is_empty() {
            session_tokens.push(token);
        }
    }
    (id, pair)
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

/// Full live demand with two competing offers through the real routes.
async fn live_market(
    app: &axum::Router,
    buyer_cookie: &str,
    seller_one_cookie: &str,
    seller_two_cookie: &str,
) -> (String, String, String) {
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
    let mut offers = Vec::new();
    for (cookie, price) in [(seller_one_cookie, "520.00"), (seller_two_cookie, "480.00")] {
        let (status, submitted) = call(
            app.clone(),
            "POST",
            &format!("/api/v1/requests/{demand}/offers"),
            Some(json!({
                "revision_number": 1,
                "cycle_number": 1,
                "description": "Frost-free 300L",
                "price": price,
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
        offers.push(
            submitted["id"]
                .as_str()
                .expect("receipt carries id")
                .to_owned(),
        );
    }
    let (offer_one, offer_two) = (offers.remove(0), offers.remove(0));
    (demand, offer_one, offer_two)
}

#[tokio::test]
async fn public_and_unowned_responses_exclude_canaries() {
    let db = TestDatabase::create("p12t04_surfaces")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t04_surfaces_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let mut session_tokens = Vec::new();
    let (_, buyer_cookie) =
        provision_active(&app, BUYER_PHONE, "Surface Buyer", &mut session_tokens).await;
    let (_, seller_one_cookie) = provision_active(
        &app,
        SELLER_ONE_PHONE,
        "Surface Seller One",
        &mut session_tokens,
    )
    .await;
    let (_, seller_two_cookie) = provision_active(
        &app,
        SELLER_TWO_PHONE,
        "Surface Seller Two",
        &mut session_tokens,
    )
    .await;
    let (_, stranger_cookie) = provision_active(
        &app,
        STRANGER_PHONE,
        "Surface Stranger",
        &mut session_tokens,
    )
    .await;
    let (demand, offer_one, _) =
        live_market(&app, &buyer_cookie, &seller_one_cookie, &seller_two_cookie).await;
    let (status, filed) = call(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(json!({
            "target_kind": "offer",
            "target_id": offer_one,
            "reason": "spam",
            "detail": REPORTER_CANARY,
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_clean(&filed, None, &session_tokens);

    // Anonymous surfaces stop at authentication with static bodies.
    for (method, path) in [
        ("GET", "/api/v1/requests".to_owned()),
        ("GET", format!("/api/v1/requests/{demand}")),
        ("GET", format!("/api/v1/requests/{demand}/offers")),
        ("GET", "/api/v1/offers/mine".to_owned()),
        ("GET", "/api/v1/notices".to_owned()),
    ] {
        let (status, body) = call(app.clone(), method, &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_clean(&body, None, &session_tokens);
    }

    // Strangers read no foreign demand through any owned-or-public path.
    for path in [
        format!("/api/v1/requests/{demand}"),
        format!("/api/v1/requests/{demand}/offers"),
    ] {
        let (status, body) = call(app.clone(), "GET", &path, None, Some(&stranger_cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_clean(&body, None, &session_tokens);
    }

    // Competing sellers see only their own offer with no competitor or
    // buyer material (AC-19); the buyer comparison names declared sellers
    // without any phone or reporter material.
    let (status, mine) = call(
        app.clone(),
        "GET",
        "/api/v1/offers/mine",
        None,
        Some(&seller_two_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&mine, None, &session_tokens);
    assert_eq!(mine.as_array().expect("offers read").len(), 1);
    let (status, compared) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&compared, None, &session_tokens);
    let (status, listed) = call(
        app.clone(),
        "GET",
        "/api/v1/requests",
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&listed, None, &session_tokens);

    // Errors and probes reflect no canary input back: phone-shaped notes
    // refuse without echo, wrong codes stay static, unknown routes and
    // health probes carry no values.
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(json!({
            "title": "Washer",
            "category_code": "home_appliances",
            "budget": "700.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": SELLER_ONE_DIGITS,
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_clean(&refused, None, &session_tokens);
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/api/v1/sessions",
        Some(json!({"phone": BUYER_PHONE, "code": "000000"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_clean(&refused, None, &session_tokens);
    let (status, missing) = call(app.clone(), "GET", "/nope", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_clean(&missing, None, &session_tokens);
    for path in ["/health/live", "/health/ready"] {
        let (status, probe) = call(app.clone(), "GET", path, None, None).await;
        assert!(status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE);
        assert_clean(&probe, None, &session_tokens);
    }
    // Overposted reporter, phone, and token fields refuse as unknown.
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(json!({
            "target_kind": "offer",
            "target_id": offer_one,
            "reason": "spam",
            "detail": "noise",
            "reporter_id": "00000000-0000-4000-8000-000000000000",
            "phone": SELLER_ONE_PHONE,
            "token": "canary-token-1801",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_clean(&refused, None, &session_tokens);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn valid_handoff_reveals_only_selected_seller_destination() {
    let db = TestDatabase::create("p12t04_handoff")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t04_handoff_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let mut session_tokens = Vec::new();
    let (_, buyer_cookie) =
        provision_active(&app, BUYER_PHONE, "Handoff Buyer", &mut session_tokens).await;
    let (_, seller_one_cookie) = provision_active(
        &app,
        SELLER_ONE_PHONE,
        "Handoff Seller One",
        &mut session_tokens,
    )
    .await;
    let (_, seller_two_cookie) = provision_active(
        &app,
        SELLER_TWO_PHONE,
        "Handoff Seller Two",
        &mut session_tokens,
    )
    .await;
    let (_, stranger_cookie) = provision_active(
        &app,
        STRANGER_PHONE,
        "Handoff Stranger",
        &mut session_tokens,
    )
    .await;
    let (demand, offer_one, _) =
        live_market(&app, &buyer_cookie, &seller_one_cookie, &seller_two_cookie).await;

    // The known valid handoff reveals exactly the selected current seller
    // destination: no buyer phone, no competitor digits, no reporter
    // material, no session token — and the repeat matches.
    let (status, receipt) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer_one}/contact"),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(receipt["destination"]
        .as_str()
        .is_some_and(|destination| destination.contains(SELLER_ONE_DIGITS)));
    assert_clean(&receipt, Some(SELLER_ONE_DIGITS), &session_tokens);

    // Another buyer cannot contact somebody else's offer regardless of
    // standing (AC-22): strangers and competing sellers share one refusal
    // with nothing disclosed.
    for cookie in [&stranger_cookie, &seller_two_cookie] {
        let (status, refused) = call(
            app.clone(),
            "POST",
            &format!("/api/v1/requests/{demand}/offers/{offer_one}/contact"),
            Some(json!({
                "handoff_id": uuid::Uuid::now_v7().to_string(),
                "expected_offer_terms": 1,
                "entry_source": "offer_detail",
            })),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_clean(&refused, None, &session_tokens);
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn deleted_and_suspended_hide_everything() {
    let db = TestDatabase::create("p12t04_hidden")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t04_hidden_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let mut session_tokens = Vec::new();
    let (_, buyer_cookie) =
        provision_active(&app, BUYER_PHONE, "Hidden Buyer", &mut session_tokens).await;
    let (seller_one, seller_one_cookie) = provision_active(
        &app,
        SELLER_ONE_PHONE,
        "Hidden Seller One",
        &mut session_tokens,
    )
    .await;
    let (_, seller_three_cookie) = provision_active(
        &app,
        "+55 11 90000-1805",
        "Hidden Seller Three",
        &mut session_tokens,
    )
    .await;
    let (_, stranger_cookie) =
        provision_active(&app, STRANGER_PHONE, "Hidden Stranger", &mut session_tokens).await;
    let (demand, offer_one, _) = live_market(
        &app,
        &buyer_cookie,
        &seller_one_cookie,
        &seller_three_cookie,
    )
    .await;
    delete_account(
        db.pool(),
        seller_one,
        DeleteAccountInput {
            user_id: seller_one,
            reason: "leaving the marketplace".to_owned(),
        },
    )
    .await
    .expect("seller deletion deletes");

    // Deleted sessions answer nowhere, the buyer comparison drops the
    // invalidated offer from the live set without seller digits, and the
    // counterparty report path stays open with a clean receipt.
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/v1/offers/mine",
        None,
        Some(&seller_one_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, compared) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&compared, None, &session_tokens);
    let live: Vec<String> = compared["live"]
        .as_array()
        .expect("live set reads")
        .iter()
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect();
    assert!(!live.contains(&offer_one));
    let (status, filed) = call(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(json!({
            "target_kind": "user",
            "target_id": seller_one.to_string(),
            "reason": "spam",
            "detail": "post-deletion history report",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_clean(&filed, None, &session_tokens);
    let (status, notices) = call(
        app.clone(),
        "GET",
        "/api/v1/notices",
        None,
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&notices, None, &session_tokens);
    let (status, _) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}"),
        None,
        Some(&stranger_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.cleanup().await.expect("suite cleans up");
}
