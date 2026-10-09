//! Reputation acceptance (P14-T02): labeled evidence only.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real public reputation HTTP route with real
//! sessions where provisioning needs them, with fixtures through the
//! real operations. Proves:
//! - contact initiation alone claims no verified sale and labels
//!   nothing beyond factual age and activity;
//! - negative unreviewed feedback publishes no complaint badge of any
//!   kind;
//! - profile responses exclude phones, exact activity history, reporter
//!   identities, and professional classification material.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::feedback::{submit_feedback, FeedbackInput};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::professional_profile::{declare_profile, NewProfessional};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::profiles::{routes as profile_routes, ProfilesState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::PhoneKeys;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p14t02-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p14t02-test-only-encryption-key";

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
    .merge(profile_routes(ProfilesState::new(db.pool().clone())))
}

/// Test-only phone keys. Never production material.
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

/// One live demand with one contacted offer through the real operations.
async fn live_contact(
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
    start_contact(
        pool,
        &test_keys(),
        author,
        demand,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts");
    (demand, offer)
}

/// Profile bodies hold exactly the modest allowlist: user id, age days,
/// recent-activity flag, and the resolutions label only when supported.
fn assert_modest_shape(body: &Value) {
    let object = body.as_object().expect("profile object reads");
    for key in object.keys() {
        assert!(
            [
                "user_id",
                "account_age_days",
                "active_recently",
                "resolutions_reported"
            ]
            .contains(&key.as_str()),
            "no {key} beyond the modest projection"
        );
    }
    let rendered = body.to_string();
    for absent in [
        "90000",
        "55119",
        "verified",
        "sales",
        "sale",
        "trust",
        "score",
        "rating",
        "stars",
        "badge",
        "complaint",
        "negative",
        "dispute",
        "phone",
        "reporter",
        "detail",
        "token",
        "secret",
        "created_at",
        "occurred_at",
        "shop",
        "business",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in profile output");
    }
}

#[tokio::test]
async fn contact_alone_claims_no_verified_sale() {
    let db = TestDatabase::create("p14t02_nosale")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t02_nosale_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2801", "Nosale Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-2802", "Nosale Seller").await;
    live_contact(db.pool(), buyer, seller).await;

    // Contact initiation alone labels nothing: no resolutions key, no
    // verified-sale vocabulary, and only the exact modest keys.
    for account in [buyer, seller] {
        let (status, profile) = call(
            app.clone(),
            "GET",
            &format!("/api/v1/profiles/{account}/reputation"),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(profile["user_id"], account.to_string());
        assert!(profile.get("resolutions_reported").is_none());
        assert!(profile["account_age_days"].as_i64().unwrap_or(-1) >= 0);
        assert_modest_shape(&profile);
    }
    let (status, missing) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/profiles/{}/reputation", uuid::Uuid::now_v7()),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["code"], "not_found");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn negative_unreviewed_feedback_publishes_no_badge() {
    let db = TestDatabase::create("p14t02_nobadge")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t02_nobadge_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2803", "Nobadge Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-2804", "Nobadge Seller").await;
    let (_, offer) = live_contact(db.pool(), buyer, seller).await;
    let contact: uuid::Uuid = sqlx::query_scalar("SELECT id FROM contacts WHERE offer_id = $1")
        .bind(offer)
        .fetch_one(db.pool())
        .await
        .expect("contact reads");
    sqlx::query("UPDATE contacts SET initiated_at = $2 WHERE id = $1")
        .bind(contact)
        .bind(chrono::Utc::now() - chrono::Duration::days(2))
        .execute(db.pool())
        .await
        .expect("synthetic contact age applies");
    submit_feedback(
        db.pool(),
        buyer,
        FeedbackInput {
            contact_id: contact,
            answer: "no".to_owned(),
        },
    )
    .await
    .expect("negative feedback files");

    // An unreviewed negative answer changes nothing public: still no
    // resolutions key, no complaint vocabulary, exact keys only.
    let (status, profile) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/profiles/{seller}/reputation"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(profile.get("resolutions_reported").is_none());
    assert_modest_shape(&profile);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn profile_excludes_private_history() {
    let db = TestDatabase::create("p14t02_private")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t02_private_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-2805", "Private Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-2806", "Private Seller").await;
    declare_profile(
        db.pool(),
        seller,
        NewProfessional {
            business_name: "Private Shop".to_owned(),
            business_type: "shop".to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
        },
    )
    .await
    .expect("professional classification declares");
    let (_, offer) = live_contact(db.pool(), buyer, seller).await;
    let contact: uuid::Uuid = sqlx::query_scalar("SELECT id FROM contacts WHERE offer_id = $1")
        .bind(offer)
        .fetch_one(db.pool())
        .await
        .expect("contact reads");
    sqlx::query("UPDATE contacts SET initiated_at = $2 WHERE id = $1")
        .bind(contact)
        .bind(chrono::Utc::now() - chrono::Duration::days(2))
        .execute(db.pool())
        .await
        .expect("synthetic contact age applies");
    submit_feedback(
        db.pool(),
        buyer,
        FeedbackInput {
            contact_id: contact,
            answer: "yes".to_owned(),
        },
    )
    .await
    .expect("positive feedback files");

    // One distinct yes-reporting buyer labels exactly "1" with the basis
    // in the name — and nothing else: no phones, no timestamps, no
    // reporter detail, no professional classification material.
    let (status, profile) = call(
        app.clone(),
        "GET",
        &format!("/api/v1/profiles/{seller}/reputation"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(profile["resolutions_reported"], 1);
    assert_modest_shape(&profile);
    db.cleanup().await.expect("suite cleans up");
}
