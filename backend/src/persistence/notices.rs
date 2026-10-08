//! In-product notices: private per-recipient facts about business events.
//!
//! A notice tells exactly one account that something it may act on happened.
//! Notices are recorded in the same transaction as their mutation and event,
//! deduplicated per (event, recipient), and acknowledged only by their
//! recipient — a notice is never visible to, or completable by, another
//! account (INV-31). Bodies are short templates without private material:
//! phone destinations, reporter identities, and secrets have no parameter
//! anywhere in this module.

/// Maximum notice body length in scalar values (mirrors the database backstop).
pub const NOTICE_BODY_MAX_CHARS: usize = 1000;

/// A new notice for one recipient about one recorded event.
#[derive(Debug, Clone)]
pub struct NewNotice {
    /// Intended recipient account. The only reader allowed.
    pub account_id: uuid::Uuid,
    /// Notice kind (`offer.received`, `contact.initiated`, ...).
    pub kind: &'static str,
    /// Resource family of the underlying fact.
    pub resource_kind: &'static str,
    /// Affected resource identifier.
    pub resource_id: uuid::Uuid,
    /// The recorded event this notice explains. Must already exist in the
    /// same transaction.
    pub event_id: uuid::Uuid,
    /// Short templated body. No phone, reporter, or secret material.
    pub body: String,
}

/// A persisted notice.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Intended recipient account.
    pub account_id: uuid::Uuid,
    /// Notice kind.
    pub kind: String,
    /// Resource family.
    pub resource_kind: String,
    /// Affected resource identifier.
    pub resource_id: uuid::Uuid,
    /// The explained event.
    pub event_id: uuid::Uuid,
    /// Short body, exactly as supplied.
    pub body: String,
    /// Acknowledgment instant, if the recipient confirmed.
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Typed notice-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeError {
    /// Body exceeds its scalar bound.
    BodyTooLong,
    /// This recipient already has a notice for this event (INV-34).
    Duplicate,
    /// The insert failed (missing event, connection, transaction state).
    StorageFailed,
}

impl std::fmt::Display for NoticeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::BodyTooLong => "notice body exceeds its bound",
            Self::Duplicate => "notice already recorded for this event",
            Self::StorageFailed => "notice storage failed",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for NoticeError {}

/// Record one notice through the caller's executor (transaction or pool).
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`NoticeError::BodyTooLong`] before any insert, [`NoticeError::Duplicate`]
/// when this recipient already has a notice for the event, else
/// [`NoticeError::StorageFailed`]. Reasons are static.
pub async fn record<'e, E>(executor: E, notice: NewNotice) -> Result<Notice, NoticeError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    if notice.body.chars().count() > NOTICE_BODY_MAX_CHARS {
        return Err(NoticeError::BodyTooLong);
    }
    let row = sqlx::query(
        r#"INSERT INTO notices
            (account_id, kind, resource_kind, resource_id, event_id, body)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, account_id, kind, resource_kind, resource_id,
                     event_id, body, acknowledged_at"#,
    )
    .bind(notice.account_id)
    .bind(notice.kind)
    .bind(notice.resource_kind)
    .bind(notice.resource_id)
    .bind(notice.event_id)
    .bind(notice.body)
    .fetch_one(executor)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(|database| database.code().as_deref() == Some("23505"))
        {
            NoticeError::Duplicate
        } else {
            NoticeError::StorageFailed
        }
    })?;
    use sqlx::Row;
    Ok(Notice {
        id: row.try_get("id").map_err(|_| NoticeError::StorageFailed)?,
        account_id: row
            .try_get("account_id")
            .map_err(|_| NoticeError::StorageFailed)?,
        kind: row
            .try_get("kind")
            .map_err(|_| NoticeError::StorageFailed)?,
        resource_kind: row
            .try_get("resource_kind")
            .map_err(|_| NoticeError::StorageFailed)?,
        resource_id: row
            .try_get("resource_id")
            .map_err(|_| NoticeError::StorageFailed)?,
        event_id: row
            .try_get("event_id")
            .map_err(|_| NoticeError::StorageFailed)?,
        body: row
            .try_get("body")
            .map_err(|_| NoticeError::StorageFailed)?,
        acknowledged_at: row
            .try_get("acknowledged_at")
            .map_err(|_| NoticeError::StorageFailed)?,
    })
}

/// Acknowledge a notice as its recipient. Returns `true` when exactly this
/// recipient's notice transitioned; `false` for unknown ids and for other
/// accounts' notices (indistinguishable by design).
///
/// # Errors
///
/// Returns [`NoticeError::StorageFailed`] on database failure only.
pub async fn acknowledge<'e, E>(
    executor: E,
    notice_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> Result<bool, NoticeError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let affected = sqlx::query(
        "UPDATE notices SET acknowledged_at = now()
          WHERE id = $1 AND account_id = $2 AND acknowledged_at IS NULL",
    )
    .bind(notice_id)
    .bind(account_id)
    .execute(executor)
    .await
    .map_err(|_| NoticeError::StorageFailed)?;
    Ok(affected.rows_affected() == 1)
}
