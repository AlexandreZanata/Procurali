//! Report and case intake storage: allegations with grouped identity.
//!
//! Canonical rules: INV-31 (reporter identities never reach public or
//! target-facing surfaces — the reporter lives only on the private row and
//! the target-facing projection below carries no reporter, detail, phone,
//! address, token, or secret material), INV-36 (a report stays an
//! allegation until assessed — the `reports` row and its `report_cases`
//! row are independent counts, and assessing a case never rewrites its
//! reports), EC-14 with EC-38 (closing a request or soft-deleting an
//! account never erases the allegation — targets travel as kind plus
//! identifier with an immutable submitted snapshot, and both links use
//! `ON DELETE RESTRICT` so evidence survives review while public reuse
//! stays closed).
//!
//! There is deliberately no public accusation, verdict, guilt, ban, phone,
//! or secret column here. Snapshots carry only the permitted business
//! context the reporter acted on. Status columns transition (`open`,
//! `review`, `valid`, `invalid`, `duplicate`, `withdrawn`); snapshot
//! columns have no UPDATE path.

use serde::Serialize;

/// Allowed report target families.
pub const TARGET_KINDS: [&str; 3] = ["request", "offer", "user"];

/// Allowed allegation reasons (spec 16.1, snake_case storage).
pub const REPORT_REASONS: [&str; 7] = [
    "fraud",
    "spam",
    "false_information",
    "prohibited_item",
    "inappropriate_behavior",
    "inappropriate_content",
    "other",
];

/// Allowed report/case statuses.
pub const REPORT_STATUSES: [&str; 6] = [
    "open",
    "review",
    "valid",
    "invalid",
    "duplicate",
    "withdrawn",
];

/// Allowed triage severities (spec 16.2). `None` means untriaged: an
/// allegation never assumes its own impact.
pub const REPORT_SEVERITIES: [&str; 4] = ["critical", "high", "normal", "low"];

/// Detail bound in scalar values (mirrors the database backstop).
pub const REPORT_DETAIL_MAX_CHARS: usize = 1000;

/// Snapshot title bound in scalar values (mirrors the database backstop).
pub const REPORT_TITLE_SNAPSHOT_MAX_CHARS: usize = 120;

/// Snapshot context bound in scalar values (mirrors the database backstop).
pub const REPORT_CONTEXT_SNAPSHOT_MAX_CHARS: usize = 1000;

/// A new allegation inside an existing grouped case. The case must already
/// exist; grouping and withdrawal arrive in later cards.
#[derive(Debug, Clone)]
pub struct NewReport {
    /// Grouped incident identity this allegation adds information to.
    pub case_id: uuid::Uuid,
    /// Reporting account. Private by design: never projected to targets.
    pub reporter_id: uuid::Uuid,
    /// Target family (`request`, `offer`, or `user`).
    pub target_kind: String,
    /// Target identifier (kind plus id; no hard foreign key so closed or
    /// soft-deleted references keep their snapshot).
    pub target_id: uuid::Uuid,
    /// Allegation reason (one of [`REPORT_REASONS`]).
    pub reason: String,
    /// Optional bounded context (`other` explanations belong here; the
    /// owning submission card enforces that requirement).
    pub detail: String,
    /// Immutable business-context title snapshot (what the reporter acted
    /// on). Never updated afterwards.
    pub target_title_snapshot: String,
    /// Immutable business-context detail snapshot. Never updated afterwards.
    pub target_context_snapshot: String,
}

/// One persisted allegation: private reporter plus immutable context.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Grouped incident identity.
    pub case_id: uuid::Uuid,
    /// Reporting account. Private: staff review only.
    pub reporter_id: uuid::Uuid,
    /// Target family.
    pub target_kind: String,
    /// Target identifier.
    pub target_id: uuid::Uuid,
    /// Allegation reason.
    pub reason: String,
    /// Bounded reporter-supplied context.
    pub detail: String,
    /// Allegation status (transitions only; never rewrites the snapshot).
    pub status: String,
    /// Immutable title snapshot at submission.
    pub target_title_snapshot: String,
    /// Immutable context snapshot at submission.
    pub target_context_snapshot: String,
    /// Submission instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One grouped incident: the assessment identity shared by allegations.
/// Distinct from any single report on purpose (INV-36).
#[derive(Debug, Clone, PartialEq)]
pub struct ReportCase {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Assessment status (transitions only).
    pub status: String,
    /// Triage severity, if triaged yet.
    pub severity: Option<String>,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last assessment instant (database clock).
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Target-facing projection: allowlist only. No reporter identifier,
/// reporter detail, phone, address, token, session, or secret material —
/// serialization of this shape can never leak who reported.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetFacingReport {
    /// Allegation identifier.
    pub id: uuid::Uuid,
    /// Grouped incident identity.
    pub case_id: uuid::Uuid,
    /// Target family.
    pub target_kind: String,
    /// Target identifier.
    pub target_id: uuid::Uuid,
    /// Allegation reason category (no accusation text).
    pub reason: String,
    /// Allegation status.
    pub status: String,
    /// Immutable title snapshot.
    pub target_title_snapshot: String,
    /// Immutable context snapshot.
    pub target_context_snapshot: String,
    /// Submission instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Project the private row into its target-facing allowlist (drops the
/// reporter and the reporter's free-text detail).
#[must_use]
pub fn to_target_facing(report: &Report) -> TargetFacingReport {
    TargetFacingReport {
        id: report.id,
        case_id: report.case_id,
        target_kind: report.target_kind.clone(),
        target_id: report.target_id,
        reason: report.reason.clone(),
        status: report.status.clone(),
        target_title_snapshot: report.target_title_snapshot.clone(),
        target_context_snapshot: report.target_context_snapshot.clone(),
        created_at: report.created_at,
    }
}

/// Typed report-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportError {
    /// A reason, target, detail, snapshot, severity, or status is missing
    /// or out of bounds.
    InvalidField,
    /// The referenced case or report does not exist.
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid report field"),
            Self::NotFound => f.write_str("report not found"),
            Self::StorageFailed => f.write_str("report storage failed"),
        }
    }
}

impl std::error::Error for ReportError {}

fn is_known(value: &str, known: &[&str]) -> bool {
    known.contains(&value)
}

fn check_new(input: &NewReport) -> Result<(), ReportError> {
    if !is_known(&input.target_kind, &TARGET_KINDS) {
        return Err(ReportError::InvalidField);
    }
    if !is_known(&input.reason, &REPORT_REASONS) {
        return Err(ReportError::InvalidField);
    }
    if input.detail.chars().count() > REPORT_DETAIL_MAX_CHARS {
        return Err(ReportError::InvalidField);
    }
    let title_count = input.target_title_snapshot.chars().count();
    if title_count == 0 || title_count > REPORT_TITLE_SNAPSHOT_MAX_CHARS {
        return Err(ReportError::InvalidField);
    }
    if input.target_context_snapshot.chars().count() > REPORT_CONTEXT_SNAPSHOT_MAX_CHARS {
        return Err(ReportError::InvalidField);
    }
    Ok(())
}

fn read_report(row: &sqlx::postgres::PgRow) -> Result<Report, ReportError> {
    use sqlx::Row;
    Ok(Report {
        id: row.try_get("id").map_err(|_| ReportError::StorageFailed)?,
        case_id: row
            .try_get("case_id")
            .map_err(|_| ReportError::StorageFailed)?,
        reporter_id: row
            .try_get("reporter_id")
            .map_err(|_| ReportError::StorageFailed)?,
        target_kind: row
            .try_get("target_kind")
            .map_err(|_| ReportError::StorageFailed)?,
        target_id: row
            .try_get("target_id")
            .map_err(|_| ReportError::StorageFailed)?,
        reason: row
            .try_get("reason")
            .map_err(|_| ReportError::StorageFailed)?,
        detail: row
            .try_get("detail")
            .map_err(|_| ReportError::StorageFailed)?,
        status: row
            .try_get("status")
            .map_err(|_| ReportError::StorageFailed)?,
        target_title_snapshot: row
            .try_get("target_title_snapshot")
            .map_err(|_| ReportError::StorageFailed)?,
        target_context_snapshot: row
            .try_get("target_context_snapshot")
            .map_err(|_| ReportError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| ReportError::StorageFailed)?,
    })
}

fn read_case(row: &sqlx::postgres::PgRow) -> Result<ReportCase, ReportError> {
    use sqlx::Row;
    Ok(ReportCase {
        id: row.try_get("id").map_err(|_| ReportError::StorageFailed)?,
        status: row
            .try_get("status")
            .map_err(|_| ReportError::StorageFailed)?,
        severity: row
            .try_get("severity")
            .map_err(|_| ReportError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| ReportError::StorageFailed)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| ReportError::StorageFailed)?,
    })
}

const REPORT_COLUMNS: &str = "id, case_id, reporter_id, target_kind, target_id, reason, detail, status, target_title_snapshot, target_context_snapshot, created_at";
const CASE_COLUMNS: &str = "id, status, severity, created_at, updated_at";

/// Open one grouped incident identity. Severity is optional triage: `None`
/// records an untriaged allegation without assuming impact.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`ReportError::InvalidField`] for an unknown severity, else
/// [`ReportError::StorageFailed`]. Reasons are static.
pub async fn create_case(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    severity: Option<&str>,
) -> Result<ReportCase, ReportError> {
    if let Some(severity) = severity {
        if !is_known(severity, &REPORT_SEVERITIES) {
            return Err(ReportError::InvalidField);
        }
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO report_cases (severity) VALUES ($1) RETURNING {CASE_COLUMNS}"
    ))
    .bind(severity)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| ReportError::StorageFailed)?;
    read_case(&row)
}

/// Record one allegation inside its grouped case with an immutable
/// submitted snapshot. The case must already exist.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`ReportError::InvalidField`] for an unknown kind, reason, or
/// out-of-bounds detail/snapshot, else [`ReportError::StorageFailed`]
/// (including a missing case or reporter through the foreign keys).
/// Reasons are static.
pub async fn create_report(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: NewReport,
) -> Result<Report, ReportError> {
    check_new(&input)?;
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO reports
            (case_id, reporter_id, target_kind, target_id, reason, detail,
             target_title_snapshot, target_context_snapshot)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {REPORT_COLUMNS}"
    ))
    .bind(input.case_id)
    .bind(input.reporter_id)
    .bind(&input.target_kind)
    .bind(input.target_id)
    .bind(&input.reason)
    .bind(&input.detail)
    .bind(&input.target_title_snapshot)
    .bind(&input.target_context_snapshot)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| ReportError::StorageFailed)?;
    read_report(&row)
}

/// One allegation by id, if it exists.
///
/// # Errors
///
/// Returns [`ReportError::StorageFailed`] on database failure only.
pub async fn report<'e, E>(executor: E, id: uuid::Uuid) -> Result<Option<Report>, ReportError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {REPORT_COLUMNS} FROM reports WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(|_| ReportError::StorageFailed)?;
    row.map(|row| read_report(&row)).transpose()
}

/// One grouped case by id, if it exists.
///
/// # Errors
///
/// Returns [`ReportError::StorageFailed`] on database failure only.
pub async fn report_case<'e, E>(
    executor: E,
    id: uuid::Uuid,
) -> Result<Option<ReportCase>, ReportError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CASE_COLUMNS} FROM report_cases WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(|_| ReportError::StorageFailed)?;
    row.map(|row| read_case(&row)).transpose()
}

/// One case's allegations in submission order. Private by design: callers
/// must already hold staff purpose before reading reporter identities.
///
/// # Errors
///
/// Returns [`ReportError::StorageFailed`] on database failure only.
pub async fn reports_for_case<'e, E>(
    executor: E,
    case_id: uuid::Uuid,
) -> Result<Vec<Report>, ReportError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {REPORT_COLUMNS} FROM reports WHERE case_id = $1 ORDER BY created_at, id"
    ))
    .bind(case_id)
    .fetch_all(executor)
    .await
    .map_err(|_| ReportError::StorageFailed)?;
    rows.iter().map(read_report).collect()
}

/// Transition one allegation's status. Snapshots are never touched here:
/// the UPDATE names the status column only.
///
/// # Errors
///
/// Returns [`ReportError::InvalidField`] for an unknown status,
/// [`ReportError::NotFound`] for an unknown report, else
/// [`ReportError::StorageFailed`]. Reasons are static.
pub async fn update_report_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: uuid::Uuid,
    status: &str,
) -> Result<Report, ReportError> {
    if !is_known(status, &REPORT_STATUSES) {
        return Err(ReportError::InvalidField);
    }
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "UPDATE reports SET status = $2 WHERE id = $1 RETURNING {REPORT_COLUMNS}"
    ))
    .bind(id)
    .bind(status)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ReportError::StorageFailed)?;
    row.map(|row| read_report(&row))
        .transpose()?
        .ok_or(ReportError::NotFound)
}

/// Transition one grouped case's assessment. Member reports keep whatever
/// status they held: assessment never rewrites allegations.
///
/// # Errors
///
/// Returns [`ReportError::InvalidField`] for an unknown status,
/// [`ReportError::NotFound`] for an unknown case, else
/// [`ReportError::StorageFailed`]. Reasons are static.
pub async fn update_case_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: uuid::Uuid,
    status: &str,
) -> Result<ReportCase, ReportError> {
    if !is_known(status, &REPORT_STATUSES) {
        return Err(ReportError::InvalidField);
    }
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "UPDATE report_cases SET status = $2, updated_at = now()
         WHERE id = $1 RETURNING {CASE_COLUMNS}"
    ))
    .bind(id)
    .bind(status)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ReportError::StorageFailed)?;
    row.map(|row| read_case(&row))
        .transpose()?
        .ok_or(ReportError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_facing_drops_reporter_and_detail() {
        let reporter = uuid::Uuid::now_v7();
        let report = Report {
            id: uuid::Uuid::now_v7(),
            case_id: uuid::Uuid::now_v7(),
            reporter_id: reporter,
            target_kind: "request".to_owned(),
            target_id: uuid::Uuid::now_v7(),
            reason: "spam".to_owned(),
            detail: "canary-detail-report".to_owned(),
            status: "open".to_owned(),
            target_title_snapshot: "Refrigerator".to_owned(),
            target_context_snapshot: "Frost-free 300L".to_owned(),
            created_at: chrono::Utc::now(),
        };
        let rendered =
            serde_json::to_string(&to_target_facing(&report)).expect("target-facing serializes");
        assert!(rendered.contains("spam"), "reason stays visible");
        let reporter_text = reporter.to_string();
        for absent in [
            reporter_text.as_str(),
            "reporter",
            "canary-detail-report",
            "detail",
            "phone",
            "lookup",
            "cipher",
            "token",
            "session",
            "address",
            "secret",
        ] {
            assert!(
                !rendered.contains(absent),
                "no {absent} in target-facing output"
            );
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ReportError::InvalidField,
            ReportError::NotFound,
            ReportError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
