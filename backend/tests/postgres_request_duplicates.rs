//! Exact-duplicate acceptance (P05-T06): normalized same-owner matching.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - an exact normalized duplicate (spacing/casing variants) is refused with
//!   the existing need identified for the edit/renew path;
//! - meaningfully different requirements and same-region-different-city
//!   needs are not falsely identical, within or across accounts;
//! - concurrent identical publications serialize into exactly one active
//!   matching intent.
//!
//! Setup publishes travel the real draft and publication operations; guarded
//! compositions mirror publication storage until the wiring card calls the
//! guard inside publication itself. All phones, names, and codes below are
//! synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::duplicate_intent::{
    check_duplicate_intent, matching_open_request, DuplicateError, NewIntent,
};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::transaction::{
    run_serializable, AttemptError, TransactionError,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p05t06-test-only-lookup-key",
        encryption_key: "p05t06-test-only-encryption-key",
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
    upsert_region(&mut tx, "campinas", "norte", "Norte")
        .await
        .expect("second region stores");
    upsert_city(&mut tx, "valinhos", "Valinhos", true)
        .await
        .expect("second city stores");
    upsert_region(&mut tx, "valinhos", "centro", "Centro")
        .await
        .expect("same-named region stores");
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

fn intent(title: &str) -> NewIntent {
    NewIntent {
        title: title.to_owned(),
        category_code: "home_appliances".to_owned(),
        budget_cents: 52_000,
        condition: "either".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
    }
}

/// One genuinely open request through the real draft and publication paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
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
            notes: None,
        },
    )
    .await
    .expect("fixture draft validates");
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

/// Guard verdict without mutating: check inside a transaction, roll back.
async fn guard(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    candidate: &NewIntent,
) -> Result<(), DuplicateError> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let verdict = check_duplicate_intent(&mut tx, author, candidate).await;
    tx.rollback().await.expect("probe rolls back");
    match verdict {
        Ok(()) => Ok(()),
        Err(AttemptError::Abort(reason)) => Err(reason),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

async fn matching(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    candidate: &NewIntent,
) -> Option<uuid::Uuid> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let found = matching_open_request(&mut tx, author, candidate).await;
    tx.rollback().await.expect("probe rolls back");
    match found {
        Ok(found) => found,
        Err(AttemptError::Abort(_)) => panic!("probe refusal impossible"),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

async fn open_count(pool: &sqlx::PgPool, author: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM requests WHERE author_id = $1 AND state = 'active'")
        .bind(author)
        .fetch_one(pool)
        .await
        .expect("open rows read")
}

/// Open-request mechanics for the race: genuine rows (live request,
/// immutable revision, live cycle, publication fact) written with direct SQL
/// so the original `sqlx::Error` reaches the serializable runner. Values are
/// known-valid fixtures; validation lives in the publication operation.
async fn open_matching_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author: uuid::Uuid,
    candidate: &NewIntent,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<uuid::Uuid, AttemptError<DuplicateError>> {
    let id = uuid::Uuid::now_v7();
    let revision_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO requests
            (id, author_id, title, category_code, budget_cents, \"condition\",
             city_code, region_code, notes, state, visibility,
             current_cycle_number, current_revision_number)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '', 'active', 'public', 1, 1)",
    )
    .bind(id)
    .bind(author)
    .bind(&candidate.title)
    .bind(&candidate.category_code)
    .bind(candidate.budget_cents)
    .bind(&candidate.condition)
    .bind(&candidate.city_code)
    .bind(&candidate.region_code)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    sqlx::query(
        "INSERT INTO request_revisions
            (id, request_id, revision_number, title, category_code,
             budget_cents, \"condition\", city_code, region_code, notes)
         VALUES ($1, $2, 1, $3, $4, $5, $6, $7, $8, '')",
    )
    .bind(revision_id)
    .bind(id)
    .bind(&candidate.title)
    .bind(&candidate.category_code)
    .bind(candidate.budget_cents)
    .bind(&candidate.condition)
    .bind(&candidate.city_code)
    .bind(&candidate.region_code)
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
    .bind(now + chrono::Duration::days(7))
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
async fn exact_normalized_duplicate_is_refused_with_its_existing_need() {
    let db = TestDatabase::create("p05t06_exact")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t06_exact_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Exact Owner", "+55 11 90000-0031").await;
    let open = publish_open(db.pool(), author, "Refrigerator").await;

    // The byte-identical need and its spacing/casing variants all match the
    // one open row, which the finder names for the edit/renew suggestion.
    for title in [
        "Refrigerator",
        "  REFRIGERATOR ",
        "refrigerator",
        "Refrigerator\n",
    ] {
        assert_eq!(
            guard(db.pool(), author, &intent(title)).await,
            Err(DuplicateError::DuplicateIntent),
            "{title:?} matches the open need"
        );
        assert_eq!(
            matching(db.pool(), author, &intent(title)).await,
            Some(open)
        );
    }
    // Nothing was created by the refusals.
    assert_eq!(open_count(db.pool(), author).await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn near_misses_are_not_falsely_identical() {
    let db = TestDatabase::create("p05t06_nearmiss")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t06_nearmiss_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Near Owner", "+55 11 90000-0032").await;
    let other = seed_author(db.pool(), "Other Owner", "+55 11 90000-0033").await;
    publish_open(db.pool(), author, "Refrigerator").await;
    let base = intent("Refrigerator");

    // One differing identity field breaks the match — including the same
    // region label under another city and another region in the same city.
    let near_misses = [
        NewIntent {
            title: "Washing machine".to_owned(),
            ..base.clone()
        },
        NewIntent {
            title: "Refrigerator Pro".to_owned(),
            ..base.clone()
        },
        NewIntent {
            category_code: "furniture".to_owned(),
            ..base.clone()
        },
        NewIntent {
            budget_cents: 52_001,
            ..base.clone()
        },
        NewIntent {
            condition: "new".to_owned(),
            ..base.clone()
        },
        NewIntent {
            city_code: "valinhos".to_owned(),
            ..base.clone()
        },
        NewIntent {
            region_code: "norte".to_owned(),
            ..base.clone()
        },
    ];
    for candidate in &near_misses {
        assert!(
            guard(db.pool(), author, candidate).await.is_ok(),
            "{candidate:?} is a distinct need"
        );
        assert_eq!(matching(db.pool(), author, candidate).await, None);
    }
    // Matching never crosses accounts: the same content is fresh elsewhere.
    assert!(guard(db.pool(), other, &base).await.is_ok());
    assert_eq!(matching(db.pool(), other, &base).await, None);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_publications_produce_one_active_intent() {
    let db = TestDatabase::create("p05t06_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t06_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Race Owner", "+55 11 90000-0034").await;
    let candidate = intent("Refrigerator");

    // Both publishers observe no open need and race in serializable
    // transactions: one commits the single intent, the other retries,
    // observes the winner, and takes the duplicate refusal.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let publish = |barrier: std::sync::Arc<tokio::sync::Barrier>, candidate: NewIntent| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            run_serializable(&pool, |tx, _attempt| {
                let candidate = candidate.clone();
                Box::pin(async move {
                    check_duplicate_intent(tx, author, &candidate).await?;
                    open_matching_request(tx, author, &candidate, chrono::Utc::now()).await?;
                    Ok::<_, AttemptError<DuplicateError>>(())
                })
            })
            .await
        }
    };
    let (first, second) = tokio::join!(
        publish(std::sync::Arc::clone(&barrier), candidate.clone()),
        publish(barrier, candidate),
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "exactly one publisher creates the intent"
    );
    assert!(
        outcomes.iter().any(|outcome| matches!(
            outcome,
            Err(TransactionError::Aborted(DuplicateError::DuplicateIntent))
        )),
        "the loser takes the duplicate refusal, not a silent success"
    );

    // One active matching intent stands: a single open row, a single fact.
    assert_eq!(open_count(db.pool(), author).await, 1);
    let matched: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM requests WHERE author_id = $1 AND state = 'active'")
            .bind(author)
            .fetch_all(db.pool())
            .await
            .expect("open rows read");
    assert_eq!(matched.len(), 1);
    let facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE actor_id = $1 AND kind = 'request.published'",
    )
    .bind(author)
    .fetch_one(db.pool())
    .await
    .expect("events read");
    assert_eq!(facts, 1);
    db.cleanup().await.expect("suite cleans up");
}
