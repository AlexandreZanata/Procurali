//! Policy-change acceptance (P11-T08): rules change, facts stand.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Category and version decisions travel the real audited
//! operations (no policy HTTP surface exists yet — "api" here is the
//! audited Rust operation surface, the same direct-proof pattern as the
//! staff-grant cards). Proves:
//! - professional standing grants no prohibition exception, and no paid
//!   bypass exists anywhere in the schema or on the decision path;
//! - a prohibition-winning contact race refuses with no new destination,
//!   while a contact-winning race keeps its legitimate initiation;
//! - future-dated versions change nothing yet, later effective versions
//!   gate new writes only, and historical facts keep their recorded
//!   version and category while reinstatement re-enables new use without
//!   unhiding old rows.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use chrono::SubsecRound;
use database::TestDatabase;
use procurali_backend::application::policy_changes::{
    change_category, create_policy_version, CategoryInput, PolicyError, VersionInput,
};
use procurali_backend::application::professional_profile::{declare_profile, NewProfessional};
use procurali_backend::application::publish_request::{publish_request, PublishError};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::staff_permissions::{
    bootstrap_grant, grant_role, BootstrapInput, GrantInput,
};
use procurali_backend::application::start_contact::{start_contact, ContactError, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput, SubmitError};
use procurali_backend::application::suspend_content::{suspend_content, SuspendInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p11t08-test-only-lookup-key",
        encryption_key: "p11t08-test-only-encryption-key",
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

/// One policy-scoped administrator behind the real audited grant chain.
async fn seed_policy_admin(pool: &sqlx::PgPool) -> uuid::Uuid {
    let operator = seed_active(pool, "Policy Operator", "+55 11 90000-1401").await;
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
    let admin = seed_active(pool, "Policy Admin", "+55 11 90000-1402").await;
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

/// One moderator behind the real audited grant chain.
async fn seed_moderator(pool: &sqlx::PgPool, admin: uuid::Uuid) -> uuid::Uuid {
    let moderator = seed_active(pool, "Policy Moderator", "+55 11 90000-1403").await;
    grant_role(
        pool,
        admin,
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

/// One live demand with one offer in the given category through the real
/// operations; return ids.
async fn live_offer_in(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
    category: &str,
) -> (uuid::Uuid, uuid::Uuid) {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some("Refrigerator".to_owned()),
            category_code: Some(category.to_owned()),
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

fn prohibit_input(category: &str, version: &str) -> CategoryInput {
    CategoryInput {
        category_code: category.to_owned(),
        new_status: "prohibited".to_owned(),
        reason: "safety review finding".to_owned(),
        policy_version: version.to_owned(),
    }
}

#[tokio::test]
async fn paid_professional_no_prohibition_exception() {
    let db = TestDatabase::create("p11t08_paid")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t08_paid_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let admin = seed_policy_admin(db.pool()).await;
    let buyer = seed_active(db.pool(), "Pro Buyer", "+55 11 90000-1404").await;
    let seller = seed_active(db.pool(), "Pro Seller", "+55 11 90000-1405").await;
    for (account, business) in [(buyer, "Buyer Shop"), (seller, "Seller Shop")] {
        declare_profile(
            db.pool(),
            account,
            NewProfessional {
                business_name: business.to_owned(),
                business_type: "shop".to_owned(),
                city: "Campinas".to_owned(),
                region: "SP".to_owned(),
            },
        )
        .await
        .expect("professional classification declares");
    }
    let (demand, offer) = live_offer_in(db.pool(), buyer, seller, "home_appliances").await;

    // Prohibition hides professional content exactly like ordinary
    // content, with owner notices carrying the policy basis.
    let change = change_category(db.pool(), admin, prohibit_input("home_appliances", "v1"))
        .await
        .expect("prohibition applies");
    assert!(change.transitioned);
    assert_eq!(change.hidden_requests, 1);
    assert_eq!(change.hidden_offers, 1);
    assert_eq!(change.notified_owners, 2);
    let demand_standing: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("demand reads");
    assert_eq!(demand_standing, ("active".to_owned(), "hidden".to_owned()));
    let notices: Vec<(String, String)> = sqlx::query_as(
        "SELECT kind, body FROM notices WHERE kind = 'policy.prohibition' ORDER BY created_at",
    )
    .fetch_all(db.pool())
    .await
    .expect("notices read");
    assert_eq!(notices.len(), 2);
    for (_, body) in &notices {
        assert!(body.contains("v1"));
        for absent in ["reporter", "phone", "90000", "token", "cipher"] {
            assert!(!body.contains(absent), "no {absent} in notice body");
        }
    }

    // New professional writes refuse with no exception path: publication,
    // submission, and contact share the prohibition refusals.
    let draft = create_draft(
        db.pool(),
        buyer,
        DraftInput {
            title: Some("Washer".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("700.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("draft shape validates");
    assert_eq!(
        publish_request(db.pool(), buyer, draft.id).await,
        Err(PublishError::ProhibitedCategory)
    );
    // The hidden demand refuses first by standing precedence (same rule
    // as content suspension); the category vocabulary still names the
    // prohibition at publication time above.
    assert_eq!(
        submit_offer(
            db.pool(),
            seller,
            demand,
            OfferInput {
                revision_number: Some(1),
                cycle_number: Some(1),
                description: Some("Late unit".to_owned()),
                price: Some("500.00".to_owned()),
                condition: Some("used".to_owned()),
                city_code: Some("campinas".to_owned()),
                region_code: Some("centro".to_owned()),
                notes: None,
                available: Some(true),
                available_in_city: Some(true),
            },
        )
        .await,
        Err(SubmitError::ForbiddenState)
    );
    assert!(matches!(
        start_contact(
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
        .await,
        Err(ContactError::ForbiddenState)
    ));

    // No paid, subscription, or plan column exists anywhere a prohibition
    // could consult: the exception is impossible by construction.
    for table in [
        "catalog_categories",
        "requests",
        "offers",
        "users",
        "policy_versions",
    ] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns WHERE table_name = $1",
        )
        .bind(table)
        .fetch_all(db.pool())
        .await
        .expect("columns read");
        for name in &columns {
            assert!(
                !["paid", "subscription", "plan", "tier", "premium"].contains(&name.as_str()),
                "no {name} column on {table}"
            );
        }
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prohibition_winning_contact_race_reveals_nothing() {
    let db = TestDatabase::create("p11t08_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t08_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let admin = seed_policy_admin(db.pool()).await;
    let buyer = seed_active(db.pool(), "Race Buyer", "+55 11 90000-1406").await;
    let seller = seed_active(db.pool(), "Race Seller", "+55 11 90000-1407").await;
    let (demand, offer) = live_offer_in(db.pool(), buyer, seller, "tools").await;

    // Prohibition races with contact initiation: the barrier releases both
    // at once. A winning prohibition refuses with a defined error and no
    // destination; a winning contact keeps its legitimate pre-prohibition
    // initiation before the row hides.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let prohibit = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            change_category(&pool, admin, prohibit_input("tools", "v1")).await
        }
    };
    let handoff = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            start_contact(
                &pool,
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
        }
    };
    let (prohibited, started) =
        tokio::join!(prohibit(std::sync::Arc::clone(&barrier)), handoff(barrier),);
    assert!(prohibited.is_ok(), "prohibition lands in every branch");
    match started {
        Ok(handoff) => {
            assert!(!handoff.repeat);
            let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts WHERE offer_id = $1")
                .bind(offer)
                .fetch_one(db.pool())
                .await
                .expect("contacts read");
            assert_eq!(rows, 1);
        }
        Err(ContactError::ForbiddenState) => {
            let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts WHERE offer_id = $1")
                .bind(offer)
                .fetch_one(db.pool())
                .await
                .expect("contacts read");
            assert_eq!(rows, 0);
        }
        outcome => panic!("unexpected race outcome: {outcome:?}"),
    }
    let status: String =
        sqlx::query_scalar("SELECT status FROM catalog_categories WHERE code = 'tools'")
            .fetch_one(db.pool())
            .await
            .expect("category reads");
    assert_eq!(status, "prohibited");
    let visibility: String = sqlx::query_scalar("SELECT visibility FROM requests WHERE id = $1")
        .bind(demand)
        .fetch_one(db.pool())
        .await
        .expect("demand reads");
    assert_eq!(visibility, "hidden");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn threshold_change_prospective_history_traceable() {
    let db = TestDatabase::create("p11t08_prospective")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t08_prospective_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let admin = seed_policy_admin(db.pool()).await;
    let moderator = seed_moderator(db.pool(), admin).await;
    let buyer = seed_active(db.pool(), "Prospective Buyer", "+55 11 90000-1408").await;
    let seller = seed_active(db.pool(), "Prospective Seller", "+55 11 90000-1409").await;
    let (demand, offer) = live_offer_in(db.pool(), buyer, seller, "home_appliances").await;
    suspend_content(
        db.pool(),
        moderator,
        SuspendInput {
            target_kind: "request".to_owned(),
            target_id: demand,
            reason: "credible fraud pattern".to_owned(),
            policy_version: "v1".to_owned(),
            purpose: "hide pending review".to_owned(),
            case_id: None,
        },
    )
    .await
    .expect("demand suspends");

    // A future-dated version changes nothing yet: decisions under it
    // refuse, and current actions still evaluate under v1.
    create_policy_version(
        db.pool(),
        admin,
        VersionInput {
            version: "v2".to_owned(),
            effective_from: chrono::Utc::now() + chrono::Duration::days(1),
            description: "Second policy set.".to_owned(),
        },
    )
    .await
    .expect("future version records");
    assert_eq!(
        change_category(db.pool(), admin, prohibit_input("home_appliances", "v2")).await,
        Err(PolicyError::InvalidState)
    );
    let draft = create_draft(
        db.pool(),
        buyer,
        DraftInput {
            title: Some("Dryer".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("650.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("draft validates");
    publish_request(db.pool(), buyer, draft.id)
        .await
        .expect("v1 rules still publish");

    // An effective version retires with current cycles finishing: the live
    // tools handoff still succeeds while new tools writes refuse.
    // Truncated to microseconds: timestamptz round-trips exactly, so the
    // identical repeat below compares equal.
    let v3_at = chrono::Utc::now().round_subsecs(6);
    create_policy_version(
        db.pool(),
        admin,
        VersionInput {
            version: "v3".to_owned(),
            effective_from: v3_at,
            description: "Third policy set.".to_owned(),
        },
    )
    .await
    .expect("effective version records");
    let (tools_demand, tools_offer) = live_offer_in(db.pool(), buyer, seller, "tools").await;
    change_category(
        db.pool(),
        admin,
        CategoryInput {
            category_code: "tools".to_owned(),
            new_status: "retired".to_owned(),
            reason: "category review".to_owned(),
            policy_version: "v3".to_owned(),
        },
    )
    .await
    .expect("retirement applies");
    let handoff = start_contact(
        db.pool(),
        &test_keys(),
        buyer,
        tools_demand,
        tools_offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("retired cycle still contacts");
    assert!(!handoff.repeat);
    let retired_draft = create_draft(
        db.pool(),
        buyer,
        DraftInput {
            title: Some("Drill".to_owned()),
            category_code: Some("tools".to_owned()),
            budget: Some("300.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("retired draft shape validates");
    assert_eq!(
        publish_request(db.pool(), buyer, retired_draft.id).await,
        Err(PublishError::RetiredCategory)
    );

    // Prohibition under the effective version hides current rows while
    // historical facts keep their recorded version and category, and
    // reinstatement re-enables new use without unhiding old rows.
    change_category(db.pool(), admin, prohibit_input("home_appliances", "v3"))
        .await
        .expect("prohibition applies");
    let old_policy: String = sqlx::query_scalar(
        "SELECT payload->>'policy_version' FROM business_events
         WHERE resource_kind = 'request' AND kind = 'request.suspended' LIMIT 1",
    )
    .fetch_one(db.pool())
    .await
    .expect("old fact reads");
    assert_eq!(old_policy, "v1");
    let old_category: String = sqlx::query_scalar(
        "SELECT category_code FROM request_revisions
         WHERE request_id = $1 ORDER BY revision_number DESC LIMIT 1",
    )
    .bind(demand)
    .fetch_one(db.pool())
    .await
    .expect("revision reads");
    assert_eq!(old_category, "home_appliances");
    change_category(
        db.pool(),
        admin,
        CategoryInput {
            category_code: "home_appliances".to_owned(),
            new_status: "allowed".to_owned(),
            reason: "review cleared the class".to_owned(),
            policy_version: "v3".to_owned(),
        },
    )
    .await
    .expect("reinstatement applies");
    let reinstated_draft = create_draft(
        db.pool(),
        buyer,
        DraftInput {
            title: Some("Freezer".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("900.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("reinstated draft validates");
    publish_request(db.pool(), buyer, reinstated_draft.id)
        .await
        .expect("new use resumes");
    let old_visibility: String =
        sqlx::query_scalar("SELECT visibility FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("old demand reads");
    assert_eq!(old_visibility, "hidden", "reinstatement unhides nothing");

    // Version identity is stable: identical repeats converge, forks refuse.
    let converged = create_policy_version(
        db.pool(),
        admin,
        VersionInput {
            version: "v3".to_owned(),
            effective_from: v3_at,
            description: "Third policy set.".to_owned(),
        },
    )
    .await
    .expect("identical repeat converges");
    assert_eq!(converged.version, "v3");
    assert_eq!(
        create_policy_version(
            db.pool(),
            admin,
            VersionInput {
                version: "v3".to_owned(),
                effective_from: v3_at,
                description: "Rewritten third set.".to_owned(),
            },
        )
        .await,
        Err(PolicyError::InvalidField)
    );
    let _ = offer;
    db.cleanup().await.expect("suite cleans up");
}
