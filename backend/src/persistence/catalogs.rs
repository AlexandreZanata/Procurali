//! Category and locality catalogs: stable identities for request drafting.
//!
//! Canonical rules: INV-08 (active requests need an allowed category for
//! their cycle and an enabled city — retired categories finish existing
//! cycles only, prohibited never), INV-11 (one item need per request —
//! categories stay single and distinct), INV-22 (offers respect the city
//! scope), EC-35 (renames keep stable codes; scope changes version instead of
//! rewriting meaning).
//!
//! Categories are the six seeded rows; excluded classes have no rows and no
//! client label can create one. Cities serve only when explicitly enabled;
//! regions hang under their city with composite keys, so same-named regions
//! in different cities stay distinct. Readers serve the public catalog and
//! the request writers of later cards; writers here exist for fixtures,
//! operations, and lifecycle transitions (retire/prohibit/relabel).

/// Lifecycle status of one category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryStatus {
    /// Usable for new requests.
    Allowed,
    /// Existing cycles may finish; new use is refused.
    Retired,
    /// Never usable.
    Prohibited,
}

impl CategoryStatus {
    /// Parse the stored status string.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allowed" => Some(Self::Allowed),
            "retired" => Some(Self::Retired),
            "prohibited" => Some(Self::Prohibited),
            _ => None,
        }
    }

    /// Stored status string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Retired => "retired",
            Self::Prohibited => "prohibited",
        }
    }
}

/// One catalog category: stable code, human label, scope, lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    /// Stable identity (never renamed by label changes).
    pub code: String,
    /// Human label.
    pub label: String,
    /// Short scope description.
    pub scope: String,
    /// Lifecycle status.
    pub status: CategoryStatus,
}

/// Whether one category may back a new request cycle: allowed only.
#[must_use]
pub fn usable_for_new_cycle(category: &Category) -> bool {
    category.status == CategoryStatus::Allowed
}

/// One catalog city: stable code, name, and explicit enabled flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct City {
    /// Stable identity.
    pub code: String,
    /// Display name.
    pub name: String,
    /// Whether the city serves traffic (explicitly enabled only).
    pub enabled: bool,
}

/// One catalog region: stable code within its city.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Owning city code.
    pub city_code: String,
    /// Stable identity within the city.
    pub code: String,
    /// Display name.
    pub name: String,
}

/// Typed catalog failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    /// A code, name, label, or scope is missing or out of bounds.
    InvalidField,
    /// No such category, city, or region exists.
    Unknown,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid catalog field"),
            Self::Unknown => f.write_str("unknown catalog entry"),
            Self::StorageFailed => f.write_str("catalog storage failed"),
        }
    }
}

impl std::error::Error for CatalogError {}

fn check_text(value: &str, max_chars: usize) -> Result<(), CatalogError> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(CatalogError::InvalidField);
    }
    Ok(())
}

fn check_code(value: &str) -> Result<(), CatalogError> {
    if value.trim().is_empty()
        || value.chars().count() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(CatalogError::InvalidField);
    }
    Ok(())
}

fn read_category(row: &sqlx::postgres::PgRow) -> Result<Category, CatalogError> {
    use sqlx::Row;
    let status: String = row
        .try_get("status")
        .map_err(|_| CatalogError::StorageFailed)?;
    Ok(Category {
        code: row
            .try_get("code")
            .map_err(|_| CatalogError::StorageFailed)?,
        label: row
            .try_get("label")
            .map_err(|_| CatalogError::StorageFailed)?,
        scope: row
            .try_get("scope")
            .map_err(|_| CatalogError::StorageFailed)?,
        status: CategoryStatus::parse(&status).ok_or(CatalogError::StorageFailed)?,
    })
}

/// All categories in code order, whatever their status.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`CatalogError::StorageFailed`] on database failure only.
pub async fn categories<'e, E>(executor: E) -> Result<Vec<Category>, CatalogError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> =
        sqlx::query("SELECT code, label, scope, status FROM catalog_categories ORDER BY code")
            .fetch_all(executor)
            .await
            .map_err(|_| CatalogError::StorageFailed)?;
    rows.iter().map(read_category).collect()
}

/// One category by code, if seeded.
///
/// # Errors
///
/// Returns [`CatalogError::StorageFailed`] on database failure only. Excluded
/// classes answer `None`: no row can exist for them.
pub async fn category<'e, E>(executor: E, code: &str) -> Result<Option<Category>, CatalogError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> =
        sqlx::query("SELECT code, label, scope, status FROM catalog_categories WHERE code = $1")
            .bind(code)
            .fetch_optional(executor)
            .await
            .map_err(|_| CatalogError::StorageFailed)?;
    row.map(|row| read_category(&row)).transpose()
}

/// Transition one category's lifecycle status, keeping its identity.
///
/// # Errors
///
/// Returns [`CatalogError::StorageFailed`] on database failure only, and
/// reports `false` when the code does not exist.
pub async fn set_category_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    code: &str,
    status: CategoryStatus,
) -> Result<bool, CatalogError> {
    let affected = sqlx::query("UPDATE catalog_categories SET status = $2 WHERE code = $1")
        .bind(code)
        .bind(status.as_str())
        .execute(&mut **tx)
        .await
        .map_err(|_| CatalogError::StorageFailed)?;
    Ok(affected.rows_affected() == 1)
}

/// Relabel one category (label and scope), keeping its stable code.
///
/// # Errors
///
/// Returns [`CatalogError::InvalidField`] for malformed text,
/// [`CatalogError::StorageFailed`] on database failure, and reports `false`
/// when the code does not exist.
pub async fn relabel_category(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    code: &str,
    label: &str,
    scope: &str,
) -> Result<bool, CatalogError> {
    check_text(label, 120)?;
    check_text(scope, 280)?;
    let affected =
        sqlx::query("UPDATE catalog_categories SET label = $2, scope = $3 WHERE code = $1")
            .bind(code)
            .bind(label)
            .bind(scope)
            .execute(&mut **tx)
            .await
            .map_err(|_| CatalogError::StorageFailed)?;
    Ok(affected.rows_affected() == 1)
}

/// Upsert one city with its explicit enabled flag (fixtures and operations).
///
/// # Errors
///
/// Returns [`CatalogError::InvalidField`] for malformed codes or names, else
/// [`CatalogError::StorageFailed`].
pub async fn upsert_city(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    code: &str,
    name: &str,
    enabled: bool,
) -> Result<(), CatalogError> {
    check_code(code)?;
    check_text(name, 120)?;
    sqlx::query(
        "INSERT INTO catalog_cities (code, name, enabled) VALUES ($1, $2, $3)
         ON CONFLICT (code) DO UPDATE SET name = EXCLUDED.name, enabled = EXCLUDED.enabled",
    )
    .bind(code)
    .bind(name)
    .bind(enabled)
    .execute(&mut **tx)
    .await
    .map_err(|_| CatalogError::StorageFailed)?;
    Ok(())
}

/// Cities in code order; optionally only explicitly enabled ones.
///
/// # Errors
///
/// Returns [`CatalogError::StorageFailed`] on database failure only.
pub async fn cities<'e, E>(executor: E, only_enabled: bool) -> Result<Vec<City>, CatalogError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = if only_enabled {
        sqlx::query("SELECT code, name, enabled FROM catalog_cities WHERE enabled ORDER BY code")
            .fetch_all(executor)
            .await
            .map_err(|_| CatalogError::StorageFailed)?
    } else {
        sqlx::query("SELECT code, name, enabled FROM catalog_cities ORDER BY code")
            .fetch_all(executor)
            .await
            .map_err(|_| CatalogError::StorageFailed)?
    };
    use sqlx::Row;
    rows.iter()
        .map(|row| {
            Ok(City {
                code: row
                    .try_get("code")
                    .map_err(|_| CatalogError::StorageFailed)?,
                name: row
                    .try_get("name")
                    .map_err(|_| CatalogError::StorageFailed)?,
                enabled: row
                    .try_get("enabled")
                    .map_err(|_| CatalogError::StorageFailed)?,
            })
        })
        .collect()
}

/// Upsert one region under its city (composite identity).
///
/// # Errors
///
/// Returns [`CatalogError::InvalidField`] for malformed codes or names, else
/// [`CatalogError::StorageFailed`] (including an unknown city).
pub async fn upsert_region(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    city_code: &str,
    code: &str,
    name: &str,
) -> Result<(), CatalogError> {
    check_code(city_code)?;
    check_code(code)?;
    check_text(name, 120)?;
    sqlx::query(
        "INSERT INTO catalog_regions (city_code, code, name) VALUES ($1, $2, $3)
         ON CONFLICT (city_code, code) DO UPDATE SET name = EXCLUDED.name",
    )
    .bind(city_code)
    .bind(code)
    .bind(name)
    .execute(&mut **tx)
    .await
    .map_err(|_| CatalogError::StorageFailed)?;
    Ok(())
}

/// One region by its composite identity, if it exists.
///
/// # Errors
///
/// Returns [`CatalogError::StorageFailed`] on database failure only.
pub async fn region<'e, E>(
    executor: E,
    city_code: &str,
    code: &str,
) -> Result<Option<Region>, CatalogError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT city_code, code, name FROM catalog_regions WHERE city_code = $1 AND code = $2",
    )
    .bind(city_code)
    .bind(code)
    .fetch_optional(executor)
    .await
    .map_err(|_| CatalogError::StorageFailed)?;
    row.map(|row| {
        use sqlx::Row;
        Ok(Region {
            city_code: row
                .try_get("city_code")
                .map_err(|_| CatalogError::StorageFailed)?,
            code: row
                .try_get("code")
                .map_err(|_| CatalogError::StorageFailed)?,
            name: row
                .try_get("name")
                .map_err(|_| CatalogError::StorageFailed)?,
        })
    })
    .transpose()
}

/// All regions of one city in code order.
///
/// # Errors
///
/// Returns [`CatalogError::StorageFailed`] on database failure only.
pub async fn regions_for_city<'e, E>(
    executor: E,
    city_code: &str,
) -> Result<Vec<Region>, CatalogError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT city_code, code, name FROM catalog_regions WHERE city_code = $1 ORDER BY code",
    )
    .bind(city_code)
    .fetch_all(executor)
    .await
    .map_err(|_| CatalogError::StorageFailed)?;
    use sqlx::Row;
    rows.iter()
        .map(|row| {
            Ok(Region {
                city_code: row
                    .try_get("city_code")
                    .map_err(|_| CatalogError::StorageFailed)?,
                code: row
                    .try_get("code")
                    .map_err(|_| CatalogError::StorageFailed)?,
                name: row
                    .try_get("name")
                    .map_err(|_| CatalogError::StorageFailed)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_parse_and_gate_new_use() {
        assert_eq!(
            CategoryStatus::parse("allowed"),
            Some(CategoryStatus::Allowed)
        );
        assert_eq!(
            CategoryStatus::parse("retired"),
            Some(CategoryStatus::Retired)
        );
        assert_eq!(
            CategoryStatus::parse("prohibited"),
            Some(CategoryStatus::Prohibited)
        );
        assert_eq!(CategoryStatus::parse("archived"), None);
        let allowed = Category {
            code: "tools".to_owned(),
            label: "Tools".to_owned(),
            scope: "Scope".to_owned(),
            status: CategoryStatus::Allowed,
        };
        assert!(usable_for_new_cycle(&allowed));
        assert!(!usable_for_new_cycle(&Category {
            status: CategoryStatus::Retired,
            ..allowed.clone()
        }));
        assert!(!usable_for_new_cycle(&Category {
            status: CategoryStatus::Prohibited,
            ..allowed
        }));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            CatalogError::InvalidField,
            CatalogError::Unknown,
            CatalogError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
