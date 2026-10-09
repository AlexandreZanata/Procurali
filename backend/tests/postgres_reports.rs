//! Report-intake acceptance (P10-T03): allegations with grouped identity.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - a report stays an allegation while its case is assessed (distinct
//!   counts, no auto-ban);
//! - reporter identity never reaches the target-facing projection or the
//!   schema's accusation-free shape;
//! - a closed request and a soft-deleted account keep the submitted
//!   snapshot readable for review.
//!
//! All phones and names below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::reports::{
    create_case, create_report, report, report_case, reports_for_case, to_target_facing,
    update_case_status, NewReport, ReportError,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p10t03-test-only-lookup-key",
        encryption_key: "p10t03-test-only-encryption-key",
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

async fn open_case(pool: &sqlx::PgPool) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let case = create_case(&mut tx, None).await.expect("case opens");
    tx.commit().await.expect("case commits");
    assert_eq!(case.status, "open");
    assert_eq!(case.severity, None);
    case.id
}

async fn file_report(
    pool: &sqlx::PgPool,
    case_id: uuid::Uuid,
    reporter: uuid::Uuid,
    target: uuid::Uuid,
    detail: &str,
) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let stored = create_report(
        &mut tx,
        NewReport {
            case_id,
            reporter_id: reporter,
            target_kind: "request".to_owned(),
            target_id: target,
            reason: "spam".to_owned(),
            detail: detail.to_owned(),
            target_title_snapshot: "Refrigerator".to_owned(),
            target_context_snapshot: "Frost-free 300L context".to_owned(),
        },
    )
    .await
    .expect("report records");
    tx.commit().await.expect("report commits");
    assert_eq!(stored.status, "open");
    stored.id
}

#[tokio::test]
async fn report_is_distinct_from_validated_incident() {
    let db = TestDatabase::create("p10t03_distinct")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t03_distinct_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let reporter = seed_active(db.pool(), "Distinct Reporter", "+55 11 90000-0301").await;
    let owner = seed_active(db.pool(), "Distinct Owner", "+55 11 90000-0302").await;
    let request_id = publish_open(db.pool(), owner).await;

    let case_id = open_case(db.pool()).await;
    let report_id = file_report(db.pool(), case_id, reporter, request_id, "repeated noise").await;

    // One allegation inside one grouped incident: independent counts.
    let case = report_case(db.pool(), case_id)
        .await
        .expect("case reads")
        .expect("case exists");
    let stored = report(db.pool(), report_id)
        .await
        .expect("report reads")
        .expect("report exists");
    assert_eq!(case.status, "open");
    assert_eq!(stored.status, "open");
    assert_eq!(stored.case_id, case_id);
    assert_eq!(
        reports_for_case(db.pool(), case_id)
            .await
            .expect("case members read")
            .len(),
        1
    );

    // Assessing the case never rewrites its allegation: the report row
    // keeps its own status while the case moves to a finding.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let assessed = update_case_status(&mut tx, case_id, "valid")
        .await
        .expect("case assesses");
    tx.commit().await.expect("assessment commits");
    assert_eq!(assessed.status, "valid");
    let stored = report(db.pool(), report_id)
        .await
        .expect("report re-reads")
        .expect("report exists");
    assert_eq!(stored.status, "open", "assessment leaves allegations alone");

    // Raw volume alone bans nobody: both accounts stay exactly as they
    // were, and the schema refuses invented verdicts at the backstop.
    let states: Vec<String> = sqlx::query_scalar("SELECT state FROM users ORDER BY created_at")
        .fetch_all(db.pool())
        .await
        .expect("account states read");
    assert!(states.iter().all(|state| state == "active"));
    let raw = sqlx::query("UPDATE report_cases SET status = 'banned' WHERE id = $1")
        .bind(case_id)
        .execute(db.pool())
        .await;
    assert!(raw.is_err(), "CHECK backstop refuses invented case status");
    let raw = sqlx::query(
        "INSERT INTO reports
            (case_id, reporter_id, target_kind, target_id, reason,
             target_title_snapshot)
         VALUES ($1, $2, 'request', $3, 'guilty', 'Refrigerator')",
    )
    .bind(case_id)
    .bind(reporter)
    .bind(request_id)
    .execute(db.pool())
    .await;
    assert!(raw.is_err(), "CHECK backstop refuses invented reason");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        update_case_status(&mut tx, case_id, "banned").await,
        Err(ReportError::InvalidField)
    );
    tx.rollback().await.expect("probe rolls back");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn reporter_identity_absent_from_target_facing_dto() {
    let db = TestDatabase::create("p10t03_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t03_privacy_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let reporter = seed_active(db.pool(), "Privacy Reporter", "+55 11 90000-0303").await;
    let owner = seed_active(db.pool(), "Privacy Owner", "+55 11 90000-0304").await;
    let request_id = publish_open(db.pool(), owner).await;

    let case_id = open_case(db.pool()).await;
    let report_id = file_report(
        db.pool(),
        case_id,
        reporter,
        request_id,
        "canary-reporter-detail-0303",
    )
    .await;
    let stored = report(db.pool(), report_id)
        .await
        .expect("report reads")
        .expect("report exists");

    // The target-facing allowlist carries the allegation category and the
    // business snapshot only: no reporter, no free-text detail, no phone
    // or secret material of any kind.
    let rendered =
        serde_json::to_string(&to_target_facing(&stored)).expect("target-facing serializes");
    assert!(rendered.contains("spam"), "reason stays visible");
    assert!(rendered.contains("Refrigerator"), "snapshot stays visible");
    let reporter_text = reporter.to_string();
    for absent in [
        reporter_text.as_str(),
        "reporter",
        "canary-reporter-detail-0303",
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
        assert!(
            !rendered.contains(absent),
            "no {absent} in target-facing output"
        );
    }

    // The schema keeps the reporter on the private row only, with no
    // public accusation, verdict, guilt, ban, phone, or secret column on
    // either table — now or by migration convention.
    for table in ["reports", "report_cases"] {
        let columns: Vec<(String, String)> = sqlx::query_as(
            "SELECT column_name, udt_name FROM information_schema.columns
             WHERE table_name = $1",
        )
        .bind(table)
        .fetch_all(db.pool())
        .await
        .expect("columns read");
        assert!(!columns.is_empty(), "table {table} exists");
        for (name, _) in &columns {
            assert!(
                ![
                    "accusation",
                    "verdict",
                    "guilt",
                    "guilty",
                    "ban",
                    "phone",
                    "phone_number",
                    "phone_plaintext",
                    "plaintext",
                    "destination",
                    "address",
                    "token",
                    "secret",
                ]
                .contains(&name.as_str()),
                "no {name} column on {table}"
            );
        }
    }
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'reports'",
    )
    .fetch_all(db.pool())
    .await
    .expect("report columns read");
    assert!(columns.contains(&"reporter_id".to_owned()));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn closed_deleted_reference_preserves_permitted_context() {
    let db = TestDatabase::create("p10t03_retained")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t03_retained_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let reporter = seed_active(db.pool(), "Retained Reporter", "+55 11 90000-0305").await;
    let owner = seed_active(db.pool(), "Retained Owner", "+55 11 90000-0306").await;
    let request_id = publish_open(db.pool(), owner).await;

    let case_id = open_case(db.pool()).await;
    let report_id = file_report(db.pool(), case_id, reporter, request_id, "late review").await;

    // Closure does not dismiss the allegation: the request moves to a
    // terminal state while the report keeps its identifier and snapshot.
    sqlx::query("UPDATE requests SET state = 'completed' WHERE id = $1")
        .bind(request_id)
        .execute(db.pool())
        .await
        .expect("synthetic closure applies");
    // Account deletion during the open case stops marketplace access but
    // preserves the incident row: soft delete keeps the identifier while
    // clearing current interaction.
    sqlx::query("UPDATE users SET state = 'deleted', deleted_at = now() WHERE id = $1")
        .bind(owner)
        .execute(db.pool())
        .await
        .expect("synthetic deletion applies");

    let reread = report(db.pool(), report_id)
        .await
        .expect("report re-reads")
        .expect("report survives closure and deletion");
    assert_eq!(reread.target_id, request_id);
    assert_eq!(reread.target_title_snapshot, "Refrigerator");
    assert_eq!(reread.target_context_snapshot, "Frost-free 300L context");
    assert_eq!(reread.status, "open");
    let members = reports_for_case(db.pool(), case_id)
        .await
        .expect("case members re-read");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0], reread);
    let state: String = sqlx::query_scalar("SELECT state FROM requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(db.pool())
        .await
        .expect("closed request still exists");
    assert_eq!(state, "completed");
    db.cleanup().await.expect("suite cleans up");
}
