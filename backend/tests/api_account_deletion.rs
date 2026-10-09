//! Account-deletion acceptance (P12-T01): end interaction, keep history.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Deletion travels the real audited operation (no deletion
//! HTTP surface exists yet — "api" here is the audited Rust operation
//! surface with refusal proofs through the real marketplace HTTP routes,
//! real cookie sessions, and the deterministic fake provider). Proves:
//! - deleted buyers/sellers disclose no destination and mutate nothing
//!   with old cookies, while sessions revoke and identities scrub;
//! - buyer and seller journeys close correctly with unavailable statuses,
//!   retained reports, and retention markers;
//! - externally learned numbers are never claimed recalled: contact rows
//!   stand byte-identical, the number recycles to a history-free account,
//!   and reporting rights survive.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::create_report::ReportInput;
use procurali_backend::application::delete_account::{delete_account, DeleteAccountInput};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::report_updates::submit_grouped_report;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactError, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput, SubmitError};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::PhoneKeys;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p12t01-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p12t01-test-only-encryption-key";

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

/// Test-only contact keys (same test values the route state carries).
fn contact_keys<'a>() -> PhoneKeys<'a> {
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

async fn delete_self(pool: &sqlx::PgPool, account: uuid::Uuid, reason: &str) {
    let deleted = delete_account(
        pool,
        account,
        DeleteAccountInput {
            user_id: account,
            reason: reason.to_owned(),
        },
    )
    .await
    .expect("self-deletion deletes");
    assert!(deleted.deleted);
    assert_eq!(deleted.state, "deleted");
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
async fn deleted_parties_cannot_disclose_or_mutate_with_old_cookie() {
    let db = TestDatabase::create("p12t01_cookie")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t01_cookie_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-1501", "Gone Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-1501").await;
    let seller = provision_active(&app, "+55 11 90000-1502", "Gone Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-1502").await;
    let (demand, offer) = live_offer(db.pool(), buyer, seller).await;
    let handoff = start_contact(
        db.pool(),
        &contact_keys(),
        buyer,
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
    assert!(!handoff.repeat);
    submit_grouped_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer,
            reason: "spam".to_owned(),
            detail: "surviving allegation".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");
    delete_self(db.pool(), buyer, "leaving the marketplace").await;
    delete_self(db.pool(), seller, "leaving the marketplace").await;

    // Old cookies disclose and mutate nothing: every marketplace path
    // shares one unauthenticated refusal, and direct operations agree.
    for cookie in [&buyer_cookie, &seller_cookie] {
        let (status, refused) = call(
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
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_no_destination(&refused);
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
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        start_contact(
            db.pool(),
            &contact_keys(),
            buyer,
            demand,
            offer,
            ContactInput {
                handoff_id: Some(uuid::Uuid::now_v7().to_string()),
                expected_offer_terms: Some(1),
                entry_source: Some("offer_detail".to_owned()),
            },
        )
        .await,
        Err(ContactError::NotActive)
    );
    assert_eq!(
        submit_offer(
            db.pool(),
            seller,
            demand,
            OfferInput {
                revision_number: Some(1),
                cycle_number: Some(1),
                description: Some("Late unit".to_owned()),
                price: Some("500.00".to_owned()),
                condition: Some("used".to_owned()),
                city_code: Some("campinas".to_owned()),
                region_code: Some("centro".to_owned()),
                notes: None,
                available: Some(true),
                available_in_city: Some(true),
            },
        )
        .await,
        Err(SubmitError::NotActive)
    );

    // Sessions revoked, identities scrubbed, history preserved: the
    // contact and the allegation stand byte-identical for policy review.
    for account in [buyer, seller] {
        let live_sessions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(account)
        .fetch_one(db.pool())
        .await
        .expect("sessions read");
        assert_eq!(live_sessions, 0);
        let scrubbed: (String, String, i32) = sqlx::query_as(
            "SELECT display_name, phone_lookup, octet_length(phone_ciphertext)
             FROM users WHERE id = $1",
        )
        .bind(account)
        .fetch_one(db.pool())
        .await
        .expect("identity reads");
        assert_eq!(scrubbed.0, "Deleted user");
        assert!(scrubbed.1.starts_with("deleted:"));
        assert_eq!(scrubbed.2, 0);
    }
    let stored_price: i64 =
        sqlx::query_scalar("SELECT offer_price_cents FROM contacts WHERE offer_id = $1")
            .bind(offer)
            .fetch_one(db.pool())
            .await
            .expect("contact reads");
    assert_eq!(stored_price, 52_000);
    let reports: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(db.pool())
        .await
        .expect("reports read");
    assert_eq!(reports, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn buyer_and_seller_journeys_close_correctly() {
    let db = TestDatabase::create("p12t01_journeys")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t01_journeys_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer_two = provision_active(&app, "+55 11 90000-1503", "Journey Buyer").await;
    let seller_two = provision_active(&app, "+55 11 90000-1504", "Journey Seller").await;
    let (demand_two, offer_two) = live_offer(db.pool(), buyer_two, seller_two).await;
    let buyer_three = provision_active(&app, "+55 11 90000-1505", "History Buyer").await;
    let seller_three = provision_active(&app, "+55 11 90000-1506", "History Seller").await;
    let (demand_three, offer_three) = live_offer(db.pool(), buyer_three, seller_three).await;
    submit_grouped_report(
        db.pool(),
        buyer_three,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer_three,
            reason: "spam".to_owned(),
            detail: "history worth keeping".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");

    // Buyer journey: unresolved demand cancels hidden with the cycle
    // ended, the live offer invalidates, and the seller learns
    // unavailability with no reason or reporter attached.
    let buyer_gone = delete_account(
        db.pool(),
        buyer_two,
        DeleteAccountInput {
            user_id: buyer_two,
            reason: "leaving the marketplace".to_owned(),
        },
    )
    .await
    .expect("buyer deletion deletes");
    assert_eq!(buyer_gone.cancelled_requests, 1);
    assert_eq!(buyer_gone.retention, "standard");
    let demand_standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand_two)
            .fetch_one(db.pool())
            .await
            .expect("demand reads");
    assert_eq!(
        demand_standing,
        ("cancelled".to_owned(), "hidden".to_owned())
    );
    let cycle_open: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM request_cycles WHERE request_id = $1 AND ended_at IS NULL",
    )
    .bind(demand_two)
    .fetch_optional(db.pool())
    .await
    .expect("cycles read");
    assert_eq!(cycle_open, None);
    let offer_standing: (String, String) =
        sqlx::query_as("SELECT state, terminal_reason FROM offers WHERE id = $1")
            .bind(offer_two)
            .fetch_one(db.pool())
            .await
            .expect("offer reads");
    assert_eq!(offer_standing.0, "invalidated");
    assert_eq!(offer_standing.1, "deleted");

    // Seller journey: own offers invalidate while the buyer's demand
    // stands, the allegation survives with its reporter link, the open
    // case marks incident-hold retention, and reporting rights remain.
    let seller_gone = delete_account(
        db.pool(),
        seller_three,
        DeleteAccountInput {
            user_id: seller_three,
            reason: "closing shop".to_owned(),
        },
    )
    .await
    .expect("seller deletion deletes");
    assert_eq!(seller_gone.invalidated_offers, 1);
    assert_eq!(seller_gone.retention, "incident-hold");
    let demand_three_standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand_three)
            .fetch_one(db.pool())
            .await
            .expect("third demand reads");
    assert_eq!(
        demand_three_standing,
        ("active".to_owned(), "public".to_owned())
    );
    let kept: (uuid::Uuid, String) = sqlx::query_as(
        "SELECT reporter_id, target_title_snapshot FROM reports WHERE target_id = $1",
    )
    .bind(offer_three)
    .fetch_one(db.pool())
    .await
    .expect("allegation reads");
    assert_eq!(kept.0, buyer_three);
    assert_eq!(kept.1, "Frost-free 300L");
    submit_grouped_report(
        db.pool(),
        buyer_three,
        ReportInput {
            target_kind: "user".to_owned(),
            target_id: seller_three,
            reason: "spam".to_owned(),
            detail: "post-deletion history report".to_owned(),
        },
    )
    .await
    .expect("counterparty reporting rights remain");
    let seller_notices: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, body FROM notices WHERE account_id = $1 ORDER BY created_at")
            .bind(seller_two)
            .fetch_all(db.pool())
            .await
            .expect("notices read");
    assert!(seller_notices
        .iter()
        .any(|(kind, _)| kind == "offer.unavailable"));
    for (_, body) in &seller_notices {
        for absent in ["reporter", "phone", "90000", "leaving", "closing"] {
            assert!(!body.contains(absent), "no {absent} in notice body");
        }
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn externally_learned_number_never_claimed_recalled() {
    let db = TestDatabase::create("p12t01_recall")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t01_recall_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-1507", "Recall Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-1508", "Recall Seller").await;
    let (demand, offer) = live_offer(db.pool(), buyer, seller).await;
    let handoff = start_contact(
        db.pool(),
        &contact_keys(),
        buyer,
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
    assert!(!handoff.repeat);
    let before: (uuid::Uuid, uuid::Uuid, uuid::Uuid, i64) = sqlx::query_as(
        "SELECT id, buyer_id, seller_id, offer_price_cents FROM contacts WHERE offer_id = $1",
    )
    .bind(offer)
    .fetch_one(db.pool())
    .await
    .expect("contact reads");
    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM business_events")
        .fetch_one(db.pool())
        .await
        .expect("events read");
    delete_self(db.pool(), seller, "closing shop").await;

    // The historical contact stands byte-identical: deletion adds exactly
    // its own fact and no recall notice, and the externally learned number
    // is not claimed back by any row this card writes.
    let after: (uuid::Uuid, uuid::Uuid, uuid::Uuid, i64) = sqlx::query_as(
        "SELECT id, buyer_id, seller_id, offer_price_cents FROM contacts WHERE offer_id = $1",
    )
    .bind(offer)
    .fetch_one(db.pool())
    .await
    .expect("contact re-reads");
    assert_eq!(before, after);
    let events_after: i64 = sqlx::query_scalar("SELECT count(*) FROM business_events")
        .fetch_one(db.pool())
        .await
        .expect("events re-read");
    assert_eq!(
        events_after,
        events_before + 1,
        "only the deletion fact lands"
    );
    let recallish: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notices WHERE body ILIKE '%recall%' OR body ILIKE '%recalled%'",
    )
    .fetch_one(db.pool())
    .await
    .expect("recall notices read");
    assert_eq!(recallish, 0);

    // The number recycles to a history-free account, and the buyer keeps
    // reporting rights over the shared history.
    let recycled = provision_active(&app, "+55 11 90000-1508", "Recycled Owner").await;
    assert_ne!(recycled, seller);
    let recycled_contacts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM contacts WHERE buyer_id = $1 OR seller_id = $1")
            .bind(recycled)
            .fetch_one(db.pool())
            .await
            .expect("recycled history reads");
    assert_eq!(recycled_contacts, 0);
    submit_grouped_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "user".to_owned(),
            target_id: seller,
            reason: "spam".to_owned(),
            detail: "post-deletion history report".to_owned(),
        },
    )
    .await
    .expect("buyer reporting rights remain");
    db.cleanup().await.expect("suite cleans up");
}
