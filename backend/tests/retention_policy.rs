//! Retention-policy acceptance (P12-T02): windows with owners.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Classification, review, and release travel the real audited
//! operations over real PostgreSQL (no retention HTTP surface exists yet —
//! "api" here is the audited Rust operation surface, the same direct-proof
//! pattern as the staff-grant cards). Proves:
//! - ordinary and incident fixtures receive distinct due dates and
//!   purposes from the canonical windows;
//! - an unreviewed expired hold stays open without silently becoming
//!   permanent, and only explicit review or release moves it;
//! - obligation exceptions need basis, scope, owner, and review condition
//!   from an administrator, with regular and moderator callers refused.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use chrono::SubsecRound;
use database::TestDatabase;
use procurali_backend::application::create_report::ReportInput;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::report_updates::submit_grouped_report;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::retention_policy::{
    classify_hold, due_holds, release_hold, review_hold, ClassifyInput, RetentionError,
};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p12t02-test-only-lookup-key",
        encryption_key: "p12t02-test-only-encryption-key",
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

/// One moderator plus one administrator behind the real audited grant
/// chain; return both ids.
async fn seed_staff(pool: &sqlx::PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let operator = seed_active(pool, "Retention Operator", "+55 11 90000-1601").await;
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
    let moderator = seed_active(pool, "Retention Moderator", "+55 11 90000-1602").await;
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
    let admin = seed_active(pool, "Retention Admin", "+55 11 90000-1603").await;
    grant_role(
        pool,
        operator,
        GrantInput {
            user_id: admin,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "duty cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("administrator granted");
    (moderator, admin)
}

/// One live demand with one uncontacted offer through the real operations.
async fn live_offer_only(
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

/// One live demand with one contacted offer through the real operations.
async fn live_contact(
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
    start_contact(
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
    .expect("fixture handoff starts");
    (demand, offer)
}

fn ordinary_input(
    subject_kind: &str,
    subject_id: uuid::Uuid,
    anchor: chrono::DateTime<chrono::Utc>,
) -> ClassifyInput {
    ClassifyInput {
        subject_kind: subject_kind.to_owned(),
        subject_id,
        class: "ordinary".to_owned(),
        purpose: "recent dispute cover".to_owned(),
        anchor_at: anchor,
        ends_at: None,
        basis: None,
        review_condition: None,
        owner_id: None,
    }
}

#[tokio::test]
async fn ordinary_and_incident_get_distinct_dues_and_purposes() {
    let db = TestDatabase::create("p12t02_windows")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t02_windows_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let (moderator, _) = seed_staff(db.pool()).await;
    let buyer = seed_active(db.pool(), "Window Buyer", "+55 11 90000-1604").await;
    let seller = seed_active(db.pool(), "Window Seller", "+55 11 90000-1605").await;
    let (_, offer) = live_contact(db.pool(), buyer, seller).await;
    let contact_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM contacts WHERE offer_id = $1")
        .bind(offer)
        .fetch_one(db.pool())
        .await
        .expect("contact reads");

    // Ordinary classification runs 90 days from its anchor — here an older
    // closure the operation backfills (truncated to microseconds: the
    // database round-trips timestamps exactly) — while incident intake
    // reviews 90 days from now, each with its own stated purpose.
    let anchor = (chrono::Utc::now() - chrono::Duration::days(30)).round_subsecs(6);
    let ordinary = classify_hold(
        db.pool(),
        None,
        ordinary_input("contact", contact_id, anchor),
    )
    .await
    .expect("ordinary hold classifies");
    assert_eq!(ordinary.class, "ordinary");
    assert_eq!(ordinary.due_at, anchor + chrono::Duration::days(90));
    assert_eq!(ordinary.created_by, None);
    let filed = submit_grouped_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer,
            reason: "spam".to_owned(),
            detail: "window fixture noise".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");
    let before = chrono::Utc::now();
    let incident = classify_hold(
        db.pool(),
        Some(moderator),
        ClassifyInput {
            subject_kind: "case".to_owned(),
            subject_id: filed.case_id,
            class: "incident".to_owned(),
            purpose: "open fraud triage".to_owned(),
            anchor_at: before,
            ends_at: None,
            basis: None,
            review_condition: None,
            owner_id: None,
        },
    )
    .await
    .expect("incident hold classifies");
    assert_eq!(incident.class, "incident");
    assert!(incident.due_at >= before + chrono::Duration::days(90));
    assert!(incident.due_at <= chrono::Utc::now() + chrono::Duration::days(90));
    assert_ne!(ordinary.purpose, incident.purpose);
    assert_ne!(ordinary.due_at, incident.due_at);
    assert_eq!(ordinary.created_by, None);
    assert_eq!(incident.created_by, Some(moderator));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn expired_hold_never_silently_permanent() {
    let db = TestDatabase::create("p12t02_expired")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t02_expired_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let (moderator, _) = seed_staff(db.pool()).await;
    let buyer = seed_active(db.pool(), "Expired Buyer", "+55 11 90000-1606").await;
    let seller = seed_active(db.pool(), "Expired Seller", "+55 11 90000-1607").await;
    let (_, offer) = live_contact(db.pool(), buyer, seller).await;
    let contact_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM contacts WHERE offer_id = $1")
        .bind(offer)
        .fetch_one(db.pool())
        .await
        .expect("contact reads");

    // A hold whose due date passed stays open: expiry writes nothing and
    // flips nothing by itself.
    let anchor = chrono::Utc::now() - chrono::Duration::days(100);
    let hold = classify_hold(
        db.pool(),
        Some(moderator),
        ordinary_input("contact", contact_id, anchor),
    )
    .await
    .expect("ordinary hold classifies");
    assert_eq!(hold.status, "open");
    assert!(hold.due_at < chrono::Utc::now());
    let reread: (String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT status, due_at FROM retention_holds WHERE id = $1")
            .bind(hold.id)
            .fetch_one(db.pool())
            .await
            .expect("hold re-reads");
    assert_eq!(reread.0, "open");
    assert_eq!(reread.1, hold.due_at);
    // No permanent standing exists to flip into: the backstop refuses it.
    let permanent = sqlx::query("UPDATE retention_holds SET status = 'permanent' WHERE id = $1")
        .bind(hold.id)
        .execute(db.pool())
        .await;
    assert!(
        permanent.is_err(),
        "CHECK backstop refuses permanent status"
    );
    // The worker surface reports the expired hold as-is, and only
    // explicit review or release moves it — ordinary review keeps its due.
    let due = due_holds(db.pool(), 10).await.expect("due holds read");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, hold.id);
    let reviewed = review_hold(
        db.pool(),
        moderator,
        hold.id,
        "dispute window still relevant".to_owned(),
    )
    .await
    .expect("expired hold reviews explicitly");
    assert_eq!(reviewed.status, "open");
    assert_eq!(reviewed.due_at, hold.due_at);
    assert!(reviewed.reviewed_at.is_some());
    let released = release_hold(
        db.pool(),
        moderator,
        hold.id,
        "dispute window elapsed".to_owned(),
    )
    .await
    .expect("hold releases explicitly");
    assert_eq!(released.status, "released");
    assert!(released.released_at.is_some());
    let again = release_hold(db.pool(), moderator, hold.id, "repeat release".to_owned())
        .await
        .expect("repeat release converges");
    assert_eq!(again.id, released.id);
    assert_eq!(
        review_hold(db.pool(), moderator, hold.id, "too late".to_owned()).await,
        Err(RetentionError::InvalidState)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn exception_needs_basis_scope_owner_review_admin_only() {
    let db = TestDatabase::create("p12t02_exception")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t02_exception_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let (moderator, admin) = seed_staff(db.pool()).await;
    let buyer = seed_active(db.pool(), "Exception Buyer", "+55 11 90000-1608").await;
    let seller = seed_active(db.pool(), "Exception Seller", "+55 11 90000-1609").await;
    let (_, offer) = live_offer_only(db.pool(), buyer, seller).await;
    let filed = submit_grouped_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer,
            reason: "fraud".to_owned(),
            detail: "exception fixture claim".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");

    // Moderators and regular accounts cannot mint obligation exceptions.
    let mut attempted = ClassifyInput {
        subject_kind: "case".to_owned(),
        subject_id: filed.case_id,
        class: "exception".to_owned(),
        purpose: "court order cover".to_owned(),
        anchor_at: chrono::Utc::now(),
        ends_at: Some(chrono::Utc::now() + chrono::Duration::days(365)),
        basis: Some("court order 123".to_owned()),
        review_condition: Some("review on order expiry".to_owned()),
        owner_id: Some(admin),
    };
    assert_eq!(
        classify_hold(db.pool(), Some(moderator), attempted.clone()).await,
        Err(RetentionError::NotPermitted)
    );
    assert_eq!(
        classify_hold(db.pool(), Some(buyer), attempted.clone()).await,
        Err(RetentionError::NotPermitted)
    );
    // Undocumented exceptions refuse field by field.
    attempted.basis = None;
    assert_eq!(
        classify_hold(db.pool(), Some(admin), attempted.clone()).await,
        Err(RetentionError::InvalidField)
    );
    attempted.basis = Some("court order 123".to_owned());
    attempted.owner_id = None;
    assert_eq!(
        classify_hold(db.pool(), Some(admin), attempted.clone()).await,
        Err(RetentionError::InvalidField)
    );
    attempted.owner_id = Some(admin);
    attempted.ends_at = Some(chrono::Utc::now() - chrono::Duration::days(1));
    assert_eq!(
        classify_hold(db.pool(), Some(admin), attempted.clone()).await,
        Err(RetentionError::InvalidField)
    );

    // A documented administrator exception records with its exact end and
    // named accountability, scoped to its subject.
    attempted.ends_at = Some(chrono::Utc::now() + chrono::Duration::days(365));
    let held = classify_hold(db.pool(), Some(admin), attempted.clone())
        .await
        .expect("documented exception classifies");
    assert_eq!(held.class, "exception");
    assert_eq!(held.subject_id, filed.case_id);
    assert_eq!(held.basis.as_deref(), Some("court order 123"));
    assert_eq!(held.owner_id, Some(admin));
    assert_eq!(
        held.review_condition.as_deref(),
        Some("review on order expiry")
    );
    assert!(held.due_at > chrono::Utc::now());
    db.cleanup().await.expect("suite cleans up");
}
