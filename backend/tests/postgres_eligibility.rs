//! Eligibility-guard acceptance against real PostgreSQL (P04-T10).
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Seeds real identity rows plus synthetic block rows directly
//! and proves the guarded reads over real PostgreSQL 18.6:
//! - an empty real block ledger permits otherwise eligible pairs;
//! - either-direction fixtures prevent offer/contact eligibility;
//! - suspended, banned, and deleted states are denied even with previously
//!   valid sessions standing by.
//!
//! All names, numbers, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::eligibility::{
    check_actor, check_pair, CheckOutcome, RefusalReason,
};
use procurali_backend::persistence::users::{create_session, create_user, NewUser, PhoneKeys};

fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p04t10-test-only-lookup-key",
        encryption_key: "p04t10-test-only-encryption-key",
    }
}

async fn provision_active(pool: &sqlx::PgPool, phone: &str, name: &str) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let user = create_user(
        &mut tx,
        NewUser {
            display_name: name.to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
            policy_version: "v1".to_owned(),
            policy_accepted_at: chrono::Utc::now(),
            phone: phone.to_owned(),
        },
        test_keys(),
    )
    .await
    .expect("user registers");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .expect("activation applies");
    tx.commit().await.expect("provision commits");
    user.id
}

async fn open_session(pool: &sqlx::PgPool, user_id: uuid::Uuid, token: &str) {
    let mut tx = pool.begin().await.expect("transaction begins");
    create_session(
        &mut tx,
        user_id,
        token,
        chrono::Utc::now() + chrono::Duration::days(1),
    )
    .await
    .expect("session records");
    tx.commit().await.expect("session commits");
}

async fn seed_block(pool: &sqlx::PgPool, blocker: uuid::Uuid, blocked: uuid::Uuid) {
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)")
        .bind(blocker)
        .bind(blocked)
        .execute(pool)
        .await
        .expect("synthetic block seeds");
}

#[tokio::test]
async fn empty_real_block_ledger_permits_eligible_pair() {
    let db = TestDatabase::create("p04t10_empty")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t10_empty_"),
        "known suite identity in the database name"
    );
    let first = provision_active(db.pool(), "+5511911111111", "First").await;
    let second = provision_active(db.pool(), "+5511922222222", "Second").await;
    assert_eq!(
        check_actor(db.pool(), first).await.expect("actor checks"),
        CheckOutcome::Permitted
    );
    assert_eq!(
        check_actor(db.pool(), second).await.expect("actor checks"),
        CheckOutcome::Permitted
    );
    assert_eq!(
        check_pair(db.pool(), first, second)
            .await
            .expect("pair checks"),
        CheckOutcome::Permitted
    );
    assert_eq!(
        check_pair(db.pool(), second, first)
            .await
            .expect("pair checks"),
        CheckOutcome::Permitted
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn either_direction_block_prevents_eligibility() {
    let db = TestDatabase::create("p04t10_blocked")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t10_blocked_"),
        "known suite identity in the database name"
    );
    let first = provision_active(db.pool(), "+5511933333333", "First").await;
    let second = provision_active(db.pool(), "+5511944444444", "Second").await;
    let third = provision_active(db.pool(), "+5511955555555", "Third").await;
    // One directed row blocks the pair both ways; uninvolved pairs pass.
    seed_block(db.pool(), first, second).await;
    for (viewer, other) in [(first, second), (second, first)] {
        assert_eq!(
            check_pair(db.pool(), viewer, other)
                .await
                .expect("pair checks"),
            CheckOutcome::Refused(RefusalReason::BlockedRelationship)
        );
    }
    assert_eq!(
        check_pair(db.pool(), first, third)
            .await
            .expect("pair checks"),
        CheckOutcome::Permitted
    );
    assert_eq!(
        check_pair(db.pool(), third, second)
            .await
            .expect("pair checks"),
        CheckOutcome::Permitted
    );
    // The reverse direction alone blocks just as firmly.
    seed_block(db.pool(), third, first).await;
    assert_eq!(
        check_pair(db.pool(), first, third)
            .await
            .expect("pair checks"),
        CheckOutcome::Refused(RefusalReason::BlockedRelationship)
    );
    assert_eq!(
        check_pair(db.pool(), third, first)
            .await
            .expect("pair checks"),
        CheckOutcome::Refused(RefusalReason::BlockedRelationship)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn restricted_states_denied_despite_valid_session() {
    let db = TestDatabase::create("p04t10_restricted")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t10_restricted_"),
        "known suite identity in the database name"
    );
    let steady = provision_active(db.pool(), "+5511966666666", "Steady").await;
    let shaky = provision_active(db.pool(), "+5511977777777", "Shaky").await;
    open_session(db.pool(), shaky, "synthetic-session-token-p04t10").await;

    // Suspension, ban, and deletion each deny, while the session row stands by.
    for state in ["suspended", "banned"] {
        sqlx::query("UPDATE users SET state = $1 WHERE id = $2")
            .bind(state)
            .bind(shaky)
            .execute(db.pool())
            .await
            .expect("restriction applies");
        assert_eq!(
            check_actor(db.pool(), shaky).await.expect("actor checks"),
            CheckOutcome::Refused(RefusalReason::AccountNotActive)
        );
        assert_eq!(
            check_pair(db.pool(), shaky, steady)
                .await
                .expect("pair checks"),
            CheckOutcome::Refused(RefusalReason::AccountNotActive)
        );
        assert_eq!(
            check_pair(db.pool(), steady, shaky)
                .await
                .expect("pair checks"),
            CheckOutcome::Refused(RefusalReason::AccountNotActive)
        );
    }
    sqlx::query("UPDATE users SET state = 'deleted', deleted_at = now() WHERE id = $1")
        .bind(shaky)
        .execute(db.pool())
        .await
        .expect("deletion applies");
    assert_eq!(
        check_actor(db.pool(), shaky).await.expect("actor checks"),
        CheckOutcome::Refused(RefusalReason::AccountNotActive)
    );
    assert_eq!(
        check_pair(db.pool(), steady, shaky)
            .await
            .expect("pair checks"),
        CheckOutcome::Refused(RefusalReason::AccountNotActive)
    );
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(db.pool())
        .await
        .expect("sessions read");
    assert_eq!(sessions, 1, "denial reads state, never wipes sessions");
    // Missing accounts share the indistinguishable refusal.
    assert_eq!(
        check_actor(db.pool(), uuid::Uuid::nil())
            .await
            .expect("actor checks"),
        CheckOutcome::Refused(RefusalReason::AccountNotActive)
    );
    db.cleanup().await.expect("suite cleans up");
}
