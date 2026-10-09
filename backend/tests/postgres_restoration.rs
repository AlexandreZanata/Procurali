//! Restoration-precedence acceptance (P11-T06): current facts decide.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL with the real audited
//! operations for grants, suspension, bans, blocks, and restoration:
//! - lifting a content restriction retains under an active account ban
//!   or pair block with the row byte-identical;
//! - restoration after the deadline settles into expired hidden/history
//!   context instead of reviving;
//! - already terminal offers refuse restoration, while a clean suspended
//!   row returns to its prior live standing with history intact.
//!
//! Setup travels the real creation, publication, submission, and grant
//! paths with real phone cryptography; only lifecycle flips owned by the
//! verification flow are staged. All phones, names, and keys below are
//! synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::ban_user::{ban_user, BanUserInput};
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::restore_content::{
    restore_content, RestoreContentError, RestoreContentInput, RestoreOutcome,
};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::suspend_content::{suspend_content, SuspendInput};
use procurali_backend::application::withdraw_offer::{withdraw_offer, WithdrawReason};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p11t06-test-only-lookup-key",
        encryption_key: "p11t06-test-only-encryption-key",
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
    let operator = seed_active(pool, "Restore Operator", "+55 11 90000-1201").await;
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
    let moderator = seed_active(pool, "Restore Moderator", "+55 11 90000-1202").await;
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
    let admin = seed_active(pool, "Restore Admin", "+55 11 90000-1203").await;
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

/// One genuinely open either/600 demand with one sent used/520 offer
/// through the real operations.
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

fn suspend_input(kind: &str, target: uuid::Uuid) -> SuspendInput {
    SuspendInput {
        target_kind: kind.to_owned(),
        target_id: target,
        reason: "credible fraud pattern".to_owned(),
        policy_version: "v1".to_owned(),
        purpose: "hide pending review".to_owned(),
        case_id: None,
    }
}

fn restore_input(kind: &str, target: uuid::Uuid) -> RestoreContentInput {
    RestoreContentInput {
        target_kind: kind.to_owned(),
        target_id: target,
        reason: "cleared on re-review".to_owned(),
        policy_version: "v1".to_owned(),
        purpose: "restoration review".to_owned(),
    }
}

fn ban_input(user_id: uuid::Uuid) -> BanUserInput {
    BanUserInput {
        user_id,
        reason: "trafficking pattern".to_owned(),
        evidence: "substantiated reports plus contact pattern".to_owned(),
        policy_version: "v1".to_owned(),
        purpose: "indefinite exclusion".to_owned(),
        case_id: None,
    }
}

#[tokio::test]
async fn lifting_cannot_override_ban_or_block() {
    let db = TestDatabase::create("p11t06_retained")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t06_retained_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let (moderator, admin) = seed_staff(db.pool()).await;
    let buyer = seed_active(db.pool(), "Retained Buyer", "+55 11 90000-1204").await;
    let seller = seed_active(db.pool(), "Retained Seller", "+55 11 90000-1205").await;
    let (demand, offer) = live_offer(db.pool(), buyer, seller).await;
    suspend_content(db.pool(), moderator, suspend_input("offer", offer))
        .await
        .expect("offer suspends");

    // A banned seller's offer retains with its row byte-identical: the
    // content restriction cannot override the account ban.
    ban_user(db.pool(), admin, ban_input(seller))
        .await
        .expect("seller banned");
    let retained = restore_content(db.pool(), moderator, restore_input("offer", offer))
        .await
        .expect("restoration answers");
    match retained {
        RestoreOutcome::Retained(retained) => {
            assert_eq!(retained.state, "suspended");
            assert!(retained.reasons.contains(&"account_banned".to_owned()));
        }
        RestoreOutcome::Restored(_) => panic!("ban must hold the offer back"),
    }
    let standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM offers WHERE id = $1")
            .bind(offer)
            .fetch_one(db.pool())
            .await
            .expect("offer reads");
    assert_eq!(standing, ("suspended".to_owned(), "hidden".to_owned()));

    // A blocked pair's offer never returns either: blocking invalidates
    // atomically (established writer behavior), so restoration meets the
    // terminal row and refuses with everything standing. The pair_blocked
    // precedence inside restoration stays as last-line defense for any
    // suspended row a future writer might stage beside a live block.
    let buyer_two = seed_active(db.pool(), "Retained Buyer Two", "+55 11 90000-1206").await;
    let seller_two = seed_active(db.pool(), "Retained Seller Two", "+55 11 90000-1207").await;
    let (_, offer_two) = live_offer(db.pool(), buyer_two, seller_two).await;
    suspend_content(db.pool(), moderator, suspend_input("offer", offer_two))
        .await
        .expect("second offer suspends");
    block_user(db.pool(), buyer_two, seller_two)
        .await
        .expect("pair blocks");
    assert_eq!(
        restore_content(db.pool(), moderator, restore_input("offer", offer_two)).await,
        Err(RestoreContentError::InvalidState)
    );
    let blocked_standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM offers WHERE id = $1")
            .bind(offer_two)
            .fetch_one(db.pool())
            .await
            .expect("blocked offer reads");
    assert_eq!(blocked_standing.0, "invalidated");
    let block_stands: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
            .bind(buyer_two)
            .bind(seller_two)
            .fetch_optional(db.pool())
            .await
            .expect("block reads");
    assert!(block_stands.is_some(), "block survives the attempt");

    // A banned owner's request retains as well: lifting the content
    // restriction overrides no account standing.
    suspend_content(db.pool(), moderator, suspend_input("request", demand))
        .await
        .expect("demand suspends");
    ban_user(db.pool(), admin, ban_input(buyer))
        .await
        .expect("buyer banned");
    let retained = restore_content(db.pool(), moderator, restore_input("request", demand))
        .await
        .expect("restoration answers");
    match retained {
        RestoreOutcome::Retained(retained) => {
            assert_eq!(retained.state, "suspended");
            assert!(retained.reasons.contains(&"account_banned".to_owned()));
        }
        RestoreOutcome::Restored(_) => panic!("ban must hold the demand back"),
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn restoration_after_deadline_returns_expired_hidden() {
    let db = TestDatabase::create("p11t06_expired")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t06_expired_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let (moderator, _) = seed_staff(db.pool()).await;
    let buyer = seed_active(db.pool(), "Expired Buyer", "+55 11 90000-1208").await;
    let seller = seed_active(db.pool(), "Expired Seller", "+55 11 90000-1209").await;
    let (demand, _) = live_offer(db.pool(), buyer, seller).await;
    suspend_content(db.pool(), moderator, suspend_input("request", demand))
        .await
        .expect("demand suspends");

    // The original deadline passes under suspension: restoration settles
    // the row into expired hidden/history context instead of reviving it.
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(demand)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic deadline passage applies");
    let settled = restore_content(db.pool(), moderator, restore_input("request", demand))
        .await
        .expect("restoration answers");
    match settled {
        RestoreOutcome::Retained(retained) => {
            assert_eq!(retained.state, "expired");
            assert_eq!(retained.visibility, "hidden");
            assert!(retained.reasons.contains(&"deadline_elapsed".to_owned()));
        }
        RestoreOutcome::Restored(_) => panic!("elapsed deadline must settle, not revive"),
    }
    let facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = 'request.expired'",
    )
    .bind(demand)
    .fetch_one(db.pool())
    .await
    .expect("expiry facts read");
    assert_eq!(facts, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn terminal_offers_are_not_resurrected() {
    let db = TestDatabase::create("p11t06_terminal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t06_terminal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let (moderator, _) = seed_staff(db.pool()).await;
    let buyer = seed_active(db.pool(), "Terminal Buyer", "+55 11 90000-1210").await;
    let seller = seed_active(db.pool(), "Terminal Seller", "+55 11 90000-1211").await;
    let (_, offer) = live_offer(db.pool(), buyer, seller).await;

    // The permitted path first: a clean suspended offer returns to its
    // prior live standing with both facts on record.
    suspend_content(db.pool(), moderator, suspend_input("offer", offer))
        .await
        .expect("offer suspends");
    let returned = restore_content(db.pool(), moderator, restore_input("offer", offer))
        .await
        .expect("restoration answers");
    match returned {
        RestoreOutcome::Restored(restored) => {
            assert!(restored.restored);
            assert_eq!(restored.state, "sent");
            assert_eq!(restored.visibility, "visible");
        }
        RestoreOutcome::Retained(_) => panic!("clean row must restore"),
    }
    let suspension_facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'offer' AND resource_id = $1 AND kind = 'offer.suspended'",
    )
    .bind(offer)
    .fetch_one(db.pool())
    .await
    .expect("suspension facts read");
    let restoration_facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'offer' AND resource_id = $1 AND kind = 'offer.restored'",
    )
    .bind(offer)
    .fetch_one(db.pool())
    .await
    .expect("restoration facts read");
    assert_eq!((suspension_facts, restoration_facts), (1, 1));

    // Terminal rows refuse: withdrawn and blocked-invalidated offers stay
    // terminal no matter how often restoration is attempted.
    withdraw_offer(db.pool(), seller, offer, WithdrawReason::Withdrawn)
        .await
        .expect("offer withdraws");
    assert_eq!(
        restore_content(db.pool(), moderator, restore_input("offer", offer)).await,
        Err(RestoreContentError::InvalidState)
    );
    let (_, offer_two) = live_offer(db.pool(), buyer, seller).await;
    block_user(db.pool(), buyer, seller)
        .await
        .expect("pair blocks");
    assert_eq!(
        restore_content(db.pool(), moderator, restore_input("offer", offer_two)).await,
        Err(RestoreContentError::InvalidState)
    );
    db.cleanup().await.expect("suite cleans up");
}
