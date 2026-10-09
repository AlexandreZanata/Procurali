//! Structured contacted-party feedback: one observation set per contact.
//!
//! Canonical rules: reputation 14.2 (buyers optionally answer whether the
//! seller actually had the offered item with yes, no, or unable to
//! confirm; the window opens 24 hours after first contact and closes 14
//! days after it; one set per buyer, seller, request, and cycle with
//! buyer corrections inside the same window and preserved revision
//! history; a negative answer only invites a separate report), INV-34
//! (repeat handoffs and revisions never multiply credit — the set lives
//! on the unique contact row), INV-36 with AC-42 (a negative answer is
//! never an automatic validated incident or ban), and INV-42 (duplicate
//! observations stay excluded by construction; corrections append).
//!
//! Only the contacting buyer files, and only while active. Sellers reach
//! moderation review through disputes on later cards — never by filing
//! here, and never by reading protected reporter details.

use serde::Serialize;

/// Feedback answers: whether the seller actually had the offered item.
pub const FEEDBACK_ANSWERS: [&str; 3] = ["yes", "no", "unable"];
/// Window opens this long after first contact.
pub const FEEDBACK_OPENS_AFTER: chrono::Duration = chrono::Duration::hours(24);
/// Window closes this long after first contact.
pub const FEEDBACK_CLOSES_AFTER: chrono::Duration = chrono::Duration::days(14);

/// One feedback submission as supplied: the buyer's current answer.
#[derive(Debug, Clone)]
pub struct FeedbackInput {
    /// Contacted handoff under review (must already exist).
    pub contact_id: uuid::Uuid,
    /// `yes`, `no`, or `unable to confirm` (stored as `unable`).
    pub answer: String,
}

/// One filed observation set: the current answer with its standing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Feedback {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Observed contact.
    pub contact_id: uuid::Uuid,
    /// Filing buyer.
    pub buyer_id: uuid::Uuid,
    /// Observed seller.
    pub seller_id: uuid::Uuid,
    /// Observed demand.
    pub request_id: uuid::Uuid,
    /// Observed cycle.
    pub cycle_number: i32,
    /// Current answer.
    pub answer: String,
    /// Filing count for this contact (corrections increment).
    pub version: i32,
    /// Whether a `no` answer invites a separate report (never automatic).
    pub suggest_report: bool,
}

/// Typed feedback failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackError {
    /// Unknown answer vocabulary.
    InvalidField,
    /// The filing account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such contact exists.
    NotFound,
    /// The caller is not the contacting buyer.
    NotPermitted,
    /// The window has not opened yet or already closed.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for FeedbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid feedback field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("feedback contact not found"),
            Self::NotPermitted => f.write_str("feedback not permitted"),
            Self::InvalidState => f.write_str("feedback window closed"),
            Self::StorageFailed => f.write_str("feedback storage failed"),
        }
    }
}

impl std::error::Error for FeedbackError {}

/// Window standing for one contact at one instant: open from exactly 24
/// hours after first contact through exactly 14 days after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowStanding {
    Open,
    TooEarly,
    Closed,
}

fn window_state(
    initiated_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> WindowStanding {
    if now < initiated_at + FEEDBACK_OPENS_AFTER {
        WindowStanding::TooEarly
    } else if now > initiated_at + FEEDBACK_CLOSES_AFTER {
        WindowStanding::Closed
    } else {
        WindowStanding::Open
    }
}

fn read_feedback(row: &sqlx::postgres::PgRow) -> Result<Feedback, FeedbackError> {
    use sqlx::Row;
    let answer: String = row
        .try_get("answer")
        .map_err(|_| FeedbackError::StorageFailed)?;
    Ok(Feedback {
        id: row
            .try_get("id")
            .map_err(|_| FeedbackError::StorageFailed)?,
        contact_id: row
            .try_get("contact_id")
            .map_err(|_| FeedbackError::StorageFailed)?,
        buyer_id: row
            .try_get("buyer_id")
            .map_err(|_| FeedbackError::StorageFailed)?,
        seller_id: row
            .try_get("seller_id")
            .map_err(|_| FeedbackError::StorageFailed)?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| FeedbackError::StorageFailed)?,
        cycle_number: row
            .try_get("cycle_number")
            .map_err(|_| FeedbackError::StorageFailed)?,
        version: row
            .try_get("version")
            .map_err(|_| FeedbackError::StorageFailed)?,
        suggest_report: answer == "no",
        answer,
    })
}

const FEEDBACK_COLUMNS: &str =
    "id, contact_id, buyer_id, seller_id, request_id, cycle_number, answer, version";

/// File or correct one observation set for the contacting buyer: initial
/// filings and same-window corrections share this path, with every filing
/// appended to revision history. Repeat handoffs resolve to the same
/// contact row, so credit never multiplies.
///
/// # Errors
///
/// Returns [`FeedbackError::InvalidField`] for unknown answers,
/// [`FeedbackError::NotActive`] for restricted filers,
/// [`FeedbackError::NotFound`] for missing contacts,
/// [`FeedbackError::NotPermitted`] for non-buyer callers,
/// [`FeedbackError::InvalidState`] outside the contact window, else
/// [`FeedbackError::StorageFailed`]. Reasons are static.
pub async fn submit_feedback(
    pool: &sqlx::PgPool,
    buyer_id: uuid::Uuid,
    input: FeedbackInput,
) -> Result<Feedback, FeedbackError> {
    if !FEEDBACK_ANSWERS.contains(&input.answer.as_str()) {
        return Err(FeedbackError::InvalidField);
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| FeedbackError::StorageFailed)?;
    let caller: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(buyer_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| FeedbackError::StorageFailed)?;
    match caller {
        Some((state, None)) if state == "active" => {}
        _ => {
            tx.rollback()
                .await
                .map_err(|_| FeedbackError::StorageFailed)?;
            return Err(FeedbackError::NotActive);
        }
    }
    let contact: Option<(
        uuid::Uuid,
        uuid::Uuid,
        uuid::Uuid,
        i32,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT buyer_id, seller_id, request_id, cycle_number, initiated_at
         FROM contacts WHERE id = $1",
    )
    .bind(input.contact_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| FeedbackError::StorageFailed)?;
    let (contact_buyer, seller_id, request_id, cycle_number, initiated_at) = match contact {
        Some(contact) => contact,
        None => {
            tx.rollback()
                .await
                .map_err(|_| FeedbackError::StorageFailed)?;
            return Err(FeedbackError::NotFound);
        }
    };
    if contact_buyer != buyer_id {
        tx.rollback()
            .await
            .map_err(|_| FeedbackError::StorageFailed)?;
        return Err(FeedbackError::NotPermitted);
    }
    let now = chrono::Utc::now();
    if window_state(initiated_at, now) != WindowStanding::Open {
        tx.rollback()
            .await
            .map_err(|_| FeedbackError::StorageFailed)?;
        return Err(FeedbackError::InvalidState);
    }
    let existing: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {FEEDBACK_COLUMNS} FROM contact_feedback WHERE contact_id = $1 LIMIT 1"
    ))
    .bind(input.contact_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| FeedbackError::StorageFailed)?;
    let stored = match existing {
        Some(row) => {
            let standing = read_feedback(&row)?;
            let updated: sqlx::postgres::PgRow = sqlx::query(&format!(
                "UPDATE contact_feedback SET answer = $2, version = version + 1,
                        updated_at = now()
                 WHERE id = $1 RETURNING {FEEDBACK_COLUMNS}"
            ))
            .bind(standing.id)
            .bind(&input.answer)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| FeedbackError::StorageFailed)?;
            sqlx::query(
                "INSERT INTO feedback_revisions (feedback_id, previous_answer, answer)
                 VALUES ($1, $2, $3)",
            )
            .bind(standing.id)
            .bind(&standing.answer)
            .bind(&input.answer)
            .execute(&mut *tx)
            .await
            .map_err(|_| FeedbackError::StorageFailed)?;
            read_feedback(&updated)?
        }
        None => {
            let row: sqlx::postgres::PgRow = sqlx::query(&format!(
                "INSERT INTO contact_feedback
                    (contact_id, buyer_id, seller_id, request_id, cycle_number, answer)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 RETURNING {FEEDBACK_COLUMNS}"
            ))
            .bind(input.contact_id)
            .bind(buyer_id)
            .bind(seller_id)
            .bind(request_id)
            .bind(cycle_number)
            .bind(&input.answer)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| FeedbackError::StorageFailed)?;
            let filed = read_feedback(&row)?;
            sqlx::query(
                "INSERT INTO feedback_revisions (feedback_id, previous_answer, answer)
                 VALUES ($1, NULL, $2)",
            )
            .bind(filed.id)
            .bind(&input.answer)
            .execute(&mut *tx)
            .await
            .map_err(|_| FeedbackError::StorageFailed)?;
            filed
        }
    };
    tx.commit()
        .await
        .map_err(|_| FeedbackError::StorageFailed)?;
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_answers_are_closed() {
        for answer in ["yes", "no", "unable"] {
            assert!(FEEDBACK_ANSWERS.contains(&answer));
        }
        assert!(!FEEDBACK_ANSWERS.contains(&"maybe"));
        assert!(!FEEDBACK_ANSWERS.contains(&"stars"));
    }

    #[test]
    fn window_bounds_hold() {
        assert!(FEEDBACK_OPENS_AFTER < FEEDBACK_CLOSES_AFTER);
        assert_eq!(FEEDBACK_OPENS_AFTER.num_hours(), 24);
        assert_eq!(FEEDBACK_CLOSES_AFTER.num_days(), 14);
    }

    #[test]
    fn window_edges_are_exact() {
        let initiated = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed time parses")
            .with_timezone(&chrono::Utc);
        assert_eq!(
            window_state(initiated, initiated + chrono::Duration::hours(24)),
            WindowStanding::Open
        );
        assert_eq!(
            window_state(initiated, initiated + chrono::Duration::days(14)),
            WindowStanding::Open
        );
        assert_eq!(
            window_state(
                initiated,
                initiated + chrono::Duration::hours(24) - chrono::Duration::seconds(1)
            ),
            WindowStanding::TooEarly
        );
        assert_eq!(
            window_state(
                initiated,
                initiated + chrono::Duration::days(14) + chrono::Duration::seconds(1)
            ),
            WindowStanding::Closed
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            FeedbackError::InvalidField,
            FeedbackError::NotActive,
            FeedbackError::NotFound,
            FeedbackError::NotPermitted,
            FeedbackError::InvalidState,
            FeedbackError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
