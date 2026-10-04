//! Row decode structs for raw SQL query results (sqlx edition).
//!
//! sqlx decodes through [`Row::try_get`] instead of diesel's
//! `QueryableByName` derives: each struct below has a `from_row` decoder
//! reading by BOTH column name and position (the probe-verified form), and
//! the `to_record` / `to_key` translations perform the identical sentinel
//! `''` -> `None` mapping as the diesel reference adapter (`rows.rs`): the
//! sentinel never crosses the SPI boundary.
//!
//! Projections are explicit everywhere (`role_row.name AS name, ...`) so
//! the `&str` column indexes below always resolve; the store never emits
//! `SELECT *` (joined name clashes, the diesel Pitfall 9).

use rolify_core::role::{ResourceId, RoleName, RoleRecord, SCOPE_SENTINEL};
use rolify_core::store::ResourceKey;
use sqlx::Row;
use sqlx::database::Database;

/// A role-table projection row: the name plus the sentinel-encoded scope
/// columns.
#[derive(Debug, Clone)]
pub(crate) struct RoleRow {
    /// Role name (byte-exact).
    pub name: String,
    /// Physical `resource_type` storage value (sentinel `''` when global).
    pub resource_type: String,
    /// Physical `resource_id` storage value (sentinel `''` for global/class).
    pub resource_id: String,
}

impl RoleRow {
    /// Decode one row of a `name, resource_type, resource_id` projection.
    ///
    /// # Errors
    ///
    /// Propagates `sqlx::Error` when a column is missing or its stored
    /// type is incompatible with `String`.
    pub(crate) fn from_row<DB>(row: &<DB as Database>::Row) -> Result<Self, sqlx::Error>
    where
        DB: Database,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
    {
        Ok(Self {
            name: row.try_get("name")?,
            resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?,
        })
    }

    /// Translate the physical row to a semantic [`RoleRecord`].
    ///
    /// The sentinel `''` becomes `None` for both scope columns, so the
    /// SPI never sees the physical format (D-02).
    #[must_use]
    pub(crate) fn to_record(&self) -> RoleRecord {
        RoleRecord::new(
            RoleName::new(self.name.clone()),
            from_storage(&self.resource_type),
            resource_id_from_storage(&self.resource_id),
        )
    }
}

/// Single-column row for `SELECT id FROM roles ...` (link building and the
/// orphan sweep resolve the generated primary key).
#[derive(Debug, Clone)]
pub(crate) struct IdRow {
    /// The role row's generated primary key.
    pub id: i64,
}

impl IdRow {
    /// Decode one `id` projection row.
    ///
    /// # Errors
    ///
    /// Propagates `sqlx::Error` when the column is missing or its stored
    /// type is incompatible with `i64`.
    pub(crate) fn from_row<DB>(row: &<DB as Database>::Row) -> Result<Self, sqlx::Error>
    where
        DB: Database,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
    {
        Ok(Self {
            id: row.try_get(0)?,
        })
    }
}

/// Row for `holders_where` / `all_holders`: a holder id from the
/// consumer's holder table.
///
/// Holder primary keys are stringified at the SPI boundary, but the
/// fixture holder tables behind the tests use integer autoincrement keys
/// while the gem's `teams` case uses a string key. The decoder therefore
/// tries `String` first and falls back to `i64` (stringified), covering
/// both shapes on every engine without engine-specific SQL casts.
#[derive(Debug, Clone)]
pub(crate) struct HolderIdRow {
    /// The holder's stringified primary key.
    pub user_id: String,
}

impl HolderIdRow {
    /// Decode one `user_id` projection row (string first, integer
    /// fallback).
    ///
    /// # Errors
    ///
    /// Propagates `sqlx::Error` when the column is missing or decodable
    /// as neither `String` nor `i64`.
    pub(crate) fn from_row<DB>(row: &<DB as Database>::Row) -> Result<Self, sqlx::Error>
    where
        DB: Database,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
    {
        let user_id = match row.try_get::<String, _>("user_id") {
            Ok(value) => value,
            Err(_) => row.try_get::<i64, _>("user_id")?.to_string(),
        };
        Ok(Self { user_id })
    }
}

/// Row for the `roles_matching` catalog read: the same projection shape as
/// [`RoleRow`], kept as its own struct so the catalog path mirrors the
/// diesel reference adapter's struct set.
#[derive(Debug, Clone)]
pub(crate) struct ResourceKeyRow {
    /// Role name (byte-exact).
    pub name: String,
    /// Physical `resource_type` storage value (never the sentinel on this
    /// path: the catalog query filters globals out).
    pub resource_type: String,
    /// Physical `resource_id` storage value.
    pub resource_id: String,
}

impl ResourceKeyRow {
    /// Decode one row of a `name, resource_type, resource_id` projection.
    ///
    /// # Errors
    ///
    /// Propagates `sqlx::Error` when a column is missing or its stored
    /// type is incompatible with `String`.
    pub(crate) fn from_row<DB>(row: &<DB as Database>::Row) -> Result<Self, sqlx::Error>
    where
        DB: Database,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
    {
        Ok(Self {
            name: row.try_get("name")?,
            resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?,
        })
    }

    /// Translate to a [`ResourceKey`] (the `resources_find` and catalog
    /// paths never return sentinel-scoped rows for the id column, so a
    /// sentinel `resource_id` here is a query bug and fails loudly).
    #[must_use]
    pub(crate) fn to_key(&self) -> ResourceKey {
        ResourceKey::new(
            self.resource_type.clone(),
            resource_id_from_storage(&self.resource_id).unwrap_or_else(|| {
                panic!("ResourceKeyRow.resource_id is never sentinel; catalog queries filter globals out")
            }),
        )
    }

    /// Translate to a semantic [`RoleRecord`] (same sentinel mapping as
    /// [`RoleRow::to_record`]).
    #[must_use]
    pub(crate) fn to_record(&self) -> RoleRecord {
        RoleRecord::new(
            RoleName::new(self.name.clone()),
            from_storage(&self.resource_type),
            resource_id_from_storage(&self.resource_id),
        )
    }
}

/// Physical `resource_type` storage value back to the semantic `Option`.
#[inline]
#[must_use]
fn from_storage(value: &str) -> Option<String> {
    if value == SCOPE_SENTINEL {
        None
    } else {
        Some(value.to_owned())
    }
}

/// Physical `resource_id` storage value back to `Option<ResourceId>`.
#[inline]
#[must_use]
fn resource_id_from_storage(value: &str) -> Option<ResourceId> {
    if value == SCOPE_SENTINEL {
        None
    } else {
        Some(ResourceId::new(value))
    }
}

/// Single-column row for `SELECT COUNT(*) AS count FROM roles` (role row count).
#[derive(Debug, Clone)]
pub struct CountRow {
    /// The count value.
    pub count: i64,
}

impl CountRow {
    /// Decode one `count` projection row.
    ///
    /// # Errors
    ///
    /// Propagates `sqlx::Error` when the column is missing or its stored
    /// type is incompatible with `i64`.
    pub fn from_row<DB>(row: &<DB as Database>::Row) -> Result<Self, sqlx::Error>
    where
        DB: Database,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
    {
        Ok(Self {
            count: row.try_get("count")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_row_to_record_translates_every_scope() {
        let global = RoleRow {
            name: "admin".into(),
            resource_type: String::new(),
            resource_id: String::new(),
        };
        let record = global.to_record();
        assert!(record.is_global());
        assert_eq!(record.name.as_str(), "admin");

        let class = RoleRow {
            name: "manager".into(),
            resource_type: "Forum".into(),
            resource_id: String::new(),
        };
        let record = class.to_record();
        assert!(record.is_class_scoped_to("Forum"));
        assert_eq!(record.name.as_str(), "manager");

        let instance = RoleRow {
            name: "moderator".into(),
            resource_type: "Forum".into(),
            resource_id: "42".into(),
        };
        let record = instance.to_record();
        assert!(record.is_instance_scoped_to("Forum", &ResourceId::from("42")));
        assert_eq!(record.name.as_str(), "moderator");
    }

    #[test]
    fn resource_key_row_translations_match_role_row() {
        let row = ResourceKeyRow {
            name: "moderator".into(),
            resource_type: "Forum".into(),
            resource_id: "42".into(),
        };
        let record = row.to_record();
        assert!(record.is_instance_scoped_to("Forum", &ResourceId::from("42")));
        let key = row.to_key();
        assert_eq!(key.resource_type, "Forum");
        assert_eq!(key.resource_id, ResourceId::from("42"));
    }

    #[test]
    #[should_panic(
        expected = "ResourceKeyRow.resource_id is never sentinel; catalog queries filter globals out"
    )]
    fn resource_key_row_to_key_rejects_sentinel_ids() {
        let row = ResourceKeyRow {
            name: "admin".into(),
            resource_type: "Forum".into(),
            resource_id: String::new(),
        };
        let _ = row.to_key();
    }
}
