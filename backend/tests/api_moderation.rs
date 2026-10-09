//! Moderation-review acceptance (P11-T02): prioritized queues, human decisions.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Staff grants travel the real audited operations (no staff
//! HTTP surface on earlier cards); review intake fixtures travel the real
//! submission operations; the queue, review-opening, and disposition paths
//! under test run through the real moderation HTTP routes with real
//! cookie sessions and the deterministic fake provider. Proves:
//! - the staff queue orders critical/high/normal/low deterministically
//!   with visible sampled targets and no reporter material;
//! - an invalid disposition reduces no target eligibility and writes no
//!   malicious-report finding;
//! - ungranted and anonymous callers can neither read the queue nor move
//!   a case, with refusals auditing nothing.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

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
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::moderation::{routes as moderation_routes, ModerationState};
use procurali_backend::http::reports::{routes as report_routes, ReportsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p11t02-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p11t02-test-only-encryption-key";

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
    .merge(report_routes(ReportsState::new(db.pool().clone())))
    .merge(moderation_routes(ModerationState::new(db.pool().clone())))
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

/// One moderator with a live safety grant plus session; return id + cookie.
async fn seed_moderator(app: &axum::Router, pool: &sqlx::PgPool) -> (uuid::Uuid, String) {
    let operator = provision_active(app, "+55 11 90000-0901", "Queue Operator").await;
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
    let moderator = provision_active(app, "+55 11 90000-0902", "Queue Moderator").await;
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
    let cookie = login_cookie(app.clone(), "+55 11 90000-0902").await;
    (moderator, cookie)
}

async fn open_review(
    app: &axum::Router,
    case_id: uuid::Uuid,
    severity: &str,
    cookie: &str,
) -> Value {
    let (status, opened) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/moderation/cases/{case_id}/review"),
        Some(json!({
            "severity": severity,
            "policy_version": "v1",
            "purpose": "triage for the open intake",
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opened["status"], "review");
    opened
}

/// Queue rows carry sampled targets but no reporter, detail, or secret.
fn assert_queue_private(body: &Value) {
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
        "55119",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in queue output");
    }
}

#[tokio::test]
async fn severity_queue_order_is_deterministic() {
    let db = TestDatabase::create("p11t02_queue")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t02_queue_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let (_, moderator_cookie) = seed_moderator(&app, db.pool()).await;
    let buyer_one = provision_active(&app, "+55 11 90000-0903", "Queue Buyer One").await;
    let buyer_two = provision_active(&app, "+55 11 90000-0904", "Queue Buyer Two").await;
    let seller = provision_active(&app, "+55 11 90000-0905", "Queue Seller").await;
    let (_, offer_one) = live_offer(db.pool(), buyer_one, seller).await;
    let (_, offer_two) = live_offer(db.pool(), buyer_two, seller).await;

    // Four distinct incidents plus one untriaged intake.
    let low = submit_grouped_report(
        db.pool(),
        buyer_one,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer_one,
            reason: "spam".to_owned(),
            detail: "first noise".to_owned(),
        },
    )
    .await
    .expect("low intake files");
    let critical = submit_grouped_report(
        db.pool(),
        buyer_one,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer_one,
            reason: "fraud".to_owned(),
            detail: "stolen goods claim".to_owned(),
        },
    )
    .await
    .expect("critical intake files");
    let normal = submit_grouped_report(
        db.pool(),
        buyer_one,
        ReportInput {
            target_kind: "user".to_owned(),
            target_id: seller,
            reason: "spam".to_owned(),
            detail: "seller noise".to_owned(),
        },
    )
    .await
    .expect("normal intake files");
    let high = submit_grouped_report(
        db.pool(),
        buyer_two,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer_two,
            reason: "fraud".to_owned(),
            detail: "second claim".to_owned(),
        },
    )
    .await
    .expect("high intake files");
    let untriaged = submit_grouped_report(
        db.pool(),
        buyer_two,
        ReportInput {
            target_kind: "user".to_owned(),
            target_id: seller,
            reason: "other".to_owned(),
            detail: "uncategorized concern".to_owned(),
        },
    )
    .await
    .expect("untriaged intake files");

    open_review(&app, low.case_id, "low", &moderator_cookie).await;
    open_review(&app, critical.case_id, "critical", &moderator_cookie).await;
    open_review(&app, normal.case_id, "normal", &moderator_cookie).await;
    open_review(&app, high.case_id, "high", &moderator_cookie).await;

    // Deterministic severity order with the untriaged intake last, sampled
    // targets visible on every row, and total covering the queue.
    let (status, page) = call(
        app.clone(),
        "GET",
        "/api/v1/moderation/cases",
        None,
        Some(&moderator_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 5);
    let order: Vec<String> = page["cases"]
        .as_array()
        .expect("cases read")
        .iter()
        .filter_map(|entry| entry["case_id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        order,
        [
            critical.case_id.to_string(),
            high.case_id.to_string(),
            normal.case_id.to_string(),
            low.case_id.to_string(),
            untriaged.case_id.to_string(),
        ]
    );
    for entry in page["cases"].as_array().expect("cases read") {
        let targets = entry["targets"].as_array().expect("targets read");
        assert!(!targets.is_empty(), "sample review targets visible");
        assert!(
            targets.iter().any(|target| target["title_snapshot"]
                .as_str()
                .is_some_and(|title| !title.is_empty())),
            "sample titles visible"
        );
    }
    assert_queue_private(&page);

    // Bounded paging keeps the head of the same order.
    let (status, head) = call(
        app.clone(),
        "GET",
        "/api/v1/moderation/cases?limit=2",
        None,
        Some(&moderator_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(head["total"], 5);
    assert_eq!(head["cases"].as_array().expect("cases read").len(), 2);
    assert_eq!(head["cases"][0]["case_id"], critical.case_id.to_string());
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn invalid_disposition_reduces_nothing() {
    let db = TestDatabase::create("p11t02_invalid")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t02_invalid_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let (_, moderator_cookie) = seed_moderator(&app, db.pool()).await;
    let buyer = provision_active(&app, "+55 11 90000-0906", "Invalid Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-0906").await;
    let seller = provision_active(&app, "+55 11 90000-0907", "Invalid Seller").await;
    let (demand, offer) = live_offer(db.pool(), buyer, seller).await;

    let before_users: Vec<String> =
        sqlx::query_scalar("SELECT state FROM users ORDER BY created_at")
            .fetch_all(db.pool())
            .await
            .expect("account states read");
    let before_request: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("request reads");
    let before_offer: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer)
        .fetch_one(db.pool())
        .await
        .expect("offer reads");

    // The buyer files through the real report route, then staff triage and
    // invalidate through the real moderation routes.
    let (status, filed) = call(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(json!({
            "target_kind": "offer",
            "target_id": offer.to_string(),
            "reason": "spam",
            "detail": "uncertain noise",
        })),
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let case_id = filed["case_id"].as_str().expect("receipt carries case");
    open_review(
        &app,
        case_id.parse().expect("case id parses"),
        "normal",
        &moderator_cookie,
    )
    .await;
    let (status, decided) = call(
        app.clone(),
        "POST",
        &format!("/api/v1/moderation/cases/{case_id}/decision"),
        Some(json!({
            "disposition": "invalid",
            "reason": "single uncertain allegation, no corroboration",
            "evidence": "insufficient evidence",
            "policy_version": "v1",
            "purpose": "decision on the open intake",
        })),
        Some(&moderator_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(decided["status"], "invalid");

    // An invalid allegation penalizes nobody: every standing is
    // byte-identical, the allegation row stands, and the decision is
    // auditable as a business fact with no malicious-report finding
    // anywhere in the schema.
    let after_users: Vec<String> =
        sqlx::query_scalar("SELECT state FROM users ORDER BY created_at")
            .fetch_all(db.pool())
            .await
            .expect("account states re-read");
    assert_eq!(before_users, after_users);
    let after_request: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("request re-reads");
    assert_eq!(before_request, after_request);
    let after_offer: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer)
        .fetch_one(db.pool())
        .await
        .expect("offer re-reads");
    assert_eq!(before_offer, after_offer);
    let reports: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(db.pool())
        .await
        .expect("reports read");
    assert_eq!(reports, 1);
    let fact: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'disposition' FROM business_events
         WHERE resource_kind = 'case' AND kind = 'case.decided'",
    )
    .fetch_optional(db.pool())
    .await
    .expect("decision fact reads");
    assert_eq!(fact.as_deref(), Some("invalid"));
    for table in ["report_cases", "reports"] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns WHERE table_name = $1",
        )
        .bind(table)
        .fetch_all(db.pool())
        .await
        .expect("columns read");
        for name in &columns {
            // `reporter_id` on `reports` is the private allegation row by
            // design (P10-T03); the guarantee is that it never reaches a
            // projection, proven by the serialization assertions above.
            assert!(
                ![
                    "malicious",
                    "penalty",
                    "fraud_score",
                    "trust_label",
                    "ban",
                    "phone",
                ]
                .contains(&name.as_str()),
                "no {name} column on {table}"
            );
        }
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn unprivileged_cannot_inspect_or_decide() {
    let db = TestDatabase::create("p11t02_denied")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t02_denied_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    seed_moderator(&app, db.pool()).await;
    let buyer = provision_active(&app, "+55 11 90000-0909", "Denied Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-0910", "Denied Seller").await;
    provision_active(&app, "+55 11 90000-0911", "Denied Stranger").await;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-0911").await;
    let (_, offer) = live_offer(db.pool(), buyer, seller).await;
    let filed = submit_grouped_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer,
            reason: "spam".to_owned(),
            detail: "denied noise".to_owned(),
        },
    )
    .await
    .expect("fixture intake files");

    // Signed-in strangers read no queue and move no case.
    for (method, path, body) in [
        ("GET", "/api/v1/moderation/cases".to_owned(), None),
        (
            "POST",
            format!("/api/v1/moderation/cases/{}/review", filed.case_id),
            Some(json!({
                "severity": "high",
                "policy_version": "v1",
                "purpose": "forged triage",
            })),
        ),
        (
            "POST",
            format!("/api/v1/moderation/cases/{}/decision", filed.case_id),
            Some(json!({
                "disposition": "valid",
                "reason": "forged finding",
                "policy_version": "v1",
                "purpose": "forged decision",
            })),
        ),
    ] {
        let (status, refused) =
            call(app.clone(), method, &path, body, Some(&stranger_cookie)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(refused["code"], "forbidden_role");
        assert_queue_private(&refused);
    }

    // Anonymous callers share one refusal on every moderation path.
    for (method, path) in [
        ("GET", "/api/v1/moderation/cases".to_owned()),
        (
            "POST",
            format!("/api/v1/moderation/cases/{}/review", filed.case_id),
        ),
    ] {
        let (status, refused) = call(app.clone(), method, &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(refused["code"], "unauthenticated");
    }

    // Nothing moved: the intake stays open with no facts and no audits
    // beyond the two staff grants.
    let standing: String = sqlx::query_scalar("SELECT status FROM report_cases WHERE id = $1")
        .bind(filed.case_id)
        .fetch_one(db.pool())
        .await
        .expect("case reads");
    assert_eq!(standing, "open");
    let facts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM business_events WHERE resource_kind = 'case'")
            .fetch_one(db.pool())
            .await
            .expect("facts read");
    assert_eq!(facts, 0);
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM staff_access_audit")
        .fetch_one(db.pool())
        .await
        .expect("audits read");
    assert_eq!(audits, 2, "grants only; refusals audit nothing");
    db.cleanup().await.expect("suite cleans up");
}
