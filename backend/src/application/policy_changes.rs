//! Category prohibition and prospective policy versions: change the rules
//! without rewriting the past.
//!
//! Canonical rules: INV-08 (an active request needs an allowed category —
//! writers already refuse retired/prohibited classes for new use; this
//! module moves the standing), INV-15 (no count resets here — rolling
//! abuse accounting is untouched), INV-37 with AC-39 (actor, reason,
//! scope, time, and previous/resulting standing recorded in a durable
//! fact on the policy version row; the staff inspection vocabulary covers
//! person/resource reads, so policy decisions audit through facts, which
//! carry the identical shape), INV-38 with EC-36 (independent rows keep
//! their standing — prohibition hides but never cancels, terminalizes, or
//! deletes), INV-42 with AC-50 and EC-35 (versions date effectiveness;
//! future versions change nothing yet; historical facts keep the version
//! and category they recorded; codes keep identity across label edits),
//! AC-43 with EC-16 (an effective prohibition hides identified live
//! content, disables contact through shared eligibility, notifies owners
//! with the policy basis, and paid/professional standing grants no
//! exception — no paid input exists anywhere on this path by
//! construction), and spec 16.6 (a retired class finishes current
//! eligible cycles while refusing new use; reinstatement re-enables new
//! use without unhiding previously hidden rows).
//!
//! Category policy approval needs a live administrator grant in the policy
//! scope — safety-scoped administrators and moderators refuse here.

use crate::application::staff_permissions::StaffError;
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::notices::{record as record_notice, NewNotice};

/// Category standings under policy control.
pub const CATEGORY_STANDINGS: [&str; 3] = ["allowed", "retired", "prohibited"];
/// Policy scope required for category and version decisions.
pub const POLICY_SCOPE: &str = "policy";
/// Version bound in scalar values (mirrors the database backstop).
pub const POLICY_VERSION_MAX_CHARS: usize = 32;
/// Description bound in scalar values (mirrors the backstop).
pub const POLICY_DESCRIPTION_MAX_CHARS: usize = 1000;
/// Reason bound in scalar values.
pub const POLICY_REASON_MAX_CHARS: usize = 1000;

/// One policy version as supplied: an effective-dated rules identity.
#[derive(Debug, Clone)]
pub struct VersionInput {
    /// Version identity (e.g. `v2`).
    pub version: String,
    /// Instant new actions start evaluating under it (future allowed).
    pub effective_from: chrono::DateTime<chrono::Utc>,
    /// What this version changes and why.
    pub description: String,
}

/// One category standing change as supplied: what moves and under which
/// effective version.
#[derive(Debug, Clone)]
pub struct CategoryInput {
    /// Catalog category code (must already exist).
    pub category_code: String,
    /// `retired`, `prohibited`, or `allowed` (reinstatement).
    pub new_status: String,
    /// Why this standing changes.
    pub reason: String,
    /// Effective policy version recording the change.
    pub policy_version: String,
}

/// One persisted policy version.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyVersion {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Version identity.
    pub version: String,
    /// Effectiveness instant.
    pub effective_from: chrono::DateTime<chrono::Utc>,
    /// Recorded change rationale.
    pub description: String,
    /// Approving administrator, if any (launch `v1` records none).
    pub created_by: Option<uuid::Uuid>,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One recorded category change: standing moved plus cascade counts.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryChange {
    /// Changed category code.
    pub category_code: String,
    /// Standing before the write.
    pub previous_status: String,
    /// Standing after the write.
    pub new_status: String,
    /// Recording policy version.
    pub policy_version: String,
    /// Live requests hidden by a prohibition (zero otherwise).
    pub hidden_requests: i64,
    /// Live offers hidden by a prohibition (zero otherwise).
    pub hidden_offers: i64,
    /// Distinct owners notified by a prohibition (zero otherwise).
    pub notified_owners: i64,
    /// False when the standing already held (no new fact).
    pub transitioned: bool,
}

/// Typed policy-change failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    /// Unknown standing or bad version/description/reason text.
    InvalidField,
    /// The caller's account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such category or policy version exists.
    NotFound,
    /// The caller holds no live policy-scoped administrator grant.
    NotPermitted,
    /// The version is not yet effective, or the transition is illegal.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid policy field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("policy target not found"),
            Self::NotPermitted => f.write_str("policy change not permitted"),
            Self::InvalidState => f.write_str("policy change outside its standing"),
            Self::StorageFailed => f.write_str("policy storage failed"),
        }
    }
}

impl std::error::Error for PolicyError {}

fn check_bounded(value: &str, max: usize) -> Result<(), PolicyError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(PolicyError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> PolicyError {
    match error {
        StaffError::InvalidField => PolicyError::InvalidField,
        StaffError::NotActive => PolicyError::NotActive,
        StaffError::NotFound => PolicyError::NotFound,
        StaffError::NotPermitted => PolicyError::NotPermitted,
        StaffError::StorageFailed => PolicyError::StorageFailed,
    }
}

fn read_version(row: &sqlx::postgres::PgRow) -> Result<PolicyVersion, PolicyError> {
    use sqlx::Row;
    Ok(PolicyVersion {
        id: row.try_get("id").map_err(|_| PolicyError::StorageFailed)?,
        version: row
            .try_get("version")
            .map_err(|_| PolicyError::StorageFailed)?,
        effective_from: row
            .try_get("effective_from")
            .map_err(|_| PolicyError::StorageFailed)?,
        description: row
            .try_get("description")
            .map_err(|_| PolicyError::StorageFailed)?,
        created_by: row
            .try_get("created_by")
            .map_err(|_| PolicyError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| PolicyError::StorageFailed)?,
    })
}

const VERSION_COLUMNS: &str = "id, version, effective_from, description, created_by, created_at";

/// Policy approval gate: a live administrator grant in the policy scope.
/// Safety-scoped administrators refuse alongside moderators and regular
/// accounts.
async fn policy_admin(pool: &sqlx::PgPool, admin_id: uuid::Uuid) -> Result<(), PolicyError> {
    let grant = crate::application::staff_permissions::require_admin(pool, admin_id)
        .await
        .map_err(staff_error)?;
    if grant.scope != POLICY_SCOPE {
        return Err(PolicyError::NotPermitted);
    }
    Ok(())
}

/// Record one prospective policy version: an effective-dated rules
/// identity for later changes to reference. Future-dated versions change
/// nothing until effective. Repeating an identical version converges;
/// redefining one refuses as an identity fork.
///
/// # Errors
///
/// Returns [`PolicyError::InvalidField`] for bad text or forked identity,
/// [`PolicyError::NotActive`] for restricted callers,
/// [`PolicyError::NotPermitted`] without a policy-scoped administrator
/// grant, else [`PolicyError::StorageFailed`]. Reasons are static.
pub async fn create_policy_version(
    pool: &sqlx::PgPool,
    admin_id: uuid::Uuid,
    input: VersionInput,
) -> Result<PolicyVersion, PolicyError> {
    check_bounded(&input.version, POLICY_VERSION_MAX_CHARS)?;
    check_bounded(&input.description, POLICY_DESCRIPTION_MAX_CHARS)?;
    policy_admin(pool, admin_id).await?;
    let mut tx = pool.begin().await.map_err(|_| PolicyError::StorageFailed)?;
    let existing: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {VERSION_COLUMNS} FROM policy_versions WHERE version = $1 LIMIT 1"
    ))
    .bind(&input.version)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| PolicyError::StorageFailed)?;
    if let Some(row) = existing {
        let standing = read_version(&row)?;
        tx.rollback()
            .await
            .map_err(|_| PolicyError::StorageFailed)?;
        if standing.effective_from == input.effective_from
            && standing.description == input.description
        {
            return Ok(standing);
        }
        return Err(PolicyError::InvalidField);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO policy_versions (version, effective_from, description, created_by)
         VALUES ($1, $2, $3, $4) RETURNING {VERSION_COLUMNS}"
    ))
    .bind(&input.version)
    .bind(input.effective_from)
    .bind(&input.description)
    .bind(admin_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| PolicyError::StorageFailed)?;
    let recorded = read_version(&row)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(admin_id),
            resource_kind: "policy",
            resource_id: recorded.id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "policy.version_created",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "version": recorded.version,
                "effective_from": recorded.effective_from,
                "description": recorded.description,
            }),
        },
    )
    .await
    .map_err(|_| PolicyError::StorageFailed)?;
    tx.commit().await.map_err(|_| PolicyError::StorageFailed)?;
    Ok(recorded)
}

/// Move one category standing under an effective policy version: retire
/// (existing cycles finish, new use refuses), prohibit (new writes stop
/// and identified live content hides with owner notices), or reinstate to
/// allowed (new use resumes; previously hidden rows stay hidden for
/// explicit per-row review). Paid and professional standing grant no
/// exception — no such input exists on this path.
///
/// # Errors
///
/// Returns [`PolicyError::InvalidField`] for unknown standings or bad
/// text, [`PolicyError::NotActive`] for restricted callers,
/// [`PolicyError::NotFound`] for missing categories or versions,
/// [`PolicyError::NotPermitted`] without a policy-scoped administrator
/// grant, [`PolicyError::InvalidState`] for not-yet-effective versions or
/// illegal transitions, else [`PolicyError::StorageFailed`]. Reasons are
/// static.
pub async fn change_category(
    pool: &sqlx::PgPool,
    admin_id: uuid::Uuid,
    input: CategoryInput,
) -> Result<CategoryChange, PolicyError> {
    if !CATEGORY_STANDINGS.contains(&input.new_status.as_str()) {
        return Err(PolicyError::InvalidField);
    }
    check_bounded(&input.reason, POLICY_REASON_MAX_CHARS)?;
    policy_admin(pool, admin_id).await?;
    let mut tx = pool.begin().await.map_err(|_| PolicyError::StorageFailed)?;
    let version: Option<PolicyVersion> = {
        let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
            "SELECT {VERSION_COLUMNS} FROM policy_versions WHERE version = $1 LIMIT 1"
        ))
        .bind(&input.policy_version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| PolicyError::StorageFailed)?;
        row.map(|row| read_version(&row)).transpose()?
    };
    let version = match version {
        Some(version) => version,
        None => {
            tx.rollback()
                .await
                .map_err(|_| PolicyError::StorageFailed)?;
            return Err(PolicyError::NotFound);
        }
    };
    if version.effective_from > chrono::Utc::now() {
        tx.rollback()
            .await
            .map_err(|_| PolicyError::StorageFailed)?;
        return Err(PolicyError::InvalidState);
    }
    let category: Option<(String, String)> =
        sqlx::query_as("SELECT label, status FROM catalog_categories WHERE code = $1")
            .bind(&input.category_code)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| PolicyError::StorageFailed)?;
    let (label, previous) = match category {
        Some(category) => category,
        None => {
            tx.rollback()
                .await
                .map_err(|_| PolicyError::StorageFailed)?;
            return Err(PolicyError::NotFound);
        }
    };
    if previous == input.new_status {
        tx.rollback()
            .await
            .map_err(|_| PolicyError::StorageFailed)?;
        return Ok(CategoryChange {
            category_code: input.category_code,
            previous_status: previous,
            new_status: input.new_status,
            policy_version: version.version,
            hidden_requests: 0,
            hidden_offers: 0,
            notified_owners: 0,
            transitioned: false,
        });
    }
    let legal = matches!(
        (previous.as_str(), input.new_status.as_str()),
        ("allowed", "retired")
            | ("allowed", "prohibited")
            | ("retired", "prohibited")
            | ("retired", "allowed")
            | ("prohibited", "allowed")
    );
    if !legal {
        tx.rollback()
            .await
            .map_err(|_| PolicyError::StorageFailed)?;
        return Err(PolicyError::InvalidState);
    }
    sqlx::query("UPDATE catalog_categories SET status = $2 WHERE code = $1")
        .bind(&input.category_code)
        .bind(&input.new_status)
        .execute(&mut *tx)
        .await
        .map_err(|_| PolicyError::StorageFailed)?;
    let mut hidden_requests = 0i64;
    let mut hidden_offers = 0i64;
    let mut notified = 0i64;
    if input.new_status == "prohibited" {
        // Hide identified live content without touching states, deadlines,
        // or cycles: eligibility refuses hidden rows, expiry keeps them
        // hidden, and history keeps its recorded category.
        hidden_requests = sqlx::query(
            "UPDATE requests SET visibility = 'hidden', updated_at = now()
             FROM request_revisions AS revision
             WHERE requests.id = revision.request_id
               AND revision.revision_number = requests.current_revision_number
               AND revision.category_code = $1
               AND requests.state = 'active' AND requests.visibility = 'public'",
        )
        .bind(&input.category_code)
        .execute(&mut *tx)
        .await
        .map_err(|_| PolicyError::StorageFailed)?
        .rows_affected() as i64;
        hidden_offers = sqlx::query(
            "UPDATE offers SET visibility = 'hidden', updated_at = now()
             FROM requests JOIN request_revisions AS revision
               ON revision.request_id = requests.id
              AND revision.revision_number = requests.current_revision_number
             WHERE offers.request_id = requests.id
               AND revision.category_code = $1
               AND offers.state IN ('sent', 'viewed', 'contacted')
               AND offers.visibility = 'visible'",
        )
        .bind(&input.category_code)
        .execute(&mut *tx)
        .await
        .map_err(|_| PolicyError::StorageFailed)?
        .rows_affected() as i64;
        let owners: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT requests.author_id FROM requests
             JOIN request_revisions AS revision
               ON revision.request_id = requests.id
              AND revision.revision_number = requests.current_revision_number
             WHERE revision.category_code = $1
            UNION
            SELECT DISTINCT offers.seller_id FROM offers
             JOIN requests ON requests.id = offers.request_id
             JOIN request_revisions AS revision
               ON revision.request_id = requests.id
              AND revision.revision_number = requests.current_revision_number
             WHERE revision.category_code = $1",
        )
        .bind(&input.category_code)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| PolicyError::StorageFailed)?;
        let fact = record_event(
            &mut *tx,
            NewEvent {
                actor_id: Some(admin_id),
                resource_kind: "policy",
                resource_id: version.id,
                cycle: None,
                revision: None,
                effective_at: chrono::Utc::now(),
                kind: "policy.category_prohibited",
                policy: "mvp-free",
                source: "staff",
                payload: serde_json::json!({
                    "category_code": input.category_code,
                    "previous_status": previous,
                    "reason": input.reason,
                    "policy_version": version.version,
                    "hidden_requests": hidden_requests,
                    "hidden_offers": hidden_offers,
                }),
            },
        )
        .await
        .map_err(|_| PolicyError::StorageFailed)?;
        for owner in owners {
            record_notice(
                &mut *tx,
                NewNotice {
                    account_id: owner,
                    kind: "policy.prohibition",
                    resource_kind: "policy",
                    resource_id: version.id,
                    event_id: fact.id,
                    body: format!(
                        "New prohibition ({label}) under policy {}: affected content is hidden.",
                        version.version
                    ),
                },
            )
            .await
            .map_err(|_| PolicyError::StorageFailed)?;
            notified += 1;
        }
    } else {
        let kind = match input.new_status.as_str() {
            "retired" => "policy.category_retired",
            _ => "policy.category_reinstated",
        };
        record_event(
            &mut *tx,
            NewEvent {
                actor_id: Some(admin_id),
                resource_kind: "policy",
                resource_id: version.id,
                cycle: None,
                revision: None,
                effective_at: chrono::Utc::now(),
                kind,
                policy: "mvp-free",
                source: "staff",
                payload: serde_json::json!({
                    "category_code": input.category_code,
                    "previous_status": previous,
                    "reason": input.reason,
                    "policy_version": version.version,
                }),
            },
        )
        .await
        .map_err(|_| PolicyError::StorageFailed)?;
    }
    tx.commit().await.map_err(|_| PolicyError::StorageFailed)?;
    Ok(CategoryChange {
        category_code: input.category_code,
        previous_status: previous,
        new_status: input.new_status,
        policy_version: version.version,
        hidden_requests,
        hidden_offers,
        notified_owners: notified,
        transitioned: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_vocabulary_is_closed() {
        for standing in ["allowed", "retired", "prohibited"] {
            assert!(CATEGORY_STANDINGS.contains(&standing));
        }
        assert!(!CATEGORY_STANDINGS.contains(&"archived"));
        assert_eq!(POLICY_SCOPE, "policy");
    }

    #[test]
    fn policy_text_has_bounds() {
        assert!(check_bounded("v2", POLICY_VERSION_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", POLICY_VERSION_MAX_CHARS),
            Err(PolicyError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), POLICY_DESCRIPTION_MAX_CHARS),
            Err(PolicyError::InvalidField)
        );
        assert_eq!(
            check_bounded("", POLICY_REASON_MAX_CHARS),
            Err(PolicyError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            PolicyError::InvalidField,
            PolicyError::NotActive,
            PolicyError::NotFound,
            PolicyError::NotPermitted,
            PolicyError::InvalidState,
            PolicyError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
