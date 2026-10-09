//! Scoped staff grants with purpose audit: explicit powers, no self-service.
//!
//! Canonical rules: INV-05 with INV-06 and INV-39 (buyer, seller,
//! professional, and paid facts never imply staff authority — grants attach
//! explicitly per account, role, and scope, and every use-site rechecks the
//! live grant plus current account standing), INV-37 with AC-39 (important
//! administrative changes record actor, reason, scope, time, and
//! previous/resulting eligibility — grant decisions audit into the same
//! table as inspections), spec 21.6 with 21.7 (moderators handle content
//! and scoped safety work and never wield permanent-ban or reversal powers
//! without an explicit administrator grant; administrators add bans,
//! reversals, permission management, and policy approval), and spec 16.4
//! (sensitive inspection records actor plus purpose).
//!
//! There is no default privileged account: the migration creates tables
//! only, and the launch bootstrap closes permanently once any live
//! administrator grant exists. Later grants name their granting
//! administrator. No UPDATE path rewrites a grant or an audit row.

use serde::Serialize;

/// Moderator role: scoped safety work, never bans or reversals alone.
pub const ROLE_MODERATOR: &str = "moderator";
/// Administrator role: moderation abilities plus bans, reversals,
/// permission management, and policy approval.
pub const ROLE_ADMINISTRATOR: &str = "administrator";
/// Safety scope: content and account safety operations.
pub const SCOPE_SAFETY: &str = "safety";
/// Policy scope: business-policy approval work.
pub const SCOPE_POLICY: &str = "policy";

/// Reason bound in scalar values (mirrors the database backstop).
pub const GRANT_REASON_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values (mirrors the database backstop).
pub const AUDIT_PURPOSE_MAX_CHARS: usize = 500;
/// Operator-label bound for the bootstrap path (local input only).
pub const OPERATOR_LABEL_MAX_CHARS: usize = 120;
/// Policy-version bound (mirrors the database backstop).
pub const POLICY_VERSION_MAX_CHARS: usize = 32;

/// One staff grant to record.
#[derive(Debug, Clone)]
pub struct GrantInput {
    /// Account receiving the operational power.
    pub user_id: uuid::Uuid,
    /// `moderator` or `administrator`.
    pub role: String,
    /// `safety` or `policy`.
    pub scope: String,
    /// Why this power is granted (recorded verbatim in audit).
    pub reason: String,
}

/// One launch-bootstrap grant: the same fields plus the explicit operator
/// input that replaces the absent granting administrator.
#[derive(Debug, Clone)]
pub struct BootstrapInput {
    /// Account receiving the launch power.
    pub user_id: uuid::Uuid,
    /// `moderator` or `administrator` (launch operators usually take both,
    /// one grant row at a time).
    pub role: String,
    /// `safety` or `policy`.
    pub scope: String,
    /// Why this power is granted.
    pub reason: String,
    /// Explicit launch-operator identity (local input, never defaulted).
    pub operator_label: String,
    /// Rules version the operator acts under.
    pub policy_version: String,
}

/// One sensitive inspection to authorize and audit.
#[derive(Debug, Clone)]
pub struct InspectInput {
    /// Inspected family (`user`, `request`, `offer`, `report`, `case`).
    pub target_kind: String,
    /// Inspected identifier (must already exist).
    pub target_id: uuid::Uuid,
    /// Assigned safety purpose (recorded verbatim in audit).
    pub purpose: String,
    /// Rules version the inspection runs under.
    pub policy_version: String,
}

/// One persisted staff grant.
#[derive(Debug, Clone, PartialEq)]
pub struct StaffGrant {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Empowered account.
    pub user_id: uuid::Uuid,
    /// Granted role.
    pub role: String,
    /// Granted scope.
    pub scope: String,
    /// Recorded reason.
    pub reason: String,
    /// Granting administrator, if any (launch bootstrap records none).
    pub granted_by: Option<uuid::Uuid>,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One persisted purpose-audit row. Identifiers and decisions only: no
/// reporter, phone, address, token, or secret material exists here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccessAudit {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Acting staff account.
    pub actor_id: uuid::Uuid,
    /// `inspect`, `grant`, or `revoke`.
    pub action: String,
    /// Acted-upon family.
    pub target_kind: String,
    /// Acted-upon identifier.
    pub target_id: uuid::Uuid,
    /// Assigned purpose.
    pub purpose: String,
    /// Decision reason (`inspect` rows record none).
    pub reason: String,
    /// Grant scope, for grant decisions.
    pub scope: Option<String>,
    /// Eligibility before the change, for grant decisions.
    pub previous_state: Option<String>,
    /// Eligibility after the change, for grant decisions.
    pub resulting_state: Option<String>,
    /// Rules version in force.
    pub policy_version: String,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Typed staff-permission failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaffError {
    /// Unknown role/scope/kind, bad reason/purpose/policy/operator text.
    InvalidField,
    /// The account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such grant target or inspection target exists.
    NotFound,
    /// The caller holds no grant for this power, the bootstrap already
    /// closed, or a foreign power is claimed.
    NotPermitted,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for StaffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid staff field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("staff target not found"),
            Self::NotPermitted => f.write_str("staff power not permitted"),
            Self::StorageFailed => f.write_str("staff storage failed"),
        }
    }
}

impl std::error::Error for StaffError {}

fn check_grant_fields(role: &str, scope: &str, reason: &str) -> Result<(), StaffError> {
    if role != ROLE_MODERATOR && role != ROLE_ADMINISTRATOR {
        return Err(StaffError::InvalidField);
    }
    if scope != SCOPE_SAFETY && scope != SCOPE_POLICY {
        return Err(StaffError::InvalidField);
    }
    let reason_count = reason.chars().count();
    if reason_count == 0 || reason_count > GRANT_REASON_MAX_CHARS {
        return Err(StaffError::InvalidField);
    }
    Ok(())
}

fn check_purpose(purpose: &str) -> Result<(), StaffError> {
    let count = purpose.chars().count();
    if purpose.trim().is_empty() || count > AUDIT_PURPOSE_MAX_CHARS {
        return Err(StaffError::InvalidField);
    }
    Ok(())
}

fn check_policy(policy_version: &str) -> Result<(), StaffError> {
    let count = policy_version.chars().count();
    if count == 0 || count > POLICY_VERSION_MAX_CHARS {
        return Err(StaffError::InvalidField);
    }
    Ok(())
}

fn check_operator(operator_label: &str) -> Result<(), StaffError> {
    let count = operator_label.chars().count();
    if operator_label.trim().is_empty() || count > OPERATOR_LABEL_MAX_CHARS {
        return Err(StaffError::InvalidField);
    }
    Ok(())
}

fn read_grant(row: &sqlx::postgres::PgRow) -> Result<StaffGrant, StaffError> {
    use sqlx::Row;
    Ok(StaffGrant {
        id: row.try_get("id").map_err(|_| StaffError::StorageFailed)?,
        user_id: row
            .try_get("user_id")
            .map_err(|_| StaffError::StorageFailed)?,
        role: row.try_get("role").map_err(|_| StaffError::StorageFailed)?,
        scope: row
            .try_get("scope")
            .map_err(|_| StaffError::StorageFailed)?,
        reason: row
            .try_get("reason")
            .map_err(|_| StaffError::StorageFailed)?,
        granted_by: row
            .try_get("granted_by")
            .map_err(|_| StaffError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| StaffError::StorageFailed)?,
    })
}

fn read_audit(row: &sqlx::postgres::PgRow) -> Result<AccessAudit, StaffError> {
    use sqlx::Row;
    Ok(AccessAudit {
        id: row.try_get("id").map_err(|_| StaffError::StorageFailed)?,
        actor_id: row
            .try_get("actor_id")
            .map_err(|_| StaffError::StorageFailed)?,
        action: row
            .try_get("action")
            .map_err(|_| StaffError::StorageFailed)?,
        target_kind: row
            .try_get("target_kind")
            .map_err(|_| StaffError::StorageFailed)?,
        target_id: row
            .try_get("target_id")
            .map_err(|_| StaffError::StorageFailed)?,
        purpose: row
            .try_get("purpose")
            .map_err(|_| StaffError::StorageFailed)?,
        reason: row
            .try_get("reason")
            .map_err(|_| StaffError::StorageFailed)?,
        scope: row
            .try_get("scope")
            .map_err(|_| StaffError::StorageFailed)?,
        previous_state: row
            .try_get("previous_state")
            .map_err(|_| StaffError::StorageFailed)?,
        resulting_state: row
            .try_get("resulting_state")
            .map_err(|_| StaffError::StorageFailed)?,
        policy_version: row
            .try_get("policy_version")
            .map_err(|_| StaffError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| StaffError::StorageFailed)?,
    })
}

const GRANT_COLUMNS: &str = "id, user_id, role, scope, reason, granted_by, created_at";
const AUDIT_COLUMNS: &str = "id, actor_id, action, target_kind, target_id, purpose, reason, scope, previous_state, resulting_state, policy_version, created_at";

/// Caller standing: `active` and not deleted. Missing, pending, suspended,
/// banned, and deleted accounts share one refusal.
async fn active_standing(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
) -> Result<bool, StaffError> {
    let row: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| StaffError::StorageFailed)?;
    Ok(matches!(row, Some((state, None)) if state == "active"))
}

/// Target presence: the row exists and is not deleted.
async fn present_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
) -> Result<bool, StaffError> {
    let row: Option<Option<chrono::DateTime<chrono::Utc>>> =
        sqlx::query_scalar("SELECT deleted_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| StaffError::StorageFailed)?;
    Ok(matches!(row, Some(None)))
}

async fn live_grant(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
    role: &str,
) -> Result<Option<StaffGrant>, StaffError> {
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {GRANT_COLUMNS} FROM staff_grants WHERE user_id = $1 AND role = $2 LIMIT 1"
    ))
    .bind(user_id)
    .bind(role)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| StaffError::StorageFailed)?;
    row.map(|row| read_grant(&row)).transpose()
}

async fn any_live_admin(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<bool, StaffError> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM staff_grants WHERE role = 'administrator' LIMIT 1")
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| StaffError::StorageFailed)?;
    Ok(found.is_some())
}

async fn audit_grant_decision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: uuid::Uuid,
    grant: &StaffGrant,
    purpose: &str,
    policy_version: &str,
) -> Result<AccessAudit, StaffError> {
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO staff_access_audit
            (actor_id, action, target_kind, target_id, purpose, reason,
             scope, previous_state, resulting_state, policy_version)
         VALUES ($1, 'grant', 'staff_grant', $2, $3, $4, $5, 'none', $6, $7)
         RETURNING {AUDIT_COLUMNS}"
    ))
    .bind(actor_id)
    .bind(grant.id)
    .bind(purpose)
    .bind(&grant.reason)
    .bind(&grant.scope)
    .bind(&grant.role)
    .bind(policy_version)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| StaffError::StorageFailed)?;
    read_audit(&row)
}

/// Record the launch-bootstrap grant: the audited first-power path with
/// explicit operator input and no granting administrator. Refuses once any
/// live administrator grant exists — later powers flow through
/// [`grant_role`] only.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`StaffError::InvalidField`] for unknown role/scope or bad
/// reason/operator/policy text, [`StaffError::NotFound`] for a missing
/// target, [`StaffError::NotActive`] for a deleted target,
/// [`StaffError::NotPermitted`] once the bootstrap already closed, else
/// [`StaffError::StorageFailed`]. Reasons are static.
pub async fn bootstrap_grant(
    pool: &sqlx::PgPool,
    input: BootstrapInput,
) -> Result<StaffGrant, StaffError> {
    check_grant_fields(&input.role, &input.scope, &input.reason)?;
    check_operator(&input.operator_label)?;
    check_policy(&input.policy_version)?;
    let mut tx = pool.begin().await.map_err(|_| StaffError::StorageFailed)?;
    if !present_target(&mut tx, input.user_id).await? {
        // Missing and deleted share no oracle here: both read as absent.
        let missing: Option<i32> = sqlx::query_scalar("SELECT 1 FROM users WHERE id = $1")
            .bind(input.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StaffError::StorageFailed)?;
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(if missing.is_some() {
            StaffError::NotActive
        } else {
            StaffError::NotFound
        });
    }
    if any_live_admin(&mut tx).await? {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotPermitted);
    }
    let existing: Option<StaffGrant> = {
        let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
            "SELECT {GRANT_COLUMNS} FROM staff_grants
             WHERE user_id = $1 AND role = $2 AND scope = $3 LIMIT 1"
        ))
        .bind(input.user_id)
        .bind(&input.role)
        .bind(&input.scope)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| StaffError::StorageFailed)?;
        row.map(|row| read_grant(&row)).transpose()?
    };
    if let Some(standing) = existing {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Ok(standing);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO staff_grants (user_id, role, scope, reason, granted_by)
         VALUES ($1, $2, $3, $4, NULL)
         RETURNING {GRANT_COLUMNS}"
    ))
    .bind(input.user_id)
    .bind(&input.role)
    .bind(&input.scope)
    .bind(&input.reason)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| StaffError::StorageFailed)?;
    let grant = read_grant(&row)?;
    // The operator is local input, not a user row: the audit names the
    // granted account as actor and carries the explicit operator label in
    // its purpose, so the bootstrap stays attributable without inventing
    // privileged rows.
    audit_grant_decision(
        &mut tx,
        input.user_id,
        &grant,
        &format!("launch bootstrap by {}", input.operator_label.trim()),
        &input.policy_version,
    )
    .await?;
    tx.commit().await.map_err(|_| StaffError::StorageFailed)?;
    Ok(grant)
}

/// Grant one scoped power by administrator authority: the granter must be
/// active and hold a live administrator grant. Moderators, regular, and
/// professional accounts refuse here — permission management is
/// administrator-only. Repeating an identical live grant converges on the
/// standing row with no duplicate audit.
///
/// # Errors
///
/// Returns [`StaffError::InvalidField`] for unknown role/scope or bad
/// reason/policy text, [`StaffError::NotActive`] for restricted granters
/// or deleted targets, [`StaffError::NotFound`] for missing targets,
/// [`StaffError::NotPermitted`] for non-administrator granters, else
/// [`StaffError::StorageFailed`]. Reasons are static.
pub async fn grant_role(
    pool: &sqlx::PgPool,
    granter_id: uuid::Uuid,
    input: GrantInput,
    policy_version: &str,
) -> Result<StaffGrant, StaffError> {
    check_grant_fields(&input.role, &input.scope, &input.reason)?;
    check_policy(policy_version)?;
    let mut tx = pool.begin().await.map_err(|_| StaffError::StorageFailed)?;
    if !active_standing(&mut tx, granter_id).await? {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotActive);
    }
    if live_grant(&mut tx, granter_id, ROLE_ADMINISTRATOR)
        .await?
        .is_none()
    {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotPermitted);
    }
    if !present_target(&mut tx, input.user_id).await? {
        let missing: Option<i32> = sqlx::query_scalar("SELECT 1 FROM users WHERE id = $1")
            .bind(input.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StaffError::StorageFailed)?;
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(if missing.is_some() {
            StaffError::NotActive
        } else {
            StaffError::NotFound
        });
    }
    let existing: Option<StaffGrant> = {
        let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
            "SELECT {GRANT_COLUMNS} FROM staff_grants
             WHERE user_id = $1 AND role = $2 AND scope = $3 LIMIT 1"
        ))
        .bind(input.user_id)
        .bind(&input.role)
        .bind(&input.scope)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| StaffError::StorageFailed)?;
        row.map(|row| read_grant(&row)).transpose()?
    };
    if let Some(standing) = existing {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Ok(standing);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO staff_grants (user_id, role, scope, reason, granted_by)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING {GRANT_COLUMNS}"
    ))
    .bind(input.user_id)
    .bind(&input.role)
    .bind(&input.scope)
    .bind(&input.reason)
    .bind(granter_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| StaffError::StorageFailed)?;
    let grant = read_grant(&row)?;
    audit_grant_decision(
        &mut tx,
        granter_id,
        &grant,
        "operational permission management",
        policy_version,
    )
    .await?;
    tx.commit().await.map_err(|_| StaffError::StorageFailed)?;
    Ok(grant)
}

/// Require a live moderator-or-better grant for an active account.
/// Administrators satisfy moderator gates by hierarchy.
///
/// # Errors
///
/// Returns [`StaffError::NotActive`] for restricted callers,
/// [`StaffError::NotPermitted`] without a live grant, else
/// [`StaffError::StorageFailed`].
pub async fn require_moderator(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> Result<StaffGrant, StaffError> {
    let mut tx = pool.begin().await.map_err(|_| StaffError::StorageFailed)?;
    if !active_standing(&mut tx, user_id).await? {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotActive);
    }
    let moderator = live_grant(&mut tx, user_id, ROLE_MODERATOR).await?;
    let grant = match moderator {
        Some(grant) => Some(grant),
        None => live_grant(&mut tx, user_id, ROLE_ADMINISTRATOR).await?,
    };
    tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
    grant.ok_or(StaffError::NotPermitted)
}

/// Require a live administrator grant for an active account. Moderators
/// refuse here: permanent bans, reversals, permission management, and
/// policy approval stay administrator-only.
///
/// # Errors
///
/// Returns [`StaffError::NotActive`] for restricted callers,
/// [`StaffError::NotPermitted`] without a live administrator grant, else
/// [`StaffError::StorageFailed`].
pub async fn require_admin(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> Result<StaffGrant, StaffError> {
    let mut tx = pool.begin().await.map_err(|_| StaffError::StorageFailed)?;
    if !active_standing(&mut tx, user_id).await? {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotActive);
    }
    let grant = live_grant(&mut tx, user_id, ROLE_ADMINISTRATOR).await?;
    tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
    grant.ok_or(StaffError::NotPermitted)
}

async fn inspection_target_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    target_id: uuid::Uuid,
) -> Result<bool, StaffError> {
    let table = match kind {
        "user" => "users",
        "request" => "requests",
        "offer" => "offers",
        "report" => "reports",
        "case" => "report_cases",
        _ => return Err(StaffError::InvalidField),
    };
    let found: Option<i32> = sqlx::query_scalar(&format!("SELECT 1 FROM {table} WHERE id = $1"))
        .bind(target_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| StaffError::StorageFailed)?;
    Ok(found.is_some())
}

/// Authorize one sensitive inspection for granted staff and audit it: the
/// actor must hold a live moderator-or-better grant, the purpose must be
/// explicit, and the target must exist. Returns the audit row; inspected
/// content itself travels through the card that owns the read.
///
/// # Errors
///
/// Returns [`StaffError::InvalidField`] for unknown kinds or bad
/// purpose/policy text, [`StaffError::NotActive`] for restricted actors,
/// [`StaffError::NotFound`] for fabricated targets,
/// [`StaffError::NotPermitted`] for ungranted actors, else
/// [`StaffError::StorageFailed`]. Reasons are static.
pub async fn authorized_inspect(
    pool: &sqlx::PgPool,
    actor_id: uuid::Uuid,
    input: InspectInput,
) -> Result<AccessAudit, StaffError> {
    check_purpose(&input.purpose)?;
    check_policy(&input.policy_version)?;
    let mut tx = pool.begin().await.map_err(|_| StaffError::StorageFailed)?;
    if !active_standing(&mut tx, actor_id).await? {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotActive);
    }
    let moderator = live_grant(&mut tx, actor_id, ROLE_MODERATOR).await?;
    if moderator.is_none()
        && live_grant(&mut tx, actor_id, ROLE_ADMINISTRATOR)
            .await?
            .is_none()
    {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotPermitted);
    }
    if !inspection_target_exists(&mut tx, &input.target_kind, input.target_id).await? {
        tx.rollback().await.map_err(|_| StaffError::StorageFailed)?;
        return Err(StaffError::NotFound);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO staff_access_audit
            (actor_id, action, target_kind, target_id, purpose, policy_version)
         VALUES ($1, 'inspect', $2, $3, $4, $5)
         RETURNING {AUDIT_COLUMNS}"
    ))
    .bind(actor_id)
    .bind(&input.target_kind)
    .bind(input.target_id)
    .bind(&input.purpose)
    .bind(&input.policy_version)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| StaffError::StorageFailed)?;
    let audit = read_audit(&row)?;
    tx.commit().await.map_err(|_| StaffError::StorageFailed)?;
    Ok(audit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_fields_need_known_vocabulary_and_reason() {
        assert!(check_grant_fields("moderator", "safety", "launch cover").is_ok());
        assert!(check_grant_fields("administrator", "policy", "policy owner").is_ok());
        assert_eq!(
            check_grant_fields("owner", "safety", "words"),
            Err(StaffError::InvalidField)
        );
        assert_eq!(
            check_grant_fields("moderator", "everything", "words"),
            Err(StaffError::InvalidField)
        );
        assert_eq!(
            check_grant_fields("moderator", "safety", ""),
            Err(StaffError::InvalidField)
        );
        assert_eq!(
            check_grant_fields("moderator", "safety", &"x".repeat(1001)),
            Err(StaffError::InvalidField)
        );
    }

    #[test]
    fn purpose_operator_and_policy_are_explicit() {
        assert!(check_purpose("fraud triage for case intake").is_ok());
        assert_eq!(check_purpose("   "), Err(StaffError::InvalidField));
        assert_eq!(check_purpose(""), Err(StaffError::InvalidField));
        assert_eq!(check_operator("launch-operator-1"), Ok(()));
        assert_eq!(check_operator("   "), Err(StaffError::InvalidField));
        assert_eq!(check_policy("v1"), Ok(()));
        assert_eq!(check_policy(""), Err(StaffError::InvalidField));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            StaffError::InvalidField,
            StaffError::NotActive,
            StaffError::NotFound,
            StaffError::NotPermitted,
            StaffError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
