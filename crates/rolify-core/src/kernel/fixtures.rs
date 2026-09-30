//! Shared fixtures for the kernel test submodules (SC-1 matrix).
//!
//! Holder scenarios model the gem's `shared_examples_for_has_role.rb` setup:
//! the "holder rows" slice is what `user.roles` would contain.

use crate::query::{ResourceFilter, RoleQuery};
use crate::role::{ResourceId, RoleName, RoleRecord};

/// Test-local filter encoding so rstest `#[case]` tables stay plain data.
#[derive(Clone, Copy, Debug)]
pub enum FilterKind {
    Global,
    Class,
    Instance,
    Any,
}

/// Build a `RoleQuery` from plain case data.
pub fn make_query<'a>(
    name: &'a RoleName,
    kind: FilterKind,
    type_name: &'a str,
    id: &'a ResourceId,
) -> RoleQuery<'a> {
    let filter = match kind {
        FilterKind::Global => ResourceFilter::Global,
        FilterKind::Class => ResourceFilter::Class(type_name),
        FilterKind::Instance => ResourceFilter::Instance(type_name, id),
        FilterKind::Any => ResourceFilter::Any,
    };
    RoleQuery { name, filter }
}

/// Holder: global `admin` (spec L8-16 context).
pub fn global_admin_rows() -> Vec<RoleRecord> {
    vec![RoleRecord::global("admin")]
}

/// Holder: class-scoped `manager` on Forum (spec L50-64 context).
pub fn class_manager_rows() -> Vec<RoleRecord> {
    vec![RoleRecord::for_class("manager", "Forum")]
}

/// Holder: instance-scoped `moderator` on Forum first (id 1) (spec L84-92
/// reverse context + instance contexts).
pub fn instance_moderator_rows() -> Vec<RoleRecord> {
    vec![RoleRecord::for_instance("moderator", "Forum", 1_i64)]
}
