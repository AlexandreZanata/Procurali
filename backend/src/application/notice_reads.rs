//! Private notice reads: owner lists and acknowledgments.
//!
//! Canonical rules: INV-05 (no role input — every read scopes by the
//! authenticated account identifier against the recipient column, never
//! labels), INV-31 (notices carry safe templates only — bodies name no
//! phone, address, reporter, or secret, a property enforced where each
//! notice is produced and projected here verbatim), INV-34 (repeats
//! duplicate nothing — acknowledgment flips an unset timestamp once, and
//! repeat calls return the same row with no new fact of any kind).
//!
//! Acknowledgment is bookkeeping, never engagement: it changes no offer
//! view state, no outcome, no contact, and no delivery. Listing is bounded
//! and newest-first with the owned total.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::persistence::notices::acknowledge;

/// Default page size for owner notice lists.
pub const DEFAULT_PAGE_LIMIT: u32 = 20;

/// Hard ceiling for one owner notice page.
pub const MAX_PAGE_LIMIT: u32 = 50;

/// One owner-visible notice: the stored template plus its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeView {
    /// Notice identifier.
    pub id: uuid::Uuid,
    /// Notice kind (`offer.received`, `request.expired`, ...).
    pub kind: String,
    /// Resource family.
    pub resource_kind: String,
    /// Affected resource identifier.
    pub resource_id: uuid::Uuid,
    /// The explained event.
    pub event_id: uuid::Uuid,
    /// Short body, exactly as recorded.
    pub body: String,
    /// Acknowledgment instant, once confirmed.
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Recording instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Typed notice-read failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeReadError {
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such notice for this owner (missing or foreign —
    /// deliberately indistinguishable).
    NotFound,
    /// Pagination is malformed.
    InvalidPage,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for NoticeReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("notice not found"),
            Self::InvalidPage => f.write_str("invalid pagination"),
            Self::StorageFailed => f.write_str("notice lookup failed"),
        }
    }
}

impl std::error::Error for NoticeReadError {}

async fn eligible_owner(
    pool: &sqlx::PgPool,
    account_id: uuid::Uuid,
) -> Result<(), NoticeReadError> {
    match check_actor(pool, account_id)
        .await
        .map_err(|_| NoticeReadError::StorageFailed)?
    {
        CheckOutcome::Permitted => Ok(()),
        CheckOutcome::Refused(_) => Err(NoticeReadError::NotActive),
    }
}

fn read_view(row: &sqlx::postgres::PgRow) -> Result<NoticeView, NoticeReadError> {
    use sqlx::Row;
    Ok(NoticeView {
        id: row
            .try_get("id")
            .map_err(|_| NoticeReadError::StorageFailed)?,
        kind: row
            .try_get("kind")
            .map_err(|_| NoticeReadError::StorageFailed)?,
        resource_kind: row
            .try_get("resource_kind")
            .map_err(|_| NoticeReadError::StorageFailed)?,
        resource_id: row
            .try_get("resource_id")
            .map_err(|_| NoticeReadError::StorageFailed)?,
        event_id: row
            .try_get("event_id")
            .map_err(|_| NoticeReadError::StorageFailed)?,
        body: row
            .try_get("body")
            .map_err(|_| NoticeReadError::StorageFailed)?,
        acknowledged_at: row
            .try_get("acknowledged_at")
            .map_err(|_| NoticeReadError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| NoticeReadError::StorageFailed)?,
    })
}

async fn view_by_id(
    pool: &sqlx::PgPool,
    account_id: uuid::Uuid,
    notice_id: uuid::Uuid,
) -> Result<NoticeView, NoticeReadError> {
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT id, kind, resource_kind, resource_id, event_id, body,
                acknowledged_at, created_at
         FROM notices WHERE id = $1 AND account_id = $2",
    )
    .bind(notice_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| NoticeReadError::StorageFailed)?;
    match row {
        Some(row) => read_view(&row),
        None => Err(NoticeReadError::NotFound),
    }
}

/// Bounded owner notice list, newest first, with the total owned count.
///
/// # Errors
///
/// Returns [`NoticeReadError::NotActive`] for restricted accounts,
/// [`NoticeReadError::InvalidPage`] for malformed pagination, else
/// [`NoticeReadError::StorageFailed`].
pub async fn list_notices(
    pool: &sqlx::PgPool,
    account_id: uuid::Uuid,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<(Vec<NoticeView>, i64), NoticeReadError> {
    eligible_owner(pool, account_id).await?;
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    let offset = offset.unwrap_or(0);
    if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
        return Err(NoticeReadError::InvalidPage);
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM notices WHERE account_id = $1")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .map_err(|_| NoticeReadError::StorageFailed)?;
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT id, kind, resource_kind, resource_id, event_id, body,
                acknowledged_at, created_at
         FROM notices WHERE account_id = $1 ORDER BY created_at DESC, id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(account_id)
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await
    .map_err(|_| NoticeReadError::StorageFailed)?;
    let mut views = Vec::with_capacity(rows.len());
    for row in &rows {
        views.push(read_view(row)?);
    }
    Ok((views, total))
}

/// Acknowledge one owned notice: flips an unset timestamp once and returns
/// the row. Repeats return the same acknowledged row with no new write and
/// no fact of any kind — acknowledgment is never view, outcome, contact,
/// or delivery.
///
/// # Errors
///
/// Returns [`NoticeReadError::NotActive`] for restricted accounts,
/// [`NoticeReadError::NotFound`] for missing or foreign rows, else
/// [`NoticeReadError::StorageFailed`].
pub async fn acknowledge_notice(
    pool: &sqlx::PgPool,
    account_id: uuid::Uuid,
    notice_id: uuid::Uuid,
) -> Result<NoticeView, NoticeReadError> {
    eligible_owner(pool, account_id).await?;
    view_by_id(pool, account_id, notice_id).await?;
    acknowledge(pool, notice_id, account_id)
        .await
        .map_err(|_| NoticeReadError::StorageFailed)?;
    view_by_id(pool, account_id, notice_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_match_policy_defaults() {
        assert_eq!(DEFAULT_PAGE_LIMIT, 20);
        assert_eq!(MAX_PAGE_LIMIT, 50);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            NoticeReadError::NotActive,
            NoticeReadError::NotFound,
            NoticeReadError::InvalidPage,
            NoticeReadError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
