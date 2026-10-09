//! Public-projection acceptance (P13-T02): details or limited status.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real public HTTP route with real cookie
//! sessions and the deterministic fake provider, with fixtures through
//! the real operations. Proves:
//! - anonymous visitors understand active demand before registration,
//!   with safe fields only;
//! - suspended and removed links expose no protected content or
//!   allegation, sharing one generic body with missing rows;
//! - terminal links expose standing without interaction, and blocked
//!   signed-in viewers receive the same unavailable body — while
//!   private offers and phones never appear in any public response.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::create_report::ReportInput;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::policy_changes::{change_category, CategoryInput};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::report_updates::submit_grouped_report;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_eligibility::expire_if_elapsed;
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::public_requests::{routes as public_routes, PublicRequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p13t02-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p13t02-test-only-encryption-key";

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
    .merge(public_routes(PublicRequestsState::new(db.pool().clone())))
}

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: LOOKUP_KEY,
        encryption_key: ENCRYPTION_KEY,
    }
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

/// One active account through the real creation path (no session needed).
async fn seed_active(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let user = create_user(
        &mut tx,
        NewUser {
            display_name: display.to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
            policy_version: "v1".to_owned(),
            policy_accepted_at: chrono::Utc::now(),
            phone: phone.to_owned(),
        },
        test_keys(),
    )
    .await
    .expect("fixture account stores");
    tx.commit().await.expect("account commits");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    user.id
}

/// One published demand through the real operations.
async fn publish_demand(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
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
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

/// Public bodies carry business fields and declared names only: no
/// phone, notes, offers, reporter, token, or secret material.
fn assert_public(body: &Value) {
    let rendered = body.to_string();
    for absent in [
        "90000", "55119", "phone", "lookup", "cipher", "token", "notes", "offers", "reporter",
        "detail", "secret", "address",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in public response");
    }
}

#[tokio::test]
async fn anonymous_understands_active_demand() {
    let db = TestDatabase::create("p13t02_open")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t02_open_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2101", "Open Buyer").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;

    // Anonymous visitors read the eligible demand before registration:
    // item, budget, condition, category, locality, display name, and the
    // open offer action — deterministically on repeat.
    let (status, first) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/public/requests/{demand}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["id"], demand.to_string());
    assert_eq!(first["title"], "Refrigerator");
    assert_eq!(first["category_code"], "home_appliances");
    assert_eq!(first["budget_cents"], 60_000);
    assert_eq!(first["condition"], "either");
    assert_eq!(first["city_code"], "campinas");
    assert_eq!(first["region_code"], "centro");
    assert_eq!(first["author_name"], "Open Buyer");
    assert_eq!(first["availability"], "available");
    assert_eq!(first["offer_action"], true);
    assert_public(&first);
    let (status, second) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/public/requests/{demand}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first, second);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_removed_link_exposes_nothing() {
    let db = TestDatabase::create("p13t02_hidden")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t02_hidden_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let operator = seed_active(db.pool(), "Hidden Operator", "+55 11 90000-2102").await;
    bootstrap_grant(
        db.pool(),
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
    let admin = seed_active(db.pool(), "Hidden Admin", "+55 11 90000-2103").await;
    grant_role(
        db.pool(),
        operator,
        GrantInput {
            user_id: admin,
            role: "administrator".to_owned(),
            scope: "policy".to_owned(),
            reason: "policy cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("policy administrator granted");
    let buyer = provision_active(&app, "+55 11 90000-2104", "Hidden Buyer").await;
    let reporter = provision_active(&app, "+55 11 90000-2105", "Hidden Reporter").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    submit_grouped_report(
        db.pool(),
        reporter,
        ReportInput {
            target_kind: "request".to_owned(),
            target_id: demand,
            reason: "spam".to_owned(),
            detail: "canary-reporter-detail-2105".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");

    // Terminal links expose standing without interaction or content.
    let done = publish_demand(db.pool(), buyer, "Done Fridge").await;
    close_request(
        db.pool(),
        buyer,
        done,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("demand completes");
    let gone = publish_demand(db.pool(), buyer, "Gone Fridge").await;
    close_request(db.pool(), buyer, gone, BuyerOutcome::NotNeeded)
        .await
        .expect("demand cancels");
    let old = publish_demand(db.pool(), buyer, "Old Fridge").await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(old)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic deadline passage applies");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    expire_if_elapsed(&mut tx, old, now)
        .await
        .expect("expiry evaluates");
    tx.commit().await.expect("expiry commits");
    for (id, status) in [(done, "completed"), (gone, "cancelled"), (old, "expired")] {
        let (code, limited) = call(
            app.clone(),
            "GET",
            &format!("/api/v1/public/requests/{id}"),
            None,
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(limited["id"], id.to_string());
        assert_eq!(limited["status"], status);
        assert_eq!(limited["availability"], "unavailable");
        assert_eq!(limited["offer_action"], false);
        assert!(limited.get("title").is_none());
        assert!(limited.get("budget_cents").is_none());
        assert_public(&limited);
    }

    // Suspended, removed, prohibited, draft, and missing links share one
    // generic body with no protected content or allegation.
    sqlx::query("UPDATE requests SET state = 'suspended', visibility = 'hidden' WHERE id = $1")
        .bind(demand)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    let private = publish_demand(db.pool(), buyer, "Private Fridge").await;
    sqlx::query("UPDATE requests SET visibility = 'private' WHERE id = $1")
        .bind(private)
        .execute(db.pool())
        .await
        .expect("synthetic removal applies");
    change_category(
        db.pool(),
        admin,
        CategoryInput {
            category_code: "home_appliances".to_owned(),
            new_status: "prohibited".to_owned(),
            reason: "safety review finding".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("prohibition applies");
    let ghost = uuid::Uuid::now_v7();
    let mut generics = Vec::new();
    for id in [demand, private, ghost] {
        let (code, body) = call(
            app.clone(),
            "GET",
            &format!("/api/v1/public/requests/{id}"),
            None,
            None,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        assert_public(&body);
        assert!(!body.to_string().contains("canary-reporter-detail-2105"));
        generics.push(body);
    }
    assert_eq!(generics[0], generics[1]);
    assert_eq!(generics[1], generics[2]);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn blocked_viewer_gets_same_unavailable() {
    let db = TestDatabase::create("p13t02_blocked")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t02_blocked_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2106", "Blocked Buyer").await;
    let viewer = provision_active(&app, "+55 11 90000-2107", "Blocked Viewer").await;
    let viewer_cookie = login_cookie(app.clone(), "+55 11 90000-2107").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    block_user(db.pool(), buyer, viewer)
        .await
        .expect("pair blocks");

    // The signed-in blocked viewer receives the exact unavailable body an
    // anonymous visitor gets for a missing row: no block oracle, no
    // content, no phone — while strangers still read the demand.
    let (status, refused) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/public/requests/{demand}"),
        None,
        Some(&viewer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let ghost = uuid::Uuid::now_v7();
    let (status, missing) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/public/requests/{ghost}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(refused, missing);
    assert_public(&refused);
    let stranger = provision_active(&app, "+55 11 90000-2108", "Blocked Stranger").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-2108").await;
    let (status, visible) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/public/requests/{demand}"),
        None,
        Some(&stranger_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(visible["title"], "Refrigerator");
    assert_public(&visible);
    let _ = stranger;
    db.cleanup().await.expect("suite cleans up");
}
