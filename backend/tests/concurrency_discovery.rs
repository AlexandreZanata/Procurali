//! Discovery-under-change acceptance (P13-T06): eligibility stays current.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL with the real creation,
//! publication, revision, renewal, closure, block, and category
//! operations:
//! - cursor pages stay exact across expiry, blocks, prohibition, and
//!   removal between reads: eligible rows only, deterministic order, no
//!   duplicates, and no private data in any page or filter variant;
//! - original recency and renewal classification stay correct after
//!   changes, with no freshness claim beyond current eligibility.
//!
//! Setup travels the real paths with real phone cryptography; only the
//! lifecycle flips owned by the verification flow are staged (activation,
//! suspension/hiding rows, deadline passage). All phones, names, codes,
//! and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::discovery::{discover_demands, DiscoveryFilters};
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
        lookup_key: "p13t06-test-only-lookup-key",
        encryption_key: "p13t06-test-only-encryption-key",
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
    let operator = seed_active(pool, "Change Operator", "+55 11 90000-2501").await;
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
    let admin = seed_active(pool, "Change Admin", "+55 11 90000-2502").await;
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
async fn publish_demand(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    title: &str,
    budget: &str,
    city: &str,
    region: &str,
) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
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

/// Listed pages carry business fields and declared names only: no phone
/// digits, reporter, token, secret, or freshness-claim material.
fn assert_page_clean(items: &[procurali_backend::persistence::discovery::DiscoveredRequest]) {
    let rendered = serde_json::to_string(items).expect("page serializes");
    for absent in [
        "90000", "55119", "phone", "lookup", "cipher", "token", "notes", "offers", "reporter",
        "detail", "secret", "renewed", "boost", "fresh", "score",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in discovery page");
    }
}

#[tokio::test]
async fn changes_between_pages_keep_cursor_exact() {
    let db = TestDatabase::create("p13t06_pages")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t06_pages_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let admin = seed_policy_admin(db.pool()).await;
    let viewer = seed_active(db.pool(), "Pages Viewer", "+55 11 90000-2503").await;
    let author_one = seed_active(db.pool(), "Pages Author One", "+55 11 90000-2504").await;
    let author_two = seed_active(db.pool(), "Pages Author Two", "+55 11 90000-2505").await;
    let mut published = Vec::new();
    for (title, budget, region, author) in [
        ("Alpha Fridge", "600.00", "centro", author_one),
        ("Beta Fridge", "610.00", "norte", author_one),
        ("Gamma Fridge", "620.00", "centro", author_one),
        ("Delta Fridge", "630.00", "norte", author_two),
    ] {
        published.push(publish_demand(db.pool(), author, title, budget, "campinas", region).await);
    }
    assert_eq!(published.len(), 4);
    let (alpha, beta, gamma, delta) = (published[0], published[1], published[2], published[3]);
    // A removed-analogue row never lists anywhere in this journey.
    let removed = publish_demand(
        db.pool(),
        author_one,
        "Gone Fridge",
        "640.00",
        "campinas",
        "centro",
    )
    .await;
    sqlx::query("UPDATE requests SET visibility = 'private' WHERE id = $1")
        .bind(removed)
        .execute(db.pool())
        .await
        .expect("synthetic removal applies");

    // First page walks the two newest; then expiry, a block, and a
    // prohibition land before the cursor continues.
    let first = discover_demands(
        db.pool(),
        viewer,
        DiscoveryFilters {
            limit: Some(2),
            ..city_filters("campinas")
        },
    )
    .await
    .expect("first page answers");
    assert_eq!(
        first.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [delta, gamma]
    );
    assert_page_clean(&first.items);
    let cursor = first.next_cursor.expect("second page exists");
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(beta)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic deadline passage applies");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    expire_if_elapsed(&mut tx, beta, now)
        .await
        .expect("expiry evaluates");
    tx.commit().await.expect("expiry commits");
    block_user(db.pool(), viewer, author_two)
        .await
        .expect("pair blocks");
    let unblocked = discover_demands(db.pool(), viewer, city_filters("campinas"))
        .await
        .expect("post-block listing answers");
    assert_eq!(
        unblocked
            .items
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [gamma, alpha]
    );

    // The cursor continues exactly over the still-eligible rows — no
    // duplicates, no expired rows, no blocked authors — and the later
    // prohibition empties the listing without error.
    let second = discover_demands(
        db.pool(),
        viewer,
        DiscoveryFilters {
            limit: Some(2),
            cursor: Some(cursor),
            ..city_filters("campinas")
        },
    )
    .await
    .expect("second page answers");
    assert_eq!(
        second.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [alpha]
    );
    assert_eq!(second.next_cursor, None);
    assert_page_clean(&second.items);
    change_category(
        db.pool(),
        admin,
        CategoryInput {
            category_code: "home_appliances".to_owned(),
            new_status: "prohibited".to_owned(),
            reason: "safety review finding".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("prohibition applies");
    let emptied = discover_demands(db.pool(), viewer, city_filters("campinas"))
        .await
        .expect("emptied listing answers");
    assert!(emptied.items.is_empty());
    assert_eq!(emptied.next_cursor, None);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn recency_and_renewal_stay_correct_after_changes() {
    let db = TestDatabase::create("p13t06_recency")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t06_recency_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let viewer = seed_active(db.pool(), "Recency Viewer", "+55 11 90000-2506").await;
    let author = seed_active(db.pool(), "Recency Author", "+55 11 90000-2507").await;
    let older = publish_demand(
        db.pool(),
        author,
        "Old Fridge",
        "600.00",
        "campinas",
        "centro",
    )
    .await;
    let newer = publish_demand(
        db.pool(),
        author,
        "New Fridge",
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

    // Renewal and a material edit under later changes keep original rank:
    // genuinely newer demand still leads, with no freshness marker on
    // any row — only the exact projection keys serialize.
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
            title: "Old Fridge".to_owned(),
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
            title: "Old Fridge Pro".to_owned(),
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
    let page = discover_demands(db.pool(), viewer, city_filters("campinas"))
        .await
        .expect("discovery answers");
    assert_eq!(
        page.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [newer, older]
    );
    assert_page_clean(&page.items);
    let rendered = serde_json::to_string(&page.items).expect("page serializes");
    let keys: Vec<String> = serde_json::from_str::<Vec<serde_json::Value>>(&rendered)
        .expect("items parse")
        .into_iter()
        .flat_map(|item| {
            item.as_object()
                .expect("item object")
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect();
    for key in keys {
        assert!(
            [
                "id",
                "title",
                "category_code",
                "budget_cents",
                "condition",
                "city_code",
                "region_code",
                "author_name",
                "original_published_at"
            ]
            .contains(&key.as_str()),
            "no {key} beyond the exact projection"
        );
    }
    db.cleanup().await.expect("suite cleans up");
}
