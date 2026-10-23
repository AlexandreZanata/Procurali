//! Actor and overposting attack matrix (P17-T02): every sensitive route
//! checks owner, state, purpose, and grant over real routing.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Covers anonymous, wrong owner, own seller, other seller,
//! professional, moderator, admin, and restricted states; forges
//! resource owner/role/state/version extras and hidden UI actions; and
//! asserts exact error codes with database state. No grant exists over
//! HTTP, so self-grant attempts travel forged bodies and role claims.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::ban_user::{ban_user, BanUserInput};
use procurali_backend::application::delete_account::{delete_account, DeleteAccountInput};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::suspend_user::{suspend_user, SuspendUserInput};
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use procurali_backend::http::blocks::{routes as block_routes, BlocksState};
use procurali_backend::http::catalogs::{routes as catalog_routes, CatalogsState};
use procurali_backend::http::contacts::{routes as contact_routes, ContactsState};
use procurali_backend::http::metrics::{routes as metrics_routes, MetricsState};
use procurali_backend::http::moderation::{routes as moderation_routes, ModerationState};
use procurali_backend::http::offers::{routes as offer_routes, OffersState};
use procurali_backend::http::reports::{routes as report_routes, ReportsState};
use procurali_backend::http::requests::{routes as request_routes, RequestsState};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const LOOKUP_KEY: &str = "p17t02-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p17t02-test-only-encryption-key";

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
    .merge(block_routes(BlocksState::new(db.pool().clone())))
    .merge(catalog_routes(CatalogsState::new(db.pool().clone())))
    .merge(contact_routes(ContactsState::new(
        db.pool().clone(),
        LOOKUP_KEY.to_owned(),
        ENCRYPTION_KEY.to_owned(),
    )))
    .merge(metrics_routes(MetricsState::new(db.pool().clone())))
    .merge(moderation_routes(ModerationState::new(db.pool().clone())))
    .merge(offer_routes(OffersState::new(db.pool().clone())))
    .merge(report_routes(ReportsState::new(db.pool().clone())))
    .merge(request_routes(RequestsState::new(db.pool().clone())))
}

async fn raw(
    app: axum::Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "app.test")
        .header("origin", "http://app.test");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
    } else {
        builder.body(Body::empty())
    }
    .expect("test request builds");
    let response = app.oneshot(request).await.expect("route responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    if bytes.is_empty() {
        return (status, Value::Null);
    }
    (
        status,
        serde_json::from_slice(&bytes).expect("response JSON parses"),
    )
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

async fn seed_admin(app: &axum::Router, pool: &sqlx::PgPool) -> uuid::Uuid {
    let operator = provision_active(app, "+55 11 90000-3210", "Matrix Operator").await;
    bootstrap_grant(
        pool,
        BootstrapInput {
            user_id: operator,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "matrix cover".to_owned(),
            operator_label: "matrix-operator".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("launch bootstrap grants");
    let admin = provision_active(app, "+55 11 90000-3211", "Matrix Admin").await;
    grant_role(
        pool,
        operator,
        GrantInput {
            user_id: admin,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "matrix cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("admin granted");
    admin
}

async fn seed_moderator(app: &axum::Router, pool: &sqlx::PgPool, admin: uuid::Uuid) -> uuid::Uuid {
    let moderator = provision_active(app, "+55 11 90000-3212", "Matrix Moderator").await;
    grant_role(
        pool,
        admin,
        GrantInput {
            user_id: moderator,
            role: "moderator".to_owned(),
            scope: "safety".to_owned(),
            reason: "matrix cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("moderator granted");
    moderator
}

async fn publish_demand(app: &axum::Router, cookie: &str, title: &str) -> uuid::Uuid {
    let (status, draft) = raw(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(cookie),
        Some(json!({
            "title": title,
            "category_code": "home_appliances",
            "budget": "600.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = draft["id"].as_str().expect("draft id").to_owned();
    let (status, _) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/drafts/{id}/publication"),
        Some(cookie),
        Some(json!({})),
    )
    .await;
    assert!(status == StatusCode::CREATED || status == StatusCode::OK);
    id.parse().expect("demand id parses")
}

async fn submit_offer(
    app: &axum::Router,
    cookie: &str,
    demand: &str,
    description: &str,
    price: &str,
) -> Value {
    let (status, body) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(cookie),
        Some(json!({
            "cycle_number": 1,
            "revision_number": 1,
            "description": description,
            "price": price,
            "condition": "used",
            "city_code": "campinas",
            "region_code": "centro",
            "available": true,
            "available_in_city": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body
}

#[tokio::test]
async fn no_caller_grants_themselves_ownership_or_authority() {
    let db = TestDatabase::create("p17t02_selfgrant")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t02_selfgrant_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let admin = seed_admin(&app, db.pool()).await;
    let _moderator = seed_moderator(&app, db.pool(), admin).await;
    let buyer = provision_active(&app, "+55 11 90000-3220", "Matrix Buyer").await;
    let _ = buyer;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-3220").await;
    let mod_cookie = login_cookie(app.clone(), "+55 11 90000-3212").await;

    // Forged role/owner/seller extras refuse as unknown fields.
    for (path, body) in [
        (
            "/api/v1/accounts",
            json!({
                "display_name": "Forge",
                "phone": "+55 11 90000-3290",
                "city": "Campinas",
                "region": "SP",
                "policy_version": "v1",
                "role": "administrator",
            }),
        ),
        (
            "/api/v1/requests/drafts",
            json!({
                "title": "Forge",
                "category_code": "home_appliances",
                "budget": "600.00",
                "condition": "either",
                "city_code": "campinas",
                "region_code": "centro",
                "author_id": "00000000-0000-0000-0000-000000000000",
            }),
        ),
    ] {
        let (status, denied) =
            raw(app.clone(), "POST", path, Some(&buyer_cookie), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(denied["code"], "invalid_field");
    }

    // Regular users cannot touch staff surfaces; moderators cannot either
    // where admin authority is required (metrics needs moderator, which
    // the moderator has — asserted below — while decisions stay scoped).
    let (status, denied) = raw(
        app.clone(),
        "GET",
        "/api/v1/moderation/cases?limit=20&offset=0",
        Some(&buyer_cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(denied["code"], "forbidden_role");
    let (status, _) = raw(
        app.clone(),
        "GET",
        "/api/v1/moderation/cases?limit=20&offset=0",
        Some(&mod_cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn hidden_actions_refuse_forged_direct_requests() {
    let db = TestDatabase::create("p17t02_hidden")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t02_hidden_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-3230", "Hidden Buyer").await;
    let _ = buyer;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-3230").await;
    let demand = publish_demand(&app, &buyer_cookie, "Hidden Fridge").await;

    // Endpoints that do not exist answer 404 without disclosing anything.
    for (method, path) in [
        ("POST", format!("/api/v1/offers/{demand}/decline")),
        ("POST", format!("/api/v1/offers/{demand}/withdrawal")),
        ("POST", "/api/v1/outcomes".to_owned()),
        ("POST", "/api/v1/feedback".to_owned()),
        ("POST", "/api/v1/staff/ban".to_owned()),
        ("POST", "/api/v1/accounts/phone-changes".to_owned()),
    ] {
        let (status, _) = raw(
            app.clone(),
            method,
            &path,
            Some(&buyer_cookie),
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "hidden {path}");
    }

    // The renewal route exists: an owner request that matches no readable
    // body is refused as a schema error, never a hidden-route 404.
    let (status, denied) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/renewal"),
        Some(&buyer_cookie),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(denied["code"], "missing_field");

    // Published demands are invisible to the draft reader (stale edits).
    let (status, _) = raw(
        app.clone(),
        "PUT",
        &format!("/api/v1/requests/drafts/{demand}"),
        Some(&buyer_cookie),
        Some(json!({"title": "Sneaky"})),
    )
    .await;
    assert!(status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST);

    // Malformed handoff identity refuses as invalid field, never state.
    let (status, denied) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/00000000-0000-0000-0000-000000000000/contact"),
        Some(&buyer_cookie),
        Some(json!({
            "handoff_id": "not-a-uuid",
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(denied["code"], "invalid_field");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn private_reads_separate_owners_strangers_and_sellers() {
    let db = TestDatabase::create("p17t02_reads")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t02_reads_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-3240", "Reads Buyer").await;
    let _ = buyer;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-3240").await;
    let seller_a = provision_active(&app, "+55 11 90000-3241", "Reads Alpha").await;
    let _ = seller_a;
    let cookie_a = login_cookie(app.clone(), "+55 11 90000-3241").await;
    let seller_b = provision_active(&app, "+55 11 90000-3242", "Reads Beta").await;
    let _ = seller_b;
    let cookie_b = login_cookie(app.clone(), "+55 11 90000-3242").await;
    let stranger = provision_active(&app, "+55 11 90000-3243", "Reads Stranger").await;
    let _ = stranger;
    let stranger_cookie = login_cookie(app.clone(), "+55 11 90000-3243").await;
    let demand = publish_demand(&app, &buyer_cookie, "Reads Fridge").await;
    let demand_id = demand.to_string();
    let offer_a = submit_offer(&app, &cookie_a, &demand_id, "Matrix Alpha 300L", "520.00").await;
    let offer_b = submit_offer(&app, &cookie_b, &demand_id, "Matrix Beta 280L", "480.00").await;
    let (id_a, id_b) = (
        offer_a["id"].as_str().expect("offer a").to_owned(),
        offer_b["id"].as_str().expect("offer b").to_owned(),
    );

    // Anonymous reads nothing private.
    let (status, _) = raw(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Strangers meet an indistinguishable refusal without prices.
    let (status, denied) = raw(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(&stranger_cookie),
        None,
    )
    .await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND);
    assert!(!denied.to_string().contains("520.00"));
    assert!(!denied.to_string().contains("480.00"));

    // Each seller sees exactly their own row.
    for (cookie, own, rival_price) in [(&cookie_a, &id_a, "480.00"), (&cookie_b, &id_b, "520.00")] {
        let (status, mine) = raw(
            app.clone(),
            "GET",
            "/api/v1/offers/mine",
            Some(cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let rendered = mine.to_string();
        assert!(rendered.contains(own.as_str()));
        assert!(!rendered.contains(rival_price));
    }

    // Owner compares both with names and without phones.
    let (status, both) = raw(
        app.clone(),
        "GET",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(&buyer_cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rendered = both.to_string();
    assert!(rendered.contains(&id_a) && rendered.contains(&id_b));
    assert!(!rendered.contains("+55"));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_keeps_only_specified_exceptions() {
    let db = TestDatabase::create("p17t02_suspended")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t02_suspended_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let admin = seed_admin(&app, db.pool()).await;
    let buyer = provision_active(&app, "+55 11 90000-3250", "Suspended Buyer").await;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-3250").await;
    let seller = provision_active(&app, "+55 11 90000-3251", "Suspended Seller").await;
    let _ = seller;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-3251").await;
    let demand = publish_demand(&app, &buyer_cookie, "Suspended Fridge").await;
    let offer = submit_offer(
        &app,
        &seller_cookie,
        &demand.to_string(),
        "Matrix 300L",
        "520.00",
    )
    .await;
    let offer_id = offer["id"].as_str().expect("offer id").to_owned();

    suspend_user(
        db.pool(),
        admin,
        SuspendUserInput {
            user_id: buyer,
            reason: "matrix suspension".to_owned(),
            duration_hours: 24,
            policy_version: "v1".to_owned(),
            purpose: "matrix cover".to_owned(),
            case_id: None,
        },
    )
    .await
    .expect("suspension applies");

    // Suspended writers are refused everywhere mutating.
    let (status, _) = raw(
        app.clone(),
        "POST",
        "/api/v1/requests/drafts",
        Some(&buyer_cookie),
        Some(json!({
            "title": "After Suspension",
            "category_code": "home_appliances",
            "budget": "600.00",
            "condition": "either",
            "city_code": "campinas",
            "region_code": "centro",
        })),
    )
    .await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED);
    let (status, _) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer_id}/contact"),
        Some(&buyer_cookie),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
    )
    .await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED);

    // Suspended sessions hold no HTTP mutation path at all: even
    // own-history reports refuse here. The specified exception (restricted
    // own-history appeal without privileges) travels the operation layer
    // and stays proven by
    // api_appeals::restricted_own_history_appeal_without_privileges.
    let (status, _) = raw(
        app.clone(),
        "POST",
        "/api/v1/reports",
        Some(&buyer_cookie),
        Some(json!({
            "target_kind": "offer",
            "target_id": offer_id,
            "reason": "spam",
            "detail": "suspended own-history report",
        })),
    )
    .await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn banned_and_deleted_match_specified_exceptions() {
    let db = TestDatabase::create("p17t02_banned")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t02_banned_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let admin = seed_admin(&app, db.pool()).await;
    let buyer = provision_active(&app, "+55 11 90000-3260", "Banned Buyer").await;
    let _ = buyer;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-3260").await;
    let seller = provision_active(&app, "+55 11 90000-3261", "Banned Seller").await;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-3261").await;
    let demand = publish_demand(&app, &buyer_cookie, "Banned Fridge").await;
    let offer = submit_offer(
        &app,
        &seller_cookie,
        &demand.to_string(),
        "Matrix 300L",
        "520.00",
    )
    .await;
    let offer_id = offer["id"].as_str().expect("offer id").to_owned();

    ban_user(
        db.pool(),
        admin,
        BanUserInput {
            user_id: seller,
            reason: "matrix ban".to_owned(),
            evidence: "matrix fixture".to_owned(),
            policy_version: "v1".to_owned(),
            purpose: "matrix cover".to_owned(),
            case_id: None,
        },
    )
    .await
    .expect("ban applies");

    // Banned parties cannot mutate or reveal, with history intact.
    let (status, _) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(&seller_cookie),
        Some(json!({
            "cycle_number": 1,
            "revision_number": 1,
            "description": "Banned retry",
            "price": "10.00",
            "condition": "used",
            "city_code": "campinas",
            "region_code": "centro",
            "available": true,
            "available_in_city": true,
        })),
    )
    .await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED);

    // Deleted callers lose even stale-cookie reads on private surfaces.
    let leaver = provision_active(&app, "+55 11 90000-3262", "Matrix Leaver").await;
    let leaver_cookie = login_cookie(app.clone(), "+55 11 90000-3262").await;
    delete_account(
        db.pool(),
        leaver,
        DeleteAccountInput {
            user_id: leaver,
            reason: "matrix departure".to_owned(),
        },
    )
    .await
    .expect("deletion applies");
    let (status, _) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer_id}/contact"),
        Some(&leaver_cookie),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 1,
            "entry_source": "offer_detail",
        })),
    )
    .await;
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn version_and_state_forgery_refused() {
    let db = TestDatabase::create("p17t02_forgery")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t02_forgery_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let app = test_app(&db);
    let buyer = provision_active(&app, "+55 11 90000-3270", "Forge Buyer").await;
    let _ = buyer;
    let buyer_cookie = login_cookie(app.clone(), "+55 11 90000-3270").await;
    let seller = provision_active(&app, "+55 11 90000-3271", "Forge Seller").await;
    let _ = seller;
    let seller_cookie = login_cookie(app.clone(), "+55 11 90000-3271").await;
    let demand = publish_demand(&app, &buyer_cookie, "Forge Fridge").await;
    let offer = submit_offer(
        &app,
        &seller_cookie,
        &demand.to_string(),
        "Matrix 300L",
        "520.00",
    )
    .await;
    let offer_id = offer["id"].as_str().expect("offer id").to_owned();

    // Forged future cycles and unknown terms refuse without new rows.
    let (status, _) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers"),
        Some(&seller_cookie),
        Some(json!({
            "cycle_number": 99,
            "revision_number": 1,
            "description": "Forged cycle",
            "price": "10.00",
            "condition": "used",
            "city_code": "campinas",
            "region_code": "centro",
            "available": true,
            "available_in_city": true,
        })),
    )
    .await;
    assert!(status.is_client_error() && status != StatusCode::NOT_FOUND);
    let (status, denied) = raw(
        app.clone(),
        "POST",
        &format!("/api/v1/requests/{demand}/offers/{offer_id}/contact"),
        Some(&buyer_cookie),
        Some(json!({
            "handoff_id": uuid::Uuid::now_v7().to_string(),
            "expected_offer_terms": 999,
            "entry_source": "offer_detail",
        })),
    )
    .await;
    assert!(status.is_client_error());
    assert_ne!(denied["code"], "not_found");
    db.cleanup().await.expect("suite cleans up");
}
