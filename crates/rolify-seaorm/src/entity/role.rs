//! Flat static entity for the `roles` table (2.0 dense format).
//!
//! Per D-02 as corrected by `05-RESEARCH.md` (the runtime `sea_orm::dynamic`
//! module is Unstable and built for unknown-at-compile-time schemas): the
//! roles schema is fixed and known, so the adapter uses flat static
//! entities plus `WHERE` on the string scope columns. Polymorphism in
//! rolify is a data convention (type-name strings), not a schema problem
//! (`resource_adapter.rb` `relation_types_for`).
//!
//! The scope columns store the `''` sentinel for absent scope (D-06):
//! global rows persist `resource_type = ''` and `resource_id = ''`, and
//! class rows persist `resource_id = ''`. The `None` <-> `''` translation
//! lives entirely at the adapter boundary (this module); the core keeps
//! `Option` semantics and the sentinel never crosses the SPI. Read side
//! mirrors gem `role_adapter.rb:106-121` equality branches.
//!
//! Note: the physical table also carries `created_at`/`updated_at`
//! columns with `DEFAULT CURRENT_TIMESTAMP`; the adapter never reads or
//! writes them, so the model intentionally omits them (database defaults
//! fill them on insert).

use rolify_core::role::{ResourceId, RoleName, RoleRecord, SCOPE_SENTINEL};
use sea_orm::entity::prelude::*;

/// Role row: one role name at one scope, with the `user_ids` link rows
/// living in the separate `users_roles` join entity (`join.rs`).
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "roles")]
pub struct Model {
    /// `BIGINT GENERATED ALWAYS AS IDENTITY` primary key (canonical schema).
    #[sea_orm(primary_key)]
    pub id: i64,
    /// Role name; byte-exact comparison, never normalized (gem parity).
    pub name: String,
    /// Resource class name, or `''` sentinel for global scope (D-06).
    pub resource_type: String,
    /// Stringified resource PK, or `''` sentinel for class/global scope (D-06).
    pub resource_id: String,
}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Convert the physical row to a semantic `RoleRecord`.
    ///
    /// The sentinel `''` becomes `None` via the boundary helpers below,
    /// so the SPI never sees the physical format (mirrors the diesel
    /// adapter's `RoleRow::to_record`).
    #[must_use]
    pub fn to_record(&self) -> RoleRecord {
        RoleRecord::new(
            RoleName::new(self.name.clone()),
            from_storage(&self.resource_type),
            resource_id_from_storage(&self.resource_id),
        )
    }

    /// Convert a semantic `RoleRecord` to a write-ready `ActiveModel`.
    ///
    /// `id` stays unset so the identity column is filled by the database.
    /// Scope columns receive the `''` sentinel for absent scope (write
    /// side of D-06).
    #[must_use]
    pub fn from_record(record: &RoleRecord) -> ActiveModel {
        ActiveModel {
            name: sea_orm::Set(RoleName::as_str(&record.name).to_owned()),
            resource_type: sea_orm::Set(to_storage(record.resource_type.as_deref()).to_owned()),
            resource_id: sea_orm::Set(resource_id_to_storage(record.resource_id.as_ref()).clone()),
            ..Default::default()
        }
    }
}

/// Write side of D-06: semantic `None` becomes the sentinel `''`.
#[inline]
#[must_use]
pub fn to_storage(value: Option<&str>) -> &str {
    value.unwrap_or(SCOPE_SENTINEL)
}

/// Write side of D-06 for resource ids.
#[inline]
#[must_use]
pub fn resource_id_to_storage(value: Option<&ResourceId>) -> String {
    value.map_or_else(|| SCOPE_SENTINEL.to_owned(), |id| id.as_str().to_owned())
}

/// Read side of D-06: the sentinel `''` becomes semantic `None`.
#[inline]
#[must_use]
pub fn from_storage(value: &str) -> Option<String> {
    if value == SCOPE_SENTINEL {
        None
    } else {
        Some(value.to_owned())
    }
}

/// Read side of D-06 for resource ids.
#[inline]
#[must_use]
pub fn resource_id_from_storage(value: &str) -> Option<ResourceId> {
    if value == SCOPE_SENTINEL {
        None
    } else {
        Some(ResourceId::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_record_maps_global_sentinel_to_none() {
        let model = Model {
            id: 1,
            name: "admin".to_owned(),
            resource_type: String::new(),
            resource_id: String::new(),
        };
        let record = model.to_record();
        assert!(record.is_global());
        assert_eq!(record.resource_type, None);
        assert_eq!(record.resource_id, None);
    }

    #[test]
    fn to_record_maps_class_to_some_type() {
        let model = Model {
            id: 2,
            name: "manager".to_owned(),
            resource_type: "Forum".to_owned(),
            resource_id: String::new(),
        };
        let record = model.to_record();
        assert!(record.is_class_scoped_to("Forum"));
        assert_eq!(record.resource_type, Some("Forum".to_owned()));
        assert_eq!(record.resource_id, None);
    }

    #[test]
    fn to_record_maps_instance_to_some_pair() {
        let model = Model {
            id: 3,
            name: "moderator".to_owned(),
            resource_type: "Forum".to_owned(),
            resource_id: "42".to_owned(),
        };
        let record = model.to_record();
        assert!(record.is_instance_scoped_to("Forum", &ResourceId::from("42")));
    }

    #[test]
    fn from_record_writes_sentinel_for_absent_scope() {
        let record = RoleRecord::global("admin");
        let active = Model::from_record(&record);
        assert_eq!(
            active.resource_type,
            sea_orm::ActiveValue::Set(String::new())
        );
        assert_eq!(active.resource_id, sea_orm::ActiveValue::Set(String::new()));
    }

    #[test]
    fn sentinel_roundtrip_preserves_semantics() {
        assert_eq!(from_storage(to_storage(None)), None);
        assert_eq!(
            from_storage(to_storage(Some("Forum"))),
            Some("Forum".to_owned())
        );
        let id = ResourceId::from("123");
        assert_eq!(
            resource_id_from_storage(&resource_id_to_storage(Some(&id))),
            Some(id)
        );
    }
}
