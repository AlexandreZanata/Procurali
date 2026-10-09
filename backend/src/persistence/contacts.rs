//! Private contact initiations: frozen context, idempotent identity.
//!
//! Canonical rules: INV-24 (past contact snapshots stay intact —
//! initiation freezes requirement and terms snapshots plus the effective
//! time, immune to later edits), INV-29 (no unrestricted user-phone lookup
//! — only a frozen ciphertext copy is kept, never plaintext, never live),
//! INV-30 with INV-31 (submission and public surfaces never reveal phones
//! — the public projection below carries no destination, ciphertext, or
//! contact material of any kind), INV-33 (a handoff claims no delivery,
//! conversation, or sale — the row records initiation context only),
//! INV-34 with EC-25 (repeats duplicate nothing — the same handoff
//! identity returns its existing row, and genuinely later handoffs on an
//! established combination record repeat events instead of second unique
//! rows).
//!
//! The buyer is always the request author: initiation by anyone else is
//! refused. Offer, cycle, and revision consistency are checked against live
//! rows; lifecycle and deadline eligibility belong to the handoff writers,
//! not this storage.

use serde::Serialize;

/// One contact initiation as supplied: identities plus entry source. Every
/// snapshot (requirements, terms, destination, time) resolves server-side
/// at initiation.
#[derive(Debug, Clone)]
pub struct NewContact {
    /// Caller-supplied handoff identity: retries reuse it, later handoffs
    /// mint a fresh one.
    pub handoff_id: uuid::Uuid,
    /// Initiating buyer; must own the request.
    pub buyer_id: uuid::Uuid,
    /// Contacted seller; must differ from the buyer.
    pub seller_id: uuid::Uuid,
    /// Target request; must already exist.
    pub request_id: uuid::Uuid,
    /// Target cycle number; must already exist on the request.
    pub cycle_number: i32,
    /// Target offer; must belong to the request and cycle.
    pub offer_id: uuid::Uuid,
    /// Where the buyer chose contact (bounded free text).
    pub entry_source: String,
}

/// One persisted contact: frozen initiation context with its destination
/// history. The ciphertext field is administrative and purpose-limited: it
/// never serializes anywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Handoff identity for idempotent retries.
    pub handoff_id: uuid::Uuid,
    /// Initiating buyer (the request author).
    pub buyer_id: uuid::Uuid,
    /// Contacted seller.
    pub seller_id: uuid::Uuid,
    /// Target request.
    pub request_id: uuid::Uuid,
    /// Target cycle number.
    pub cycle_number: i32,
    /// Target offer.
    pub offer_id: uuid::Uuid,
    /// Requirement revision in force at initiation.
    pub request_revision_number: i32,
    /// Offer terms number in force at initiation.
    pub offer_terms_number: i32,
    /// Requirement title snapshot.
    pub request_title: String,
    /// Requirement budget snapshot in minor units.
    pub request_budget_cents: i64,
    /// Terms description snapshot.
    pub offer_description: String,
    /// Terms price snapshot in minor units.
    pub offer_price_cents: i64,
    /// Frozen seller destination ciphertext at initiation.
    pub destination_ciphertext: Vec<u8>,
    /// Entry source.
    pub entry_source: String,
    /// Effective initiation time (database clock).
    pub initiated_at: chrono::DateTime<chrono::Utc>,
}

/// One initiation result: the unique row plus whether this call repeated an
/// established combination.
#[derive(Debug, Clone, PartialEq)]
pub struct InitiatedContact {
    /// The unique contact row (newly created or pre-existing).
    pub contact: Contact,
    /// True when the combination already stood (retry or later handoff).
    pub repeat: bool,
}

/// Public contact projection: allowlist only. No destination, ciphertext,
/// phone, party identifier beyond the row itself, or contact material —
/// serialization of this shape can never leak the destination history.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicContact {
    /// Contact identifier.
    pub id: uuid::Uuid,
    /// Target request identifier.
    pub request_id: uuid::Uuid,
    /// Target cycle number.
    pub cycle_number: i32,
    /// Target offer identifier.
    pub offer_id: uuid::Uuid,
    /// Requirement title snapshot.
    pub request_title: String,
    /// Requirement budget rendering.
    pub request_budget: String,
    /// Terms description snapshot.
    pub offer_description: String,
    /// Terms price rendering.
    pub offer_price: String,
    /// Entry source.
    pub entry_source: String,
    /// Effective initiation time.
    pub initiated_at: chrono::DateTime<chrono::Utc>,
    /// Whether this call repeated an established combination.
    pub repeat: bool,
}

/// Render integer minor units as an exact decimal string (`"520.00"`).
#[must_use]
pub fn format_contact_money(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// Project the private row into its public allowlist (drops the parties
/// and the destination history).
#[must_use]
pub fn to_public(contact: &Contact, repeat: bool) -> PublicContact {
    PublicContact {
        id: contact.id,
        request_id: contact.request_id,
        cycle_number: contact.cycle_number,
        offer_id: contact.offer_id,
        request_title: contact.request_title.clone(),
        request_budget: format_contact_money(contact.request_budget_cents),
        offer_description: contact.offer_description.clone(),
        offer_price: format_contact_money(contact.offer_price_cents),
        entry_source: contact.entry_source.clone(),
        initiated_at: contact.initiated_at,
        repeat,
    }
}

/// Typed contact-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactError {
    /// A reference or entry source is missing or out of bounds.
    InvalidField,
    /// The buyer does not own the request, or the offer belongs elsewhere.
    WrongParties,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ContactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid contact field"),
            Self::WrongParties => f.write_str("contact parties do not match"),
            Self::StorageFailed => f.write_str("contact storage failed"),
        }
    }
}

impl std::error::Error for ContactError {}

fn read_contact(row: &sqlx::postgres::PgRow) -> Result<Contact, ContactError> {
    use sqlx::Row;
    Ok(Contact {
        id: row.try_get("id").map_err(|_| ContactError::StorageFailed)?,
        handoff_id: row
            .try_get("handoff_id")
            .map_err(|_| ContactError::StorageFailed)?,
        buyer_id: row
            .try_get("buyer_id")
            .map_err(|_| ContactError::StorageFailed)?,
        seller_id: row
            .try_get("seller_id")
            .map_err(|_| ContactError::StorageFailed)?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| ContactError::StorageFailed)?,
        cycle_number: row
            .try_get("cycle_number")
            .map_err(|_| ContactError::StorageFailed)?,
        offer_id: row
            .try_get("offer_id")
            .map_err(|_| ContactError::StorageFailed)?,
        request_revision_number: row
            .try_get("request_revision_number")
            .map_err(|_| ContactError::StorageFailed)?,
        offer_terms_number: row
            .try_get("offer_terms_number")
            .map_err(|_| ContactError::StorageFailed)?,
        request_title: row
            .try_get("request_title")
            .map_err(|_| ContactError::StorageFailed)?,
        request_budget_cents: row
            .try_get("request_budget_cents")
            .map_err(|_| ContactError::StorageFailed)?,
        offer_description: row
            .try_get("offer_description")
            .map_err(|_| ContactError::StorageFailed)?,
        offer_price_cents: row
            .try_get("offer_price_cents")
            .map_err(|_| ContactError::StorageFailed)?,
        destination_ciphertext: row
            .try_get("destination_ciphertext")
            .map_err(|_| ContactError::StorageFailed)?,
        entry_source: row
            .try_get("entry_source")
            .map_err(|_| ContactError::StorageFailed)?,
        initiated_at: row
            .try_get("initiated_at")
            .map_err(|_| ContactError::StorageFailed)?,
    })
}

const CONTACT_COLUMNS: &str = "id, handoff_id, buyer_id, seller_id, request_id, cycle_number, offer_id, request_revision_number, offer_terms_number, request_title, request_budget_cents, offer_description, offer_price_cents, destination_ciphertext, entry_source, initiated_at";

/// Record one contact initiation, or return its existing row.
///
/// The buyer must own the request and the offer must belong to the request
/// and cycle; requirement and terms snapshots plus the seller's current
/// destination ciphertext freeze at initiation. A retried handoff identity
/// returns its existing row; a fresh handoff on an established combination
/// records a repeat event and returns the standing row with `repeat` set.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`ContactError::InvalidField`] for malformed input,
/// [`ContactError::WrongParties`] for mismatched ownership or offer
/// placement, else [`ContactError::StorageFailed`]. Reasons are static.
pub async fn initiate_contact(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: NewContact,
) -> Result<InitiatedContact, ContactError> {
    if input.entry_source.trim().is_empty() || input.entry_source.chars().count() > 64 {
        return Err(ContactError::InvalidField);
    }
    if input.cycle_number < 1 {
        return Err(ContactError::InvalidField);
    }
    // A retried handoff identity returns its existing row, never a second.
    let retried: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CONTACT_COLUMNS} FROM contacts WHERE handoff_id = $1"
    ))
    .bind(input.handoff_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    if let Some(row) = retried {
        return Ok(InitiatedContact {
            contact: read_contact(&row)?,
            repeat: true,
        });
    }
    let request: Option<(uuid::Uuid, String, i64, i32)> = sqlx::query_as(
        "SELECT author_id, title, budget_cents, current_revision_number
         FROM requests WHERE id = $1",
    )
    .bind(input.request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    let (author_id, title, budget_cents, revision_number) =
        request.ok_or(ContactError::InvalidField)?;
    if author_id != input.buyer_id {
        return Err(ContactError::WrongParties);
    }
    let offer: Option<(uuid::Uuid, uuid::Uuid, i32, String, i64, i32)> = sqlx::query_as(
        "SELECT request_id, seller_id, cycle_number, description, price_cents,
                current_terms_number
         FROM offers WHERE id = $1",
    )
    .bind(input.offer_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    let (offer_request, offer_seller, offer_cycle, description, price_cents, terms_number) =
        offer.ok_or(ContactError::InvalidField)?;
    // Placement follows the offer row itself: same request, seller, cycle.
    if offer_request != input.request_id
        || offer_seller != input.seller_id
        || offer_cycle != input.cycle_number
    {
        return Err(ContactError::WrongParties);
    }
    let destination: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT phone_ciphertext FROM users WHERE id = $1")
            .bind(input.seller_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| ContactError::StorageFailed)?;
    let destination = destination.ok_or(ContactError::InvalidField)?;
    // An established combination repeats: one repeat event, standing row.
    let standing: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CONTACT_COLUMNS} FROM contacts
         WHERE buyer_id = $1 AND seller_id = $2 AND request_id = $3
           AND cycle_number = $4 AND offer_id = $5"
    ))
    .bind(input.buyer_id)
    .bind(input.seller_id)
    .bind(input.request_id)
    .bind(input.cycle_number)
    .bind(input.offer_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    if let Some(row) = standing {
        // The twin may have committed our own handoff identity between the
        // checks above: then this is its retry, not a later handoff, and
        // records nothing.
        let retried: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
            "SELECT {CONTACT_COLUMNS} FROM contacts WHERE handoff_id = $1"
        ))
        .bind(input.handoff_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| ContactError::StorageFailed)?;
        if let Some(retried) = retried {
            return Ok(InitiatedContact {
                contact: read_contact(&retried)?,
                repeat: true,
            });
        }
        record_repeat(&mut *tx, &input, &read_contact(&row)?.id).await?;
        return Ok(InitiatedContact {
            contact: read_contact(&row)?,
            repeat: true,
        });
    }
    // A concurrent twin may win the same handoff or combination between
    // the checks above and this insert. `ON CONFLICT DO NOTHING` keeps the
    // transaction healthy where catching a unique violation would abort it
    // (PostgreSQL poisons the transaction on any statement error), and the
    // standing row below resolves whichever twin lost.
    let inserted: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "INSERT INTO contacts
            (handoff_id, buyer_id, seller_id, request_id, cycle_number,
             offer_id, request_revision_number, offer_terms_number,
             request_title, request_budget_cents, offer_description,
             offer_price_cents, destination_ciphertext, entry_source)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT DO NOTHING
         RETURNING {CONTACT_COLUMNS}"
    ))
    .bind(input.handoff_id)
    .bind(input.buyer_id)
    .bind(input.seller_id)
    .bind(input.request_id)
    .bind(input.cycle_number)
    .bind(input.offer_id)
    .bind(revision_number)
    .bind(terms_number)
    .bind(&title)
    .bind(budget_cents)
    .bind(&description)
    .bind(price_cents)
    .bind(&destination)
    .bind(&input.entry_source)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    let row = match inserted {
        Some(row) => row,
        None => {
            // A retried handoff identity returns its existing row with no
            // new fact (INV-34: retries duplicate nothing); only a
            // genuinely later handoff on an established combination records
            // a repeat event (EC-25).
            let retried: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
                "SELECT {CONTACT_COLUMNS} FROM contacts WHERE handoff_id = $1"
            ))
            .bind(input.handoff_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| ContactError::StorageFailed)?;
            if let Some(retried) = retried {
                return Ok(InitiatedContact {
                    contact: read_contact(&retried)?,
                    repeat: true,
                });
            }
            let standing: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
                "SELECT {CONTACT_COLUMNS} FROM contacts
                 WHERE buyer_id = $1 AND seller_id = $2 AND request_id = $3
                   AND cycle_number = $4 AND offer_id = $5"
            ))
            .bind(input.buyer_id)
            .bind(input.seller_id)
            .bind(input.request_id)
            .bind(input.cycle_number)
            .bind(input.offer_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| ContactError::StorageFailed)?;
            let standing = standing.ok_or(ContactError::StorageFailed)?;
            record_repeat(&mut *tx, &input, &read_contact(&standing)?.id).await?;
            return Ok(InitiatedContact {
                contact: read_contact(&standing)?,
                repeat: true,
            });
        }
    };
    Ok(InitiatedContact {
        contact: read_contact(&row)?,
        repeat: false,
    })
}

async fn record_repeat(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &NewContact,
    contact_id: &uuid::Uuid,
) -> Result<(), ContactError> {
    sqlx::query(
        "INSERT INTO business_events
            (actor_id, resource_kind, resource_id, effective_at, kind,
             policy, source, payload)
         VALUES ($1, 'contact', $2, now(), 'contact.repeated', 'mvp-free',
                 'api', '{\"repeat\": true}')",
    )
    .bind(input.buyer_id)
    .bind(contact_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    Ok(())
}

/// One contact by id, if it exists.
///
/// # Errors
///
/// Returns [`ContactError::StorageFailed`] on database failure only.
pub async fn contact<'e, E>(executor: E, id: uuid::Uuid) -> Result<Option<Contact>, ContactError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CONTACT_COLUMNS} FROM contacts WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    row.map(|row| read_contact(&row)).transpose()
}

/// One request's contacts in initiation order.
///
/// # Errors
///
/// Returns [`ContactError::StorageFailed`] on database failure only.
pub async fn contacts_for_request<'e, E>(
    executor: E,
    request_id: uuid::Uuid,
) -> Result<Vec<Contact>, ContactError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CONTACT_COLUMNS} FROM contacts WHERE request_id = $1 ORDER BY initiated_at, id"
    ))
    .bind(request_id)
    .fetch_all(executor)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    rows.iter().map(read_contact).collect()
}

/// One offer's contacts in initiation order.
///
/// # Errors
///
/// Returns [`ContactError::StorageFailed`] on database failure only.
pub async fn contacts_for_offer<'e, E>(
    executor: E,
    offer_id: uuid::Uuid,
) -> Result<Vec<Contact>, ContactError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CONTACT_COLUMNS} FROM contacts WHERE offer_id = $1 ORDER BY initiated_at, id"
    ))
    .bind(offer_id)
    .fetch_all(executor)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    rows.iter().map(read_contact).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_renders_exactly_without_floats() {
        assert_eq!(format_contact_money(52_000), "520.00");
        assert_eq!(format_contact_money(1), "0.01");
    }

    #[test]
    fn public_projection_drops_parties_and_destination() {
        let contact = Contact {
            id: uuid::Uuid::now_v7(),
            handoff_id: uuid::Uuid::now_v7(),
            buyer_id: uuid::Uuid::now_v7(),
            seller_id: uuid::Uuid::now_v7(),
            request_id: uuid::Uuid::now_v7(),
            cycle_number: 1,
            offer_id: uuid::Uuid::now_v7(),
            request_revision_number: 1,
            offer_terms_number: 1,
            request_title: "Refrigerator".to_owned(),
            request_budget_cents: 60_000,
            offer_description: "Frost-free 300L".to_owned(),
            offer_price_cents: 52_000,
            destination_ciphertext: b"ciphertext".to_vec(),
            entry_source: "offer_detail".to_owned(),
            initiated_at: chrono::Utc::now(),
        };
        let rendered =
            serde_json::to_string(&to_public(&contact, false)).expect("public serializes");
        for absent in [
            "buyer",
            "seller",
            "destination",
            "cipher",
            "phone",
            "lookup",
            "token",
            "session",
            "address",
            "handoff",
        ] {
            assert!(!rendered.contains(absent), "no {absent} in public output");
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ContactError::InvalidField,
            ContactError::WrongParties,
            ContactError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
