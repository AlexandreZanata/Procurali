//! Session-route acceptance (P04-T04): login, current account, logout, CSRF.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Drives the real axum session routes over real PostgreSQL 18.6
//! with the deterministic fake provider. Registration and challenges reuse the
//! account routes. Proves:
//! - unauthenticated, expired, and forged sessions are refused;
//! - wrong-origin and missing-CSRF unsafe actions fail while safe reads pass;
//! - suspended and deleted accounts keep no privilege from previous cookies.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::http::auth::{routes as auth_routes, AuthState};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const ORIGIN: &str = "http://app.test";

fn test_app(db: &TestDatabase, origin: Option<&str>) -> axum::Router {
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));
    let lookup = "p04t04-test-only-lookup-key".to_owned();
    let encryption = "p04t04-test-only-encryption-key".to_owned();
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
        origin.map(str::to_owned),
    )))
}

fn json_request(
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    builder = builder.header("content-type", "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    let body = body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
    builder.body(body).expect("test request builds")
}

async fn round_trip(app: axum::Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.oneshot(request).await.expect("route responds");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    if bytes.is_empty() {
        return (status, headers, Value::Null);
    }
    let parsed: Value = serde_json::from_slice(&bytes).expect("response JSON parses");
    (status, headers, parsed)
}

fn session_cookie(headers: &HeaderMap) -> String {
    let set_cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("login sets a cookie");
    let pair = set_cookie.split(';').next().expect("cookie pair");
    let token = pair
        .strip_prefix("session=")
        .expect("session cookie by name");
    assert!(!token.is_empty(), "opaque token present");
    pair.to_owned()
}

/// Register and challenge one synthetic user (once per phone).
async fn provision(app: &axum::Router, phone: &str) {
    let (status, _, _) = round_trip(
        app.clone(),
        json_request(
            "POST",
            "/api/v1/accounts",
            Some(json!({
                "display_name": "Auth User",
                "phone": phone,
                "city": "Campinas",
                "region": "SP",
                "policy_version": "v1",
            })),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = round_trip(
        app.clone(),
        json_request(
            "POST",
            "/api/v1/accounts/challenges",
            Some(json!({"phone": phone})),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

/// Log in one provisioned phone; repeatable (idempotent completion).
/// Returns the `Cookie` header value plus the raw `Set-Cookie` value.
async fn login_cookie(app: &axum::Router, phone: &str) -> (String, String) {
    let (status, headers, body) = round_trip(
        app.clone(),
        json_request(
            "POST",
            "/api/v1/sessions",
            Some(json!({"phone": phone, "code": FAKE_CODE})),
            None,
            Some(ORIGIN),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["account_state"], "active");
    assert!(
        !body.to_string().contains("session="),
        "no token in the body"
    );
    let set_cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("login sets a cookie")
        .to_owned();
    (session_cookie(&headers), set_cookie)
}

#[tokio::test]
async fn unauthenticated_expired_and_forged_sessions_are_refused() {
    let db = TestDatabase::create("p04t04_auth")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t04_auth_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db, Some(ORIGIN));

    // Missing and forged sessions are refused without detail.
    for cookie in [
        None,
        Some("session=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
    ] {
        let (status, _, body) = round_trip(
            app.clone(),
            json_request("GET", "/api/v1/sessions/current", None, cookie, None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthenticated");
    }
    // Caller-supplied identity headers buy nothing.
    let mut smuggled = json_request(
        "GET",
        "/api/v1/sessions/current",
        None,
        Some("session=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
        None,
    );
    smuggled.headers_mut().insert(
        "x-user-id",
        HeaderValue::from_static("123e4567-e89b-42d3-a456-426614174001"),
    );
    let (status, _, _) = round_trip(app.clone(), smuggled).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A live session resolves, slides its idle expiry, and then expires.
    provision(&app, "+5511911111111").await;
    let (cookie, _) = login_cookie(&app, "+5511911111111").await;
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT expires_at FROM sessions")
            .fetch_one(db.pool())
            .await
            .expect("expiry reads");
    let (status, _, body) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["account_state"], "active");
    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT expires_at FROM sessions")
            .fetch_one(db.pool())
            .await
            .expect("expiry reads");
    assert!(after >= before, "idle expiry slides forward");

    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 minute'")
        .execute(db.pool())
        .await
        .expect("session expires");
    let (status, _, body) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthenticated");

    // The absolute bound holds even with a fresh idle expiry.
    let (cookie, _) = login_cookie(&app, "+5511911111111").await;
    sqlx::query("UPDATE users SET created_at = now() - interval '8 days'")
        .execute(db.pool())
        .await
        .expect("account ages out");
    sqlx::query("UPDATE sessions SET expires_at = now() + interval '1 day'")
        .execute(db.pool())
        .await
        .expect("idle expiry refreshes");
    let (status, _, _) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn wrong_origin_or_missing_csrf_fails_unsafe_actions() {
    let db = TestDatabase::create("p04t04_csrf")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t04_csrf_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db, Some(ORIGIN));
    provision(&app, "+5511922222222").await;
    let (cookie, _) = login_cookie(&app, "+5511922222222").await;

    // Unsafe actions without an origin, or with a foreign one, fail closed.
    for origin in [None, Some("https://evil.test")] {
        for (method, path) in [
            ("POST", "/api/v1/sessions"),
            ("DELETE", "/api/v1/sessions/current"),
        ] {
            let body =
                (method == "POST").then(|| json!({"phone": "+5511922222222", "code": FAKE_CODE}));
            let (status, _, body) = round_trip(
                app.clone(),
                json_request(method, path, body, Some(&cookie), origin),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body["code"], "forbidden_origin");
        }
    }
    // The refused logout kept the session alive.
    let (status, _, _) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Referer fallback and exact-origin requests pass; logout revokes.
    let referer = Request::builder()
        .method("DELETE")
        .uri("/api/v1/sessions/current")
        .header("cookie", &cookie)
        .header("referer", "http://app.test/some/page?x=1")
        .body(Body::empty())
        .expect("test request builds");
    let (status, headers, _) = round_trip(app.clone(), referer).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        headers
            .get("set-cookie")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("Max-Age=0")),
        "logout clears the cookie"
    );
    let (status, _, _) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Safe reads never demand an origin; Secure tracks the https origin.
    let (cookie, _) = login_cookie(&app, "+5511922222222").await;
    let (status, _, _) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let secure_app = test_app(&db, Some("https://app.test"));
    let (status, headers, _) = round_trip(
        secure_app.clone(),
        json_request(
            "POST",
            "/api/v1/sessions",
            Some(json!({"phone": "+5511922222222", "code": FAKE_CODE})),
            None,
            Some("https://app.test"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get("set-cookie")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("; Secure")),
        "https origins secure the cookie"
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspended_account_cannot_write_with_previous_cookie() {
    let db = TestDatabase::create("p04t04_suspended")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t04_suspended_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db, Some(ORIGIN));
    provision(&app, "+5511933333333").await;
    let (cookie, _) = login_cookie(&app, "+5511933333333").await;
    let (status, _, _) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Suspension (then deletion) takes effect immediately on old cookies.
    for state in ["suspended", "banned"] {
        sqlx::query("UPDATE users SET state = $1 WHERE state = 'active'")
            .bind(state)
            .execute(db.pool())
            .await
            .expect("restriction applies");
        let (status, _, body) = round_trip(
            app.clone(),
            json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthenticated");
        let (status, _, _) = round_trip(
            app.clone(),
            json_request(
                "DELETE",
                "/api/v1/sessions/current",
                None,
                Some(&cookie),
                Some(ORIGIN),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "no privilege retained");
    }
    sqlx::query("UPDATE users SET state = 'deleted', deleted_at = now() WHERE state = 'banned'")
        .execute(db.pool())
        .await
        .expect("deletion applies");
    let (status, _, _) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Restriction preserves the record; it never resurrects access.
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(db.pool())
        .await
        .expect("records read");
    assert_eq!(users, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn session_surfaces_carry_no_phone_material() {
    let db = TestDatabase::create("p04t04_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t04_privacy_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db, Some(ORIGIN));
    provision(&app, "+5511944444444").await;
    let (cookie, set_cookie) = login_cookie(&app, "+5511944444444").await;
    let (status, _, body) = round_trip(
        app.clone(),
        json_request("GET", "/api/v1/sessions/current", None, Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.to_string().contains("55119"));
    for marker in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age="] {
        assert!(set_cookie.contains(marker), "cookie carries {marker}");
    }
    assert!(!set_cookie.contains("55119"));
    // Wrong-code login bodies stay generic and phone-free.
    let (status, _, body) = round_trip(
        app.clone(),
        json_request(
            "POST",
            "/api/v1/sessions",
            Some(json!({"phone": "+5511944444444", "code": "000000"})),
            None,
            Some(ORIGIN),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "verification_failed");
    assert!(!body.to_string().contains("55119"));
    db.cleanup().await.expect("suite cleans up");
}
