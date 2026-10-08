//! Request cycle and revision schema acceptance (P05-T02).
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - no orphan cycle/revision or negative budget is persisted;
//! - historical revision reload is stable after current requirements change;
//! - public serialization never exposes the phone destination.
//!
//! All phones, cities, and regions below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::requests::{
    create_request, cycle, cycles_for_request, insert_revision, request, requests_for_author,
    revision, revisions_for_request, start_cycle, to_public, update_current_requirements,
    NewRequest, RequestError, RequirementSnapshot,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p05t02-test-only-lookup-key",
        encryption_key: "p05t02-test-only-encryption-key",
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

fn draft_for(author_id: uuid::Uuid) -> NewRequest {
    NewRequest {
        author_id,
        title: "Refrigerator".to_owned(),
        category_code: "home_appliances".to_owned(),
        budget_cents: 52_000,
        condition: "either".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Preferably frost-free.".to_owned(),
    }
}

fn snapshot_v1() -> RequirementSnapshot {
    RequirementSnapshot {
        title: "Refrigerator".to_owned(),
        category_code: "home_appliances".to_owned(),
        budget_cents: 52_000,
        condition: "either".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Preferably frost-free.".to_owned(),
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

async fn seed_author(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let user = create_user(&mut tx, probe_user(display, phone), test_keys())
        .await
        .expect("fixture author stores");
    tx.commit().await.expect("author commits");
    user.id
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

#[tokio::test]
async fn no_orphan_cycle_revision_or_negative_budget_is_persisted() {
    let db = TestDatabase::create("p05t02_orphans")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t02_orphans_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Orphan Owner", "+55 11 90000-0001").await;

    // One coherent draft persists with lifecycle and visibility separated.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let stored = create_request(&mut tx, draft_for(author))
        .await
        .expect("valid draft stores");
    tx.commit().await.expect("draft commits");
    assert_eq!(stored.author_id, author);
    assert_eq!(stored.state, "draft");
    assert_eq!(stored.visibility, "private");
    assert_eq!(stored.current_cycle_number, 0);
    assert_eq!(stored.current_revision_number, 0);
    assert!(stored.original_published_at.is_none());
    assert_eq!(table_count(db.pool(), "requests").await, 1);

    // Zero and negative budgets never persist (application validation).
    for bad in [0, -52_000] {
        let mut tx = db.pool().begin().await.expect("transaction begins");
        let outcome = create_request(
            &mut tx,
            NewRequest {
                budget_cents: bad,
                ..draft_for(author)
            },
        )
        .await;
        assert_eq!(outcome, Err(RequestError::InvalidField));
        tx.rollback().await.expect("refusal rolls back");
    }
    // The database backstop agrees: a raw negative insert fails.
    let raw = sqlx::query(
        "INSERT INTO requests
            (author_id, title, category_code, budget_cents, \"condition\",
             city_code, region_code, notes)
         VALUES ($1, 'Refrigerator', 'home_appliances', -1, 'either',
                 'campinas', 'centro', '')",
    )
    .bind(author)
    .execute(db.pool())
    .await;
    assert!(raw.is_err(), "CHECK backstop refuses negative money");
    assert_eq!(table_count(db.pool(), "requests").await, 1);

    // Orphan revisions and cycles reference nothing: both are refused.
    let ghost = uuid::Uuid::now_v7();
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        insert_revision(&mut tx, ghost, 1, &snapshot_v1()).await,
        Err(RequestError::Unknown)
    );
    tx.rollback().await.expect("refusal rolls back");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let start = chrono::Utc::now();
    assert_eq!(
        start_cycle(&mut tx, ghost, 1, start, start + chrono::Duration::days(7)).await,
        Err(RequestError::Unknown)
    );
    tx.rollback().await.expect("refusal rolls back");
    assert_eq!(table_count(db.pool(), "request_revisions").await, 0);
    assert_eq!(table_count(db.pool(), "request_cycles").await, 0);

    // Well-formed but unseeded catalog codes are refused without a row.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        create_request(
            &mut tx,
            NewRequest {
                category_code: "motor_vehicles".to_owned(),
                ..draft_for(author)
            },
        )
        .await,
        Err(RequestError::Unknown)
    );
    tx.rollback().await.expect("refusal rolls back");
    assert_eq!(table_count(db.pool(), "requests").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn historical_revision_reload_is_stable_after_current_change() {
    let db = TestDatabase::create("p05t02_history")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t02_history_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "History Owner", "+55 11 90000-0002").await;

    let mut tx = db.pool().begin().await.expect("transaction begins");
    let stored = create_request(&mut tx, draft_for(author))
        .await
        .expect("valid draft stores");
    tx.commit().await.expect("draft commits");

    // Revision one records the original terms; promoting it leaves it intact.
    let first = snapshot_v1();
    let mut tx = db.pool().begin().await.expect("transaction begins");
    insert_revision(&mut tx, stored.id, 1, &first)
        .await
        .expect("first revision stores");
    let promoted = update_current_requirements(&mut tx, stored.id, 1, &first)
        .await
        .expect("current promotes");
    tx.commit().await.expect("promotion commits");
    assert_eq!(promoted.current_revision_number, 1);

    // A material change promotes revision two with new terms.
    let second = RequirementSnapshot {
        title: "Double-door refrigerator".to_owned(),
        budget_cents: 80_000,
        notes: "Needs to fit a 70cm niche.".to_owned(),
        ..snapshot_v1()
    };
    let mut tx = db.pool().begin().await.expect("transaction begins");
    insert_revision(&mut tx, stored.id, 2, &second)
        .await
        .expect("second revision stores");
    let promoted = update_current_requirements(&mut tx, stored.id, 2, &second)
        .await
        .expect("current promotes");
    tx.commit().await.expect("promotion commits");
    assert_eq!(promoted.title, "Double-door refrigerator");
    assert_eq!(promoted.budget_cents, 80_000);
    assert_eq!(promoted.current_revision_number, 2);

    // The historical revision reloads byte-stable after the current moved on.
    let reloaded = revision(db.pool(), stored.id, 1)
        .await
        .expect("revision reads")
        .expect("first revision reads");
    assert_eq!(reloaded.title, first.title);
    assert_eq!(reloaded.budget_cents, first.budget_cents);
    assert_eq!(reloaded.notes, first.notes);
    assert_eq!(reloaded.category_code, first.category_code);
    let history = revisions_for_request(db.pool(), stored.id)
        .await
        .expect("history reads");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].revision_number, 1);
    assert_eq!(history[1].revision_number, 2);

    // Numbers never move backwards: reuse and duplicate numbers are refused.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        update_current_requirements(&mut tx, stored.id, 1, &first).await,
        Err(RequestError::InvalidField)
    );
    tx.rollback().await.expect("refusal rolls back");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(
        insert_revision(&mut tx, stored.id, 2, &second)
            .await
            .is_err(),
        "duplicate revision numbers conflict"
    );
    tx.rollback().await.expect("refusal rolls back");
    assert_eq!(table_count(db.pool(), "request_revisions").await, 2);

    // Cycles advance monotonically; the first publication time never rewrites.
    // Whole-second instants: the database keeps microseconds, so chrono
    // nanoseconds would never compare exactly.
    let first_start = chrono::DateTime::from_timestamp(chrono::Utc::now().timestamp(), 0)
        .expect("whole-second instant builds");
    let first_deadline = first_start + chrono::Duration::days(7);
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let opened = start_cycle(&mut tx, stored.id, 1, first_start, first_deadline)
        .await
        .expect("first cycle starts");
    tx.commit().await.expect("cycle commits");
    assert_eq!(opened.deadline, first_deadline);
    let second_start = first_deadline + chrono::Duration::seconds(1);
    let second_deadline = second_start + chrono::Duration::days(7);
    let mut tx = db.pool().begin().await.expect("transaction begins");
    start_cycle(&mut tx, stored.id, 2, second_start, second_deadline)
        .await
        .expect("renewal starts a fresh cycle");
    tx.commit().await.expect("cycle commits");
    let renewed = request(db.pool(), stored.id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(renewed.current_cycle_number, 2);
    assert_eq!(renewed.original_published_at, Some(first_start));
    let cycles = cycles_for_request(db.pool(), stored.id)
        .await
        .expect("cycles read");
    assert_eq!(cycles.len(), 2);
    assert_eq!(
        cycle(db.pool(), stored.id, 1)
            .await
            .expect("cycle reads")
            .expect("first cycle reads")
            .deadline,
        first_deadline
    );
    // A reused number or an inverted window is refused.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        start_cycle(&mut tx, stored.id, 2, second_start, second_deadline).await,
        Err(RequestError::InvalidField)
    );
    tx.rollback().await.expect("refusal rolls back");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        start_cycle(&mut tx, stored.id, 3, second_deadline, second_deadline).await,
        Err(RequestError::InvalidField)
    );
    tx.rollback().await.expect("refusal rolls back");
    assert_eq!(table_count(db.pool(), "request_cycles").await, 2);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn public_serialization_never_exposes_the_phone_destination() {
    let db = TestDatabase::create("p05t02_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p05t02_privacy_"),
        "known suite identity in the database name"
    );
    // Unmistakable synthetic canary: no test value may survive serialization.
    const CANARY_DIGITS: &str = "5511900000003";
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Privacy Owner", "+55 11 90000-0003").await;

    let mut tx = db.pool().begin().await.expect("transaction begins");
    let stored = create_request(&mut tx, draft_for(author))
        .await
        .expect("valid draft stores");
    tx.commit().await.expect("draft commits");

    // The public allowlist carries requirements only: no author, no phone.
    let rendered = serde_json::to_string(&to_public(&stored)).expect("public serializes");
    assert!(rendered.contains("520.00"), "budget renders exactly");
    for absent in [
        "author",
        CANARY_DIGITS,
        "90000-0003",
        "90000",
        "phone",
        "ciphertext",
        "lookup",
        "destination",
        "address",
        "token",
        "session",
        "reporter",
        "secret",
        "visibility",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in public output");
    }
    // Owner reads still resolve through the private path with the author intact.
    let private = request(db.pool(), stored.id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(private.author_id, author);
    assert_eq!(
        requests_for_author(db.pool(), author)
            .await
            .expect("owner reads")
            .len(),
        1
    );
    // The schema itself holds no phone-shaped column on any request table.
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name IN ('requests', 'request_revisions', 'request_cycles')",
    )
    .fetch_all(db.pool())
    .await
    .expect("columns read");
    assert!(!columns.is_empty(), "request tables exist");
    for column in &columns {
        for absent in [
            "phone",
            "cipher",
            "lookup",
            "address",
            "reporter",
            "token",
            "secret",
            "destination",
        ] {
            assert!(
                !column.contains(absent),
                "no {absent} column in request tables"
            );
        }
    }
    db.cleanup().await.expect("suite cleans up");
}
