//! `QueryableByName` row structs for deserializing raw SQL query results.
//!
//! Diesel's `sql_query()` deserializes **by column name** into structs
//! deriving `QueryableByName`. Every column in the SQL projection must be
//! explicitly aliased (`AS name`) to match the struct field names. We never
//! use `SELECT *` — joined tables would produce name clashes (Pitfall 9).

use diesel::deserialize::QueryableByName;
use diesel::sql_types::Text;
use rolify_core::role::{ResourceId, RoleName, RoleRecord, SCOPE_SENTINEL};

/// Row returned by the role-table SELECT projections.
///
/// Maps directly to the `roles` table columns plus the sentinel-encoded
/// scope columns. The `to_record()` method translates the physical
/// `''` sentinel back to semantic `Option` via `sentinel::from_storage`.
#[derive(Debug, Clone, QueryableByName)]
#[cfg_attr(feature = "postgres", diesel(check_for_backend(diesel::pg::Pg)))]
#[cfg_attr(feature = "mysql", diesel(check_for_backend(diesel::mysql::Mysql)))]
#[cfg_attr(feature = "sqlite", diesel(check_for_backend(diesel::sqlite::Sqlite)))]
pub(crate) struct RoleRow {
    #[diesel(sql_type = Text)]
    pub name: String,
    #[diesel(sql_type = Text)]
    pub resource_type: String,
    #[diesel(sql_type = Text)]
    pub resource_id: String,
}

impl RoleRow {
    /// Convert the physical row to a semantic `RoleRecord`.
    ///
    /// Uses `sentinel::from_storage` / `resource_id_from_storage` so the
    /// sentinel `''` becomes `None` and the SPI never sees the physical format.
    #[must_use]
    pub fn to_record(&self) -> RoleRecord {
        use crate::sentinel::{from_storage, resource_id_from_storage};

        RoleRecord::new(
            RoleName::new(self.name.clone()),
            from_storage(&self.resource_type),
            resource_id_from_storage(&self.resource_id),
        )
    }
}

/// Single-column row for `SELECT id FROM roles ...` (re-SELECT after INSERT).
#[derive(Debug, Clone, QueryableByName)]
#[cfg_attr(feature = "postgres", diesel(check_for_backend(diesel::pg::Pg)))]
#[cfg_attr(feature = "mysql", diesel(check_for_backend(diesel::mysql::Mysql)))]
#[cfg_attr(feature = "sqlite", diesel(check_for_backend(diesel::sqlite::Sqlite)))]
pub struct IdRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub id: i64,
}

/// Single-column row for `SELECT COUNT(*) AS count ...` probes.
///
/// Mirrors [`IdRow`]: the same derive plus per-engine backend check plus
/// the single `BigInt` column. `COUNT(*)` reports `int8` on Postgres, an
/// integer on MySQL/SQLite; `BigInt` decodes all three.
#[derive(Debug, Clone, QueryableByName)]
#[cfg_attr(feature = "postgres", diesel(check_for_backend(diesel::pg::Pg)))]
#[cfg_attr(feature = "mysql", diesel(check_for_backend(diesel::mysql::Mysql)))]
#[cfg_attr(feature = "sqlite", diesel(check_for_backend(diesel::sqlite::Sqlite)))]
pub struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
}

/// Row for `holders_where` / `all_holders` — holder ids from the join.
#[derive(Debug, Clone, QueryableByName)]
#[cfg_attr(feature = "postgres", diesel(check_for_backend(diesel::pg::Pg)))]
#[cfg_attr(feature = "mysql", diesel(check_for_backend(diesel::mysql::Mysql)))]
#[cfg_attr(feature = "sqlite", diesel(check_for_backend(diesel::sqlite::Sqlite)))]
pub(crate) struct HolderIdRow {
    #[diesel(sql_type = Text)]
    pub user_id: String,
}

/// Row for `resources_find` / `in_list` / `roles_matching` — resource keys from the catalog.
#[derive(Debug, Clone, QueryableByName)]
#[cfg_attr(feature = "postgres", diesel(check_for_backend(diesel::pg::Pg)))]
#[cfg_attr(feature = "mysql", diesel(check_for_backend(diesel::mysql::Mysql)))]
#[cfg_attr(feature = "sqlite", diesel(check_for_backend(diesel::sqlite::Sqlite)))]
pub(crate) struct ResourceKeyRow {
    #[diesel(sql_type = Text)]
    pub name: String,
    #[diesel(sql_type = Text)]
    pub resource_type: String,
    #[diesel(sql_type = Text)]
    pub resource_id: String,
}

impl ResourceKeyRow {
    #[must_use]
    pub fn to_key(&self) -> rolify_core::store::ResourceKey {
        use crate::sentinel::resource_id_from_storage;

        rolify_core::store::ResourceKey::new(
            self.resource_type.clone(),
            resource_id_from_storage(&self.resource_id).expect(
                "ResourceKeyRow.resource_id is never sentinel; catalog queries filter globals out",
            ),
        )
    }

    #[must_use]
    pub fn to_record(&self) -> RoleRecord {
        use crate::sentinel::{from_storage, resource_id_from_storage};

        RoleRecord::new(
            RoleName::new(self.name.clone()),
            from_storage(&self.resource_type),
            resource_id_from_storage(&self.resource_id),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolify_core::role::RoleRecord;

    #[test]
    fn role_row_to_record_global() {
        let row = RoleRow {
            name: "admin".into(),
            resource_type: "".into(),
            resource_id: "".into(),
        };
        let record = row.to_record();
        assert!(record.is_global());
        assert_eq!(record.name.as_str(), "admin");
    }

    #[test]
    fn role_row_to_record_class() {
        let row = RoleRow {
            name: "manager".into(),
            resource_type: "Forum".into(),
            resource_id: "".into(),
        };
        let record = row.to_record();
        assert!(record.is_class_scoped_to("Forum"));
        assert_eq!(record.name.as_str(), "manager");
    }

    #[test]
    fn role_row_to_record_instance() {
        let row = RoleRow {
            name: "moderator".into(),
            resource_type: "Forum".into(),
            resource_id: "42".into(),
        };
        let record = row.to_record();
        assert!(record.is_instance_scoped_to("Forum", &ResourceId::from("42")));
        assert_eq!(record.name.as_str(), "moderator");
    }
}
