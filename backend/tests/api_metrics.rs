//! Staff-metrics acceptance (P14-T06): gated aggregates, safe corrections.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL`. Staff grants travel
//! the real audited operations; the aggregate and correction paths run
//! through the real metrics HTTP routes (aggregates) and application
//! operations (corrections/policy) over real PostgreSQL. Proves:
//! - regular users cannot read private operational incident metrics;
//! - no aggregate payload carries destination or reporter canaries;
//! - prospective policy change and correction take effect independently.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::liquidity::classify_request;
use procurali_backend::application::metric_corrections::{
    apply_offer_correction, create_prospective_version, effective_policy_version,
};
use procurali_backend::application::metrics::{compute_cohort, CohortScope};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::metrics::{routes as metrics_routes, MetricsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p14t06-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p14t06-test-only-encryption-key";

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
    .merge(metrics_routes(MetricsState::new(db.pool().clone())))
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "app.test")
        .header("origin", "http://app.test");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).expect("test request builds"))
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

async fn provision_active(app: &axum::Router, phone: &str, name: &str) -> uuid::Uuid {
    let (status, body) = {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/accounts")
                    .header("content-type", "application/json")
                    .header("host", "app.test")
                    .header("origin", "http://app.test")
                    .body(Body::from(
                        json!({
                            "display_name": name,
                            "phone": phone,
                            "city": "Campinas",
                            "region": "SP",
                            "policy_version": "v1",
                        })
                        .to_string(),
                    ))
                    .expect("test request builds"),
            )
            .await
            .expect("route responds");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .expect("body reads");
        (
            status,
            serde_json::from_slice::<Value>(&bytes).expect("receipt parses"),
        )
    };
    assert_eq!(status, StatusCode::CREATED);
    let challenge = app
        .clone()
        .oneshot(
            Request::post("/api/v1/accounts/challenges")
                .header("content-type", "application/json")
                .header("host", "app.test")
                .header("origin", "http://app.test")
                .body(Body::from(json!({"phone": phone}).to_string()))
                .expect("test request builds"),
        )
        .await
        .expect("route responds");
    assert_eq!(challenge.status(), StatusCode::ACCEPTED);
    let confirm = app
        .clone()
        .oneshot(
            Request::post("/api/v1/accounts/challenges/confirmations")
                .header("content-type", "application/json")
                .header("host", "app.test")
                .header("origin", "http://app.test")
                .body(Body::from(
                    json!({"phone": phone, "code": FAKE_CODE}).to_string(),
                ))
                .expect("test request builds"),
        )
        .await
        .expect("route responds");
    assert_eq!(confirm.status(), StatusCode::OK);
    body["account_id"]
        .as_str()
        .expect("receipt carries id")
        .parse()
        .expect("receipt id parses")
}

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
    response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("login sets a cookie")
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

async fn seed_staff(app: &axum::Router, pool: &sqlx::PgPool) -> (uuid::Uuid, String, String) {
    let operator = provision_active(app, "+55 11 90000-2970", "Metrics Operator").await;
    bootstrap_grant(
        pool,
        BootstrapInput {
            user_id: operator,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "launch cover".to_owned(),
            operator_label: "launch-operator-metrics".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("launch bootstrap grants");
    let moderator = provision_active(app, "+55 11 90000-2971", "Metrics Moderator").await;
    grant_role(
        pool,
        operator,
        GrantInput {
            user_id: moderator,
            role: "moderator".to_owned(),
            scope: "safety".to_owned(),
            reason: "metrics cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("moderator granted");
    let moderator_cookie = login_cookie(app.clone(), "+55 11 90000-2971").await;
    let operator_cookie = login_cookie(app.clone(), "+55 11 90000-2970").await;
    (operator, moderator_cookie, operator_cookie)
}

fn assert_no_canary(body: &Value) {
    let rendered = body.to_string();
    for absent in [
        "90000-29",
        "+55",
        "canary",
        "reporter",
        "destination",
        "cipher",
        "lookup",
        "token",
        "secret",
        "phone",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in aggregate");
    }
}

#[tokio::test]
async fn regular_user_cannot_read_private_metrics() {
    let db = TestDatabase::create("p14t06_denied")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t06_denied_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let (_operator, moderator_cookie, _operator_cookie) = seed_staff(&app, db.pool()).await;
    let buyer = provision_active(&app, "+55 11 90000-2972", "Metrics Buyer").await;
    let _ = buyer;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-2972").await;

    let (status, _) = call(app.clone(), "GET", "/api/v1/metrics/operational", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, denied) = call(
        app.clone(),
        "GET",
        "/api/v1/metrics/operational?purpose=operations+review",
        Some(&buyer_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(denied["code"], "forbidden_role");
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/v1/metrics/operational",
        Some(&moderator_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, aggregate) = call(
        app.clone(),
        "GET",
        "/api/v1/metrics/operational?purpose=operations+review",
        Some(&moderator_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(aggregate.get("incidents").is_some());
    assert!(aggregate.get("sharing").is_some());
    assert!(aggregate.get("professionals").is_some());
    assert_eq!(aggregate["policy_version"], "v1");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn aggregate_payload_carries_no_canary() {
    let db = TestDatabase::create("p14t06_canary")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t06_canary_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let (_operator, moderator_cookie, _) = seed_staff(&app, db.pool()).await;
    // Canary account whose phone and identity must never enter aggregates.
    provision_active(&app, "+55 11 98888-7777", "Canary Reporter 7777").await;
    let (status, aggregate) = call(
        app.clone(),
        "GET",
        "/api/v1/metrics/operational?purpose=canary+sweep",
        Some(&moderator_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_no_canary(&aggregate);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn policy_change_and_correction_differ_independently() {
    let db = TestDatabase::create("p14t06_correct")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t06_correct_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let (operator, moderator_cookie, _) = seed_staff(&app, db.pool()).await;
    let moderator: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE display_name = 'Metrics Moderator'")
            .fetch_one(db.pool())
            .await
            .expect("moderator reads");
    let _ = moderator_cookie;
    let buyer = provision_active(&app, "+55 11 90000-2975", "Correct Buyer").await;
    let seller = provision_active(&app, "+55 11 90000-2976", "Correct Seller").await;
    let other = provision_active(&app, "+55 11 90000-2977", "Correct Other").await;

    // One demand with one honest and one false offer: both live before.
    let draft = create_draft(
        db.pool(),
        buyer,
        DraftInput {
            title: Some("Correct Fridge".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("600.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("draft validates");
    let demand = publish_request(db.pool(), buyer, draft.id)
        .await
        .expect("demand publishes")
        .id;
    sqlx::query("UPDATE requests SET original_published_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now() - chrono::Duration::hours(2))
        .bind(demand)
        .execute(db.pool())
        .await
        .expect("publication stages");
    let honest = submit_offer(
        db.pool(),
        seller,
        demand,
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(1),
            description: Some("Honest 300L".to_owned()),
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
    .expect("honest offer submits")
    .id;
    let false_offer = submit_offer(
        db.pool(),
        other,
        demand,
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(1),
            description: Some("False 300L".to_owned()),
            price: Some("10.00".to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
            available: Some(true),
            available_in_city: Some(true),
        },
    )
    .await
    .expect("false offer submits")
    .id;
    let now = chrono::Utc::now() + chrono::Duration::seconds(10);
    let before = classify_request(db.pool(), demand, now)
        .await
        .expect("liquidity reads before");
    assert_eq!(before.live_sellers, 2);

    // Cohort baseline before the prospective version lands.
    let from = chrono::Utc::now() - chrono::Duration::hours(3);
    let scope = CohortScope {
        city_code: Some("campinas".to_owned()),
        category_code: Some("home_appliances".to_owned()),
        from,
        to: now,
    };
    let cohort_before = compute_cohort(db.pool(), scope.clone())
        .await
        .expect("cohort reads before");
    assert_eq!(
        effective_policy_version(db.pool(), chrono::Utc::now())
            .await
            .expect("policy reads"),
        "v1"
    );

    // Prospective version lands in the future: current rules untouched and
    // old cohorts byte-identical afterwards.
    create_prospective_version(
        db.pool(),
        operator,
        "v2",
        chrono::Utc::now() + chrono::Duration::days(30),
        "Prospective threshold review.",
    )
    .await
    .expect("prospective version records");
    assert_eq!(
        effective_policy_version(db.pool(), chrono::Utc::now())
            .await
            .expect("policy still current"),
        "v1"
    );
    let cohort_after = compute_cohort(db.pool(), scope)
        .await
        .expect("cohort reads after");
    assert_eq!(cohort_before, cohort_after);

    // Correction excludes only the false offer: honest history survives and
    // liveness drops by exactly one.
    let correction = apply_offer_correction(
        db.pool(),
        moderator,
        false_offer,
        "false_information",
        "v1",
        "remove fabricated supply from health",
    )
    .await
    .expect("false offer corrects");
    assert_eq!(correction.target_id, false_offer);
    let after = classify_request(db.pool(), demand, now)
        .await
        .expect("liquidity reads after");
    assert_eq!(after.live_sellers, 1);
    assert!(after.first_offer_at.is_some());
    let honest_state: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(honest)
        .fetch_one(db.pool())
        .await
        .expect("honest row reads");
    assert_ne!(honest_state, "invalidated");
    db.cleanup().await.expect("suite cleans up");
}
