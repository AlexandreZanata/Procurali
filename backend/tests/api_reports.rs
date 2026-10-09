//! Report-submission acceptance (P10-T04): relevant allegations in, denials out.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real account, session, draft, publication, offer,
//! contact, and report routes over real PostgreSQL with the deterministic
//! fake provider (closure travels the real application operation, which owns
//! no HTTP route yet — the same direct-proof pattern as the closure cards).
//! Proves:
//! - a historical contacted closed request stays reportable (AC-37, EC-14);
//! - an unrelated inaccessible private offer, self-reports, fabricated
//!   targets, unexplained `other`, and anonymous calls refuse with nothing
//!   stored;
//! - filing changes no trust label, ban, lifecycle, or visibility state
//!   (INV-36, EC-31);
//! - suspended/banned callers keep only their own-history path at the
//!   application layer with no marketplace power granted (the cookie
//!   boundary keeps the shared active-session contract; the allowance
//!   branch is proven by direct calls).
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::create_report::{submit_report, ReportInput, SubmitError};
use procurali_backend::application::eligibility::{check_actor, CheckOutcome};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::report_updates::{
    submit_grouped_report, withdraw_report, GroupError,
};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::reports::{routes as report_routes, ReportsState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p10t04-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p10t04-test-only-encryption-key";

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

async fn file_offer_report(
    app: &axum::Router,
    offer: &str,
    reason: &str,
    detail: &str,
    cookie: &str,
) -> (StatusCode, Value) {
    call(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(json!({
            "target_kind": "offer",
            "target_id": offer,
            "reason": reason,
            "detail": detail,
        })),
        Some(cookie),
    )
    .await
}

/// Receipts name the allegation only: no reporter, detail, phone, or secret.
fn assert_receipt_private(body: &Value) {
    let rendered = body.to_string();
    for absent in [
        "reporter",
        "detail",
        "phone",
        "lookup",
        "cipher",
        "token",
        "session",
        "address",
        "destination",
        "secret",
        "90000",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in report receipt");
    }
}

async fn report_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(pool)
        .await
        .expect("reports read")
}

async fn case_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM report_cases")
        .fetch_one(pool)
        .await
        .expect("cases read")
}

#[tokio::test]
async fn historical_contacted_closed_request_can_be_reported() {
    let db = TestDatabase::create("p10t04_closed")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t04_closed_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-0501", "Closed Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0501").await;
    provision_active(&app, "+55 11 90000-0502", "Closed Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0502").await;
    let (demand, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let receipt = start_handoff(&app, &demand, &offer, &buyer_cookie).await;
    assert_eq!(receipt["repeat"], false);

    // Closure travels the real operation (no HTTP route owns it yet): the
    // demand completes while contact history stands.
    let demand_id: uuid::Uuid = demand.parse().expect("demand id parses");
    let closed = close_request(
        db.pool(),
        buyer,
        demand_id,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("closure closes");
    assert_eq!(closed.state, "completed");

    // The contacted buyer reports the historical offer after closure: the
    // allegation records with a frozen snapshot, and closure stands.
    let (status, filed) = file_offer_report(
        &app,
        &offer,
        "fraud",
        "paid outside after contact",
        &buyer_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(filed["status"], "open");
    assert_eq!(filed["reason"], "fraud");
    assert_receipt_private(&filed);
    assert_eq!(report_count(db.pool()).await, 1);
    let snapshot: (String, String) =
        sqlx::query_as("SELECT target_title_snapshot, target_context_snapshot FROM reports")
            .fetch_one(db.pool())
            .await
            .expect("snapshot reads");
    assert_eq!(snapshot.0, "Frost-free 300L");
    let state: String = sqlx::query_scalar("SELECT state FROM requests WHERE id = $1")
        .bind(demand_id)
        .fetch_one(db.pool())
        .await
        .expect("closed request still exists");
    assert_eq!(state, "completed", "closure is not reopened by the report");
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn unrelated_private_offer_and_self_report_are_refused() {
    let db = TestDatabase::create("p10t04_refused")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t04_refused_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-0503", "Refused Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0503").await;
    provision_active(&app, "+55 11 90000-0504", "Refused Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0504").await;
    provision_active(&app, "+55 11 90000-0505", "Refused Stranger").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0505").await;
    let (demand, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;

    // A stranger with no view-or-history standing cannot reach into a
    // private offer: indistinguishable from missing, by design.
    let (status, body) = file_offer_report(&app, &offer, "spam", "noise", &stranger_cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    // Self-reports refuse on the target: sellers cannot report their own
    // offer, buyers cannot report their own request.
    let (status, body) = file_offer_report(&app, &offer, "spam", "noise", &seller_cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_field");
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(json!({
            "target_kind": "request",
            "target_id": demand,
            "reason": "spam",
            "detail": "my own demand",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Fabricated identifiers, unexplained `other`, and anonymous calls
    // refuse with nothing stored.
    let (status, body) = file_offer_report(
        &app,
        &uuid::Uuid::now_v7().to_string(),
        "spam",
        "ghost",
        &buyer_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let (status, body) = file_offer_report(&app, &offer, "other", "   ", &buyer_cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_field");
    let (status, body) = file_offer_report(&app, &offer, "spam", "noise", "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthenticated");
    let _ = buyer;

    assert_eq!(report_count(db.pool()).await, 0);
    assert_eq!(case_count(db.pool()).await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn creating_report_changes_no_trust_or_ban_state() {
    let db = TestDatabase::create("p10t04_steady")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t04_steady_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-0506", "Steady Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0506").await;
    provision_active(&app, "+55 11 90000-0507", "Steady Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0507").await;
    let (demand, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    start_handoff(&app, &demand, &offer, &buyer_cookie).await;

    let demand_id: uuid::Uuid = demand.parse().expect("demand id parses");
    let offer_id: uuid::Uuid = offer.parse().expect("offer id parses");
    let before_users: Vec<String> =
        sqlx::query_scalar("SELECT state FROM users ORDER BY created_at")
            .fetch_all(db.pool())
            .await
            .expect("account states read");
    let before_request: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand_id)
            .fetch_one(db.pool())
            .await
            .expect("request reads");
    let before_offer: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer_id)
        .fetch_one(db.pool())
        .await
        .expect("offer reads");

    let (status, filed) =
        file_offer_report(&app, &offer, "spam", "repeated noise", &buyer_cookie).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(filed["status"], "open");

    // The allegation changes nothing but its own rows: both accounts stay
    // as they were, lifecycle and visibility stand, the case stays open
    // for assessment, and volume alone bans nobody.
    let after_users: Vec<String> =
        sqlx::query_scalar("SELECT state FROM users ORDER BY created_at")
            .fetch_all(db.pool())
            .await
            .expect("account states re-read");
    assert_eq!(before_users, after_users);
    assert!(after_users.iter().all(|state| state == "active"));
    let after_request: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand_id)
            .fetch_one(db.pool())
            .await
            .expect("request re-reads");
    assert_eq!(before_request, after_request);
    let after_offer: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer_id)
        .fetch_one(db.pool())
        .await
        .expect("offer re-reads");
    assert_eq!(before_offer, after_offer);
    let case_status: String = sqlx::query_scalar("SELECT status FROM report_cases")
        .fetch_one(db.pool())
        .await
        .expect("case reads");
    assert_eq!(case_status, "open");
    assert_eq!(report_count(db.pool()).await, 1);
    let _ = buyer;
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_banned_keep_only_own_history_path() {
    let db = TestDatabase::create("p10t04_restricted")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t04_restricted_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-0508", "Restricted Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0508").await;
    provision_active(&app, "+55 11 90000-0509", "Restricted Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0509").await;
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Restricted Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    provision_active(&app, "+55 11 90000-0510", "Restricted Stranger").await;
    let stranger: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Restricted Stranger'")
            .fetch_one(db.pool())
            .await
            .expect("stranger reads");
    let (_, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let offer_id: uuid::Uuid = offer.parse().expect("offer id parses");
    let offer_target = ReportInput {
        target_kind: "offer".to_owned(),
        target_id: offer_id,
        reason: "spam".to_owned(),
        detail: "history report while suspended".to_owned(),
    };

    // Suspension lands after the history was earned: the buyer's own-history
    // report still files at the application layer, while a suspended
    // stranger with no standing is refused on the same target.
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id IN ($1, $2)")
        .bind(buyer)
        .bind(stranger)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    let filed = submit_report(db.pool(), buyer, offer_target.clone())
        .await
        .expect("own-history report files while suspended");
    assert_eq!(filed.status, "open");
    assert_eq!(
        submit_report(db.pool(), stranger, offer_target.clone()).await,
        Err(SubmitError::NotRelevant)
    );

    // A ban keeps the same narrow path (user target shares the contact
    // history), and filing grants no marketplace power in return.
    sqlx::query("UPDATE users SET state = 'banned' WHERE id = $1")
        .bind(buyer)
        .execute(db.pool())
        .await
        .expect("synthetic ban applies");
    submit_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "user".to_owned(),
            target_id: seller,
            reason: "inappropriate_behavior".to_owned(),
            detail: "history report while banned".to_owned(),
        },
    )
    .await
    .expect("own-history report files while banned");
    assert_eq!(
        check_actor(db.pool(), buyer).await.expect("guard reads"),
        CheckOutcome::Refused(
            procurali_backend::application::eligibility::RefusalReason::AccountNotActive
        )
    );
    let offers: i64 = sqlx::query_scalar("SELECT count(*) FROM offers")
        .fetch_one(db.pool())
        .await
        .expect("offers read");
    assert_eq!(offers, 1, "reporting wrote no marketplace row");
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 0, "reporting wrote no contact row");
    assert_eq!(report_count(db.pool()).await, 2);
    assert_eq!(case_count(db.pool()).await, 2);
    let buyer_state: String = sqlx::query_scalar("SELECT state FROM users WHERE id = $1")
        .bind(buyer)
        .fetch_one(db.pool())
        .await
        .expect("buyer reads");
    assert_eq!(buyer_state, "banned", "reporting lifted no restriction");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn duplicate_filings_group_into_one_case() {
    let db = TestDatabase::create("p10t05_grouped")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t05_grouped_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer_one = provision_active(&app, "+55 11 90000-0601", "Grouped Buyer One").await;
    let buyer_one_cookie = login_cookie(app.clone(), "+55 11 90000-0601").await;
    provision_active(&app, "+55 11 90000-0602", "Grouped Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0602").await;
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Grouped Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    provision_active(&app, "+55 11 90000-0603", "Grouped Buyer Two").await;
    let buyer_two_cookie = login_cookie(app.clone(), "+55 11 90000-0603").await;
    let buyer_two: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Grouped Buyer Two'")
            .fetch_one(db.pool())
            .await
            .expect("second buyer reads");
    let (_, offer_one) = live_demand(&app, &buyer_one_cookie, &seller_cookie).await;
    let (_, offer_two) = live_demand(&app, &buyer_two_cookie, &seller_cookie).await;
    let _ = (buyer_one, buyer_two, offer_two);

    // Same reporter refiles the same offer incident through the grouped
    // writer: one intake, the refiling as a duplicate information record.
    let first = submit_grouped_report(
        db.pool(),
        buyer_one,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer_one.parse().expect("offer id parses"),
            reason: "spam".to_owned(),
            detail: "first noise".to_owned(),
        },
    )
    .await
    .expect("first filing opens the intake");
    assert_eq!(first.status, "open");
    let repeat = submit_grouped_report(
        db.pool(),
        buyer_one,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer_one.parse().expect("offer id parses"),
            reason: "spam".to_owned(),
            detail: "still noisy".to_owned(),
        },
    )
    .await
    .expect("refiling adds information");
    assert_eq!(repeat.case_id, first.case_id, "one incident intake");
    assert_eq!(repeat.status, "duplicate");

    // Two buyers with their own seller history report the same seller for
    // the same reason: one user incident, both sources retained privately.
    let buyer_one_user = submit_grouped_report(
        db.pool(),
        buyer_one,
        ReportInput {
            target_kind: "user".to_owned(),
            target_id: seller,
            reason: "spam".to_owned(),
            detail: "buyer one context".to_owned(),
        },
    )
    .await
    .expect("first user filing opens its intake");
    let buyer_two_user = submit_grouped_report(
        db.pool(),
        buyer_two,
        ReportInput {
            target_kind: "user".to_owned(),
            target_id: seller,
            reason: "spam".to_owned(),
            detail: "buyer two context".to_owned(),
        },
    )
    .await
    .expect("second reporter groups in");
    assert_eq!(
        buyer_two_user.case_id, buyer_one_user.case_id,
        "different reporters share one case"
    );
    assert_eq!(buyer_two_user.status, "open");
    let members: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT reporter_id, detail FROM reports WHERE case_id = $1 ORDER BY created_at",
    )
    .bind(buyer_one_user.case_id)
    .fetch_all(db.pool())
    .await
    .expect("grouped sources read");
    assert_eq!(members.len(), 2);
    assert_ne!(members[0].0, members[1].0, "sources stay distinct");
    assert_ne!(members[0].1, members[1].1, "contexts stay distinct");

    // Volume alone bans nobody and assesses nothing by itself.
    assert_eq!(case_count(db.pool()).await, 2);
    assert_eq!(report_count(db.pool()).await, 4);
    let seller_state: String = sqlx::query_scalar("SELECT state FROM users WHERE id = $1")
        .bind(seller)
        .fetch_one(db.pool())
        .await
        .expect("seller reads");
    assert_eq!(seller_state, "active");
    let case_statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM report_cases ORDER BY created_at")
            .fetch_all(db.pool())
            .await
            .expect("case standings read");
    assert!(case_statuses.iter().all(|status| status == "open"));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn withdrawal_preserves_evidence_and_restrictions() {
    let db = TestDatabase::create("p10t05_withdrawn")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t05_withdrawn_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-0604", "Withdraw Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0604").await;
    provision_active(&app, "+55 11 90000-0605", "Withdraw Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-0605").await;
    let seller: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Withdraw Seller'")
            .fetch_one(db.pool())
            .await
            .expect("seller reads");
    provision_active(&app, "+55 11 90000-0606", "Withdraw Stranger").await;
    let stranger: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Withdraw Stranger'")
            .fetch_one(db.pool())
            .await
            .expect("stranger reads");
    let (_, offer) = live_demand(&app, &buyer_cookie, &seller_cookie).await;
    let (status, filed) =
        file_offer_report(&app, &offer, "spam", "withdrawn noise", &buyer_cookie).await;
    assert_eq!(status, StatusCode::CREATED);
    let report_id: uuid::Uuid = filed["id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses");
    let case_id: uuid::Uuid = filed["case_id"]
        .as_str()
        .expect("receipt carries case")
        .parse()
        .expect("receipt case parses");

    // A live pair block stands before withdrawal: withdrawing must not
    // clear it.
    let blocked = block_user(db.pool(), buyer, seller)
        .await
        .expect("block records");
    assert!(blocked.created);

    // The filing reporter withdraws: evidence stays, review continues, the
    // restriction stands, and a repeat withdrawal converges idempotently.
    let withdrawn = withdraw_report(db.pool(), buyer, report_id)
        .await
        .expect("own withdrawal records");
    assert_eq!(withdrawn.status, "withdrawn");
    assert_eq!(withdrawn.detail, "withdrawn noise");
    assert_eq!(withdrawn.target_title_snapshot, "Frost-free 300L");
    let reread: (String, String, String) =
        sqlx::query_as("SELECT status, detail, target_title_snapshot FROM reports WHERE id = $1")
            .bind(report_id)
            .fetch_one(db.pool())
            .await
            .expect("withdrawn report still reads");
    assert_eq!(
        reread,
        (
            "withdrawn".to_owned(),
            "withdrawn noise".to_owned(),
            "Frost-free 300L".to_owned()
        )
    );
    let case_status: String = sqlx::query_scalar("SELECT status FROM report_cases WHERE id = $1")
        .bind(case_id)
        .fetch_one(db.pool())
        .await
        .expect("case still reads");
    assert_eq!(case_status, "open", "review continues independently");
    let block_stands: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
            .bind(buyer)
            .bind(seller)
            .fetch_optional(db.pool())
            .await
            .expect("block reads");
    assert!(block_stands.is_some(), "withdrawal clears no restriction");
    let again = withdraw_report(db.pool(), buyer, report_id)
        .await
        .expect("repeat withdrawal converges");
    assert_eq!(again, withdrawn);
    assert_eq!(report_count(db.pool()).await, 1);

    // Foreign hands cannot withdraw: the stranger's attempt refuses with
    // the row untouched.
    assert_eq!(
        withdraw_report(db.pool(), stranger, report_id).await,
        Err(GroupError::NotPermitted)
    );
    assert_eq!(
        withdraw_report(db.pool(), buyer, uuid::Uuid::now_v7()).await,
        Err(GroupError::NotFound)
    );
    db.cleanup().await.expect("suite cleans up");
}
