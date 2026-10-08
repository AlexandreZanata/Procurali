//! Request quota acceptance (P05-T05): open slots and rolling activations.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - a fourth open request is refused while suspended rows still occupy
//!   slots and expired/terminal rows free them;
//! - a seventh in-window activation is refused while the rolling boundary
//!   ignores elapsed facts;
//! - owner removal frees an open slot without refunding activations or
//!   erasing history;
//! - two contenders for the final slot cannot overpublish.
//!
//! Setup publishes travel the real draft and publication operations; quota
//! attempts compose the transactional guard with publication mechanics until
//! the wiring card calls the guard inside publication itself. Quota tables
//! do not exist by design: both counters derive from live rows and immutable
//! facts, so nothing can reset them. All phones, names, and codes below are
//! synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_limits::{
    activation_count, check_activation_limits, open_request_count, LimitError,
};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::events::{record as record_event, NewEvent};
use procurali_backend::persistence::requests::{cycles_for_request, revisions_for_request};
use procurali_backend::persistence::transaction::{run_serializable, AttemptError};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use serde_json::json;

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p05t05-test-only-lookup-key",
        encryption_key: "p05t05-test-only-encryption-key",
    }
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
        test_keys(),
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

fn draft_input(title: &str) -> DraftInput {
    DraftInput {
        title: Some(title.to_owned()),
        category_code: Some("home_appliances".to_owned()),
        budget: Some("520.00".to_owned()),
        condition: Some("either".to_owned()),
        city_code: Some("campinas".to_owned()),
        region_code: Some("centro".to_owned()),
        notes: None,
    }
}

/// One genuinely open request through the real draft and publication paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(pool, author, draft_input(title))
        .await
        .expect("fixture draft validates");
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

/// Guard verdict without mutating: check inside a transaction, roll back.
/// A storage conflict in a lone probe transaction is a test failure, never
/// a quota verdict.
async fn guard(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), LimitError> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let verdict = check_activation_limits(&mut tx, author, now).await;
    tx.rollback().await.expect("probe rolls back");
    match verdict {
        Ok(()) => Ok(()),
        Err(AttemptError::Abort(reason)) => Err(reason),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

/// Both counters in one probe transaction.
async fn counts(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> (u32, u32) {
    fn readable(result: Result<u32, AttemptError<LimitError>>) -> u32 {
        match result {
            Ok(count) => count,
            Err(AttemptError::Abort(_)) => panic!("probe refusal impossible"),
            Err(AttemptError::Db(_)) => panic!("probe storage failed"),
        }
    }
    let mut tx = pool.begin().await.expect("transaction begins");
    let open = readable(open_request_count(&mut tx, author, now).await);
    let activations = readable(activation_count(&mut tx, author, now).await);
    tx.rollback().await.expect("probe rolls back");
    (open, activations)
}

async fn published_facts(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1
           AND kind = 'request.published'",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .expect("events read")
}

/// Open-request mechanics for guarded compositions: genuine rows (live
/// request, immutable revision, live cycle, publication fact) written with
/// direct SQL so the original `sqlx::Error` reaches the serializable runner
/// — persistence writers erase it into flat errors, which would turn a
/// retriable statement-level `40001` into a wrongful refusal. Values are
/// known-valid fixtures (the writers' own validation is proven by their
/// suites); publication validation and policy live in the publication
/// operation. This helper stands in until the wiring card calls the guard
/// inside publication itself.
async fn open_live_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author: uuid::Uuid,
    title: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<uuid::Uuid, AttemptError<LimitError>> {
    let id = uuid::Uuid::now_v7();
    let revision_id = uuid::Uuid::now_v7();
    let deadline = now + chrono::Duration::days(7);
    sqlx::query(
        "INSERT INTO requests
            (id, author_id, title, category_code, budget_cents, \"condition\",
             city_code, region_code, notes, state, visibility,
             current_cycle_number, current_revision_number)
         VALUES ($1, $2, $3, 'home_appliances', 52000, 'either',
                 'campinas', 'centro', '', 'active', 'public', 1, 1)",
    )
    .bind(id)
    .bind(author)
    .bind(title)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    sqlx::query(
        "INSERT INTO request_revisions
            (id, request_id, revision_number, title, category_code,
             budget_cents, \"condition\", city_code, region_code, notes)
         VALUES ($1, $2, 1, $3, 'home_appliances', 52000, 'either',
                 'campinas', 'centro', '')",
    )
    .bind(revision_id)
    .bind(id)
    .bind(title)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    sqlx::query(
        "INSERT INTO request_cycles
            (request_id, cycle_number, started_at, deadline)
         VALUES ($1, 1, $2, $3)",
    )
    .bind(id)
    .bind(now)
    .bind(deadline)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    sqlx::query(
        "INSERT INTO business_events
            (actor_id, resource_kind, resource_id, cycle, revision,
             effective_at, kind, policy, source, payload)
         VALUES ($1, 'request', $2, 1, $3, $4, 'request.published',
                 'mvp-free', 'api', '{\"state\": \"active\"}')",
    )
    .bind(author)
    .bind(id)
    .bind(revision_id)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    Ok(id)
}

#[tokio::test]
async fn fourth_open_request_is_refused_while_ended_rows_free_slots() {
    let db = TestDatabase::create("p05t05_slots")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t05_slots_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Slot Owner", "+55 11 90000-0021").await;
    let now = chrono::Utc::now();
    for title in ["Slot One", "Slot Two", "Slot Three"] {
        publish_open(db.pool(), author, title).await;
    }
    assert_eq!(counts(db.pool(), author, now).await, (3, 3));

    // Three genuine opens refuse a fourth.
    assert_eq!(
        guard(db.pool(), author, now).await,
        Err(LimitError::OpenSlotsExhausted)
    );

    // An unexpired suspended row still occupies its slot.
    let suspended: uuid::Uuid = sqlx::query_scalar(
        "UPDATE requests SET state = 'suspended' WHERE title = 'Slot Three' RETURNING id",
    )
    .fetch_one(db.pool())
    .await
    .expect("synthetic suspension applies");
    assert_eq!(counts(db.pool(), author, now).await, (3, 3));
    assert_eq!(
        guard(db.pool(), author, now).await,
        Err(LimitError::OpenSlotsExhausted)
    );

    // An elapsed deadline frees the slot even though the lifecycle lags. The
    // start moves back with the deadline: the CHECK backstop still orders
    // every cycle, and only the deadline position matters for openness.
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(suspended)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic expiry applies");
    assert_eq!(counts(db.pool(), author, now).await, (2, 3));
    assert!(guard(db.pool(), author, now).await.is_ok());
    publish_open(db.pool(), author, "Slot Four").await;
    assert_eq!(counts(db.pool(), author, now).await, (3, 4));

    // A terminal row frees its slot the same way.
    let fourth: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM requests WHERE title = 'Slot Four'")
            .fetch_one(db.pool())
            .await
            .expect("fourth reads");
    sqlx::query("UPDATE requests SET state = 'cancelled' WHERE id = $1")
        .bind(fourth)
        .execute(db.pool())
        .await
        .expect("synthetic closure applies");
    assert_eq!(counts(db.pool(), author, now).await, (2, 4));
    assert!(guard(db.pool(), author, now).await.is_ok());
    publish_open(db.pool(), author, "Slot Five").await;
    assert_eq!(counts(db.pool(), author, now).await, (3, 5));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn seventh_in_window_activation_is_refused() {
    let db = TestDatabase::create("p05t05_window")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t05_window_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Window Owner", "+55 11 90000-0022").await;
    let now = chrono::Utc::now();
    let mut ids = Vec::new();
    for index in 0..6 {
        ids.push(publish_open(db.pool(), author, &format!("Window {index}")).await);
    }
    // Six activations stand; close four so slots stay out of the way.
    for id in ids.iter().take(4) {
        sqlx::query("UPDATE requests SET state = 'cancelled' WHERE id = $1")
            .bind(id)
            .execute(db.pool())
            .await
            .expect("synthetic closure applies");
    }
    assert_eq!(counts(db.pool(), author, now).await, (2, 6));

    // The seventh in-window activation is refused with slots to spare.
    assert_eq!(
        guard(db.pool(), author, now).await,
        Err(LimitError::ActivationQuotaExhausted)
    );

    // Restricted accounts are refused without distinguishing states.
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(author)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        guard(db.pool(), author, now).await,
        Err(LimitError::NotActive)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn rolling_boundary_ignores_elapsed_facts() {
    let db = TestDatabase::create("p05t05_rolling")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t05_rolling_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Rolling Owner", "+55 11 90000-0023").await;
    let now = chrono::Utc::now();
    let mut ids = Vec::new();
    for index in 0..5 {
        ids.push(publish_open(db.pool(), author, &format!("Rolling {index}")).await);
    }
    for id in &ids {
        sqlx::query("UPDATE requests SET state = 'cancelled' WHERE id = $1")
            .bind(id)
            .execute(db.pool())
            .await
            .expect("synthetic closure applies");
    }
    // Elapsed facts (25h and 49h ago) predate the rolling window: five recent
    // activations still leave room for one more, with no midnight involved.
    for (age_hours, marker) in [(25, "old"), (49, "older")] {
        record_event(
            db.pool(),
            NewEvent {
                actor_id: Some(author),
                resource_kind: "request",
                resource_id: ids[0],
                cycle: Some(1),
                revision: None,
                effective_at: now - chrono::Duration::hours(age_hours),
                kind: "request.published",
                policy: "mvp-free",
                source: "api",
                payload: json!({"state": "active", "marker": marker}),
            },
        )
        .await
        .expect("elapsed fact records");
    }
    assert_eq!(counts(db.pool(), author, now).await, (0, 5));
    assert!(guard(db.pool(), author, now).await.is_ok());
    publish_open(db.pool(), author, "Rolling Sixth").await;
    assert_eq!(counts(db.pool(), author, now).await, (1, 6));

    // A fact 23 hours old still falls inside the elapsed window: the seventh
    // is refused, pinning the near edge of the same boundary.
    record_event(
        db.pool(),
        NewEvent {
            actor_id: Some(author),
            resource_kind: "request",
            resource_id: ids[0],
            cycle: Some(1),
            revision: None,
            effective_at: now - chrono::Duration::hours(23),
            kind: "request.published",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "active"}),
        },
    )
    .await
    .expect("near-edge fact records");
    assert_eq!(counts(db.pool(), author, now).await, (1, 7));
    assert_eq!(
        guard(db.pool(), author, now).await,
        Err(LimitError::ActivationQuotaExhausted)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn removal_frees_slots_without_refunding_or_erasing() {
    let db = TestDatabase::create("p05t05_removal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t05_removal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Removal Owner", "+55 11 90000-0024").await;
    let now = chrono::Utc::now();
    for title in ["Keep One", "Keep Two", "Remove Me"] {
        publish_open(db.pool(), author, title).await;
    }
    assert_eq!(
        guard(db.pool(), author, now).await,
        Err(LimitError::OpenSlotsExhausted)
    );
    let (open_before, activations_before) = counts(db.pool(), author, now).await;

    // Owner removal cancels and hides (state-transitions 6.1) without
    // deleting: the slot frees, the activation count does not move.
    let removed: uuid::Uuid = sqlx::query_scalar(
        "UPDATE requests SET state = 'cancelled', visibility = 'hidden'
         WHERE title = 'Remove Me' RETURNING id",
    )
    .fetch_one(db.pool())
    .await
    .expect("synthetic removal applies");
    let (open_after, activations_after) = counts(db.pool(), author, now).await;
    assert_eq!(open_after, open_before - 1, "removal frees an open slot");
    assert_eq!(
        activations_after, activations_before,
        "removal never refunds activations"
    );
    assert!(guard(db.pool(), author, now).await.is_ok());

    // History survives removal: revision and fact still read back.
    assert_eq!(
        revisions_for_request(db.pool(), removed)
            .await
            .expect("revisions read")
            .len(),
        1
    );
    assert_eq!(published_facts(db.pool(), removed).await, 1);

    // The freed slot accepts genuine demand again.
    publish_open(db.pool(), author, "Replacement").await;
    assert_eq!(counts(db.pool(), author, now).await, (3, 4));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn final_slot_contenders_cannot_overpublish() {
    let db = TestDatabase::create("p05t05_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t05_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Race Owner", "+55 11 90000-0025").await;
    publish_open(db.pool(), author, "Race One").await;
    publish_open(db.pool(), author, "Race Two").await;

    // Both contenders observe two open slots and race for the last one in
    // serializable transactions: one commits, the other retries, re-reads
    // three open slots, and takes the quota refusal.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let contend = |title: &'static str, barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            run_serializable(&pool, |tx, _attempt| {
                Box::pin(async move {
                    let now = chrono::Utc::now();
                    check_activation_limits(tx, author, now).await?;
                    open_live_request(tx, author, title, now).await?;
                    Ok::<_, AttemptError<LimitError>>(())
                })
            })
            .await
        }
    };
    let (first, second) = tokio::join!(
        contend("Race Three", std::sync::Arc::clone(&barrier)),
        contend("Race Four", barrier),
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "exactly one contender takes the final slot"
    );
    assert!(
        outcomes.iter().any(|outcome| matches!(
            outcome,
            Err(
                procurali_backend::persistence::transaction::TransactionError::Aborted(
                    LimitError::OpenSlotsExhausted
                )
            )
        )),
        "the loser takes the quota refusal, not a silent success"
    );

    // No overpublication: three open rows, three facts, four attempts max.
    let now = chrono::Utc::now();
    assert_eq!(counts(db.pool(), author, now).await.0, 3);
    let facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE actor_id = $1 AND kind = 'request.published'",
    )
    .bind(author)
    .fetch_one(db.pool())
    .await
    .expect("events read");
    assert_eq!(facts, 3);
    let cycles: i64 =
        sqlx::query_scalar("SELECT count(*) FROM request_cycles WHERE cycle_number = 1")
            .fetch_one(db.pool())
            .await
            .expect("cycles read");
    assert_eq!(cycles, 3);
    assert_eq!(
        cycles_for_request(
            db.pool(),
            sqlx::query_scalar("SELECT id FROM requests WHERE title = 'Race One'")
                .fetch_one(db.pool())
                .await
                .expect("fixture reads")
        )
        .await
        .expect("cycles read")
        .len(),
        1
    );
    db.cleanup().await.expect("suite cleans up");
}
