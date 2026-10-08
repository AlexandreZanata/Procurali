//! Exact duplicate intent: one open need per owner, matched mechanically.
//!
//! Canonical rules: INV-11 (one coherent physical-item need per request —
//! identity is the normalized title plus the single category, budget,
//! condition, and locality, never a list), INV-15 (nothing resets abuse
//! accounting — this check reads live open rows, so removed or terminal
//! history cannot bless a refresh), AC-07 (an identical open need refuses a
//! further identical publication, suggesting the existing edit/renew path),
//! EC-32 (removal frees a slot but never authorizes feed refreshing —
//! churn is bounded by the activation quota, which removal never refunds).
//!
//! Matching is mechanical only and reuses the domain duplicate vocabulary
//! (`normalize_for_comparison`, `same_need`): trim, collapse whitespace
//! runs, Unicode lowercase. No translation, no synonyms, no stemming, no
//! semantic matching — meaningfully different requirements and the same
//! region label under another city stay distinct. Canonical equivalence
//! beyond case and whitespace is not performed (see the domain module), so
//! such pairs count as distinct needs.
//!
//! Scope is the owner's open demand (`active` or `suspended` with an
//! unexpired current cycle, visible): drafts are not needs yet, and removed
//! or terminal rows no longer occupy demand. Like the quota guard, every
//! read runs in the caller's transaction and storage failures surface as
//! [`AttemptError::Db`], so the DEC-0003 runner retries genuine conflicts
//! instead of misreporting them.

use crate::domain::text::{normalize_for_comparison, same_need};
use crate::persistence::transaction::AttemptError;

/// A candidate need: the identity fields of one requirement set. Notes are
/// deliberately absent: free-form preferences annotate a need without
/// renaming it, so differing notes never make identical demand distinct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIntent {
    /// Item title in any common spacing or casing.
    pub title: String,
    /// Stable category code.
    pub category_code: String,
    /// Maximum budget in minor units.
    pub budget_cents: i64,
    /// Accepted condition (`new` | `used` | `either`).
    pub condition: String,
    /// Stable city code.
    pub city_code: String,
    /// Region code within the city.
    pub region_code: String,
}

/// Typed duplicate failure. Static reasons only; the matching row travels
/// through [`matching_open_request`] so refusals name no values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateError {
    /// An identical open need already exists: edit or renew it instead.
    DuplicateIntent,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for DuplicateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateIntent => f.write_str("duplicate intent exists"),
            Self::StorageFailed => f.write_str("duplicate lookup failed"),
        }
    }
}

impl std::error::Error for DuplicateError {}

/// One open row's identity fields for comparison.
struct OpenNeed {
    id: uuid::Uuid,
    title: String,
    category_code: String,
    budget_cents: i64,
    condition: String,
    city_code: String,
    region_code: String,
}

/// True when `candidate` names the same need: normalized titles match
/// (non-empty) with identical category, budget, condition, and locality.
fn is_same_need(candidate: &NewIntent, open: &OpenNeed) -> bool {
    same_need(&candidate.title, &open.title)
        && candidate.category_code == open.category_code
        && candidate.budget_cents == open.budget_cents
        && candidate.condition == open.condition
        && candidate.city_code == open.city_code
        && candidate.region_code == open.region_code
}

/// The normalized title form used for identity (exposed for tests).
#[must_use]
pub fn normalized_title(title: &str) -> String {
    normalize_for_comparison(title)
}

/// The owner's open need identical to `candidate`, if one exists. Open
/// means `active` or `suspended` with an unexpired current cycle and
/// visible content; drafts, terminal, expired, and hidden rows never match.
/// The returned identifier lets callers suggest the existing edit/renew
/// path to the owner — never to anyone else, since matching never crosses
/// accounts.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure (retryable only for
/// genuine conflicts).
pub async fn matching_open_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    candidate: &NewIntent,
) -> Result<Option<uuid::Uuid>, AttemptError<DuplicateError>> {
    if normalize_for_comparison(&candidate.title).is_empty() {
        return Ok(None);
    }
    let rows: Vec<(uuid::Uuid, String, String, i64, String, String, String)> = sqlx::query_as(
        "SELECT request.id, request.title, request.category_code,
                request.budget_cents, request.\"condition\",
                request.city_code, request.region_code
         FROM requests AS request
         JOIN request_cycles AS cycle
           ON cycle.request_id = request.id
          AND cycle.cycle_number = request.current_cycle_number
         WHERE request.author_id = $1
           AND request.state IN ('active', 'suspended')
           AND request.visibility <> 'hidden'
           AND cycle.deadline > now()",
    )
    .bind(author_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    // Openness filters in the database; identity compares here, because the
    // mechanical normalization must not be reimplemented in SQL with
    // subtly different semantics. The caller's SERIALIZABLE transaction
    // still serializes concurrent publishers.
    for (id, title, category_code, budget_cents, condition, city_code, region_code) in rows {
        let open = OpenNeed {
            id,
            title,
            category_code,
            budget_cents,
            condition,
            city_code,
            region_code,
        };
        if is_same_need(candidate, &open) {
            return Ok(Some(open.id));
        }
    }
    Ok(None)
}

/// Refuse a publication that would duplicate the owner's identical open
/// need, evaluated live in the caller's transaction.
///
/// # Errors
///
/// Returns [`AttemptError::Abort`] with [`DuplicateError::DuplicateIntent`]
/// when an identical open need exists, and [`AttemptError::Db`] on database
/// failure (retried only for genuine `40001`/`40P01` conflicts).
pub async fn check_duplicate_intent(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    candidate: &NewIntent,
) -> Result<(), AttemptError<DuplicateError>> {
    match matching_open_request(tx, author_id, candidate).await? {
        Some(_) => Err(AttemptError::Abort(DuplicateError::DuplicateIntent)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(title: &str) -> NewIntent {
        NewIntent {
            title: title.to_owned(),
            category_code: "home_appliances".to_owned(),
            budget_cents: 52_000,
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
        }
    }

    fn open(title: &str) -> OpenNeed {
        OpenNeed {
            id: uuid::Uuid::now_v7(),
            title: title.to_owned(),
            category_code: "home_appliances".to_owned(),
            budget_cents: 52_000,
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
        }
    }

    #[test]
    fn normalization_is_mechanical_only() {
        assert_eq!(normalized_title("  Refrigerator  "), "refrigerator");
        assert_eq!(
            normalized_title("DOUBLE-door\tFRIDGE"),
            "double-door fridge"
        );
        assert!(is_same_need(
            &intent("  REFRIGERATOR "),
            &open("refrigerator")
        ));
        // No synonyms, no stemming, no translation: these stay distinct.
        assert!(!is_same_need(&intent("Fridge"), &open("Refrigerator")));
        assert!(!is_same_need(
            &intent("Refrigerators"),
            &open("Refrigerator")
        ));
        assert!(!is_same_need(&intent("Geladeira"), &open("Refrigerator")));
    }

    #[test]
    fn identity_needs_every_field() {
        let base = intent("Refrigerator");
        assert!(is_same_need(&base, &open("Refrigerator")));
        for altered in [
            NewIntent {
                category_code: "furniture".to_owned(),
                ..base.clone()
            },
            NewIntent {
                budget_cents: 52_001,
                ..base.clone()
            },
            NewIntent {
                condition: "new".to_owned(),
                ..base.clone()
            },
            NewIntent {
                city_code: "valinhos".to_owned(),
                ..base.clone()
            },
            NewIntent {
                region_code: "norte".to_owned(),
                ..base.clone()
            },
        ] {
            assert!(
                !is_same_need(&altered, &open("Refrigerator")),
                "one differing field breaks identity"
            );
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            DuplicateError::DuplicateIntent,
            DuplicateError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
