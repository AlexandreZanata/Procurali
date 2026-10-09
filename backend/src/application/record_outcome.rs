//! Declared buyer outcomes: source, attribution, and corrections.
//!
//! Canonical rules: INV-13 (completion, cancellation, expiry, and removal
//! stay distinct facts — this module adds the declared-outcome row without
//! touching any of them), INV-34 with EC-27 (repeats duplicate nothing —
//! an identical answer returns its standing row while terminal rows stay
//! terminal), INV-43 with AC-28 and EC-26 (completion is declared, never a
//! verified sale — platform results link only to a historical contact the
//! buyer actually made, and unattributed completions credit nobody),
//! AC-30 (cancellation stays buyer abandonment), AC-31 (unknown stays
//! unknown — explicit, never fabricated success).
//!
//! The lifecycle moves through the existing closure transition exactly
//! once; this layer adds the declared source, the optional platform
//! attribution, and the superseding correction chain on top. Latest row
//! wins; prior rows stay readable as the correction trail.

use crate::application::close_request::{
    close_request, BuyerOutcome as CloseOutcome, OutcomeSource as CloseSource,
};
use serde::Serialize;

/// Declared completion source: the buyer's own label, never verified here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionSource {
    /// Resolved through a contacted platform offer (attribution required).
    Platform {
        /// The contacted offer receiving credit.
        offer_id: uuid::Uuid,
    },
    /// Resolved elsewhere.
    Elsewhere,
    /// The buyer does not say.
    Unknown,
}

/// The buyer's answer: found, no longer needed, or not yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeAnswer {
    /// "Yes, I found it" with the declared source.
    Completed(CompletionSource),
    /// "I no longer need it."
    Cancelled,
    /// "Not yet": explicitly unresolved.
    Unresolved,
}

/// One recorded outcome row: the latest answer with its trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedOutcome {
    /// Outcome row identifier.
    pub id: uuid::Uuid,
    /// Target request identifier.
    pub request_id: uuid::Uuid,
    /// Declared outcome (`completed`, `cancelled`, `unresolved`).
    pub outcome: String,
    /// Declared source for completions, if any.
    pub source: Option<String>,
    /// Credited offer for platform completions, if any.
    pub attributed_offer_id: Option<uuid::Uuid>,
    /// Prior row this answer supersedes, if any.
    pub supersedes: Option<uuid::Uuid>,
}

/// Typed outcome failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeError {
    /// Attribution is missing, foreign, uncontacted, or paired with a
    /// non-platform source.
    AttributionRefused,
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such request for this owner (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// The row is a draft or already terminal.
    ForbiddenState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for OutcomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AttributionRefused => f.write_str("offer cannot receive credit"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("request not found"),
            Self::ForbiddenState => f.write_str("request cannot be closed"),
            Self::StorageFailed => f.write_str("outcome storage failed"),
        }
    }
}

impl std::error::Error for OutcomeError {}

fn read_outcome(row: &sqlx::postgres::PgRow) -> Result<RecordedOutcome, OutcomeError> {
    use sqlx::Row;
    Ok(RecordedOutcome {
        id: row.try_get("id").map_err(|_| OutcomeError::StorageFailed)?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| OutcomeError::StorageFailed)?,
        outcome: row
            .try_get("outcome")
            .map_err(|_| OutcomeError::StorageFailed)?,
        source: row
            .try_get("source")
            .map_err(|_| OutcomeError::StorageFailed)?,
        attributed_offer_id: row
            .try_get("attributed_offer_id")
            .map_err(|_| OutcomeError::StorageFailed)?,
        supersedes: row
            .try_get("supersedes")
            .map_err(|_| OutcomeError::StorageFailed)?,
    })
}

const OUTCOME_COLUMNS: &str = "id, request_id, outcome, source, attributed_offer_id, supersedes";

/// Latest declared outcome for one request, if any.
async fn latest_outcome(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
) -> Result<Option<RecordedOutcome>, OutcomeError> {
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {OUTCOME_COLUMNS} FROM request_outcomes
         WHERE request_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| OutcomeError::StorageFailed)?;
    row.map(|row| read_outcome(&row)).transpose()
}

/// Record one declared buyer outcome with its source and attribution.
///
/// Platform completions credit exactly the contacted offer named — which
/// must belong to this request with a historical contact by this buyer —
/// while elsewhere and unknown completions travel unattributed. An
/// identical answer returns its standing row with no new lifecycle move
/// and no new row; a changed answer supersedes the prior row. Lifecycle
/// moves through the closure transition, so terminal and draft rows keep
/// their existing refusals.
///
/// # Errors
///
/// Returns [`OutcomeError::AttributionRefused`] for missing, foreign,
/// uncontacted, or mispaired attribution, [`OutcomeError::NotActive`] for
/// restricted accounts, [`OutcomeError::NotFound`] for missing or
/// non-owned rows, [`OutcomeError::ForbiddenState`] past closure scope,
/// else [`OutcomeError::StorageFailed`].
pub async fn record_outcome(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
    answer: OutcomeAnswer,
) -> Result<RecordedOutcome, OutcomeError> {
    let (outcome, source, attributed) = match answer {
        OutcomeAnswer::Completed(CompletionSource::Platform { offer_id }) => {
            ("completed", Some("platform"), Some(offer_id))
        }
        OutcomeAnswer::Completed(CompletionSource::Elsewhere) => {
            ("completed", Some("elsewhere"), None)
        }
        OutcomeAnswer::Completed(CompletionSource::Unknown) => ("completed", Some("unknown"), None),
        OutcomeAnswer::Cancelled => ("cancelled", None, None),
        OutcomeAnswer::Unresolved => ("unresolved", None, None),
    };
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| OutcomeError::StorageFailed)?;
    // Lock the demand row first: concurrent recordings of the same answer
    // serialize here, and every loser replays the winner's row.
    sqlx::query("SELECT 1 FROM requests WHERE id = $1 FOR UPDATE")
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| OutcomeError::StorageFailed)?;
    let stored: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| OutcomeError::StorageFailed)?;
    if stored != Some(author_id) {
        tx.rollback()
            .await
            .map_err(|_| OutcomeError::StorageFailed)?;
        return Err(OutcomeError::NotFound);
    }
    if let Some(latest) = latest_outcome(&mut tx, request_id).await? {
        if latest.outcome == outcome
            && latest.source.as_deref() == source
            && latest.attributed_offer_id == attributed
        {
            tx.rollback()
                .await
                .map_err(|_| OutcomeError::StorageFailed)?;
            return Ok(latest);
        }
    }
    if let Some(offer_id) = attributed {
        let placed: Option<(uuid::Uuid, uuid::Uuid)> =
            sqlx::query_as("SELECT request_id, seller_id FROM offers WHERE id = $1")
                .bind(offer_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| OutcomeError::StorageFailed)?;
        let contacted: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM contacts WHERE buyer_id = $1 AND offer_id = $2")
                .bind(author_id)
                .bind(offer_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| OutcomeError::StorageFailed)?;
        match (placed, contacted) {
            (Some((owning_request, _)), Some(_)) if owning_request == request_id => {}
            _ => {
                tx.rollback()
                    .await
                    .map_err(|_| OutcomeError::StorageFailed)?;
                return Err(OutcomeError::AttributionRefused);
            }
        }
    }
    tx.commit().await.map_err(|_| OutcomeError::StorageFailed)?;
    // Lifecycle moves through the existing closure transition: terminal
    // rows keep their refusal, and repeats already returned above.
    match answer {
        OutcomeAnswer::Completed(CompletionSource::Platform { .. }) => {
            close_request(
                pool,
                author_id,
                request_id,
                CloseOutcome::Found(CloseSource::Platform),
            )
            .await
        }
        OutcomeAnswer::Completed(CompletionSource::Elsewhere) => {
            close_request(
                pool,
                author_id,
                request_id,
                CloseOutcome::Found(CloseSource::Elsewhere),
            )
            .await
        }
        OutcomeAnswer::Completed(CompletionSource::Unknown) => {
            close_request(
                pool,
                author_id,
                request_id,
                CloseOutcome::Found(CloseSource::Undisclosed),
            )
            .await
        }
        OutcomeAnswer::Cancelled => {
            close_request(pool, author_id, request_id, CloseOutcome::NotNeeded).await
        }
        OutcomeAnswer::Unresolved => {
            close_request(pool, author_id, request_id, CloseOutcome::NotYet).await
        }
    }
    .map_err(|error| match error {
        crate::application::close_request::CloseError::NotActive => OutcomeError::NotActive,
        crate::application::close_request::CloseError::NotFound => OutcomeError::NotFound,
        crate::application::close_request::CloseError::ForbiddenState => {
            OutcomeError::ForbiddenState
        }
        crate::application::close_request::CloseError::StorageFailed => OutcomeError::StorageFailed,
    })?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| OutcomeError::StorageFailed)?;
    sqlx::query("SELECT 1 FROM requests WHERE id = $1 FOR UPDATE")
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| OutcomeError::StorageFailed)?;
    let prior = latest_outcome(&mut tx, request_id).await?;
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO request_outcomes
            (request_id, outcome, source, attributed_offer_id, supersedes)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING {OUTCOME_COLUMNS}"
    ))
    .bind(request_id)
    .bind(outcome)
    .bind(source)
    .bind(attributed)
    .bind(prior.map(|entry| entry.id))
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| OutcomeError::StorageFailed)?;
    let recorded = read_outcome(&row)?;
    tx.commit().await.map_err(|_| OutcomeError::StorageFailed)?;
    Ok(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_distinguish_source_and_outcome() {
        let platform = OutcomeAnswer::Completed(CompletionSource::Platform {
            offer_id: uuid::Uuid::nil(),
        });
        assert_ne!(
            platform,
            OutcomeAnswer::Completed(CompletionSource::Elsewhere)
        );
        assert_ne!(
            platform,
            OutcomeAnswer::Completed(CompletionSource::Unknown)
        );
        assert_ne!(platform, OutcomeAnswer::Cancelled);
        assert_ne!(OutcomeAnswer::Cancelled, OutcomeAnswer::Unresolved);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            OutcomeError::AttributionRefused,
            OutcomeError::NotActive,
            OutcomeError::NotFound,
            OutcomeError::ForbiddenState,
            OutcomeError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
