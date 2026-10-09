//! Staff-grant acceptance (P11-T01): explicit powers with purpose audit.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). There is no staff HTTP surface yet, so these tests drive the
//! application operations directly over real PostgreSQL — the same
//! direct-proof pattern as the closure cards ("api" here is the audited
//! Rust operation surface, with HTTP routes landing on later cards).
//! Proves:
//! - the migrated schema ships zero privileged accounts, and
//!   regular/professional users can neither self-grant nor forge a staff
//!   role (the launch bootstrap needs explicit operator input and closes
//!   once an administrator exists);
//! - a moderator passes moderator gates but fails administrator gates and
//!   cannot manage permissions, while an administrator passes both;
//! - authorized sensitive reads record actor/resource/purpose with no
//!   reporter, phone, or secret material anywhere in the audit shape.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::create_report::{submit_report, ReportInput};
use procurali_backend::application::professional_profile::{declare_profile, NewProfessional};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::staff_permissions::{
    authorized_inspect, bootstrap_grant, grant_role, require_admin, require_moderator,
    BootstrapInput, GrantInput, InspectInput, StaffError,
};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p11t01-test-only-lookup-key",
        encryption_key: "p11t01-test-only-encryption-key",
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

async fn grant_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM staff_grants")
        .fetch_one(pool)
        .await
        .expect("grants read")
}

async fn grant_audit_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM staff_access_audit WHERE action = 'grant'")
        .fetch_one(pool)
        .await
        .expect("grant audits read")
}

fn moderator_grant_for(user_id: uuid::Uuid) -> GrantInput {
    GrantInput {
        user_id,
        role: "moderator".to_owned(),
        scope: "safety".to_owned(),
        reason: "safety triage cover".to_owned(),
    }
}

#[tokio::test]
async fn regular_professional_cannot_self_grant_or_forge() {
    let db = TestDatabase::create("p11t01_selfgrant")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t01_selfgrant_"),
        "known suite identity in the database name"
    );
    // No default privileged account ships with the migration.
    assert_eq!(grant_count(db.pool()).await, 0);
    let operator = seed_active(db.pool(), "Launch Operator", "+55 11 90000-0801").await;
    let regular = seed_active(db.pool(), "Regular Buyer", "+55 11 90000-0802").await;
    let professional = seed_active(db.pool(), "Pro Seller", "+55 11 90000-0803").await;
    declare_profile(
        db.pool(),
        professional,
        NewProfessional {
            business_name: "Pro Shop".to_owned(),
            business_type: "shop".to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
        },
    )
    .await
    .expect("professional classification declares");

    // Neither marketplace identity can self-grant: permission management
    // answers to administrators only, and none exists yet.
    assert_eq!(
        grant_role(db.pool(), regular, moderator_grant_for(regular), "v1").await,
        Err(StaffError::NotPermitted)
    );
    assert_eq!(
        grant_role(
            db.pool(),
            professional,
            moderator_grant_for(professional),
            "v1"
        )
        .await,
        Err(StaffError::NotPermitted)
    );
    // A forged granter (no such administrator) cannot mint powers either.
    assert_eq!(
        grant_role(
            db.pool(),
            uuid::Uuid::now_v7(),
            moderator_grant_for(regular),
            "v1"
        )
        .await,
        Err(StaffError::NotActive)
    );

    // The launch bootstrap needs explicit operator input — blank operator
    // labels refuse — and records the first grant with no granter.
    assert_eq!(
        bootstrap_grant(
            db.pool(),
            BootstrapInput {
                user_id: operator,
                role: "administrator".to_owned(),
                scope: "safety".to_owned(),
                reason: "launch cover".to_owned(),
                operator_label: "   ".to_owned(),
                policy_version: "v1".to_owned(),
            }
        )
        .await,
        Err(StaffError::InvalidField)
    );
    let launch = bootstrap_grant(
        db.pool(),
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
    assert_eq!(launch.granted_by, None);
    assert_eq!(grant_count(db.pool()).await, 1);

    // Once an administrator exists the bootstrap closes: later regulars
    // cannot slip through it, and every live grant carries its audit.
    assert_eq!(
        bootstrap_grant(
            db.pool(),
            BootstrapInput {
                user_id: regular,
                role: "administrator".to_owned(),
                scope: "safety".to_owned(),
                reason: "late claim".to_owned(),
                operator_label: "launch-operator-1".to_owned(),
                policy_version: "v1".to_owned(),
            }
        )
        .await,
        Err(StaffError::NotPermitted)
    );
    assert_eq!(grant_count(db.pool()).await, 1);
    assert_eq!(grant_audit_count(db.pool()).await, 1);
    let purpose: String =
        sqlx::query_scalar("SELECT purpose FROM staff_access_audit WHERE action = 'grant'")
            .fetch_one(db.pool())
            .await
            .expect("bootstrap audit reads");
    assert!(purpose.contains("launch-operator-1"));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn moderator_lacks_admin_only_powers() {
    let db = TestDatabase::create("p11t01_powers")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t01_powers_"),
        "known suite identity in the database name"
    );
    let admin = seed_active(db.pool(), "Power Admin", "+55 11 90000-0804").await;
    let moderator = seed_active(db.pool(), "Power Moderator", "+55 11 90000-0805").await;
    let outsider = seed_active(db.pool(), "Power Outsider", "+55 11 90000-0806").await;
    bootstrap_grant(
        db.pool(),
        BootstrapInput {
            user_id: admin,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "launch cover".to_owned(),
            operator_label: "launch-operator-1".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("launch bootstrap grants");
    let first = grant_role(db.pool(), admin, moderator_grant_for(moderator), "v1")
        .await
        .expect("administrator grants moderator");
    assert_eq!(first.granted_by, Some(admin));

    // Repeating the identical grant converges with no duplicate audit.
    let repeat = grant_role(db.pool(), admin, moderator_grant_for(moderator), "v1")
        .await
        .expect("repeat grant converges");
    assert_eq!(repeat.id, first.id);
    assert_eq!(grant_count(db.pool()).await, 2);
    assert_eq!(grant_audit_count(db.pool()).await, 2);

    // Moderators pass moderator gates but fail administrator gates; the
    // administrator passes both by hierarchy.
    assert!(require_moderator(db.pool(), moderator).await.is_ok());
    assert_eq!(
        require_admin(db.pool(), moderator).await,
        Err(StaffError::NotPermitted)
    );
    assert!(require_admin(db.pool(), admin).await.is_ok());
    assert!(require_moderator(db.pool(), admin).await.is_ok());
    assert_eq!(
        require_moderator(db.pool(), outsider).await,
        Err(StaffError::NotPermitted)
    );

    // Permission management is administrator-only: the moderator cannot
    // mint powers for anyone.
    assert_eq!(
        grant_role(db.pool(), moderator, moderator_grant_for(outsider), "v1").await,
        Err(StaffError::NotPermitted)
    );

    // Grants recheck live standing: a suspended moderator loses the gate
    // without any row being rewritten.
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(moderator)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        require_moderator(db.pool(), moderator).await,
        Err(StaffError::NotActive)
    );
    assert_eq!(grant_count(db.pool()).await, 2);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn authorized_sensitive_read_audits_without_reporter_exposure() {
    let db = TestDatabase::create("p11t01_audit")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p11t01_audit_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let admin = seed_active(db.pool(), "Audit Admin", "+55 11 90000-0807").await;
    let moderator = seed_active(db.pool(), "Audit Moderator", "+55 11 90000-0808").await;
    let buyer = seed_active(db.pool(), "Audit Buyer", "+55 11 90000-0809").await;
    let seller = seed_active(db.pool(), "Audit Seller", "+55 11 90000-0810").await;
    bootstrap_grant(
        db.pool(),
        BootstrapInput {
            user_id: admin,
            role: "administrator".to_owned(),
            scope: "safety".to_owned(),
            reason: "launch cover".to_owned(),
            operator_label: "launch-operator-1".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("launch bootstrap grants");
    grant_role(db.pool(), admin, moderator_grant_for(moderator), "v1")
        .await
        .expect("moderator granted");

    // One live demand with one offer and one buyer allegation on it.
    let draft = create_draft(
        db.pool(),
        buyer,
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
    let demand = publish_request(db.pool(), buyer, draft.id)
        .await
        .expect("fixture draft publishes")
        .id;
    let offer = submit_offer(
        db.pool(),
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
    let filed = submit_report(
        db.pool(),
        buyer,
        ReportInput {
            target_kind: "offer".to_owned(),
            target_id: offer,
            reason: "spam".to_owned(),
            detail: "canary-reporter-detail-0809".to_owned(),
        },
    )
    .await
    .expect("fixture allegation files");

    // Authorized sensitive reads record actor, resource, and purpose.
    let report_audit = authorized_inspect(
        db.pool(),
        moderator,
        InspectInput {
            target_kind: "report".to_owned(),
            target_id: filed.id,
            purpose: "fraud triage for the open intake".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("moderator inspection audits");
    assert_eq!(report_audit.actor_id, moderator);
    assert_eq!(report_audit.target_id, filed.id);
    assert_eq!(report_audit.action, "inspect");
    authorized_inspect(
        db.pool(),
        moderator,
        InspectInput {
            target_kind: "user".to_owned(),
            target_id: seller,
            purpose: "counterparty standing check".to_owned(),
            policy_version: "v1".to_owned(),
        },
    )
    .await
    .expect("second inspection audits");

    // The audit shape carries no reporter, phone, or secret material —
    // serialized or in schema — while the allegation row keeps its
    // private reporter for staff review.
    let rendered = serde_json::to_string(&report_audit).expect("audit serializes");
    assert!(rendered.contains("fraud triage for the open intake"));
    let buyer_text = buyer.to_string();
    for absent in [
        buyer_text.as_str(),
        "reporter",
        "canary-reporter-detail-0809",
        "detail",
        "phone",
        "lookup",
        "cipher",
        "token",
        "session",
        "address",
        "secret",
        "90000",
    ] {
        assert!(!rendered.contains(absent), "no {absent} in audit output");
    }
    for table in ["staff_grants", "staff_access_audit"] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns WHERE table_name = $1",
        )
        .bind(table)
        .fetch_all(db.pool())
        .await
        .expect("columns read");
        assert!(!columns.is_empty(), "table {table} exists");
        for name in &columns {
            assert!(
                ![
                    "reporter",
                    "reporter_id",
                    "phone",
                    "phone_number",
                    "phone_plaintext",
                    "plaintext",
                    "destination",
                    "address",
                    "token",
                    "secret",
                    "accusation",
                    "verdict",
                ]
                .contains(&name.as_str()),
                "no {name} column on {table}"
            );
        }
    }

    // Ungranted readers, blank purposes, and fabricated targets refuse
    // with nothing audited for the refusal.
    assert_eq!(
        authorized_inspect(
            db.pool(),
            buyer,
            InspectInput {
                target_kind: "report".to_owned(),
                target_id: filed.id,
                purpose: "curiosity".to_owned(),
                policy_version: "v1".to_owned(),
            }
        )
        .await,
        Err(StaffError::NotPermitted)
    );
    assert_eq!(
        authorized_inspect(
            db.pool(),
            moderator,
            InspectInput {
                target_kind: "report".to_owned(),
                target_id: filed.id,
                purpose: "   ".to_owned(),
                policy_version: "v1".to_owned(),
            }
        )
        .await,
        Err(StaffError::InvalidField)
    );
    assert_eq!(
        authorized_inspect(
            db.pool(),
            moderator,
            InspectInput {
                target_kind: "offer".to_owned(),
                target_id: uuid::Uuid::now_v7(),
                purpose: "ghost hunt".to_owned(),
                policy_version: "v1".to_owned(),
            }
        )
        .await,
        Err(StaffError::NotFound)
    );
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM staff_access_audit")
        .fetch_one(db.pool())
        .await
        .expect("audits read");
    assert_eq!(
        audits, 4,
        "two grants plus two inspections, refusals audit nothing"
    );
    db.cleanup().await.expect("suite cleans up");
}
