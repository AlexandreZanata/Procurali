//! Offer schema acceptance (P07-T01): slots and immutable terms.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - no orphan offer or negative price is persisted, and owners cannot
//!   take their own slot;
//! - concurrent same-seller/cycle submissions converge on one slot;
//! - historical terms reload stably after current terms move on.
//!
//! Setup publishes travel the real draft and publication operations, which
//! provide the existing request, cycle, and revision every offer names. All
//! phones, names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::offers::{
    create_offer, insert_terms, offer, offers_for_request, terms, terms_for_offer,
    update_current_terms, NewOffer, OfferError, TermsSnapshot,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p07t01-test-only-lookup-key",
        encryption_key: "p07t01-test-only-encryption-key",
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

async fn seed_user(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let user = create_user(&mut tx, probe_user(display, phone), test_keys())
        .await
        .expect("fixture account stores");
    tx.commit().await.expect("account commits");
    user.id
}

/// A real publishing account: identity travels the real creation path with
/// real phone cryptography, and only the lifecycle flip (owned by the
/// verification flow) is staged, because publication requires eligibility.
async fn seed_author(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
    let id = seed_user(pool, display, phone).await;
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    id
}

/// One genuinely open request through the real draft and publication paths:
/// cycle 1 and requirement revision 1 exist for offers to name.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some("Refrigerator".to_owned()),
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

fn offer_for(request_id: uuid::Uuid, seller_id: uuid::Uuid) -> NewOffer {
    NewOffer {
        request_id,
        cycle_number: 1,
        revision_number: 1,
        seller_id,
        description: "Frost-free 300L".to_owned(),
        price_cents: 45_000,
        condition: "used".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Pickup only.".to_owned(),
    }
}

fn terms_v1() -> TermsSnapshot {
    TermsSnapshot {
        description: "Frost-free 300L".to_owned(),
        price_cents: 45_000,
        condition: "used".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Pickup only.".to_owned(),
    }
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

#[tokio::test]
async fn no_orphan_offer_or_negative_price_is_persisted() {
    let db = TestDatabase::create("p07t01_orphans")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t01_orphans_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Offer Owner", "+55 11 90000-0111").await;
    let seller = seed_user(db.pool(), "Offer Seller", "+55 11 90000-0112").await;
    let request_id = publish_open(db.pool(), author).await;

    // One coherent offer persists with its slot, first terms, and lifecycle.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let stored = create_offer(&mut tx, offer_for(request_id, seller))
        .await
        .expect("valid offer stores");
    tx.commit().await.expect("offer commits");
    assert_eq!(stored.seller_id, seller);
    assert_eq!(stored.state, "sent");
    assert_eq!(stored.visibility, "visible");
    assert_eq!(stored.current_terms_number, 1);
    assert_eq!(table_count(db.pool(), "offers").await, 1);

    // Zero and negative prices never persist (application validation).
    for bad in [0, -45_000] {
        let mut tx = db.pool().begin().await.expect("transaction begins");
        let outcome = create_offer(
            &mut tx,
            NewOffer {
                price_cents: bad,
                ..offer_for(request_id, seller)
            },
        )
        .await;
        assert_eq!(outcome, Err(OfferError::InvalidField));
        tx.rollback().await.expect("refusal rolls back");
    }
    // The database backstop agrees: a raw negative insert fails.
    let raw = sqlx::query(
        "INSERT INTO offers
            (request_id, cycle_number, revision_number, seller_id, description,
             price_cents, \"condition\", city_code, region_code, notes)
         VALUES ($1, 1, 1, $2, 'Frost-free', -1, 'used', 'campinas', 'centro', '')",
    )
    .bind(request_id)
    .bind(seller)
    .execute(db.pool())
    .await;
    assert!(raw.is_err(), "CHECK backstop refuses negative money");
    assert_eq!(table_count(db.pool(), "offers").await, 1);

    // Orphan references name nothing: unknown request, cycle, revision, and
    // seller are each refused without a row.
    let ghost = uuid::Uuid::now_v7();
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        create_offer(&mut tx, offer_for(ghost, seller)).await,
        Err(OfferError::Unknown)
    );
    tx.rollback().await.expect("refusal rolls back");
    for (cycle, revision) in [(2, 1), (1, 2)] {
        let mut tx = db.pool().begin().await.expect("transaction begins");
        assert_eq!(
            create_offer(
                &mut tx,
                NewOffer {
                    cycle_number: cycle,
                    revision_number: revision,
                    ..offer_for(request_id, seller)
                },
            )
            .await,
            Err(OfferError::Unknown)
        );
        tx.rollback().await.expect("refusal rolls back");
    }
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        create_offer(&mut tx, offer_for(request_id, ghost)).await,
        Err(OfferError::Unknown)
    );
    tx.rollback().await.expect("refusal rolls back");
    assert_eq!(table_count(db.pool(), "offers").await, 1);

    // Owners never take their own slot.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        create_offer(&mut tx, offer_for(request_id, author)).await,
        Err(OfferError::SelfOffer)
    );
    tx.rollback().await.expect("refusal rolls back");
    assert_eq!(table_count(db.pool(), "offers").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_seller_cycle_has_one_slot() {
    let db = TestDatabase::create("p07t01_slot")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t01_slot_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Slot Owner", "+55 11 90000-0113").await;
    let seller = seed_user(db.pool(), "Slot Seller", "+55 11 90000-0114").await;
    let request_id = publish_open(db.pool(), author).await;

    // Two submissions for one seller/cycle race in separate transactions:
    // the UNIQUE slot lets exactly one commit through.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let submit = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            let mut tx = pool.begin().await.expect("transaction begins");
            let outcome = create_offer(&mut tx, offer_for(request_id, seller)).await;
            match outcome {
                Ok(stored) => {
                    tx.commit().await.expect("winner commits");
                    Ok(stored.id)
                }
                Err(error) => {
                    tx.rollback().await.expect("loser rolls back");
                    Err(error)
                }
            }
        }
    };
    let (first, second): (
        Result<uuid::Uuid, OfferError>,
        Result<uuid::Uuid, OfferError>,
    ) = tokio::join!(submit(std::sync::Arc::clone(&barrier)), submit(barrier),);
    assert!(
        first.is_ok() != second.is_ok(),
        "exactly one submission wins"
    );
    assert!(
        [first, second].contains(&Err(OfferError::StorageFailed)),
        "the loser takes the slot conflict, not a silent success"
    );
    assert_eq!(
        offers_for_request(db.pool(), request_id)
            .await
            .expect("offers read")
            .len(),
        1
    );
    assert_eq!(table_count(db.pool(), "offer_terms").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn terms_reload_stable_after_current_update() {
    let db = TestDatabase::create("p07t01_history")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t01_history_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "History Owner", "+55 11 90000-0115").await;
    let seller = seed_user(db.pool(), "History Seller", "+55 11 90000-0116").await;
    let request_id = publish_open(db.pool(), author).await;
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let stored = create_offer(&mut tx, offer_for(request_id, seller))
        .await
        .expect("valid offer stores");
    tx.commit().await.expect("offer commits");

    // New terms promote the current row while the first terms reload
    // byte-stable: history is never rewritten by an update.
    let second = TermsSnapshot {
        description: "Frost-free 350L".to_owned(),
        price_cents: 48_000,
        ..terms_v1()
    };
    let mut tx = db.pool().begin().await.expect("transaction begins");
    insert_terms(&mut tx, stored.id, 2, &second)
        .await
        .expect("second terms store");
    let promoted = update_current_terms(&mut tx, stored.id, 2, &second)
        .await
        .expect("current promotes");
    tx.commit().await.expect("promotion commits");
    assert_eq!(promoted.current_terms_number, 2);
    assert_eq!(promoted.description, "Frost-free 350L");
    assert_eq!(promoted.price_cents, 48_000);
    let reloaded = terms(db.pool(), stored.id, 1)
        .await
        .expect("terms read")
        .expect("first terms read");
    assert_eq!(reloaded.description, "Frost-free 300L");
    assert_eq!(reloaded.price_cents, 45_000);
    let history = terms_for_offer(db.pool(), stored.id)
        .await
        .expect("history reads");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].terms_number, 1);
    assert_eq!(history[1].terms_number, 2);

    // Numbers never move backwards: reuse and duplicate numbers are refused.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        update_current_terms(&mut tx, stored.id, 1, &terms_v1()).await,
        Err(OfferError::InvalidField)
    );
    tx.rollback().await.expect("refusal rolls back");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(
        insert_terms(&mut tx, stored.id, 2, &second).await.is_err(),
        "duplicate terms numbers conflict"
    );
    tx.rollback().await.expect("refusal rolls back");
    assert_eq!(
        terms_for_offer(db.pool(), stored.id)
            .await
            .expect("history reads")
            .len(),
        2
    );
    assert_eq!(
        offer(db.pool(), stored.id)
            .await
            .expect("offer reads")
            .expect("offer reads")
            .current_terms_number,
        2
    );
    db.cleanup().await.expect("suite cleans up");
}
