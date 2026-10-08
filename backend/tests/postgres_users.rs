//! User/phone/session persistence acceptance (P04-T01).
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL 18.6:
//! - formatting variants contend for one current phone identity (INV-07);
//! - non-deleted duplicates are refused by application and database, while
//!   deleted numbers recycle into new distinct accounts (AC-03, EC-22);
//! - stored and logged values never expose plaintext sessions or destinations.
//!
//! All keys, tokens, codes, and numbers below are synthetic and reserved;
//! nothing here is provisioned, dialed, or secret.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::persistence::users::{
    blocks_relation_exists, canonicalize_phone, consume_challenge, create_challenge,
    create_session, create_user, delete_user, find_session_by_token, find_user_by_phone,
    reveal_phone, revoke_session, NewChallenge, NewUser, PhoneKeys, UserError,
};

/// Test-only phone keys, mirroring the checked-in test-database credential
/// pattern. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p04t01-test-only-lookup-key",
        encryption_key: "p04t01-test-only-encryption-key",
    }
}

fn probe_user(display: &str, phone: &str) -> NewUser {
    NewUser {
        display_name: display.to_owned(),
        city: "Campinas".to_owned(),
        region: "SP".to_owned(),
        policy_version: "v1".to_owned(),
        policy_accepted_at: chrono::Utc::now(),
        phone: phone.to_owned(),
    }
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

async fn live_user_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE deleted_at IS NULL")
        .fetch_one(pool)
        .await
        .expect("live users read")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn formatting_variants_contend_for_one_current_phone_identity() {
    let db = TestDatabase::create("p04t01_format")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t01_format_"),
        "known suite identity in the database name"
    );
    // Same number, different formatting: all contenders race at once.
    let variants = [
        "+5511987654321",
        "+55 11 98765-4321",
        "+55(11)98765.4321",
        "  +5511987654321  ",
    ];
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(4));
    let attempt = |index: usize, barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            let mut tx = pool.begin().await.expect("transaction begins");
            let outcome = create_user(
                &mut tx,
                probe_user(&format!("Format User {index}"), variants[index]),
                test_keys(),
            )
            .await;
            match outcome {
                Ok(user) => {
                    tx.commit().await.expect("winner commits");
                    Ok(user.id)
                }
                Err(UserError::DuplicatePhone) => {
                    tx.rollback().await.expect("loser rolls back");
                    Err(UserError::DuplicatePhone)
                }
                Err(other) => panic!("unexpected identity error: {other:?}"),
            }
        }
    };
    let (first, second, third, fourth) = tokio::join!(
        attempt(0, std::sync::Arc::clone(&barrier)),
        attempt(1, std::sync::Arc::clone(&barrier)),
        attempt(2, std::sync::Arc::clone(&barrier)),
        attempt(3, std::sync::Arc::clone(&barrier)),
    );
    let wins: Vec<uuid::Uuid> = [first, second, third, fourth]
        .into_iter()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(wins.len(), 1, "exactly one formatting wins the identity");
    assert_eq!(live_user_count(db.pool()).await, 1);
    assert_eq!(table_count(db.pool(), "users").await, 1);
    // Every variant resolves to the single holder.
    for variant in variants {
        let holder = find_user_by_phone(db.pool(), variant, test_keys().lookup_key)
            .await
            .expect("lookup queries")
            .expect("every variant resolves");
        assert_eq!(holder.id, wins[0]);
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn non_deleted_duplicate_rejected_by_app_and_database() {
    let db = TestDatabase::create("p04t01_duplicate")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t01_duplicate_"),
        "known suite identity in the database name"
    );
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let holder = create_user(
        &mut tx,
        probe_user("Holder One", "+5511911111111"),
        test_keys(),
    )
    .await
    .expect("first holder registers");
    tx.commit().await.expect("holder commits");

    // Application-level refusal for a live number.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let refused = create_user(
        &mut tx,
        probe_user("Holder Two", "+55 11 91111-1111"),
        test_keys(),
    )
    .await
    .expect_err("live number refused at the application");
    assert_eq!(refused, UserError::DuplicatePhone);
    tx.rollback().await.expect("refused attempt rolls back");

    // Database-level refusal for the same live lookup, bypassing the check.
    let canonical = canonicalize_phone("+5511911111111").expect("shapes");
    let conflict: Result<_, sqlx::Error> = sqlx::query(
        "INSERT INTO users
            (display_name, city, region, policy_version, policy_accepted_at,
             phone_ciphertext, phone_lookup)
         VALUES ('Bypass', 'Campinas', 'SP', 'v1', now(),
                 pgp_sym_encrypt($1, 'p04t01-test-only-encryption-key'),
                 encode(hmac(convert_to($1, 'UTF8'),
                             convert_to('p04t01-test-only-lookup-key', 'UTF8'),
                             'sha256'), 'hex'))",
    )
    .bind(&canonical)
    .execute(db.pool())
    .await;
    let conflict = conflict.expect_err("live number refused by the database");
    assert_eq!(
        conflict
            .as_database_error()
            .and_then(|database| database.code())
            .as_deref(),
        Some("23505"),
        "partial unique index guards live numbers"
    );
    assert_eq!(live_user_count(db.pool()).await, 1);

    // Deletion releases the number: a new account recycles it distinctly.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(
        delete_user(&mut tx, holder.id)
            .await
            .expect("deletion queries"),
        "holder deletion transitions"
    );
    assert!(
        !delete_user(&mut tx, holder.id)
            .await
            .expect("deletion queries"),
        "double deletion is not a second transition"
    );
    tx.commit().await.expect("deletion commits");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let recycled = create_user(
        &mut tx,
        probe_user("Holder Next", "+5511911111111"),
        test_keys(),
    )
    .await
    .expect("recycled number registers anew");
    tx.commit().await.expect("recycled registration commits");
    assert_ne!(recycled.id, holder.id, "distinct account identity");
    assert_eq!(table_count(db.pool(), "users").await, 2);
    assert_eq!(live_user_count(db.pool()).await, 1);
    let current = find_user_by_phone(db.pool(), "+5511911111111", test_keys().lookup_key)
        .await
        .expect("lookup queries")
        .expect("lookup resolves the new holder");
    assert_eq!(current.id, recycled.id);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn stored_and_logged_values_expose_no_plaintext() {
    let db = TestDatabase::create("p04t01_secrets")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t01_secrets_"),
        "known suite identity in the database name"
    );
    let phone = "+5511922222222";
    let token = "synthetic-session-token-p04t01-aaa";
    let code = "synthetic-code-135790";
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let user = create_user(&mut tx, probe_user("Secret User", phone), test_keys())
        .await
        .expect("user registers");
    let session = create_session(
        &mut tx,
        user.id,
        token,
        chrono::Utc::now() + chrono::Duration::days(1),
    )
    .await
    .expect("session records");
    create_challenge(
        &mut tx,
        NewChallenge {
            phone: phone.to_owned(),
            code: code.to_owned(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        },
        test_keys().lookup_key,
    )
    .await
    .expect("challenge records");
    tx.commit().await.expect("identity unit commits");

    // Stored ciphertext, lookups, and digests never contain the plaintext.
    let stored_cipher: Vec<u8> =
        sqlx::query_scalar("SELECT phone_ciphertext FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(db.pool())
            .await
            .expect("ciphertext reads");
    assert!(
        !stored_cipher
            .windows(phone.len())
            .any(|window| window == phone.as_bytes()),
        "ciphertext holds no plaintext destination"
    );
    let stored_lookup: String = sqlx::query_scalar("SELECT phone_lookup FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(db.pool())
        .await
        .expect("lookup reads");
    assert_ne!(stored_lookup, phone);
    assert!(!stored_lookup.contains("119222"));
    let session_digest: String =
        sqlx::query_scalar("SELECT session_digest FROM sessions WHERE id = $1")
            .bind(session.id)
            .fetch_one(db.pool())
            .await
            .expect("session digest reads");
    assert!(!session_digest.contains(token));
    let challenge_digest: String =
        sqlx::query_scalar("SELECT challenge_digest FROM phone_challenges")
            .fetch_one(db.pool())
            .await
            .expect("challenge digest reads");
    assert!(!challenge_digest.contains(code));

    // The key still opens the destination (round trip), a wrong key does not.
    let revealed = reveal_phone(db.pool(), user.id, test_keys().encryption_key)
        .await
        .expect("correct key reveals");
    assert_eq!(revealed, "+5511922222222");
    reveal_phone(db.pool(), user.id, "wrong-test-only-key")
        .await
        .expect_err("wrong key reveals nothing");

    // Sessions verify by digest and revoke cleanly; challenges consume once.
    let found = find_session_by_token(db.pool(), token)
        .await
        .expect("session lookup queries")
        .expect("live token resolves");
    assert_eq!(found.id, session.id);
    assert!(
        find_session_by_token(db.pool(), "wrong-token")
            .await
            .expect("session lookup queries")
            .is_none(),
        "unknown tokens resolve to nothing"
    );
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(
        consume_challenge(&mut tx, phone, code, test_keys().lookup_key)
            .await
            .expect("challenge consume queries"),
        "correct code consumes"
    );
    assert!(
        !consume_challenge(&mut tx, phone, code, test_keys().lookup_key)
            .await
            .expect("challenge consume queries"),
        "double consume is not a second success"
    );
    assert!(
        !consume_challenge(&mut tx, phone, "wrong-code", test_keys().lookup_key)
            .await
            .expect("challenge consume queries"),
        "wrong codes never consume"
    );
    tx.commit().await.expect("consumption commits");
    // Revocation transitions once; revoked tokens resolve to nothing.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(
        revoke_session(&mut tx, session.id)
            .await
            .expect("revocation queries"),
        "live session revokes"
    );
    assert!(
        !revoke_session(&mut tx, session.id)
            .await
            .expect("revocation queries"),
        "double revocation is not a second transition"
    );
    tx.commit().await.expect("revocation commits");
    assert!(
        find_session_by_token(db.pool(), token)
            .await
            .expect("session lookup queries")
            .is_none(),
        "revoked tokens resolve to nothing"
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn identity_tables_hold_no_private_plaintext() {
    let db = TestDatabase::create("p04t01_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p04t01_privacy_"),
        "known suite identity in the database name"
    );
    for (table, expected) in [
        (
            "users",
            vec![
                "city",
                "created_at",
                "deleted_at",
                "display_name",
                "id",
                "phone_ciphertext",
                "phone_lookup",
                "policy_accepted_at",
                "policy_version",
                "region",
                "state",
            ],
        ),
        (
            "sessions",
            vec![
                "created_at",
                "expires_at",
                "id",
                "revoked_at",
                "session_digest",
                "user_id",
            ],
        ),
        (
            "phone_challenges",
            vec![
                "attempts",
                "challenge_digest",
                "consumed_at",
                "created_at",
                "expires_at",
                "id",
                "phone_lookup",
            ],
        ),
        (
            "user_blocks",
            vec!["blocked_id", "blocker_id", "created_at", "id"],
        ),
    ] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns
              WHERE table_name = $1 ORDER BY column_name",
        )
        .bind(table)
        .fetch_all(db.pool())
        .await
        .expect("schema introspection");
        assert_eq!(columns, expected, "columns are exactly the allowlist");
    }
    // The block ledger exists, starts empty, and answers queries.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let user = create_user(
        &mut tx,
        probe_user("Block User", "+5511933333333"),
        test_keys(),
    )
    .await
    .expect("user registers");
    assert_eq!(table_count(db.pool(), "user_blocks").await, 0);
    assert!(
        !blocks_relation_exists(&mut *tx, user.id, user.id)
            .await
            .expect("block lookup queries"),
        "empty ledger relates nothing"
    );
    tx.rollback().await.expect("suite continues");
    // Error values render nothing sensitive by construction (static reasons).
    let rendered = format!(
        "{:?} {} {:?}",
        UserError::StorageFailed,
        UserError::DuplicatePhone,
        UserError::InvalidPhone
    );
    assert!(!rendered.contains("119333"));
    assert!(!rendered.contains("sess"));
    db.cleanup().await.expect("suite cleans up");
}
