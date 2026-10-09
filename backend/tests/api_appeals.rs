//! Appeal acceptance (P11-T07): bounded review without rewriting.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). There is no appeal HTTP surface yet, so these tests drive
//! the application operations directly over real PostgreSQL — the same
//! direct-proof pattern as the staff-grant cards ("api" here is the
//! audited Rust operation surface, with HTTP routes landing on later
//! work). Proves:
//! - a restricted account files its own-history appeal with no
//!   marketplace power granted;
//! - repeats converge on the standing appeal while later material
//!   evidence appends as its own rows, multiplying no case;
//! - reversal preserves the original record and an independent block:
//!   the case returns to review, facts accumulate, and terminal
//!   requests, contacts, and blocks stand untouched.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::appeals::{
    decide_appeal, file_appeal, submit_evidence, AppealDecisionInput, AppealError, AppealInput,
};
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::create_report::ReportInput;
use procurali_backend::application::moderation_review::{
    decide_case, open_review, DecideInput, OpenReviewInput,
};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::report_updates::submit_grouped_report;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::start_contact::{start_contact, ContactError, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::suspend_user::{suspend_user, SuspendUserInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p11t07-test-only-lookup-key",
        encryption_key: "p11t07-test-only-encryption-key",
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

/// One moderator behind the real audited grant chain; return its id.
async fn seed_moderator(pool: &sqlx::PgPool) -> uuid::Uuid {
    let operator = seed_active(pool, "Appeal Operator", "+55 11 90000-1301").await;
    bootstrap_grant(
        pool,
        BootstrapInput {
            user_id: operator,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "launch cover".to_owned(),
            operator_label: "launch-operator-1".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("launch bootstrap grants");
    let moderator = seed_active(pool, "Appeal Moderator", "+55 11 90000-1302").await;
    grant_role(
        pool,
        operator,
        GrantInput {
            user_id: moderator,
            role: "moderator".to_owned(),
            scope: "safety".to_owned(),
            reason: "triage cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("moderator granted");
    moderator
}

/// One live demand with one offer through the real operations; return ids.
async fn live_offer(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
) -> (uuid::Uuid, uuid::Uuid) {
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
    (demand, offer)
}

/// One decided case from a buyer allegation; return its id.
async fn decided_case(
    pool: &sqlx::PgPool,
    moderator: uuid::Uuid,
    buyer: uuid::Uuid,
    offer: uuid::Uuid,
    disposition: &str,
) -> uuid::Uuid {
    let filed = submit_grouped_report(
        pool,
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer,
            reason: "spam".to_owned(),
            detail: "appeal fixture noise".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");
    open_review(
        pool,
        moderator,
        filed.case_id,
        OpenReviewInput {
            severity: "normal".to_owned(),
            policy_version: "v1".to_owned(),
            purpose: "fixture triage".to_owned(),
        },
    )
    .await
    .expect("fixture review opens");
    decide_case(
        pool,
        moderator,
        filed.case_id,
        DecideInput {
            disposition: disposition.to_owned(),
            reason: "fixture finding".to_owned(),
            evidence: None,
            policy_version: "v1".to_owned(),
            purpose: "fixture decision".to_owned(),
        },
    )
    .await
    .expect("fixture decision records");
    filed.case_id
}

#[tokio::test]
async fn restricted_own_history_appeal_without_privileges() {
    let db = TestDatabase::create("p11t07_restricted")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t07_restricted_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let moderator = seed_moderator(db.pool()).await;
    let buyer = seed_active(db.pool(), "Restricted Buyer", "+55 11 90000-1303").await;
    let seller = seed_active(db.pool(), "Restricted Seller", "+55 11 90000-1304").await;
    let (_, offer) = live_offer(db.pool(), buyer, seller).await;
    let case_id = decided_case(db.pool(), moderator, buyer, offer, "invalid").await;
    suspend_user(
        db.pool(),
        moderator,
        SuspendUserInput {
            user_id: buyer,
            reason: "repeated low-severity abuse after warning".to_owned(),
            duration_hours: 24,
            policy_version: "v1".to_owned(),
            purpose: "pause pending review".to_owned(),
            case_id: None,
        },
    )
    .await
    .expect("buyer suspends");

    // The suspended reporter challenges the invalid finding on their own
    // history: the appeal opens, the suspension stands, and no
    // marketplace row appears anywhere.
    let appeal = file_appeal(
        db.pool(),
        buyer,
        AppealInput {
            case_id,
            grounds: "the finding missed the contact pattern".to_owned(),
        },
    )
    .await
    .expect("restricted own-history appeal files");
    assert_eq!(appeal.status, "open");
    assert_eq!(appeal.appellant_id, buyer);
    let buyer_state: String = sqlx::query_scalar("SELECT state FROM users WHERE id = $1")
        .bind(buyer)
        .fetch_one(db.pool())
        .await
        .expect("buyer reads");
    assert_eq!(buyer_state, "suspended");
    let offers: i64 = sqlx::query_scalar("SELECT count(*) FROM offers")
        .fetch_one(db.pool())
        .await
        .expect("offers read");
    assert_eq!(offers, 1);
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 0);
    let cases: i64 = sqlx::query_scalar("SELECT count(*) FROM report_cases")
        .fetch_one(db.pool())
        .await
        .expect("cases read");
    assert_eq!(cases, 1, "appeals multiply no case");

    // The appeal row carries the filer's own challenge only: other case
    // parties and phone material are absent from its shape.
    let rendered = serde_json::to_string(&appeal).expect("appeal serializes");
    assert!(rendered.contains("the finding missed the contact pattern"));
    let seller_text = seller.to_string();
    let moderator_text = moderator.to_string();
    for absent in [
        seller_text.as_str(),
        moderator_text.as_str(),
        "reporter",
        "phone",
        "lookup",
        "cipher",
        "token",
        "session",
        "address",
        "secret",
        "90000",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in appeal output");
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeat_converges_new_evidence_records() {
    let db = TestDatabase::create("p11t07_repeat")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t07_repeat_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let moderator = seed_moderator(db.pool()).await;
    let buyer = seed_active(db.pool(), "Repeat Buyer", "+55 11 90000-1305").await;
    let seller = seed_active(db.pool(), "Repeat Seller", "+55 11 90000-1306").await;
    let (_, offer) = live_offer(db.pool(), buyer, seller).await;
    let case_id = decided_case(db.pool(), moderator, buyer, offer, "valid").await;

    // Filing twice — same or restated grounds — converges on the one
    // appeal with the original statement immutable.
    let first = file_appeal(
        db.pool(),
        buyer,
        AppealInput {
            case_id,
            grounds: "the finding missed context".to_owned(),
        },
    )
    .await
    .expect("first appeal files");
    for grounds in [
        "the finding missed context",
        "restated with more words but same challenge",
    ] {
        let repeat = file_appeal(
            db.pool(),
            buyer,
            AppealInput {
                case_id,
                grounds: grounds.to_owned(),
            },
        )
        .await
        .expect("repeat converges");
        assert_eq!(repeat.id, first.id);
    }
    let appeals: i64 = sqlx::query_scalar("SELECT count(*) FROM appeals")
        .fetch_one(db.pool())
        .await
        .expect("appeals read");
    assert_eq!(appeals, 1);
    let grounds: String = sqlx::query_scalar("SELECT grounds FROM appeals WHERE id = $1")
        .bind(first.id)
        .fetch_one(db.pool())
        .await
        .expect("grounds read");
    assert_eq!(grounds, "the finding missed context");

    // Later material appends as its own rows; a second case party holds
    // their own appeal; foreign hands and decided appeals refuse.
    let one = submit_evidence(
        db.pool(),
        buyer,
        first.id,
        "contact timestamps supporting the challenge".to_owned(),
    )
    .await
    .expect("first evidence records");
    let two = submit_evidence(
        db.pool(),
        buyer,
        first.id,
        "seller message transcript".to_owned(),
    )
    .await
    .expect("second evidence records");
    assert_ne!(one.id, two.id);
    let statements: Vec<String> = sqlx::query_scalar(
        "SELECT statement FROM appeal_evidence WHERE appeal_id = $1 ORDER BY created_at",
    )
    .bind(first.id)
    .fetch_all(db.pool())
    .await
    .expect("evidence reads");
    assert_eq!(statements.len(), 2);
    let second = file_appeal(
        db.pool(),
        seller,
        AppealInput {
            case_id,
            grounds: "seller counter-statement".to_owned(),
        },
    )
    .await
    .expect("target owner holds their own appeal");
    assert_ne!(second.id, first.id);
    assert_eq!(
        submit_evidence(db.pool(), buyer, second.id, "meddling".to_owned()).await,
        Err(AppealError::NotPermitted)
    );
    decide_appeal(
        db.pool(),
        moderator,
        first.id,
        AppealDecisionInput {
            outcome: "upheld".to_owned(),
            reason: "finding stands on the evidence".to_owned(),
            policy_version: "v1".to_owned(),
            purpose: "appeal decision".to_owned(),
        },
    )
    .await
    .expect("appeal decided");
    assert_eq!(
        submit_evidence(db.pool(), buyer, first.id, "too late".to_owned()).await,
        Err(AppealError::InvalidState)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn correction_preserves_record_and_block() {
    let db = TestDatabase::create("p11t07_correct")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t07_correct_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let moderator = seed_moderator(db.pool()).await;
    let buyer = seed_active(db.pool(), "Correct Buyer", "+55 11 90000-1307").await;
    let seller = seed_active(db.pool(), "Correct Seller", "+55 11 90000-1308").await;
    let (demand, offer) = live_offer(db.pool(), buyer, seller).await;
    let handoff = start_contact(
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
    .expect("fixture handoff starts");
    assert!(!handoff.repeat);
    let case_id = decided_case(db.pool(), moderator, buyer, offer, "valid").await;
    close_request(
        db.pool(),
        buyer,
        demand,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("demand completes");
    let appeal = file_appeal(
        db.pool(),
        buyer,
        AppealInput {
            case_id,
            grounds: "new exonerating context".to_owned(),
        },
    )
    .await
    .expect("appeal files");
    block_user(db.pool(), buyer, seller)
        .await
        .expect("pair blocks");

    // Reversal returns the case to review with separate correction facts,
    // while the terminal demand, the contact history, and the block stand
    // exactly as they were.
    let decided = decide_appeal(
        db.pool(),
        moderator,
        appeal.id,
        AppealDecisionInput {
            outcome: "reversed".to_owned(),
            reason: "finding contradicted by contact history".to_owned(),
            policy_version: "v1".to_owned(),
            purpose: "appeal decision".to_owned(),
        },
    )
    .await
    .expect("appeal reversed");
    assert_eq!(decided.status, "reversed");
    let case_status: String = sqlx::query_scalar("SELECT status FROM report_cases WHERE id = $1")
        .bind(case_id)
        .fetch_one(db.pool())
        .await
        .expect("case reads");
    assert_eq!(case_status, "review");
    for kind in [
        "case.decided",
        "case.reopened",
        "appeal.decided",
        "appeal.corrected",
    ] {
        let facts: i64 = sqlx::query_scalar("SELECT count(*) FROM business_events WHERE kind = $1")
            .bind(kind)
            .fetch_one(db.pool())
            .await
            .expect("facts read");
        assert_eq!(facts, 1, "one {kind} fact stands");
    }
    let demand_state: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("demand reads");
    assert_eq!(demand_state, ("completed".to_owned(), "public".to_owned()));
    let stored_price: i64 =
        sqlx::query_scalar("SELECT offer_price_cents FROM contacts WHERE offer_id = $1")
            .bind(offer)
            .fetch_one(db.pool())
            .await
            .expect("contact reads");
    assert_eq!(stored_price, 52_000);
    let block_stands: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
            .bind(buyer)
            .bind(seller)
            .fetch_optional(db.pool())
            .await
            .expect("block reads");
    assert!(
        block_stands.is_some(),
        "independent block survives reversal"
    );
    // The block's invalidation terminalized the offer, so the retry meets
    // the terminal row first: still refused with no destination, and the
    // block row behind it stands untouched by the reversal.
    let retry = start_contact(
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
    .await;
    assert!(matches!(retry, Err(ContactError::ForbiddenState)));
    db.cleanup().await.expect("suite cleans up");
}
