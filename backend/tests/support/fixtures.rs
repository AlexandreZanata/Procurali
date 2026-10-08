//! Deterministic synthetic builders for integration fixtures.
//!
//! Pure functions of an index: the same index always yields the same values,
//! distinct indexes yield distinct identities. Phone numbers are valid-shaped
//! E.164 in a reserved synthetic range that is never provisioned, never dialed,
//! and only ever asserted against the deterministic fake provider (later card).
//! No live delivery path exists here — this module performs no I/O at all,
//! except [`mint_identity`], which inserts one proof row through a given pool.
//!
//! City pairs are a minimal placeholder until the catalog seed lands in its
//! owning card; they assert locality plumbing, never catalog completeness.

/// A synthetic person: display name plus reserved-range phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticIdentity {
    /// Builder index (deterministic source).
    pub index: u32,
    /// Display name, within the 80-scalar limit.
    pub display_name: String,
    /// Reserved synthetic E.164 number. Never provisioned, never dialed.
    pub phone_e164: String,
}

/// Deterministic identity for `index` (wraps every 10000).
#[must_use]
pub fn identity(index: u32) -> SyntheticIdentity {
    let slot = index % 10_000;
    SyntheticIdentity {
        index,
        display_name: format!("Test User {slot:04}"),
        phone_e164: format!("+55119{slot:04}7890"),
    }
}

/// A synthetic locality placeholder (city + region label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticCity {
    /// City label.
    pub city: &'static str,
    /// Region label.
    pub region: &'static str,
}

/// Fixed placeholder pairs; the owning catalog card replaces this source.
const CITIES: [SyntheticCity; 3] = [
    SyntheticCity {
        city: "São Paulo",
        region: "SP",
    },
    SyntheticCity {
        city: "Campinas",
        region: "SP",
    },
    SyntheticCity {
        city: "Guarulhos",
        region: "SP",
    },
];

/// Deterministic locality rotating through the placeholder pairs.
#[must_use]
pub fn city(index: u32) -> SyntheticCity {
    CITIES[(index as usize) % CITIES.len()]
}

/// Insert one foundation proof row through `pool` and return its server UUID.
///
/// The id comes from PostgreSQL (`DEFAULT uuidv7()`), so distinct calls yield
/// distinct version-7 values even for identical builder input.
///
/// # Errors
///
/// Propagates [`sqlx::Error`] when the insert fails.
pub async fn mint_identity(pool: &sqlx::PgPool) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("INSERT INTO foundation_ids DEFAULT VALUES RETURNING id::text")
        .fetch_one(pool)
        .await
}
