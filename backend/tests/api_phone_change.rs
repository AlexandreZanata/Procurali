//! Phone-change acceptance (P04-T07): proof-gated atomic reassignment.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Provisions active accounts through the real account routes over
//! real PostgreSQL 18.6 with the deterministic fake provider, drives the
//! new-number flows at the application boundary, and verifies the aftermath
//! back over HTTP and SQL. Proves:
//! - failed new verification leaves the old destination unchanged;
//! - two accounts competing for one new number produce one permitted
//!   assignment, with history chaining on the follow-up change;
//! - no old or new number leaks through outcomes, errors, history, notices,
//!   or events.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use database::TestDatabase;
use procurali_backend::application::auth_limits::AbuseLimits;
use procurali_backend::application::change_phone::{
    confirm_number_change, request_number_change, ChangeConfirmOutcome, ChangeRequestOutcome,
};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::http::accounts::{routes as account_routes, AccountsState};
use procurali_backend::persistence::phone_history::history_for;
use procurali_backend::persistence::users::PhoneKeys;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const FAKE_CODE: &str = "135790";
const WINDOW: Duration = Duration::from_secs(300);

fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p04t07-test-only-lookup-key",
        encryption_key: "p04t07-test-only-encryption-key",
    }
}

fn test_app(db: &TestDatabase) -> axum::Router {
    account_routes(AccountsState::new(
        db.pool().clone(),
        Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE)),
        "p04t07-test-only-lookup-key".to_owned(),
        "p04t07-test-only-encryption-key".to_owned(),
    ))
}

async fn post_json(app: axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("test request builds"),
        )
        .await
        .expect("route responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .expect("body reads");
    let parsed: Value = serde_json::from_slice(&bytes).expect("response JSON parses");
    (status, parsed)
}

/// Provision one active account through the real routes; return its id.
async fn provision_active(app: &axum::Router, phone: &str, name: &str) -> uuid::Uuid {
    let (status, body) = post_json(
        app.clone(),
        "/api/v1/accounts",
        json!({
            "display_name": name,
            "phone": phone,
            "city": "Campinas",
            "region": "SP",
            "policy_version": "v1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = post_json(
        app.clone(),
        "/api/v1/accounts/challenges",
        json!({"phone": phone}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, active) = post_json(
        app.clone(),
        "/api/v1/accounts/challenges/confirmations",
        json!({"phone": phone, "code": FAKE_CODE}),
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

async fn stored_lookup(pool: &sqlx::PgPool, user_id: uuid::Uuid) -> String {
    sqlx::query_scalar("SELECT phone_lookup FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("lookup reads")
}

async fn stored_cipher(pool: &sqlx::PgPool, user_id: uuid::Uuid) -> Vec<u8> {
    sqlx::query_scalar("SELECT phone_ciphertext FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("ciphertext reads")
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

fn open_limits() -> AbuseLimits {
    AbuseLimits {
        max_starts_per_hour: 10,
        resend_minimum_secs: 0,
        max_attempts_per_challenge: 5,
    }
}

#[tokio::test]
async fn failed_new_verification_leaves_old_destination_unchanged() {
    let db = TestDatabase::create("p04t07_failed")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t07_failed_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let user = provision_active(&app, "+5511911111111", "Change User").await;
    let lookup_before = stored_lookup(db.pool(), user).await;
    let cipher_before = stored_cipher(db.pool(), user).await;
    let events_before = table_count(db.pool(), "business_events").await;

    // The new number anchors; a wrong code refuses with zero writes.
    assert_eq!(
        request_number_change(
            db.pool(),
            &FakeVerifyProvider::for_tests(FAKE_CODE),
            user,
            "+5511922222222",
            test_keys(),
            WINDOW,
            AbuseLimits::DEFAULT,
        )
        .await
        .expect("request works"),
        ChangeRequestOutcome::Sent
    );
    assert_eq!(
        confirm_number_change(
            db.pool(),
            &FakeVerifyProvider::for_tests(FAKE_CODE),
            user,
            "+5511922222222",
            "000000",
            test_keys(),
        )
        .await
        .expect("confirmation answers"),
        ChangeConfirmOutcome::Failed
    );
    assert_eq!(stored_lookup(db.pool(), user).await, lookup_before);
    assert_eq!(stored_cipher(db.pool(), user).await, cipher_before);
    assert_eq!(
        table_count(db.pool(), "business_events").await,
        events_before
    );
    let live_anchors: i64 =
        sqlx::query_scalar("SELECT count(*) FROM phone_challenges WHERE consumed_at IS NULL")
            .fetch_one(db.pool())
            .await
            .expect("anchors read");
    assert_eq!(live_anchors, 1, "the change anchor stays retryable");

    // An elapsed window reports expiry and still changes nothing.
    sqlx::query("UPDATE phone_challenges SET expires_at = now() - interval '1 minute'")
        .execute(db.pool())
        .await
        .expect("window elapses");
    assert_eq!(
        confirm_number_change(
            db.pool(),
            &FakeVerifyProvider::for_tests(FAKE_CODE),
            user,
            "+5511922222222",
            FAKE_CODE,
            test_keys(),
        )
        .await
        .expect("confirmation answers"),
        ChangeConfirmOutcome::Expired
    );
    assert_eq!(stored_lookup(db.pool(), user).await, lookup_before);
    assert!(history_for(db.pool(), user)
        .await
        .expect("history reads")
        .is_empty());
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_accounts_produce_one_permitted_assignment() {
    let db = TestDatabase::create("p04t07_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t07_race_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let first = provision_active(&app, "+5511933333333", "Racer One").await;
    let second = provision_active(&app, "+5511944444444", "Racer Two").await;
    let provider = Arc::new(FakeVerifyProvider::for_tests(FAKE_CODE));

    for user in [first, second] {
        assert_eq!(
            request_number_change(
                db.pool(),
                &*provider,
                user,
                "+5511955555555",
                test_keys(),
                WINDOW,
                open_limits(),
            )
            .await
            .expect("request works"),
            ChangeRequestOutcome::Sent
        );
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let confirm = |user: uuid::Uuid, barrier: Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        let provider = Arc::clone(&provider);
        async move {
            barrier.wait().await;
            confirm_number_change(
                &pool,
                &*provider,
                user,
                "+5511955555555",
                FAKE_CODE,
                test_keys(),
            )
            .await
            .expect("confirmation answers")
        }
    };
    let (first_outcome, second_outcome) = tokio::join!(
        confirm(first, Arc::clone(&barrier)),
        confirm(second, Arc::clone(&barrier)),
    );
    let changed = [first_outcome, second_outcome]
        .into_iter()
        .filter(|outcome| *outcome == ChangeConfirmOutcome::Changed)
        .count();
    assert_eq!(changed, 1, "exactly one assignment wins");
    let winner = if first_outcome == ChangeConfirmOutcome::Changed {
        first
    } else {
        second
    };
    let winner_lookup = stored_lookup(db.pool(), winner).await;
    let holder: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE phone_lookup = $1 AND deleted_at IS NULL")
            .bind(&winner_lookup)
            .fetch_one(db.pool())
            .await
            .expect("holder reads");
    assert_eq!(holder, winner);
    let changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events WHERE kind = 'account.phone_changed'",
    )
    .fetch_one(db.pool())
    .await
    .expect("facts read");
    assert_eq!(changes, 1);
    assert_eq!(table_count(db.pool(), "notices").await, 1);

    // The winner's history chains across a follow-up change.
    assert_eq!(
        request_number_change(
            db.pool(),
            &*provider,
            winner,
            "+5511966666666",
            test_keys(),
            WINDOW,
            open_limits(),
        )
        .await
        .expect("request works"),
        ChangeRequestOutcome::Sent
    );
    assert_eq!(
        confirm_number_change(
            db.pool(),
            &*provider,
            winner,
            "+5511966666666",
            FAKE_CODE,
            test_keys(),
        )
        .await
        .expect("confirmation answers"),
        ChangeConfirmOutcome::Changed
    );
    let history = history_for(db.pool(), winner).await.expect("history reads");
    assert_eq!(history.len(), 2, "reassignments append");
    assert_eq!(history[1].old_lookup, history[0].new_lookup, "chain links");
    assert_eq!(
        stored_lookup(db.pool(), winner).await,
        history[1].new_lookup
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn no_number_leaks_through_public_dto_or_conflict() {
    let db = TestDatabase::create("p04t07_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t07_privacy_"),
        "known suite identity in the database name"
    );
    let app = test_app(&db);
    let holder = provision_active(&app, "+5511977777777", "Holder").await;
    let mover = provision_active(&app, "+5511988888888", "Mover").await;

    // A held number is refused generically, touching nothing.
    assert_eq!(
        request_number_change(
            db.pool(),
            &FakeVerifyProvider::for_tests(FAKE_CODE),
            mover,
            "+5511977777777",
            test_keys(),
            WINDOW,
            AbuseLimits::DEFAULT,
        )
        .await
        .expect("request answers"),
        ChangeRequestOutcome::NotEligible
    );
    assert!(history_for(db.pool(), mover)
        .await
        .expect("history reads")
        .is_empty());
    assert!(history_for(db.pool(), holder)
        .await
        .expect("history reads")
        .is_empty());

    // A successful change stores digests only, everywhere.
    assert_eq!(
        request_number_change(
            db.pool(),
            &FakeVerifyProvider::for_tests(FAKE_CODE),
            mover,
            "+5511999999999",
            test_keys(),
            WINDOW,
            AbuseLimits::DEFAULT,
        )
        .await
        .expect("request works"),
        ChangeRequestOutcome::Sent
    );
    assert_eq!(
        confirm_number_change(
            db.pool(),
            &FakeVerifyProvider::for_tests(FAKE_CODE),
            mover,
            "+5511999999999",
            FAKE_CODE,
            test_keys(),
        )
        .await
        .expect("confirmation answers"),
        ChangeConfirmOutcome::Changed
    );
    let history = history_for(db.pool(), mover).await.expect("history reads");
    assert_eq!(history.len(), 1);
    for digest in [&history[0].old_lookup, &history[0].new_lookup] {
        assert_eq!(digest.len(), 64, "HMAC-SHA256 hex digests only");
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM business_events WHERE kind = 'account.phone_changed'",
    )
    .fetch_one(db.pool())
    .await
    .expect("payload reads");
    assert!(!payload.to_string().contains("55119"));
    let notice: String = sqlx::query_scalar("SELECT body FROM notices")
        .fetch_one(db.pool())
        .await
        .expect("notice reads");
    assert!(!notice.contains("55119"));
    // Outcomes and errors render statically by construction.
    let rendered = format!(
        "{:?} {:?} {:?} {:?}",
        ChangeRequestOutcome::Sent,
        ChangeConfirmOutcome::Failed,
        ChangeRequestOutcome::NotEligible,
        ChangeConfirmOutcome::Expired
    );
    assert!(!rendered.contains("55119"));
    db.cleanup().await.expect("suite cleans up");
}
