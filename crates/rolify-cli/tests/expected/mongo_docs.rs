//! MongoDB role document template for rolify.
//!
//! Converged with `rolify-mongodb` `document.rs` (D-07, D-08, D-17).
//! No BSON-type invention beyond the spec; no camelCase fields.
//!
//! Field set (exact):
//! - `name`: String - role name (byte-exact per gem)
//! - `resource_type`: Option<String> - explicit null when global/class scope
//! - `resource_id`: Option<String> - explicit null when global/class scope
//! - `user_ids`: Vec<String> - user references (two-sided HABTM)
//!
//! Index notes (CLI documents; adapter converges at setup per Phase 5 D-10):
//! - Unique compound index on (name, resource_type, resource_id) ensured
//!   idempotently at store setup
//! - Consumer document gains `role_ids` (two-sided HABTM); removal updates
//!   BOTH sides with `$pull`/`$addToSet`; emptiness checked AFTER removal
//!
//! Table name references (substituted at generate time):
//! - Roles table: roles
//! - Join table: users_roles
//!
//! The template ends with the Markdown index notes: everything before the
//! level-one heading below ships as `mongo/role.rs`, and the notes section
//! itself ships as `mongo/INDEX_NOTES.md`.

use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// Role document as stored in MongoDB.
///
/// Sentinel nulls (not absent) for resource_type/resource_id when scope is
/// global or class - stored as explicit BSON null (D-07), not the SQL
/// `''` sentinel: the gem's Mongoid adapter writes `nil` for scopeless
/// roles (role_adapter.rb:50-54). A `None` scope
/// serializes as explicit BSON null, never as an omitted field, so the
/// derive agrees with the locked `rolify-mongodb` `document.rs` contract
/// (D-07 convergence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDoc {
    #[serde(rename = "_id")]
    pub id: ObjectId,

    pub name: String,

    pub resource_type: Option<String>,

    pub resource_id: Option<String>,

    #[serde(default)]
    pub user_ids: Vec<String>,
}

/// Consumer-side role IDs for two-sided HABTM.
///
/// When a role is removed, BOTH sides are updated with `$pull`/`$addToSet`.
/// Emptiness is checked AFTER removal (if user_ids becomes empty, the role
/// document may be removed per `remove_role_if_empty` config).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerRoleIds {
    #[serde(default)]
    pub role_ids: Vec<ObjectId>,
}
# MongoDB index setup notes

The unique compound index on (name, resource_type, resource_id) must be
created idempotently at store initialization. The adapter (`rolify-mongodb`)
owns this convergence per Phase 5 D-10; the CLI only documents it.

- Unique compound index: (name: 1, resource_type: 1, resource_id: 1)
- Ensured idempotently at store setup
- Deduplicates identically to the SQL sentinel '' strategy
