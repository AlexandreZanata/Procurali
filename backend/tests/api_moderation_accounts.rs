//! Account-suspension acceptance (P11-T04): restrict all, keep safety open.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Staff suspension and restoration travel the real audited
//! operations (no staff HTTP surface on earlier cards); refusal proofs
//! drive the real marketplace HTTP routes with real cookie sessions and
//! the deterministic fake provider. Proves:
//! - an old session cannot contact or publish after suspension, while the
//!   suspended account keeps its own-history report path;
//! - a suspended buyer completes their own request with content staying
//!   hidden and no fictional contact;
//! - restoring after expiry revives no offer: expired rows, visibility,
//!   and deadlines stand byte-identical with refusals intact.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::create_report::{submit_report, ReportInput};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_eligibility::expire_if_elapsed;
use procurali_backend::application::restore_user::{restore_user, RestoreUserInput};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::suspend_user::{suspend_user, SuspendUserInput};
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
const LOOKUP_KEY: &str = "p11t04-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p11t04-test-only-encryption-key";

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

/// One moderator behind the real audited grant chain; return its id.
async fn seed_moderator(
    app: &axum::Router,
    pool: &sqlx::PgPool,
    phone: &str,
    name: &str,
) -> uuid::Uuid {
    let operator = provision_active(app, "+55 11 90000-1001", "Pause Operator").await;
    bootstrap_grant(
        pool,
        BootstrapInput {
            user_id: operator,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "launch cover".to_owned(),
            operator_label: "launch-operator-1".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("launch bootstrap grants");
    let moderator = provision_active(app, phone, name).await;
    grant_role(
        pool,
        operator,
        GrantInput {
            user_id: moderator,
            role: "moderator".to_owned(),
            scope: "safety".to_owned(),
            reason: "triage cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("moderator granted");
    moderator
}

/// One live demand with one offer through the real operations; return ids.
async fn live_offer(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
) -> (uuid::Uuid, uuid::Uuid) {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some("Refrigerator".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("600.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("fixture draft validates");
    let demand = publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id;
    let offer = submit_offer(
        pool,
        seller,
        demand,
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(1),
            description: Some("Frost-free 300L".to_owned()),
            price: Some("520.00".to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
            available: Some(true),
            available_in_city: Some(true),
        },
    )
    .await
    .expect("fixture submission submits")
    .id;
    (demand, offer)
}

async fn suspend_account(pool: &sqlx::PgPool, moderator: uuid::Uuid, target: uuid::Uuid) {
    let suspended = suspend_user(
        pool,
        moderator,
        SuspendUserInput {
            user_id: target,
            reason: "repeated low-severity abuse after warning".to_owned(),
            duration_hours: 24,
            policy_version: "v1".to_owned(),
            purpose: "pause pending review".to_owned(),
            case_id: None,
        },
    )
    .await
    .expect("suspension suspends");
    assert!(suspended.transitioned);
    assert_eq!(suspended.state, "suspended");
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
async fn old_session_cannot_contact_or_publish_after_suspension() {
    let db = TestDatabase::create("p11t04_session")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t04_session_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let moderator = seed_moderator(&app, db.pool(), "+55 11 90000-1002", "Pause Moderator").await;
    let buyer = provision_active(&app, "+55 11 90000-1003", "Pause Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-1003").await;
    let seller = provision_active(&app, "+55 11 90000-1004", "Pause Seller").await;
    let (demand, offer) = live_offer(db.pool(), buyer, seller).await;
    suspend_account(db.pool(), moderator, buyer).await;

    // The pre-suspension session is dead for marketplace writes: contact
    // and publication share one unauthenticated refusal with nothing
    // disclosed. Sessions read current rows, not login history.
    let (status, refused) = call(
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
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused["code"], "unauthenticated");
    assert_no_destination(&refused);
    let (status, refused) = call(
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
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused["code"], "unauthenticated");

    // The allowed safety path stays open: the suspended buyer still files
    // own-history reports, and the suspension hid exactly one demand.
    submit_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer,
            reason: "spam".to_owned(),
            detail: "history report while suspended".to_owned(),
        },
    )
    .await
    .expect("own-history report files while suspended");
    let visibility: String = sqlx::query_scalar("SELECT visibility FROM requests WHERE id = $1")
        .bind(demand)
        .fetch_one(db.pool())
        .await
        .expect("demand reads");
    assert_eq!(visibility, "hidden");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_buyer_completes_own_request_hidden() {
    let db = TestDatabase::create("p11t04_complete")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t04_complete_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let moderator =
        seed_moderator(&app, db.pool(), "+55 11 90000-1005", "Complete Moderator").await;
    let buyer = provision_active(&app, "+55 11 90000-1006", "Complete Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-1007", "Complete Seller").await;
    let (demand, _) = live_offer(db.pool(), buyer, seller).await;
    suspend_account(db.pool(), moderator, buyer).await;

    // EC-20 through the real closure path: the suspended buyer completes
    // their own demand, visibility stays hidden, the buyer stays
    // suspended, and no fictional contact appears anywhere.
    let closed = close_request(
        db.pool(),
        buyer,
        demand,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("suspended buyer completes own request");
    assert_eq!(closed.state, "completed");
    let standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("demand re-reads");
    assert_eq!(standing, ("completed".to_owned(), "hidden".to_owned()));
    let buyer_state: String = sqlx::query_scalar("SELECT state FROM users WHERE id = $1")
        .bind(buyer)
        .fetch_one(db.pool())
        .await
        .expect("buyer reads");
    assert_eq!(buyer_state, "suspended");
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 0);
    let facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1",
    )
    .bind(demand)
    .fetch_one(db.pool())
    .await
    .expect("demand facts read");
    assert_eq!(facts, 2, "publication plus completion only");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn restore_after_expiry_never_revives_offer() {
    let db = TestDatabase::create("p11t04_restore")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t04_restore_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let moderator = seed_moderator(&app, db.pool(), "+55 11 90000-1008", "Restore Moderator").await;
    let buyer = provision_active(&app, "+55 11 90000-1009", "Restore Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-1010", "Restore Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-1010").await;
    let (demand, offer) = live_offer(db.pool(), buyer, seller).await;
    suspend_account(db.pool(), moderator, buyer).await;

    // The original deadline passes under suspension: expiry moves state
    // only, keeping hidden visibility.
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(demand)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic deadline passage applies");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    expire_if_elapsed(&mut tx, demand, now)
        .await
        .expect("expiry evaluates");
    tx.commit().await.expect("expiry commits");
    let before_offer: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM offers WHERE id = $1")
            .bind(offer)
            .fetch_one(db.pool())
            .await
            .expect("offer reads");

    // The suspension deadline passes too (synthetic clock fixture on the
    // fact payload), so restoration records a timed end — and still
    // revives nothing but the account row.
    sqlx::query(
        "UPDATE business_events SET payload = payload || jsonb_build_object('ends_at', $2)
         WHERE resource_kind = 'user' AND resource_id = $1 AND kind = 'user.suspended'",
    )
    .bind(buyer)
    .bind((now - chrono::Duration::hours(1)).to_rfc3339())
    .execute(db.pool())
    .await
    .expect("synthetic deadline passage applies");
    let restored = restore_user(
        db.pool(),
        moderator,
        RestoreUserInput {
            user_id: buyer,
            reason: "restriction deadline reached".to_owned(),
            policy_version: "v1".to_owned(),
            purpose: "timed restriction end".to_owned(),
        },
    )
    .await
    .expect("timed restoration restores");
    assert!(restored.restored);
    assert!(restored.timed);
    let buyer_state: String = sqlx::query_scalar("SELECT state FROM users WHERE id = $1")
        .bind(buyer)
        .fetch_one(db.pool())
        .await
        .expect("buyer reads");
    assert_eq!(buyer_state, "active");

    // Expired rows, visibility, and deadlines stand byte-identical; the
    // old offer stays unusable through the live routes.
    let standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("demand re-reads");
    assert_eq!(standing, ("expired".to_owned(), "hidden".to_owned()));
    let after_offer: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM offers WHERE id = $1")
            .bind(offer)
            .fetch_one(db.pool())
            .await
            .expect("offer re-reads");
    assert_eq!(before_offer, after_offer, "restoration revived no offer");
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-1009").await;
    let (status, refused) = call(
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
    assert_no_destination(&refused);
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(json!({
            "revision_number": 1, "cycle_number": 1,
            "description": "Late unit", "price": "500.00",
            "condition": "used", "city_code": "campinas",
            "region_code": "centro", "available": true,
            "available_in_city": true,
        })),
        Some(&seller_cookie),
    )
    .await;
    assert!(status.is_client_error());
    db.cleanup().await.expect("suite cleans up");
}
