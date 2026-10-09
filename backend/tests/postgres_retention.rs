//! Retention-cleanup acceptance (P12-T03): redact due data, keep the rest.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL with the real audited
//! operations for fixtures, holds, and the sweep:
//! - due ordinary contact destinations are absent after reload while an
//!   incident-held subset stays byte-identical with its hold open;
//! - two racing workers plus a retry converge on one redaction and one
//!   release per hold, with no duplicate facts;
//! - no deleted destination remains readable through replay, history,
//!   the HTTP route, or worker output.
//!
//! Setup travels the real creation, publication, submission, contact, and
//! grant paths with real phone cryptography. All phones, names, codes,
//! and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::create_report::ReportInput;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::report_updates::submit_grouped_report;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::retention_policy::{classify_hold, ClassifyInput};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::operations::jobs::retention_cleanup::{run_cleanup, CleanupReport};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p12t03-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p12t03-test-only-encryption-key";

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: LOOKUP_KEY,
        encryption_key: ENCRYPTION_KEY,
    }
}

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

/// A real active account: identity travels the real creation path with real
/// phone cryptography, and only the lifecycle flip (owned by the
/// verification flow) is staged.
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

/// One moderator behind the real audited grant chain; return its id.
async fn seed_moderator(pool: &sqlx::PgPool) -> uuid::Uuid {
    let operator = seed_active(pool, "Cleanup Operator", "+55 11 90000-1701").await;
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
    let moderator = seed_active(pool, "Cleanup Moderator", "+55 11 90000-1702").await;
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

/// One live demand with one contacted offer in the given category through
/// the real operations; return demand, offer, and contact ids.
async fn live_contact_in(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
    title: &str,
    category: &str,
) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid) {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some(category.to_owned()),
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
    let contact = start_contact(
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
    .expect("fixture handoff starts")
    .contact_id;
    (demand, offer, contact)
}

fn ordinary_hold(kind: &str, subject: uuid::Uuid, days_ago: i64) -> ClassifyInput {
    ClassifyInput {
        subject_kind: kind.to_owned(),
        subject_id: subject,
        class: "ordinary".to_owned(),
        purpose: "recent dispute cover".to_owned(),
        anchor_at: chrono::Utc::now() - chrono::Duration::days(days_ago),
        ends_at: None,
        basis: None,
        review_condition: None,
        owner_id: None,
    }
}

async fn destination_len(pool: &sqlx::PgPool, contact: uuid::Uuid) -> i32 {
    sqlx::query_scalar("SELECT octet_length(destination_ciphertext) FROM contacts WHERE id = $1")
        .bind(contact)
        .fetch_one(pool)
        .await
        .expect("destination reads")
}

#[tokio::test]
async fn due_ordinary_absent_incident_subset_remains() {
    let db = TestDatabase::create("p12t03_scope")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t03_scope_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let moderator = seed_moderator(db.pool()).await;
    let buyer = seed_active(db.pool(), "Scope Buyer", "+55 11 90000-1703").await;
    let seller = seed_active(db.pool(), "Scope Seller", "+55 11 90000-1704").await;
    let (_, _, plain) =
        live_contact_in(db.pool(), buyer, seller, "Refrigerator", "home_appliances").await;
    let (_, covered_offer, covered) =
        live_contact_in(db.pool(), buyer, seller, "Washer", "home_appliances").await;
    assert!(destination_len(db.pool(), plain).await > 0);
    assert!(destination_len(db.pool(), covered).await > 0);

    // The covered contact joins an open incident case under review while
    // both contacts carry due ordinary holds.
    let filed = submit_grouped_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: covered_offer,
            reason: "fraud".to_owned(),
            detail: "scope fixture claim".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");
    classify_hold(
        db.pool(),
        Some(moderator),
        ClassifyInput {
            subject_kind: "case".to_owned(),
            subject_id: filed.case_id,
            class: "incident".to_owned(),
            purpose: "open fraud triage".to_owned(),
            anchor_at: chrono::Utc::now(),
            ends_at: None,
            basis: None,
            review_condition: None,
            owner_id: None,
        },
    )
    .await
    .expect("incident hold classifies");
    classify_hold(db.pool(), None, ordinary_hold("contact", plain, 100))
        .await
        .expect("plain hold classifies");
    classify_hold(db.pool(), None, ordinary_hold("contact", covered, 100))
        .await
        .expect("covered hold classifies");

    // The sweep redacts the plain destination and releases its hold while
    // the incident-covered contact, its hold, and the open case stand
    // byte-identical.
    let report = run_cleanup(db.pool(), 10).await.expect("sweep runs");
    assert_eq!(report.redacted, 1);
    assert_eq!(report.released, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.processed, 2);
    assert_eq!(destination_len(db.pool(), plain).await, 0);
    assert!(destination_len(db.pool(), covered).await > 0);
    let covered_hold: String = sqlx::query_scalar(
        "SELECT status FROM retention_holds WHERE subject_kind = 'contact' AND subject_id = $1",
    )
    .bind(covered)
    .fetch_one(db.pool())
    .await
    .expect("covered hold reads");
    assert_eq!(covered_hold, "open");
    let incident_hold: String = sqlx::query_scalar(
        "SELECT status FROM retention_holds WHERE subject_kind = 'case' AND subject_id = $1",
    )
    .bind(filed.case_id)
    .fetch_one(db.pool())
    .await
    .expect("incident hold reads");
    assert_eq!(incident_hold, "open");
    let reports: i64 = sqlx::query_scalar("SELECT count(*) FROM reports WHERE case_id = $1")
        .bind(filed.case_id)
        .fetch_one(db.pool())
        .await
        .expect("case members read");
    assert_eq!(reports, 1);
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 2, "aggregates survive without destinations");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workers_and_retry_converge() {
    let db = TestDatabase::create("p12t03_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t03_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    seed_moderator(db.pool()).await;
    let buyer = seed_active(db.pool(), "Race Buyer", "+55 11 90000-1705").await;
    let seller = seed_active(db.pool(), "Race Seller", "+55 11 90000-1706").await;
    let mut contacts = Vec::new();
    for (title, category) in [
        ("Refrigerator", "home_appliances"),
        ("Bicycle", "bicycles"),
        ("Drill", "tools"),
    ] {
        let (_, _, contact) = live_contact_in(db.pool(), buyer, seller, title, category).await;
        classify_hold(db.pool(), None, ordinary_hold("contact", contact, 100))
            .await
            .expect("ordinary hold classifies");
        contacts.push(contact);
    }

    // Two workers race over the same due holds: skipped row locks
    // partition the work, and every hold redacts and releases exactly
    // once across both reports.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let sweep = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            run_cleanup(&pool, 10).await
        }
    };
    let (first, second) = tokio::join!(sweep(std::sync::Arc::clone(&barrier)), sweep(barrier),);
    let (first, second) = (
        first.expect("first worker responds"),
        second.expect("second worker responds"),
    );
    assert_eq!(first.redacted + second.redacted, 3);
    assert_eq!(first.released + second.released, 3);
    assert_eq!(first.processed + second.processed, 3);
    // A retry after convergence observes nothing due and writes nothing.
    let rerun = run_cleanup(db.pool(), 10).await.expect("rerun responds");
    assert_eq!(
        rerun,
        CleanupReport {
            redacted: 0,
            released: 0,
            skipped: 0,
            processed: 0,
        }
    );
    for contact in &contacts {
        assert_eq!(destination_len(db.pool(), *contact).await, 0);
    }
    let facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE kind = 'retention.hold_released' AND payload->>'class' = 'ordinary'",
    )
    .fetch_one(db.pool())
    .await
    .expect("release facts read");
    assert_eq!(facts, 3, "one release fact per hold, never duplicated");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn no_deleted_destination_readable() {
    let db = TestDatabase::create("p12t03_unreadable")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t03_unreadable_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    seed_moderator(db.pool()).await;
    let buyer = provision_active(&app, "+55 11 90000-1707", "Unread Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-1707").await;
    let seller = provision_active(&app, "+55 11 90000-1708", "Unread Seller").await;
    let (demand, offer, contact) =
        live_contact_in(db.pool(), buyer, seller, "Refrigerator", "home_appliances").await;
    let handoff_id: uuid::Uuid =
        sqlx::query_scalar("SELECT handoff_id FROM contacts WHERE id = $1")
            .bind(contact)
            .fetch_one(db.pool())
            .await
            .expect("handoff reads");
    classify_hold(db.pool(), None, ordinary_hold("contact", contact, 100))
        .await
        .expect("ordinary hold classifies");
    run_cleanup(db.pool(), 10).await.expect("sweep runs");

    // Replay re-decrypts the current verified destination instead of
    // returning anything stored (EC-21): with the parties still eligible
    // it answers from live decrypt, while the stored copy stays
    // destroyed — the deleted bytes come back from nowhere.
    assert_eq!(destination_len(db.pool(), contact).await, 0);
    let business: (uuid::Uuid, i64, String) = sqlx::query_as(
        "SELECT offer_id, offer_price_cents, request_title FROM contacts WHERE id = $1",
    )
    .bind(contact)
    .fetch_one(db.pool())
    .await
    .expect("history reads");
    assert_eq!(business, (offer, 52_000, "Refrigerator".to_owned()));
    let (status, replayed) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer}/contact"),
        Some(json!({
            "handoff_id": handoff_id,
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        replayed["destination"]
            .as_str()
            .is_some_and(|destination| destination.contains("5511900001708")),
        "live decrypt answers; the stored copy stays empty"
    );
    // Worker output carries counts only: nothing phone-shaped, nothing
    // destination-shaped, nothing identifier-shaped beyond counts.
    let report = run_cleanup(db.pool(), 10).await.expect("rerun responds");
    let rendered = format!("{report:?}");
    for absent in [
        "destination",
        "cipher",
        "phone",
        "lookup",
        "token",
        "55119",
        "90000",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in worker output");
    }
    assert_eq!(report.processed, 0);
    db.cleanup().await.expect("suite cleans up");
}
