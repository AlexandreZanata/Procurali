//! Abuse-guard acceptance against real PostgreSQL (P04-T05).
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Exercises the guarded challenge flows with a counting fake
//! provider over real PostgreSQL 18.6. Proves:
//! - the last allowed concurrent attempt has only permitted winners;
//! - resend-before-minimum and over-attempts refuse without extra provider
//!   calls;
//! - provider failures consume no allowance and leave zero rows;
//! - compiled thresholds match the frozen contract.
//!
//! All phones, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::auth_limits::{
    check_code, request_challenge, AbuseLimits, CheckGuardError, GuardedCheck, GuardedStart,
    StartGuardError,
};
use procurali_backend::application::phone_verification::{
    CheckOutcome, FakeVerifyProvider, StartOutcome, VerificationError, VerificationProvider,
};
use procurali_backend::application::register_user::CHALLENGE_WINDOW;
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const FAKE_CODE: &str = "135790";
const WINDOW: Duration = Duration::from_secs(300);

fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p04t05-test-only-lookup-key",
        encryption_key: "p04t05-test-only-encryption-key",
    }
}

/// Fake provider counting every delegated start and check.
struct CountingProvider {
    inner: FakeVerifyProvider,
    starts: AtomicUsize,
    checks: AtomicUsize,
}

impl CountingProvider {
    fn new() -> Self {
        Self {
            inner: FakeVerifyProvider::for_tests(FAKE_CODE),
            starts: AtomicUsize::new(0),
            checks: AtomicUsize::new(0),
        }
    }
}

impl VerificationProvider for CountingProvider {
    async fn start_verification(&self, phone: &str) -> Result<StartOutcome, VerificationError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.inner.start_verification(phone).await
    }

    async fn check_verification(
        &self,
        phone: &str,
        code: &str,
    ) -> Result<CheckOutcome, VerificationError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.inner.check_verification(phone, code).await
    }
}

/// Provider that always fails: every call times out.
struct FailingProvider;

impl VerificationProvider for FailingProvider {
    async fn start_verification(&self, _phone: &str) -> Result<StartOutcome, VerificationError> {
        Err(VerificationError::Timeout)
    }

    async fn check_verification(
        &self,
        _phone: &str,
        _code: &str,
    ) -> Result<CheckOutcome, VerificationError> {
        Err(VerificationError::Timeout)
    }
}

async fn provision_pending(pool: &sqlx::PgPool, phone: &str) {
    let mut tx = pool.begin().await.expect("transaction begins");
    create_user(
        &mut tx,
        NewUser {
            display_name: "Limit User".to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
            policy_version: "v1".to_owned(),
            policy_accepted_at: chrono::Utc::now(),
            phone: phone.to_owned(),
        },
        test_keys(),
    )
    .await
    .expect("pending user registers");
    tx.commit().await.expect("provision commits");
}

async fn challenge_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM phone_challenges")
        .fetch_one(pool)
        .await
        .expect("count reads")
}

async fn challenge_attempts(pool: &sqlx::PgPool) -> i32 {
    sqlx::query_scalar("SELECT attempts FROM phone_challenges ORDER BY created_at DESC LIMIT 1")
        .fetch_one(pool)
        .await
        .expect("attempts read")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn last_allowed_concurrent_start_has_only_permitted_winners() {
    let db = TestDatabase::create("p04t05_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t05_race_"),
        "known suite identity in the database name"
    );
    provision_pending(db.pool(), "+5511911111111").await;
    let provider = Arc::new(CountingProvider::new());
    let limits = AbuseLimits {
        max_starts_per_hour: 3,
        resend_minimum_secs: 0,
        max_attempts_per_challenge: 5,
    };
    let barrier = Arc::new(tokio::sync::Barrier::new(6));
    let attempt = |barrier: Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        let provider = Arc::clone(&provider);
        async move {
            barrier.wait().await;
            request_challenge(
                &pool,
                &*provider,
                "+5511911111111",
                test_keys().lookup_key,
                WINDOW,
                limits,
            )
            .await
        }
    };
    let outcomes = tokio::join!(
        attempt(Arc::clone(&barrier)),
        attempt(Arc::clone(&barrier)),
        attempt(Arc::clone(&barrier)),
        attempt(Arc::clone(&barrier)),
        attempt(Arc::clone(&barrier)),
        attempt(Arc::clone(&barrier)),
    );
    let outcomes = [
        outcomes.0, outcomes.1, outcomes.2, outcomes.3, outcomes.4, outcomes.5,
    ];
    let sent = outcomes
        .iter()
        .filter(|outcome| **outcome == Ok(GuardedStart::Sent))
        .count();
    let limited = outcomes
        .iter()
        .filter(|outcome| **outcome == Ok(GuardedStart::RateLimited))
        .count();
    assert_eq!(sent, 3, "exactly the allowed winners send");
    assert_eq!(limited, 3, "the rest refuse without provider calls");
    assert_eq!(provider.starts.load(Ordering::SeqCst), 3);
    assert_eq!(challenge_count(db.pool()).await, 3);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn resend_minimum_and_over_attempts_refuse_without_provider_call() {
    let db = TestDatabase::create("p04t05_bounds")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t05_bounds_"),
        "known suite identity in the database name"
    );
    provision_pending(db.pool(), "+5511922222222").await;
    let provider = CountingProvider::new();
    let limits = AbuseLimits::DEFAULT;

    // First send anchors; the immediate resend refuses with no extra call.
    assert_eq!(
        request_challenge(
            db.pool(),
            &provider,
            "+5511922222222",
            test_keys().lookup_key,
            WINDOW,
            limits
        )
        .await,
        Ok(GuardedStart::Sent)
    );
    assert_eq!(
        request_challenge(
            db.pool(),
            &provider,
            "+5511922222222",
            test_keys().lookup_key,
            WINDOW,
            limits
        )
        .await,
        Ok(GuardedStart::RateLimited)
    );
    assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
    assert_eq!(challenge_count(db.pool()).await, 1);

    // Five wrong codes delegate; the sixth (even correct) refuses silently.
    for _ in 0..5 {
        assert_eq!(
            check_code(
                db.pool(),
                &provider,
                "+5511922222222",
                "000000",
                test_keys().lookup_key,
                limits
            )
            .await,
            Ok(GuardedCheck::Incorrect)
        );
    }
    assert_eq!(provider.checks.load(Ordering::SeqCst), 5);
    assert_eq!(
        check_code(
            db.pool(),
            &provider,
            "+5511922222222",
            FAKE_CODE,
            test_keys().lookup_key,
            limits
        )
        .await,
        Ok(GuardedCheck::RateLimited)
    );
    assert_eq!(
        provider.checks.load(Ordering::SeqCst),
        5,
        "no extra provider call"
    );
    assert_eq!(challenge_attempts(db.pool()).await, 6);

    // The hourly cap refuses sequentially too, with no extra provider call.
    provision_pending(db.pool(), "+5511933333333").await;
    let capped = AbuseLimits {
        max_starts_per_hour: 2,
        resend_minimum_secs: 0,
        max_attempts_per_challenge: 5,
    };
    assert_eq!(
        request_challenge(
            db.pool(),
            &provider,
            "+5511933333333",
            test_keys().lookup_key,
            WINDOW,
            capped
        )
        .await,
        Ok(GuardedStart::Sent)
    );
    assert_eq!(
        request_challenge(
            db.pool(),
            &provider,
            "+5511933333333",
            test_keys().lookup_key,
            WINDOW,
            capped
        )
        .await,
        Ok(GuardedStart::Sent)
    );
    assert_eq!(
        request_challenge(
            db.pool(),
            &provider,
            "+5511933333333",
            test_keys().lookup_key,
            WINDOW,
            capped
        )
        .await,
        Ok(GuardedStart::RateLimited)
    );
    assert_eq!(provider.starts.load(Ordering::SeqCst), 3);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn provider_failure_consumes_no_allowance() {
    let db = TestDatabase::create("p04t05_failures")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t05_failures_"),
        "known suite identity in the database name"
    );
    provision_pending(db.pool(), "+5511944444444").await;
    let failing = FailingProvider;
    let limits = AbuseLimits::DEFAULT;

    // Failed starts leave zero rows and the allowance intact.
    assert_eq!(
        request_challenge(
            db.pool(),
            &failing,
            "+5511944444444",
            test_keys().lookup_key,
            WINDOW,
            limits
        )
        .await,
        Err(StartGuardError::ProviderFailed)
    );
    assert_eq!(challenge_count(db.pool()).await, 0);
    let working = CountingProvider::new();
    assert_eq!(
        request_challenge(
            db.pool(),
            &working,
            "+5511944444444",
            test_keys().lookup_key,
            WINDOW,
            limits
        )
        .await,
        Ok(GuardedStart::Sent)
    );
    assert_eq!(working.starts.load(Ordering::SeqCst), 1);

    // Failed checks surface explicitly; the spent attempt is accounted.
    assert_eq!(
        check_code(
            db.pool(),
            &failing,
            "+5511944444444",
            FAKE_CODE,
            test_keys().lookup_key,
            limits
        )
        .await,
        Err(CheckGuardError::ProviderFailed)
    );
    assert_eq!(
        check_code(
            db.pool(),
            &working,
            "+5511944444444",
            "000000",
            test_keys().lookup_key,
            limits
        )
        .await,
        Ok(GuardedCheck::Incorrect)
    );
    assert_eq!(working.checks.load(Ordering::SeqCst), 1);
    assert_eq!(challenge_attempts(db.pool()).await, 2);
    db.cleanup().await.expect("suite cleans up");
}

#[test]
fn compiled_thresholds_match_the_frozen_contract() {
    let text = std::fs::read_to_string("../contracts/auth-cases.json")
        .expect("frozen auth cases are readable");
    let contract: serde_json::Value = serde_json::from_str(&text).expect("contract parses");
    let thresholds = &contract["thresholds"];
    assert_eq!(
        AbuseLimits::DEFAULT.max_starts_per_hour,
        thresholds["challenge_starts_per_phone_per_hour"]["allowed"]
            .as_u64()
            .expect("hourly starts") as u32
    );
    assert_eq!(
        AbuseLimits::DEFAULT.resend_minimum_secs,
        thresholds["resend_minimum_seconds"]["value"]
            .as_u64()
            .expect("resend gap")
    );
    assert_eq!(
        AbuseLimits::DEFAULT.max_attempts_per_challenge,
        thresholds["confirmation_attempts_per_challenge"]["allowed"]
            .as_u64()
            .expect("attempts") as u32
    );
    assert_eq!(CHALLENGE_WINDOW.as_secs(), 300);
    assert_eq!(
        thresholds["challenge_window_minutes"]["value"]
            .as_u64()
            .expect("window"),
        5
    );
}
