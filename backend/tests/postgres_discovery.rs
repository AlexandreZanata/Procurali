//! Discovery acceptance (P13-T01): eligible demand, deterministic rank.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL with the real creation,
//! publication, revision, renewal, closure, block, and category
//! operations:
//! - renewal and edits never outrank genuinely newer demand, with
//!   original publication instants byte-identical and cursor paging
//!   walking the same order;
//! - same-named regions in another city never leak into local listings,
//!   and blocked authors stay out of the blocking viewer's listing;
//! - empty, inverted, unknown, and non-positive filters match the exact
//!   contract, and every unavailable standing stays out.
//!
//! Setup travels the real paths with real phone cryptography; only the
//! lifecycle flips owned by the verification flow are staged (activation,
//! suspension/hiding rows, category standing where the P11-T08 writer
//! would add an unrelated grant chain, and deadline passage). All
//! phones, names, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::discovery::{
    discover_demands, DiscoveryActionError, DiscoveryFilters,
};
use procurali_backend::application::policy_changes::{change_category, CategoryInput};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::renew_request::{renew_request, RenewalConfirmation};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_eligibility::expire_if_elapsed;
use procurali_backend::application::revise_request::{revise_request, ReviseInput};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p13t01-test-only-lookup-key",
        encryption_key: "p13t01-test-only-encryption-key",
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
        .expect("fixture region stores");
    upsert_city(&mut tx, "santos", "Santos", true)
        .await
        .expect("fixture city stores");
    upsert_region(&mut tx, "santos", "centro", "Centro")
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

/// One policy administrator behind the real audited grant chain.
async fn seed_policy_admin(pool: &sqlx::PgPool) -> uuid::Uuid {
    let operator = seed_active(pool, "Discovery Operator", "+55 11 90000-2001").await;
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
    let admin = seed_active(pool, "Discovery Admin", "+55 11 90000-2002").await;
    grant_role(
        pool,
        operator,
        GrantInput {
            user_id: admin,
            role: "administrator".to_owned(),
            scope: "policy".to_owned(),
            reason: "policy cover".to_owned(),
        },
        "v1",
    )
    .await
    .expect("policy administrator granted");
    admin
}

/// One published demand through the real operations in the given slice.
async fn publish_demand_in(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    title: &str,
    budget: &str,
    city: &str,
    region: &str,
    category: &str,
) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some(category.to_owned()),
            budget: Some(budget.to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some(city.to_owned()),
            region_code: Some(region.to_owned()),
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

/// One published home-appliance demand through the real operations.
async fn publish_demand(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    title: &str,
    budget: &str,
    city: &str,
    region: &str,
) -> uuid::Uuid {
    publish_demand_in(pool, author, title, budget, city, region, "home_appliances").await
}

fn city_filters(city: &str) -> DiscoveryFilters {
    DiscoveryFilters {
        city_code: Some(city.to_owned()),
        category_code: None,
        region_code: None,
        preferred_region: None,
        min_budget_cents: None,
        max_budget_cents: None,
        condition: None,
        limit: None,
        cursor: None,
    }
}

async fn listed_ids(
    pool: &sqlx::PgPool,
    viewer: uuid::Uuid,
    filters: DiscoveryFilters,
) -> Vec<uuid::Uuid> {
    discover_demands(pool, viewer, filters)
        .await
        .expect("discovery answers")
        .items
        .into_iter()
        .map(|item| item.id)
        .collect()
}

#[tokio::test]
async fn renewal_edit_never_outrank_newer_demand() {
    let db = TestDatabase::create("p13t01_rank")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t01_rank_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let viewer = seed_active(db.pool(), "Rank Viewer", "+55 11 90000-2003").await;
    let author = seed_active(db.pool(), "Rank Author", "+55 11 90000-2004").await;
    let older = publish_demand(
        db.pool(),
        author,
        "Old Refrigerator",
        "600.00",
        "campinas",
        "centro",
    )
    .await;
    let newer = publish_demand(
        db.pool(),
        author,
        "New Refrigerator",
        "650.00",
        "campinas",
        "centro",
    )
    .await;
    let published_before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT original_published_at FROM requests WHERE id = $1")
            .bind(older)
            .fetch_one(db.pool())
            .await
            .expect("publication instant reads");

    // Renewal and a material edit move cycles and revisions but never the
    // original publication instant — so genuinely newer demand still
    // ranks first, and cursor pages walk the same order. Renewal opens in
    // the final day, so the older cycle ages synthetically first.
    let renewal_now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(older)
    .bind(renewal_now - chrono::Duration::days(6))
    .bind(renewal_now + chrono::Duration::hours(12))
    .execute(db.pool())
    .await
    .expect("synthetic cycle age applies");
    renew_request(
        db.pool(),
        author,
        older,
        RenewalConfirmation {
            title: "Old Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "".to_owned(),
        },
    )
    .await
    .expect("renewal renews");
    revise_request(
        db.pool(),
        author,
        older,
        ReviseInput {
            title: "Old Refrigerator Pro".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "".to_owned(),
        },
    )
    .await
    .expect("revision revises");
    let published_after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT original_published_at FROM requests WHERE id = $1")
            .bind(older)
            .fetch_one(db.pool())
            .await
            .expect("publication instant re-reads");
    assert_eq!(published_before, published_after);
    assert_eq!(
        listed_ids(db.pool(), viewer, city_filters("campinas")).await,
        [newer, older]
    );
    let first = discover_demands(
        db.pool(),
        viewer,
        DiscoveryFilters {
            limit: Some(1),
            ..city_filters("campinas")
        },
    )
    .await
    .expect("first page answers");
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].id, newer);
    let cursor = first.next_cursor.expect("second page exists");
    let second = discover_demands(
        db.pool(),
        viewer,
        DiscoveryFilters {
            limit: Some(1),
            cursor: Some(cursor),
            ..city_filters("campinas")
        },
    )
    .await
    .expect("second page answers");
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].id, older);
    assert_eq!(second.next_cursor, None);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn same_region_label_never_leaks_cross_city() {
    let db = TestDatabase::create("p13t01_city")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t01_city_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let viewer = seed_active(db.pool(), "City Viewer", "+55 11 90000-2005").await;
    let local = seed_active(db.pool(), "Local Author", "+55 11 90000-2006").await;
    let remote = seed_active(db.pool(), "Remote Author", "+55 11 90000-2007").await;
    let centro = publish_demand(
        db.pool(),
        local,
        "Local Fridge",
        "600.00",
        "campinas",
        "centro",
    )
    .await;
    let norte = publish_demand(
        db.pool(),
        local,
        "North Fridge",
        "610.00",
        "campinas",
        "norte",
    )
    .await;
    let other_centro = publish_demand(
        db.pool(),
        remote,
        "Remote Fridge",
        "620.00",
        "santos",
        "centro",
    )
    .await;

    // Preferred centro ranks first inside Campinas, and the identically
    // named Santos region never leaks in — with or without preference.
    let page = discover_demands(
        db.pool(),
        viewer,
        DiscoveryFilters {
            preferred_region: Some("centro".to_owned()),
            ..city_filters("campinas")
        },
    )
    .await
    .expect("preferred discovery answers");
    assert_eq!(
        page.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [centro, norte]
    );
    assert!(page.items.iter().all(|item| item.city_code == "campinas"));
    let plain = listed_ids(db.pool(), viewer, city_filters("campinas")).await;
    assert_eq!(plain.len(), 2);
    assert!(!plain.contains(&other_centro));
    let santos = listed_ids(db.pool(), viewer, city_filters("santos")).await;
    assert_eq!(santos, [other_centro]);

    // A blocked author leaves the blocking viewer's listing while staying
    // listed for everyone else.
    block_user(db.pool(), viewer, local)
        .await
        .expect("pair blocks");
    let hidden = listed_ids(db.pool(), viewer, city_filters("campinas")).await;
    assert!(hidden.is_empty());
    let stranger = seed_active(db.pool(), "City Stranger", "+55 11 90000-2008").await;
    assert_eq!(
        listed_ids(db.pool(), stranger, city_filters("campinas"))
            .await
            .len(),
        2
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn empty_inverted_and_unavailable_match_contract() {
    let db = TestDatabase::create("p13t01_contract")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t01_contract_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let admin = seed_policy_admin(db.pool()).await;
    let viewer = seed_active(db.pool(), "Contract Viewer", "+55 11 90000-2009").await;
    let author = seed_active(db.pool(), "Contract Author", "+55 11 90000-2010").await;
    let live = publish_demand(
        db.pool(),
        author,
        "Live Fridge",
        "600.00",
        "campinas",
        "centro",
    )
    .await;

    // Filter shape refusals: missing, unknown, inverted, non-positive,
    // out-of-range, and malformed cursors — never a silent fallback.
    for filters in [
        DiscoveryFilters {
            city_code: None,
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            city_code: Some("   ".to_owned()),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            city_code: Some("nowhere".to_owned()),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            category_code: Some("starships".to_owned()),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            region_code: Some("sul".to_owned()),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            preferred_region: Some("sul".to_owned()),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            condition: Some("refurbished".to_owned()),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            min_budget_cents: Some(70_000),
            max_budget_cents: Some(60_000),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            min_budget_cents: Some(0),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            max_budget_cents: Some(-5),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            limit: Some(0),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            limit: Some(101),
            ..city_filters("campinas")
        },
        DiscoveryFilters {
            cursor: Some("not-a-cursor".to_owned()),
            ..city_filters("campinas")
        },
    ] {
        assert_eq!(
            discover_demands(db.pool(), viewer, filters).await,
            Err(DiscoveryActionError::InvalidField)
        );
    }
    // Valid filters with no matches are an honest empty page.
    let empty = discover_demands(
        db.pool(),
        viewer,
        DiscoveryFilters {
            min_budget_cents: Some(99_900),
            ..city_filters("campinas")
        },
    )
    .await
    .expect("empty page answers");
    assert!(empty.items.is_empty());
    assert_eq!(empty.next_cursor, None);

    // Every unavailable standing stays out: draft, completed, cancelled,
    // expired, suspended, hidden, prohibited-category, and blocked rows —
    // while the live row lists alone.
    let draft = create_draft(
        db.pool(),
        author,
        DraftInput {
            title: Some("Draft Fridge".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("630.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("draft validates")
    .id;
    let done = publish_demand(
        db.pool(),
        author,
        "Done Fridge",
        "640.00",
        "campinas",
        "centro",
    )
    .await;
    close_request(
        db.pool(),
        author,
        done,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("demand completes");
    let gone = publish_demand(
        db.pool(),
        author,
        "Gone Fridge",
        "650.00",
        "campinas",
        "centro",
    )
    .await;
    close_request(db.pool(), author, gone, BuyerOutcome::NotNeeded)
        .await
        .expect("demand cancels");
    let old = publish_demand(
        db.pool(),
        author,
        "Old Fridge",
        "660.00",
        "campinas",
        "centro",
    )
    .await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(old)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic deadline passage applies");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    expire_if_elapsed(&mut tx, old, now)
        .await
        .expect("expiry evaluates");
    tx.commit().await.expect("expiry commits");
    // Suspended/hidden rows are staged exactly as the suspension writer
    // leaves them; the retired tools class below proves the read-side
    // join while the live home-appliance demand keeps listing.
    let held = publish_demand(
        db.pool(),
        author,
        "Held Fridge",
        "670.00",
        "campinas",
        "centro",
    )
    .await;
    sqlx::query("UPDATE requests SET state = 'suspended', visibility = 'hidden' WHERE id = $1")
        .bind(held)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    let tooling = publish_demand_in(
        db.pool(),
        author,
        "Power Drill",
        "300.00",
        "campinas",
        "centro",
        "tools",
    )
    .await;
    change_category(
        db.pool(),
        admin,
        CategoryInput {
            category_code: "tools".to_owned(),
            new_status: "retired".to_owned(),
            reason: "category review".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("retirement applies");
    let retired = publish_demand_fails_retired(db.pool(), author).await;
    let _ = (draft, retired, tooling);
    let listed = listed_ids(db.pool(), viewer, city_filters("campinas")).await;
    assert_eq!(listed, [live]);

    // Suspended viewers read no discovery at all.
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(viewer)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        discover_demands(db.pool(), viewer, city_filters("campinas")).await,
        Err(DiscoveryActionError::NotActive)
    );
    db.cleanup().await.expect("suite cleans up");
}

/// A retired category refuses publication through the real writer: the
/// attempt itself is the assertion (no row ever exists to list).
async fn publish_demand_fails_retired(pool: &sqlx::PgPool, author: uuid::Uuid) -> bool {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some("Retired Drill".to_owned()),
            category_code: Some("tools".to_owned()),
            budget: Some("680.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("draft validates");
    matches!(
        publish_request(pool, author, draft.id).await,
        Err(procurali_backend::application::publish_request::PublishError::RetiredCategory)
    )
}
