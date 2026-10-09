//! Feedback acceptance (P14-T01): one observation set per contact.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). There is no feedback HTTP surface yet, so these tests
//! drive the application operations directly over real PostgreSQL — the
//! same direct-proof pattern as the staff-grant cards ("api" here is the
//! audited Rust operation surface). Proves:
//! - the window opens exactly 24 hours after first contact and closes
//!   exactly 14 days after it;
//! - repeated handoffs resolve to one credit with corrections versioned,
//!   never multiplied;
//! - uncontacted strangers, foreign parties, restricted filers, and
//!   fabricated contacts all refuse with nothing stored.
//!
//! Window edges travel synthetic timestamps owned by the verification
//! flow. All phones, names, codes, and keys below are synthetic and
//! reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::feedback::{submit_feedback, FeedbackError, FeedbackInput};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p14t01-test-only-lookup-key",
        encryption_key: "p14t01-test-only-encryption-key",
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

/// One live demand with one offer and one contact through the real
/// operations; return demand, offer, and contact ids.
async fn live_contact(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
    title: &str,
    budget: &str,
) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid) {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some(budget.to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("fixture draft validates");
    let demand = publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id;
    let offer = submit_offer(
        pool,
        seller,
        demand,
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
    .id;
    let contact = start_contact(
        pool,
        &test_keys(),
        author,
        demand,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts")
    .contact_id;
    (demand, offer, contact)
}

fn file_answer(contact: uuid::Uuid, answer: &str) -> FeedbackInput {
    FeedbackInput {
        contact_id: contact,
        answer: answer.to_owned(),
    }
}

async fn backdate_contact(pool: &sqlx::PgPool, contact: uuid::Uuid, age: chrono::Duration) {
    let now = chrono::Utc::now();
    sqlx::query("UPDATE contacts SET initiated_at = $2 WHERE id = $1")
        .bind(contact)
        .bind(now - age)
        .execute(pool)
        .await
        .expect("synthetic contact age applies");
}

#[tokio::test]
async fn window_boundaries_behave_precisely() {
    let db = TestDatabase::create("p14t01_window")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t01_window_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Window Buyer", "+55 11 90000-2701").await;
    let seller = seed_active(db.pool(), "Window Seller", "+55 11 90000-2702").await;
    // Four identical-shape contacts age around the window: before
    // opening, at opening, just inside closing, and just after closing.
    // (Exact edge equality is pinned deterministically in unit tests; a
    // moving clock cannot hold it across two round trips.)
    let mut contacts = Vec::new();
    for (title, budget) in [
        ("Early Fridge", "600.00"),
        ("Opening Fridge", "610.00"),
        ("Closing Fridge", "620.00"),
        ("Late Fridge", "630.00"),
    ] {
        let (_, _, contact) = live_contact(db.pool(), buyer, seller, title, budget).await;
        contacts.push(contact);
    }
    backdate_contact(
        db.pool(),
        contacts[0],
        chrono::Duration::hours(24) - chrono::Duration::minutes(1),
    )
    .await;
    backdate_contact(db.pool(), contacts[1], chrono::Duration::hours(24)).await;
    backdate_contact(
        db.pool(),
        contacts[2],
        chrono::Duration::days(14) - chrono::Duration::minutes(1),
    )
    .await;
    backdate_contact(
        db.pool(),
        contacts[3],
        chrono::Duration::days(14) + chrono::Duration::minutes(1),
    )
    .await;

    assert_eq!(
        submit_feedback(db.pool(), buyer, file_answer(contacts[0], "yes")).await,
        Err(FeedbackError::InvalidState)
    );
    let opened = submit_feedback(db.pool(), buyer, file_answer(contacts[1], "yes"))
        .await
        .expect("window opens exactly at 24 hours");
    assert_eq!(opened.version, 1);
    assert!(!opened.suggest_report);
    let closing = submit_feedback(db.pool(), buyer, file_answer(contacts[2], "no"))
        .await
        .expect("window still open near 14 days");
    assert!(closing.suggest_report);
    assert_eq!(
        submit_feedback(db.pool(), buyer, file_answer(contacts[3], "yes")).await,
        Err(FeedbackError::InvalidState)
    );
    let sets: i64 = sqlx::query_scalar("SELECT count(*) FROM contact_feedback")
        .fetch_one(db.pool())
        .await
        .expect("feedback sets read");
    assert_eq!(sets, 2, "only in-window filings record");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeat_handoffs_keep_single_credit() {
    let db = TestDatabase::create("p14t01_credit")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t01_credit_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Credit Buyer", "+55 11 90000-2703").await;
    let seller = seed_active(db.pool(), "Credit Seller", "+55 11 90000-2704").await;
    let (demand, offer, contact) =
        live_contact(db.pool(), buyer, seller, "Credit Fridge", "600.00").await;
    backdate_contact(db.pool(), contact, chrono::Duration::days(2)).await;

    // The first filing opens the single set; the replayed handoff resolves
    // to the same contact, so the second filing corrects it to version
    // two with revision history — never a second credit.
    let first = submit_feedback(db.pool(), buyer, file_answer(contact, "yes"))
        .await
        .expect("first filing records");
    assert_eq!(first.version, 1);
    let replay = start_contact(
        db.pool(),
        &test_keys(),
        buyer,
        demand,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("replay answers");
    assert!(replay.repeat);
    let corrected = submit_feedback(db.pool(), buyer, file_answer(contact, "no"))
        .await
        .expect("correction records");
    assert_eq!(corrected.id, first.id);
    assert_eq!(corrected.version, 2);
    assert_eq!(corrected.answer, "no");
    assert!(corrected.suggest_report);
    let sets: i64 = sqlx::query_scalar("SELECT count(*) FROM contact_feedback")
        .fetch_one(db.pool())
        .await
        .expect("feedback sets read");
    assert_eq!(sets, 1);
    let revisions: Vec<(Option<String>, String)> = sqlx::query_as(
        "SELECT previous_answer, answer FROM feedback_revisions
         WHERE feedback_id = $1 ORDER BY created_at",
    )
    .bind(first.id)
    .fetch_all(db.pool())
    .await
    .expect("revisions read");
    assert_eq!(
        revisions,
        [
            (None, "yes".to_owned()),
            (Some("yes".to_owned()), "no".to_owned())
        ]
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn stranger_and_foreign_feedback_refused() {
    let db = TestDatabase::create("p14t01_refused")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t01_refused_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Refused Buyer", "+55 11 90000-2705").await;
    let seller = seed_active(db.pool(), "Refused Seller", "+55 11 90000-2706").await;
    let stranger = seed_active(db.pool(), "Refused Stranger", "+55 11 90000-2707").await;
    let (_, _, contact) = live_contact(db.pool(), buyer, seller, "Refused Fridge", "600.00").await;
    backdate_contact(db.pool(), contact, chrono::Duration::days(2)).await;

    // Uncontacted strangers, foreign sellers, restricted filers, and
    // fabricated contacts all refuse with nothing stored.
    assert_eq!(
        submit_feedback(db.pool(), stranger, file_answer(contact, "yes")).await,
        Err(FeedbackError::NotPermitted)
    );
    assert_eq!(
        submit_feedback(db.pool(), seller, file_answer(contact, "yes")).await,
        Err(FeedbackError::NotPermitted)
    );
    assert_eq!(
        submit_feedback(db.pool(), buyer, file_answer(uuid::Uuid::now_v7(), "yes")).await,
        Err(FeedbackError::NotFound)
    );
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(buyer)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        submit_feedback(db.pool(), buyer, file_answer(contact, "yes")).await,
        Err(FeedbackError::NotActive)
    );
    assert_eq!(
        submit_feedback(db.pool(), buyer, file_answer(contact, "bogus")).await,
        Err(FeedbackError::InvalidField)
    );
    let sets: i64 = sqlx::query_scalar("SELECT count(*) FROM contact_feedback")
        .fetch_one(db.pool())
        .await
        .expect("feedback sets read");
    assert_eq!(sets, 0);
    db.cleanup().await.expect("suite cleans up");
}
