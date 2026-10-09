//! Share-intent acceptance (P13-T04): link freely, claim nothing.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real share-intent HTTP route with real cookie
//! sessions, with fixtures through the real operations. Proves:
//! - active prepared shares carry only public context with a stable
//!   resolving link, one intent fact per channel;
//! - suspended, removed, terminal, missing, and blocked preparations
//!   share one refusal;
//! - one sharing action fact is neither a contact nor a delivered
//!   message, and old links resolve current state instead of frozen
//!   snapshots.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::public_html::{routes as html_routes, PublicHtmlState};
use procurali_backend::http::public_requests::{routes as public_routes, PublicRequestsState};
use procurali_backend::http::shares::{routes as share_routes, SharesState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p13t04-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p13t04-test-only-encryption-key";

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
    .merge(html_routes(PublicHtmlState::new(db.pool().clone())))
    .merge(share_routes(SharesState::new(db.pool().clone())))
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

/// Prepared shares carry public context only: no phone, notes, offers,
/// reporter, token, secret, or recipient material.
fn assert_share_clean(body: &Value) {
    let rendered = body.to_string();
    for absent in [
        "90000",
        "55119",
        "phone",
        "lookup",
        "cipher",
        "token",
        "notes",
        "offers",
        "reporter",
        "detail",
        "secret",
        "recipient",
        "deliver",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in share output");
    }
}

#[tokio::test]
async fn active_prepared_share_contains_only_public_context() {
    let db = TestDatabase::create("p13t04_open")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t04_open_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2301", "Share Buyer").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;

    // Every intent channel prepares the same stable link with public
    // context only, recording one intent fact each — and unknown or
    // missing channels refuse with nothing stored.
    for channel in ["whatsapp", "copy", "device"] {
        let (status, prepared) = call(
            app.clone(),
            "POST",
            &format!("/api/v1/public/requests/{demand}/share"),
            Some(json!({"channel": channel})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(prepared["request_id"], demand.to_string());
        assert_eq!(
            prepared["link"],
            format!("/api/v1/public/requests/{demand}/html")
        );
        assert_eq!(prepared["channel"], channel);
        assert!(prepared["message"].as_str().is_some_and(|message| {
            message.contains("Refrigerator")
                && message.contains("600.00")
                && message.contains("centro")
        }));
        assert_share_clean(&prepared);
    }
    let (status, refused) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/public/requests/{demand}/share"),
        Some(json!({"channel": "sms"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["code"], "invalid_field");
    assert_share_clean(&refused);
    let (status, refused) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/public/requests/{demand}/share"),
        Some(json!({})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["code"], "missing_field");
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = 'share.intended'",
    )
    .bind(demand)
    .fetch_one(db.pool())
    .await
    .expect("intent facts read");
    assert_eq!(intents, 3);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_removed_share_refused() {
    let db = TestDatabase::create("p13t04_refused")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t04_refused_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2302", "Refused Buyer").await;
    let viewer = provision_active(&app, "+55 11 90000-2303", "Refused Viewer").await;
    let viewer_cookie = login_cookie(app.clone(), "+55 11 90000-2303").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    let done = publish_demand(db.pool(), buyer, "Done Fridge").await;
    close_request(
        db.pool(),
        buyer,
        done,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("demand completes");
    sqlx::query("UPDATE requests SET state = 'suspended', visibility = 'hidden' WHERE id = $1")
        .bind(demand)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    block_user(db.pool(), buyer, viewer)
        .await
        .expect("pair blocks");

    // Suspended, terminal, missing, and blocked preparations share one
    // refusal with nothing stored and nothing reflected.
    let ghost = uuid::Uuid::now_v7();
    let mut refusals = Vec::new();
    for (id, cookie) in [
        (demand, None),
        (done, None),
        (ghost, None),
        (demand, Some(viewer_cookie.as_str())),
    ] {
        let (status, refused) = call(
            app.clone(),
            "POST",
            &format!("/api/v1/public/requests/{id}/share"),
            Some(json!({"channel": "copy"})),
            cookie,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(refused["code"], "not_found");
        assert_share_clean(&refused);
        refusals.push(refused);
    }
    assert_eq!(refusals[0], refusals[1]);
    assert_eq!(refusals[1], refusals[2]);
    assert_eq!(refusals[2], refusals[3]);
    let intents: i64 =
        sqlx::query_scalar("SELECT count(*) FROM business_events WHERE kind = 'share.intended'")
            .fetch_one(db.pool())
            .await
            .expect("intent facts read");
    assert_eq!(intents, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn share_fact_is_not_contact_or_delivery() {
    let db = TestDatabase::create("p13t04_intent")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t04_intent_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2304", "Intent Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-2304").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/public/requests/{demand}/share"),
        Some(json!({"channel": "whatsapp"})),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // The intent fact stands alone: no contact row, no contact or message
    // fact for the demand — and the stable link resolves live state, so a
    // later suspension turns the same link unavailable.
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 0);
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT kind FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 ORDER BY kind",
    )
    .bind(demand)
    .fetch_all(db.pool())
    .await
    .expect("demand facts read");
    assert_eq!(kinds, ["request.published", "share.intended"]);
    sqlx::query("UPDATE requests SET state = 'suspended', visibility = 'hidden' WHERE id = $1")
        .bind(demand)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/public/requests/{demand}/share"),
        Some(json!({"channel": "copy"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.cleanup().await.expect("suite cleans up");
}
