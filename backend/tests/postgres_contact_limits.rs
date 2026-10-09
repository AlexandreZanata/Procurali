//! Contact-allowance acceptance (P08-T05): distinct sellers per cycle.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - two contenders for the final distinct slot produce exactly one
//!   permitted winner;
//! - the eleventh distinct seller is refused while repeats of contacted
//!   sellers stay eligible and failures consume nothing;
//! - several buyers and sellers contact independently with no sale,
//!   reservation, or winner label anywhere.
//!
//! Successful setup handoffs travel the real initiation operation; quota
//! attempts compose the transactional guard with record mechanics until
//! the wiring card calls the guard inside handoff itself. Allowance tables
//! do not exist by design: the count derives from live rows, so nothing
//! can reset it. All phones, names, and codes below are synthetic and
//! reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::contact_limits::{
    check_contact_limits, distinct_contact_count, ContactLimitError,
};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::transaction::{run_serializable, AttemptError};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p08t05-test-only-lookup-key",
        encryption_key: "p08t05-test-only-encryption-key",
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

/// A real active account: identity travels the real creation path with real
/// phone cryptography, and only the lifecycle flip (owned by the
/// verification flow) is staged.
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

/// One genuinely open either/600 demand through the real paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("600.00".to_owned()),
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

/// One sent used/520 offer through the real submission operation.
async fn submit_open(
    pool: &sqlx::PgPool,
    seller: uuid::Uuid,
    request_id: uuid::Uuid,
) -> uuid::Uuid {
    submit_offer(
        pool,
        seller,
        request_id,
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(1),
            description: Some("Frost-free 300L".to_owned()),
            price: Some("520.00".to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
            available: Some(true),
            available_in_city: Some(true),
        },
    )
    .await
    .expect("fixture submission submits")
    .id
}

/// Guard verdict without mutating: check inside a transaction, roll back.
async fn guard(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    buyer_id: uuid::Uuid,
    seller_id: uuid::Uuid,
) -> Result<(), ContactLimitError> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let verdict = check_contact_limits(&mut tx, request_id, 1, buyer_id, seller_id).await;
    tx.rollback().await.expect("probe rolls back");
    match verdict {
        Ok(()) => Ok(()),
        Err(AttemptError::Abort(reason)) => Err(reason),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

/// Distinct-seller count inside one probe transaction.
async fn counted(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> u32 {
    let mut tx = pool.begin().await.expect("transaction begins");
    let count = distinct_contact_count(&mut tx, request_id, 1).await;
    tx.rollback().await.expect("probe rolls back");
    match count {
        Ok(count) => count,
        Err(AttemptError::Abort(_)) => panic!("probe refusal impossible"),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

/// Handoff mechanics for guarded compositions: a genuine contact row with
/// frozen snapshots, written with direct SQL so the original `sqlx::Error`
/// reaches the serializable runner. Values resolve live inside the
/// transaction (no stale fixtures); this helper stands in until the wiring
/// card calls the guard inside handoff itself.
async fn record_live_contact(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    buyer: uuid::Uuid,
    seller: uuid::Uuid,
    request_id: uuid::Uuid,
    offer_id: uuid::Uuid,
) -> Result<uuid::Uuid, AttemptError<ContactLimitError>> {
    let demand: Option<(String, i64, i32)> = sqlx::query_as(
        "SELECT title, budget_cents, current_revision_number FROM requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    let (title, budget, revision) =
        demand.ok_or(AttemptError::Abort(ContactLimitError::StorageFailed))?;
    let terms: Option<(String, i64, i32)> = sqlx::query_as(
        "SELECT description, price_cents, current_terms_number FROM offers WHERE id = $1",
    )
    .bind(offer_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    let (description, price, terms_number) =
        terms.ok_or(AttemptError::Abort(ContactLimitError::StorageFailed))?;
    let destination: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT phone_ciphertext FROM users WHERE id = $1")
            .bind(seller)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AttemptError::Db)?;
    let destination = destination.ok_or(AttemptError::Abort(ContactLimitError::StorageFailed))?;
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO contacts
            (handoff_id, buyer_id, seller_id, request_id, cycle_number,
             offer_id, request_revision_number, offer_terms_number,
             request_title, request_budget_cents, offer_description,
             offer_price_cents, destination_ciphertext, entry_source)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7, $8, $9, $10, $11, $12, 'offer_detail')
         RETURNING id",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(buyer)
    .bind(seller)
    .bind(request_id)
    .bind(offer_id)
    .bind(revision)
    .bind(terms_number)
    .bind(title)
    .bind(budget)
    .bind(description)
    .bind(price)
    .bind(destination)
    .fetch_one(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    Ok(id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn final_distinct_slot_contenders_yield_one_winner() {
    let db = TestDatabase::create("p08t05_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t05_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Race Owner", "+55 11 90000-0301").await;
    let demand = publish_open(db.pool(), author, "Refrigerator").await;
    let mut sellers = Vec::new();
    for index in 0..11 {
        let seller = seed_active(
            db.pool(),
            &format!("Race Seller {index}"),
            &format!("+55 11 9000{index:04}"),
        )
        .await;
        submit_open(db.pool(), seller, demand).await;
        sellers.push(seller);
    }
    for seller in sellers.iter().take(9) {
        start_open(db.pool(), author, demand, *seller).await;
    }
    assert_eq!(counted(db.pool(), demand).await, 9);

    // Both contenders observe nine distinct sellers and race for the tenth
    // in serializable transactions: one commits, the other retries,
    // re-reads ten, and takes the allowance refusal.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let contend = |barrier: std::sync::Arc<tokio::sync::Barrier>, seller: uuid::Uuid| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            run_serializable(&pool, |tx, _attempt| {
                Box::pin(async move {
                    let offer: uuid::Uuid = sqlx::query_scalar(
                        "SELECT id FROM offers WHERE request_id = $1 AND seller_id = $2",
                    )
                    .bind(demand)
                    .bind(seller)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(AttemptError::Db)?
                    .ok_or(AttemptError::Abort(ContactLimitError::StorageFailed))?;
                    // Plain reads use the dereferenced connection; guard and
                    // record take the transaction itself (single star).
                    check_contact_limits(&mut *tx, demand, 1, author, seller).await?;
                    record_live_contact(&mut *tx, author, seller, demand, offer).await?;
                    Ok::<_, AttemptError<ContactLimitError>>(())
                })
            })
            .await
        }
    };
    let (first, second) = tokio::join!(
        contend(std::sync::Arc::clone(&barrier), sellers[9]),
        contend(barrier, sellers[10]),
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "exactly one contender takes the final distinct slot"
    );
    assert!(
        outcomes.iter().any(|outcome| matches!(
            outcome,
            Err(
                procurali_backend::persistence::transaction::TransactionError::Aborted(
                    ContactLimitError::AllowanceExhausted
                )
            )
        )),
        "the loser takes the allowance refusal, not a silent success"
    );
    assert_eq!(counted(db.pool(), demand).await, 10);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn eleventh_distinct_seller_is_refused_while_repeats_pass() {
    let db = TestDatabase::create("p08t05_quota")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t05_quota_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Quota Owner", "+55 11 90000-0311").await;
    let demand = publish_open(db.pool(), author, "Refrigerator").await;
    let mut sellers = Vec::new();
    for index in 0..10 {
        let seller = seed_active(
            db.pool(),
            &format!("Quota Seller {index}"),
            &format!("+55 11 9000{index:04}"),
        )
        .await;
        submit_open(db.pool(), seller, demand).await;
        start_open(db.pool(), author, demand, seller).await;
        sellers.push(seller);
    }
    assert_eq!(counted(db.pool(), demand).await, 10);

    // An eleventh distinct seller is refused with no new row, while a
    // repeat for a contacted seller stays eligible and records its fact.
    let eleventh = seed_active(db.pool(), "Quota Eleventh", "+55 11 90000-0320").await;
    submit_open(db.pool(), eleventh, demand).await;
    assert_eq!(
        guard(db.pool(), demand, author, eleventh).await,
        Err(ContactLimitError::AllowanceExhausted)
    );
    assert_eq!(counted(db.pool(), demand).await, 10);
    let repeat = start_open(db.pool(), author, demand, sellers[0]).await;
    assert!(repeat);
    assert_eq!(counted(db.pool(), demand).await, 10);

    // Failed handoffs consume nothing: a stranger attempt refuses with the
    // counts exactly as they stood.
    let stranger = seed_active(db.pool(), "Quota Stranger", "+55 11 90000-0321").await;
    assert!(start_contact_once(db.pool(), stranger, demand, sellers[0])
        .await
        .is_err());
    assert_eq!(counted(db.pool(), demand).await, 10);
    db.cleanup().await.expect("suite cleans up");
}

async fn start_open(
    pool: &sqlx::PgPool,
    buyer: uuid::Uuid,
    request_id: uuid::Uuid,
    seller: uuid::Uuid,
) -> bool {
    use procurali_backend::application::start_contact::{start_contact, ContactInput};
    let offer: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM offers WHERE request_id = $1 AND seller_id = $2")
            .bind(request_id)
            .bind(seller)
            .fetch_one(pool)
            .await
            .expect("offer reads");
    start_contact(
        pool,
        &test_keys(),
        buyer,
        request_id,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts")
    .repeat
}

async fn start_contact_once(
    pool: &sqlx::PgPool,
    buyer: uuid::Uuid,
    request_id: uuid::Uuid,
    seller: uuid::Uuid,
) -> Result<bool, ()> {
    use procurali_backend::application::start_contact::{start_contact, ContactInput};
    let offer: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM offers WHERE request_id = $1 AND seller_id = $2")
            .bind(request_id)
            .bind(seller)
            .fetch_optional(pool)
            .await
            .expect("offer reads");
    let offer = offer.ok_or(())?;
    Ok(start_contact(
        pool,
        &test_keys(),
        buyer,
        request_id,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .map_err(|_| ())?
    .repeat)
}

#[tokio::test]
async fn independent_demands_share_no_winner_or_sale() {
    let db = TestDatabase::create("p08t05_independent")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t05_independent_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let alice = seed_active(db.pool(), "Alice Buyer", "+55 11 90000-0331").await;
    let bruno = seed_active(db.pool(), "Bruno Buyer", "+55 11 90000-0332").await;
    let s1 = seed_active(db.pool(), "Shared Seller One", "+55 11 90000-0333").await;
    let s2 = seed_active(db.pool(), "Shared Seller Two", "+55 11 90000-0334").await;
    let demand_a = publish_open(db.pool(), alice, "Fridge A").await;
    let demand_b = publish_open(db.pool(), bruno, "Fridge B").await;
    for (demand, seller) in [(demand_a, s1), (demand_a, s2), (demand_b, s2)] {
        submit_open(db.pool(), seller, demand).await;
    }

    // Cross-contacted sellers keep independent counts per demand: Alice
    // reaches two distinct sellers, Bruno reaches one, and neither demand
    // labels any sale, reservation, or winner.
    start_open(db.pool(), alice, demand_a, s1).await;
    start_open(db.pool(), alice, demand_a, s2).await;
    start_open(db.pool(), bruno, demand_b, s2).await;
    assert_eq!(counted(db.pool(), demand_a).await, 2);
    assert_eq!(counted(db.pool(), demand_b).await, 1);
    for demand in [demand_a, demand_b] {
        let offers: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT state, terminal_reason FROM offers WHERE request_id = $1")
                .bind(demand)
                .fetch_all(db.pool())
                .await
                .expect("offers read");
        for (state, reason) in &offers {
            assert_eq!(state, "sent");
            assert_eq!(reason, &None);
        }
        let row: (String,) = sqlx::query_as("SELECT state FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("demand reads");
        assert_eq!(row.0, "active");
    }
    db.cleanup().await.expect("suite cleans up");
}
