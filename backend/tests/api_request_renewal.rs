//! Renewal acceptance (P06-T05): explicit confirmed renewal only.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). The renewal operation currently has no HTTP route (the
//! route-owning card lands it later), so these tests drive the application
//! operation directly over real PostgreSQL — the same direct-proof pattern
//! as the publication card. Proves:
//! - early renewal and unconfirmed interactions produce no new cycle;
//! - valid renewal increments cycle and activation count exactly once,
//!   retaining identity and first-publication age;
//! - terminal, suspended, and hidden rows cannot reactivate, and renewal
//!   records no new publication fact.
//!
//! Setup publishes travel the real draft and publication operations. All
//! phones, names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::renew_request::{
    renew_request, RenewError, RenewalConfirmation,
};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::requests::{
    cycles_for_request, request as read_request, requests_for_author,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

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

/// A real account, activated through a synthetic state fixture: identity
/// itself travels the real creation path with real phone cryptography, and
/// only the lifecycle flip (owned by the verification flow) is staged.
async fn seed_author(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
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
        PhoneKeys {
            lookup_key: "p06t05-test-only-lookup-key",
            encryption_key: "p06t05-test-only-encryption-key",
        },
    )
    .await
    .expect("fixture author stores");
    tx.commit().await.expect("author commits");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    user.id
}

const HTTP_FAKE_CODE: &str = "135790";

fn http_app(db: &TestDatabase) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(HTTP_FAKE_CODE));
    let lookup = "p06t05-test-only-lookup-key".to_owned();
    let encryption = "p06t05-test-only-encryption-key".to_owned();
    account_routes(AccountsState::new(
        db.pool().clone(),
        Arc::clone(&provider),
        lookup.clone(),
        encryption.clone(),
    ))
    .merge(auth_routes(AuthState::new(
        db.pool().clone(),
        provider,
        lookup,
        encryption,
        None,
    )))
    .merge(request_routes(RequestsState::new(db.pool().clone())))
}

async fn http_call(
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

/// Provision one active account through the real account routes and return
/// its session cookie for the HTTP renewal boundary.
async fn http_login(app: &axum::Router, phone: &str, name: &str) -> String {
    let (status, _) = http_call(
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
    let (status, _) = http_call(
        app.clone(),
        "POST",
        "/api/v1/accounts/challenges",
        Some(json!({"phone": phone})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, active) = http_call(
        app.clone(),
        "POST",
        "/api/v1/accounts/challenges/confirmations",
        Some(json!({"phone": phone, "code": HTTP_FAKE_CODE})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["account_state"], "active");
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("content-type", "application/json")
                .header("host", "app.test")
                .header("origin", "http://app.test")
                .body(Body::from(
                    json!({"phone": phone, "code": HTTP_FAKE_CODE}).to_string(),
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

/// Publish one owned demand through the real draft and publication routes.
/// Exact duplicates refuse at publication, so independent fixtures need
/// distinct titles.
async fn publish_demand_over_http(app: &axum::Router, cookie: &str, title: &str) -> String {
    let (status, draft) = http_call(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(json!({
            "title": title,
            "category_code": "home_appliances",
            "budget": "600.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
            "notes": "Preferably frost-free.",
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = draft["id"].as_str().expect("draft id").to_owned();
    let (status, _) = http_call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{id}/publication"),
        Some(json!({})),
        Some(cookie),
    )
    .await;
    assert!(status == StatusCode::CREATED || status == StatusCode::OK);
    id
}

fn confirmation() -> RenewalConfirmation {
    RenewalConfirmation {
        title: "Refrigerator".to_owned(),
        category_code: "home_appliances".to_owned(),
        budget: "520.00".to_owned(),
        condition: "either".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Preferably frost-free.".to_owned(),
    }
}

/// One genuinely open request through the real draft and publication paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid) -> uuid::Uuid {
    publish_open_titled(pool, author, "Refrigerator").await
}

/// Same as [`publish_open`] with an explicit title: exact duplicates refuse
/// at publication, so a second open demand needs a distinct title.
async fn publish_open_titled(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("520.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: Some("Preferably frost-free.".to_owned()),
        },
    )
    .await
    .expect("fixture draft validates");
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

/// Move one cycle into the past, honoring the cycle CHECK (start and
/// deadline travel together; only the deadline position matters).
async fn backdate_cycle(pool: &sqlx::PgPool, request_id: uuid::Uuid) {
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(pool)
    .await
    .expect("synthetic expiry applies");
}

async fn event_count(pool: &sqlx::PgPool, author: uuid::Uuid, kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE actor_id = $1 AND resource_kind = 'request' AND kind = $2",
    )
    .bind(author)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("events read")
}

#[tokio::test]
async fn early_renewal_and_unconfirmed_paths_produce_no_cycle() {
    let db = TestDatabase::create("p06t05_early")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t05_early_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Early Owner", "+55 11 90000-0081").await;
    let id = publish_open(db.pool(), author).await;

    // A fresh seven-day cycle is not renewable: early refusal, one cycle,
    // no renewal fact, no activation consumed.
    assert_eq!(
        renew_request(db.pool(), author, id, confirmation()).await,
        Err(RenewError::EarlyRenewal)
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 0);

    // Neither a material revision nor a stale confirmation renews: the
    // expired row gains history but no cycle, and the refusal names terms.
    backdate_cycle(db.pool(), id).await;
    let stale = RenewalConfirmation {
        budget: "800.00".to_owned(),
        ..confirmation()
    };
    assert_eq!(
        renew_request(db.pool(), author, id, stale).await,
        Err(RenewError::StaleRequirements)
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn valid_renewal_increments_cycle_and_activation_once() {
    let db = TestDatabase::create("p06t05_valid")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t05_valid_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Renew Owner", "+55 11 90000-0082").await;
    let id = publish_open(db.pool(), author).await;
    let published_at = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads")
        .original_published_at;
    backdate_cycle(db.pool(), id).await;

    let renewed = renew_request(db.pool(), author, id, confirmation())
        .await
        .expect("eligible renewal renews");
    assert_eq!(renewed.id, id);
    assert_eq!(renewed.cycle_number, 2);
    assert_eq!(renewed.revision_number, 1, "renewal revises nothing");
    assert_eq!(renewed.state, "active");
    assert_eq!(renewed.original_published_at, published_at);
    assert_eq!(
        renewed.deadline.signed_duration_since(renewed.started_at),
        chrono::Duration::days(7)
    );
    let cycles = cycles_for_request(db.pool(), id)
        .await
        .expect("cycles read");
    assert_eq!(cycles.len(), 2);
    assert!(
        cycles[0].ended_at.is_some(),
        "the prior cycle ends instead of lingering"
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 1);
    assert_eq!(event_count(db.pool(), author, "request.published").await, 1);

    // An immediate repeat is early again: still two cycles, still one fact.
    assert_eq!(
        renew_request(db.pool(), author, id, confirmation()).await,
        Err(RenewError::EarlyRenewal)
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        2
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 1);

    // An active row inside its final day renews without waiting for expiry.
    // The second demand carries a distinct title: exact duplicates refuse
    // at publication, and this test is about renewal timing, not copies.
    let second = publish_open_titled(db.pool(), author, "Microwave oven").await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(second)
    .bind(now - chrono::Duration::days(6) - chrono::Duration::hours(12))
    .bind(now + chrono::Duration::hours(12))
    .execute(db.pool())
    .await
    .expect("synthetic final day applies");
    let renewed = renew_request(
        db.pool(),
        author,
        second,
        RenewalConfirmation {
            title: "Microwave oven".to_owned(),
            ..confirmation()
        },
    )
    .await
    .expect("final-day renewal renews");
    assert_eq!(renewed.cycle_number, 2);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn terminal_suspended_and_hidden_rows_never_reactivate() {
    let db = TestDatabase::create("p06t05_terminal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t05_terminal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Terminal Owner", "+55 11 90000-0083").await;

    for (title, state, visibility) in [
        ("Terminal One", "completed", "public"),
        ("Terminal Two", "cancelled", "hidden"),
        ("Terminal Three", "suspended", "public"),
        ("Terminal Four", "expired", "hidden"),
    ] {
        let draft = create_draft(
            db.pool(),
            author,
            DraftInput {
                title: Some(title.to_owned()),
                category_code: Some("home_appliances".to_owned()),
                budget: Some("520.00".to_owned()),
                condition: Some("either".to_owned()),
                city_code: Some("campinas".to_owned()),
                region_code: Some("centro".to_owned()),
                notes: None,
            },
        )
        .await
        .expect("fixture draft validates");
        let published = publish_request(db.pool(), author, draft.id)
            .await
            .expect("fixture draft publishes")
            .id;
        sqlx::query("UPDATE requests SET state = $2, visibility = $3 WHERE id = $1")
            .bind(published)
            .bind(state)
            .bind(visibility)
            .execute(db.pool())
            .await
            .expect("synthetic terminal state applies");
        assert_eq!(
            renew_request(db.pool(), author, published, confirmation()).await,
            Err(RenewError::ForbiddenState),
            "{state}/{visibility} never reactivates"
        );
        assert_eq!(event_count(db.pool(), author, "request.renewed").await, 0);
    }

    // Strangers renew nothing, and renewal is never a publication metric:
    // one row, one publication fact, retained first-publication age.
    let other = seed_author(db.pool(), "Renew Stranger", "+55 11 90000-0084").await;
    let id = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), id).await;
    assert_eq!(
        renew_request(db.pool(), other, id, confirmation()).await,
        Err(RenewError::NotFound)
    );
    let before = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads")
        .original_published_at;
    renew_request(db.pool(), author, id, confirmation())
        .await
        .expect("eligible renewal renews");
    let after = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(after.original_published_at, before);
    assert_eq!(
        requests_for_author(db.pool(), author)
            .await
            .expect("owner reads")
            .len(),
        5,
        "renewal creates cycles, not requests"
    );
    assert_eq!(event_count(db.pool(), author, "request.published").await, 5);
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

/// The HTTP renewal route: one owned expired demand renews over the route
/// with the buyer-confirmed requirements; strangers, anonymous callers, and
/// out-of-window attempts are refused with typed errors and no new cycle.
fn route_confirmation(title: &str) -> Value {
    json!({
        "title": title,
        "category_code": "home_appliances",
        "budget": "600.00",
        "condition": "either",
        "city_code": "campinas",
        "region_code": "centro",
        "notes": "Preferably frost-free.",
    })
}

#[tokio::test]
async fn renewal_route_confirms_an_expired_demand_over_http() {
    let db = TestDatabase::create("p25t04_http")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p25t04_http_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = http_app(&db);
    let owner = http_login(&app, "+55 11 90000-0091", "Route Owner").await;
    let id = publish_demand_over_http(&app, &owner, "Route Fridge").await;

    // A fresh seven-day cycle refuses renewal: one typed refusal, one cycle.
    let (status, refused) = http_call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{id}/renewal"),
        Some(route_confirmation("Route Fridge")),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "forbidden_state");
    assert_eq!(
        cycles_for_request(db.pool(), id.parse().expect("demand id parses"))
            .await
            .expect("cycles read")
            .len(),
        1
    );

    // The expired demand renews over the route with its identity retained.
    sqlx::query("UPDATE request_cycles SET started_at = now() - interval '8 days', deadline = now() - interval '1 day' WHERE request_id = $1 AND cycle_number = 1")
        .bind(id.parse::<uuid::Uuid>().expect("demand id parses"))
        .execute(db.pool())
        .await
        .expect("synthetic expiry applies");
    let (status, renewed) = http_call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{id}/renewal"),
        Some(route_confirmation("Route Fridge")),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renewed["id"], id);
    assert_eq!(renewed["cycle_number"], 2);
    assert_eq!(renewed["revision_number"], 1);
    assert_eq!(renewed["state"], "active");
    assert!(renewed["original_published_at"].is_string());

    // The second attempt lands on the fresh cycle: refused, nothing written.
    let (status, refused) = http_call(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{id}/renewal"),
        Some(route_confirmation("Route Fridge")),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "forbidden_state");
    assert_eq!(
        cycles_for_request(db.pool(), id.parse().expect("demand id parses"))
            .await
            .expect("cycles read")
            .len(),
        2
    );

    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn renewal_route_refuses_strangers_anonymous_and_stale_bodies() {
    let db = TestDatabase::create("p25t04_route_guards")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p25t04_route_guards_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = http_app(&db);
    let owner = http_login(&app, "+55 11 90000-0092", "Route Buyer").await;
    let stranger = http_login(&app, "+55 11 90000-0093", "Route Stranger").await;
    let id = publish_demand_over_http(&app, &owner, "Route Freezer").await;
    let path = format!("/api/v1/requests/{id}/renewal");
    let demand: uuid::Uuid = id.parse().expect("demand id parses");

    // Strangers and anonymous callers never renew: one 404, one 401, no
    // disclosure of ownership or liveness beyond the refusal.
    let (status, denied) = http_call(
        app.clone(),
        "POST",
        &path,
        Some(route_confirmation("Route Freezer")),
        Some(&stranger),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(denied["code"], "not_found");
    let (status, denied) = http_call(
        app.clone(),
        "POST",
        &path,
        Some(route_confirmation("Route Freezer")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(denied["code"], "unauthenticated");

    // Unknown keys are refused as unknown: the confirmation carries exactly
    // the requirements vocabulary.
    let mut overposted = route_confirmation("Route Freezer");
    overposted["author_id"] = json!("00000000-0000-0000-0000-000000000000");
    let (status, denied) =
        http_call(app.clone(), "POST", &path, Some(overposted), Some(&owner)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(denied["code"], "invalid_field");

    // A material difference refuses first: revise, then renew.
    sqlx::query("UPDATE request_cycles SET started_at = now() - interval '8 days', deadline = now() - interval '1 day' WHERE request_id = $1 AND cycle_number = 1")
        .bind(demand)
        .execute(db.pool())
        .await
        .expect("synthetic expiry applies");
    let mut stale = route_confirmation("Route Freezer");
    stale["budget"] = json!("800.00");
    let (status, denied) = http_call(app.clone(), "POST", &path, Some(stale), Some(&owner)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(denied["code"], "forbidden_state");
    assert_eq!(
        cycles_for_request(db.pool(), demand)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    assert_eq!(
        event_count(
            db.pool(),
            demand_owner(db.pool(), demand).await,
            "request.renewed"
        )
        .await,
        0
    );

    db.cleanup().await.expect("suite cleans up");
}

/// The owner of one stored request, for fact accounting in guard tests.
async fn demand_owner(pool: &sqlx::PgPool, id: uuid::Uuid) -> uuid::Uuid {
    read_request(pool, id)
        .await
        .expect("request reads")
        .expect("request reads")
        .author_id
}
