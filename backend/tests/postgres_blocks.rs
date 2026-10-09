//! Pair-block acceptance (P10-T01): writes, guards, and privacy.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - either block direction prevents new pair interaction, and lifting
//!   restores it;
//! - self-blocks refuse while duplicate blocks converge idempotently;
//! - only live database truth decides, with no enumeration or leakage.
//!
//! All phones and names below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::pair_eligibility::{check_pair_interaction, PairBlockError};
use procurali_backend::persistence::blocks::{block, is_blocked, unblock, BlockError};
use procurali_backend::persistence::transaction::AttemptError;
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p10t01-test-only-lookup-key",
        encryption_key: "p10t01-test-only-encryption-key",
    }
}

/// A real active account: identity travels the real creation path with real
/// phone cryptography, and only the lifecycle flip (owned by the
/// verification flow) is staged, because guards read live standing.
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

/// Guard verdict without mutating: check inside a transaction, roll back.
async fn guard(
    pool: &sqlx::PgPool,
    first: uuid::Uuid,
    second: uuid::Uuid,
) -> Result<(), PairBlockError> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let verdict = check_pair_interaction(&mut tx, first, second).await;
    tx.rollback().await.expect("probe rolls back");
    match verdict {
        Ok(()) => Ok(()),
        Err(AttemptError::Abort(reason)) => Err(reason),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

async fn block_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM user_blocks")
        .fetch_one(pool)
        .await
        .expect("blocks read")
}

#[tokio::test]
async fn either_direction_prevents_new_pair_interaction() {
    let db = TestDatabase::create("p10t01_directions")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t01_directions_"),
        "known suite identity in the database name"
    );
    let alice = seed_active(db.pool(), "Directions Alice", "+55 11 90000-0901").await;
    let bruno = seed_active(db.pool(), "Directions Bruno", "+55 11 90000-0902").await;
    assert_eq!(guard(db.pool(), alice, bruno).await, Ok(()));

    // One direction blocks both: neither order interacts, and neither
    // refusal names a direction or a third party.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let (row, created) = block(&mut tx, alice, bruno).await.expect("block records");
    tx.commit().await.expect("block commits");
    assert!(created);
    assert_eq!((row.blocker_id, row.blocked_id), (alice, bruno));
    for (first, second) in [(alice, bruno), (bruno, alice)] {
        let verdict = guard(db.pool(), first, second).await;
        assert_eq!(verdict, Err(PairBlockError::Blocked));
        let rendered = format!("{verdict:?}");
        assert!(!rendered.contains(&alice.to_string()));
        assert!(!rendered.contains(&bruno.to_string()));
    }
    // Lifting restores exactly: removal is idempotent and truthful.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(unblock(&mut tx, alice, bruno).await.expect("unblock lifts"));
    tx.commit().await.expect("lift commits");
    assert_eq!(guard(db.pool(), alice, bruno).await, Ok(()));
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(!unblock(&mut tx, alice, bruno).await.expect("repeat lifts"));
    tx.commit().await.expect("lift commits");
    assert_eq!(block_count(db.pool()).await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn self_block_and_duplicate_behave_as_defined() {
    let db = TestDatabase::create("p10t01_defined")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t01_defined_"),
        "known suite identity in the database name"
    );
    let alice = seed_active(db.pool(), "Defined Alice", "+55 11 90000-0903").await;
    let bruno = seed_active(db.pool(), "Defined Bruno", "+55 11 90000-0904").await;

    // Self-blocks refuse with nothing stored, through validation and the
    // database backstop alike.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        block(&mut tx, alice, alice).await,
        Err(BlockError::SelfBlock)
    );
    tx.rollback().await.expect("refusal rolls back");
    let raw = sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $1)")
        .bind(alice)
        .execute(db.pool())
        .await;
    assert!(raw.is_err(), "CHECK backstop refuses self-blocks");
    assert_eq!(block_count(db.pool()).await, 0);

    // Duplicates converge: one row, first call created, second returned.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let (first, created) = block(&mut tx, alice, bruno).await.expect("block records");
    tx.commit().await.expect("block commits");
    assert!(created);
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let (second, created) = block(&mut tx, alice, bruno)
        .await
        .expect("repeat converges");
    tx.commit().await.expect("repeat commits");
    assert!(!created);
    assert_eq!(first.id, second.id);
    assert_eq!(block_count(db.pool()).await, 1);
    // Directionality is exact: only the recorded direction reads back.
    assert!(is_blocked(db.pool(), alice, bruno)
        .await
        .expect("read reads"));
    assert!(!is_blocked(db.pool(), bruno, alice)
        .await
        .expect("read reads"));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn only_live_database_truth_decides() {
    let db = TestDatabase::create("p10t01_truth")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t01_truth_"),
        "known suite identity in the database name"
    );
    let alice = seed_active(db.pool(), "Truth Alice", "+55 11 90000-0905").await;
    let bruno = seed_active(db.pool(), "Truth Bruno", "+55 11 90000-0906").await;

    // The guard takes identifiers only: no cached or client-supplied
    // relationship state exists to override anything, and every verdict
    // follows the rows as they stand right now.
    assert_eq!(guard(db.pool(), alice, bruno).await, Ok(()));
    let mut tx = db.pool().begin().await.expect("transaction begins");
    block(&mut tx, bruno, alice).await.expect("block records");
    tx.commit().await.expect("block commits");
    assert_eq!(
        guard(db.pool(), alice, bruno).await,
        Err(PairBlockError::Blocked)
    );
    let mut tx = db.pool().begin().await.expect("transaction begins");
    unblock(&mut tx, bruno, alice).await.expect("lift lifts");
    tx.commit().await.expect("lift commits");
    assert_eq!(guard(db.pool(), alice, bruno).await, Ok(()));

    // Restricted parties never reach the relationship question: account
    // state precedes it, so no block record leaks either way.
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(bruno)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        guard(db.pool(), alice, bruno).await,
        Err(PairBlockError::NotActive)
    );
    assert_eq!(
        guard(db.pool(), bruno, alice).await,
        Err(PairBlockError::NotActive)
    );
    db.cleanup().await.expect("suite cleans up");
}
