//! Contact-storage acceptance (P08-T01): frozen snapshots, idempotent ids.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - reloading a contact preserves its original item and price after the
//!   offer terms move on, with the destination decryptable to the number
//!   frozen at initiation;
//! - two identical handoff identities converge on one unique contact while
//!   a genuinely later handoff records a repeat without a second row;
//! - public projections and the schema itself keep the destination out of
//!   every non-ciphertext shape.
//!
//! Setup publishes and submissions travel the real operations. All phones,
//! names, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::contact_replay::{replay_contact, ReplayError};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::contacts::{
    contact, contacts_for_offer, initiate_contact, to_public, ContactError, NewContact,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys, shared between seeding and decryption: the stored
/// destination must open with exactly these.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p08t01-test-only-lookup-key",
        encryption_key: "p08t01-test-only-encryption-key",
    }
}

const SELLER_CANARY_DIGITS: &str = "5511900000202";

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
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some("Refrigerator".to_owned()),
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

fn new_contact(
    buyer: uuid::Uuid,
    seller: uuid::Uuid,
    request: uuid::Uuid,
    offer: uuid::Uuid,
) -> NewContact {
    NewContact {
        handoff_id: uuid::Uuid::now_v7(),
        buyer_id: buyer,
        seller_id: seller,
        request_id: request,
        cycle_number: 1,
        offer_id: offer,
        entry_source: "offer_detail".to_owned(),
    }
}

async fn initiate(
    pool: &sqlx::PgPool,
    input: NewContact,
) -> Result<procurali_backend::persistence::contacts::InitiatedContact, ContactError> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let initiated = initiate_contact(&mut tx, input).await;
    match initiated {
        Ok(initiated) => {
            tx.commit().await.expect("initiation commits");
            Ok(initiated)
        }
        Err(error) => {
            tx.rollback().await.expect("refusal rolls back");
            Err(error)
        }
    }
}

#[tokio::test]
async fn reload_preserves_original_item_and_price() {
    let db = TestDatabase::create("p08t01_history")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t01_history_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "History Owner", "+55 11 90000-0201").await;
    let seller = seed_active(db.pool(), "History Seller", "+55 11 90000-0202").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    let initiated = initiate(db.pool(), new_contact(author, seller, request_id, offer_id))
        .await
        .expect("initiation records");
    assert!(!initiated.repeat);
    assert_eq!(initiated.contact.request_revision_number, 1);
    assert_eq!(initiated.contact.offer_terms_number, 1);
    assert_eq!(initiated.contact.offer_price_cents, 52_000);

    // The offer terms move on through the real edit path; the frozen
    // contact still reads exactly what both parties acted on.
    procurali_backend::application::edit_offer::edit_offer(
        db.pool(),
        seller,
        offer_id,
        procurali_backend::application::edit_offer::EditTermsInput {
            expected_terms_number: Some(1),
            description: Some("Frost-free 350L".to_owned()),
            price: Some("550.00".to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("fixture edit edits");
    let reloaded = contact(db.pool(), initiated.contact.id)
        .await
        .expect("contact reads")
        .expect("contact reads");
    assert_eq!(reloaded.offer_description, "Frost-free 300L");
    assert_eq!(reloaded.offer_price_cents, 52_000);
    assert_eq!(reloaded.request_title, "Refrigerator");
    assert_eq!(reloaded.request_budget_cents, 60_000);
    assert_eq!(reloaded.offer_terms_number, 1);

    // The destination history decrypts to the number frozen at initiation.
    let decrypted: String = sqlx::query_scalar("SELECT pgp_sym_decrypt($1, $2)")
        .bind(&reloaded.destination_ciphertext)
        .bind("p08t01-test-only-encryption-key")
        .fetch_one(db.pool())
        .await
        .expect("destination decrypts");
    assert!(
        decrypted.contains(SELLER_CANARY_DIGITS),
        "frozen destination matches the seller number"
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_action_identities_create_one_unique_contact() {
    let db = TestDatabase::create("p08t01_identity")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t01_identity_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Identity Owner", "+55 11 90000-0203").await;
    let seller = seed_active(db.pool(), "Identity Seller", "+55 11 90000-0204").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    let handoff_id = uuid::Uuid::now_v7();

    // Two identical handoff identities race in separate transactions: the
    // unique constraint lets exactly one insert through, and the loser
    // falls back to the standing row.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let attempt = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            initiate(
                &pool,
                NewContact {
                    handoff_id,
                    buyer_id: author,
                    seller_id: seller,
                    request_id,
                    cycle_number: 1,
                    offer_id,
                    entry_source: "offer_detail".to_owned(),
                },
            )
            .await
        }
    };
    let (first, second) = tokio::join!(attempt(std::sync::Arc::clone(&barrier)), attempt(barrier),);
    let (first, second) = (
        first.expect("racer responds"),
        second.expect("racer responds"),
    );
    assert_eq!(first.contact.id, second.contact.id);
    assert_eq!(
        contacts_for_offer(db.pool(), offer_id)
            .await
            .expect("contacts read")
            .len(),
        1,
        "one unique contact stands"
    );
    // Same-handoff retries record no fact: the race converged silently.
    let repeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'contact' AND kind = 'contact.repeated'",
    )
    .fetch_one(db.pool())
    .await
    .expect("repeat facts read");
    assert_eq!(repeats, 0);

    // A genuinely later handoff records a repeat event on the same row:
    // still one row, now with one repeat fact.
    let repeated = initiate(db.pool(), new_contact(author, seller, request_id, offer_id))
        .await
        .expect("later handoff records");
    assert!(repeated.repeat);
    assert_eq!(repeated.contact.id, first.contact.id);
    assert_eq!(
        contacts_for_offer(db.pool(), offer_id)
            .await
            .expect("contacts read")
            .len(),
        1
    );
    let repeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'contact' AND kind = 'contact.repeated'",
    )
    .fetch_one(db.pool())
    .await
    .expect("repeat facts read");
    assert_eq!(repeats, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn public_projections_cannot_include_historical_destination() {
    let db = TestDatabase::create("p08t01_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t01_privacy_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Privacy Owner", "+55 11 90000-0205").await;
    let seller = seed_active(db.pool(), "Privacy Seller", "+55 11 90000-0206").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    let initiated = initiate(db.pool(), new_contact(author, seller, request_id, offer_id))
        .await
        .expect("initiation records");

    // The public allowlist carries context only: no parties, no
    // destination, no ciphertext, no phone material, no canary digits.
    let rendered =
        serde_json::to_string(&to_public(&initiated.contact, false)).expect("public serializes");
    assert!(rendered.contains("520.00"), "terms stay visible");
    for absent in [
        "buyer",
        "seller",
        "destination",
        "cipher",
        "phone",
        "lookup",
        "token",
        "session",
        "address",
        "handoff",
        "90000",
        SELLER_CANARY_DIGITS,
    ] {
        assert!(!rendered.contains(absent), "no {absent} in public output");
    }
    // The schema keeps exactly one destination column, binary and
    // ciphertext-only; no plaintext phone column exists anywhere here.
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT column_name, udt_name FROM information_schema.columns
         WHERE table_name = 'contacts'",
    )
    .fetch_all(db.pool())
    .await
    .expect("columns read");
    assert!(columns
        .iter()
        .any(|(name, udt)| name == "destination_ciphertext" && udt == "bytea"));
    for (name, _) in &columns {
        assert!(
            ![
                "phone",
                "phone_number",
                "phone_plaintext",
                "plaintext",
                "destination"
            ]
            .contains(&name.as_str()),
            "no plaintext destination column"
        );
    }
    db.cleanup().await.expect("suite cleans up");
}

/// Replay helpers: phone keys shared with seeding so decryption matches.
fn replay_keys<'a>() -> procurali_backend::persistence::users::PhoneKeys<'a> {
    procurali_backend::persistence::users::PhoneKeys {
        lookup_key: "p08t01-test-only-lookup-key",
        encryption_key: "p08t01-test-only-encryption-key",
    }
}

async fn start_handoff(
    pool: &sqlx::PgPool,
    buyer: uuid::Uuid,
    request: uuid::Uuid,
    offer: uuid::Uuid,
    handoff: uuid::Uuid,
) -> procurali_backend::application::start_contact::Handoff {
    start_contact(
        pool,
        &replay_keys(),
        buyer,
        request,
        offer,
        ContactInput {
            handoff_id: Some(handoff.to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("handoff starts")
}

#[tokio::test]
async fn same_retry_records_one_unique_contact() {
    let db = TestDatabase::create("p08t04_retry")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t04_retry_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Retry Owner", "+55 11 90000-0211").await;
    let seller = seed_active(db.pool(), "Retry Seller", "+55 11 90000-0212").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    let handoff_id = uuid::Uuid::now_v7();

    // The first initiation records the unique contact; every replay of the
    // same action returns it with no new row and no new fact.
    let first = start_handoff(db.pool(), author, request_id, offer_id, handoff_id).await;
    assert!(!first.repeat);
    for _ in 0..2 {
        let replayed = replay_contact(db.pool(), &replay_keys(), author, handoff_id)
            .await
            .expect("retry replays");
        assert_eq!(replayed.contact_id, first.contact_id);
        assert_eq!(replayed.destination, first.destination);
    }
    assert_eq!(
        contacts_for_offer(db.pool(), offer_id)
            .await
            .expect("contacts read")
            .len(),
        1,
        "one unique contact stands"
    );
    let repeats: i64 =
        sqlx::query_scalar("SELECT count(*) FROM business_events WHERE resource_kind = 'contact'")
            .fetch_one(db.pool())
            .await
            .expect("contact facts read");
    assert_eq!(repeats, 0, "replays record nothing");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn later_valid_handoff_records_repeat_without_inflation() {
    let db = TestDatabase::create("p08t04_later")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t04_later_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Later Owner", "+55 11 90000-0213").await;
    let seller = seed_active(db.pool(), "Later Seller", "+55 11 90000-0214").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    let first = start_handoff(
        db.pool(),
        author,
        request_id,
        offer_id,
        uuid::Uuid::now_v7(),
    )
    .await;
    assert!(!first.repeat);
    // A genuinely later handoff replays eligibility, returns the standing
    // row flagged repeat, and records exactly one repeat fact.
    let second = start_handoff(
        db.pool(),
        author,
        request_id,
        offer_id,
        uuid::Uuid::now_v7(),
    )
    .await;
    assert!(second.repeat);
    assert_eq!(second.contact_id, first.contact_id);
    assert_eq!(
        contacts_for_offer(db.pool(), offer_id)
            .await
            .expect("contacts read")
            .len(),
        1
    );
    let repeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'contact' AND kind = 'contact.repeated'",
    )
    .fetch_one(db.pool())
    .await
    .expect("repeat facts read");
    assert_eq!(repeats, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn restriction_before_replay_returns_no_destination() {
    let db = TestDatabase::create("p08t04_restricted")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t04_restricted_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Restricted Owner", "+55 11 90000-0215").await;
    let seller = seed_active(db.pool(), "Restricted Seller", "+55 11 90000-0216").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    let handoff_id = uuid::Uuid::now_v7();
    let first = start_handoff(db.pool(), author, request_id, offer_id, handoff_id).await;
    assert!(first.destination.contains("5511900000216"));

    // Withdrawal first: the recorded handoff replays into a refusal with no
    // destination, even though it succeeded minutes earlier.
    sqlx::query("UPDATE offers SET state = 'withdrawn' WHERE id = $1")
        .bind(offer_id)
        .execute(db.pool())
        .await
        .expect("synthetic withdrawal applies");
    assert_eq!(
        replay_contact(db.pool(), &replay_keys(), author, handoff_id).await,
        Err(ReplayError::ForbiddenState)
    );
    // A block lands the same way on a fresh demand.
    let blocked_request = publish_open(db.pool(), author).await;
    let blocked_offer = submit_open(db.pool(), seller, blocked_request).await;
    let blocked_handoff = uuid::Uuid::now_v7();
    start_handoff(
        db.pool(),
        author,
        blocked_request,
        blocked_offer,
        blocked_handoff,
    )
    .await;
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)")
        .bind(author)
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic block applies");
    assert_eq!(
        replay_contact(db.pool(), &replay_keys(), author, blocked_handoff).await,
        Err(ReplayError::Blocked)
    );
    // Suspension behaves identically, and a stranger's guess never resolves.
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        replay_contact(db.pool(), &replay_keys(), author, blocked_handoff).await,
        Err(ReplayError::SellerNotActive)
    );
    let stranger = seed_active(db.pool(), "Replay Stranger", "+55 11 90000-0217").await;
    assert_eq!(
        replay_contact(db.pool(), &replay_keys(), stranger, handoff_id).await,
        Err(ReplayError::NotFound)
    );

    // Control: a verified phone change flows into replays while history
    // keeps the number valid at initiation (EC-21 both directions).
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic restoration applies");
    sqlx::query("DELETE FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
        .bind(author)
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic unblock applies");
    let fresh_request = publish_open(db.pool(), author).await;
    let fresh_offer = submit_open(db.pool(), seller, fresh_request).await;
    let fresh_handoff = uuid::Uuid::now_v7();
    let before = start_handoff(db.pool(), author, fresh_request, fresh_offer, fresh_handoff).await;
    assert!(before.destination.contains("5511900000216"));
    sqlx::query(
        "UPDATE users SET phone_ciphertext = pgp_sym_encrypt($2, $3),
                 phone_lookup = encode(hmac(convert_to($2, 'UTF8'), convert_to($4, 'UTF8'), 'sha256'), 'hex')
         WHERE id = $1",
    )
    .bind(seller)
    .bind("+5511900000299")
    .bind("p08t01-test-only-encryption-key")
    .bind("p08t01-test-only-lookup-key")
    .execute(db.pool())
    .await
    .expect("synthetic verified number change applies");
    let replayed = replay_contact(db.pool(), &replay_keys(), author, fresh_handoff)
        .await
        .expect("replay follows the verified number");
    assert!(replayed.destination.contains("5511900000299"));
    let frozen = contact(db.pool(), before.contact_id)
        .await
        .expect("contact reads")
        .expect("contact reads");
    let decrypted: String = sqlx::query_scalar("SELECT pgp_sym_decrypt($1, $2)")
        .bind(&frozen.destination_ciphertext)
        .bind("p08t01-test-only-encryption-key")
        .fetch_one(db.pool())
        .await
        .expect("frozen destination decrypts");
    assert!(decrypted.contains("5511900000216"));
    db.cleanup().await.expect("suite cleans up");
}
