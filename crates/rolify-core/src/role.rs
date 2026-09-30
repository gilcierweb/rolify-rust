//! Role value types: [`RoleName`], [`ResourceId`], [`RoleRecord`].
//!
//! These mirror the gem's `roles` table row (name, `resource_type`,
//! `resource_id`) as plain data - the kernel predicates operate on them
//! without any I/O.

use core::fmt;

/// Role name - a newtype over `String` with **exact byte equality**.
///
/// The gem compares with `role.name == args[:name].to_s` (see
/// `role_adapter.rb` `find_cached`): no case-folding, no trimming, ever.
/// Normalization would be a cross-adapter drift and spoofing vector.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RoleName(String);

impl RoleName {
    /// Wrap a raw name. No normalization is applied.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The raw name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoleName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for RoleName {
    fn from(name: String) -> Self {
        Self(name)
    }
}

impl From<&str> for RoleName {
    fn from(name: &str) -> Self {
        Self(name.to_owned())
    }
}

/// Resource identity - an opaque, stringified primary key.
///
/// The gem stringifies PKs when scoping roles (see `spec/support/schema.rb`:
/// `teams.team_code` is a string PK), so ids are stored as text to keep
/// integer and string keys comparable the way adapters compare them in SQL.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(String);

impl ResourceId {
    /// Wrap a raw id. No normalization is applied.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The raw id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for ResourceId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl From<&str> for ResourceId {
    fn from(id: &str) -> Self {
        Self(id.to_owned())
    }
}

impl From<i64> for ResourceId {
    fn from(id: i64) -> Self {
        Self(id.to_string())
    }
}

impl From<u64> for ResourceId {
    fn from(id: u64) -> Self {
        Self(id.to_string())
    }
}

/// A persisted role row: `name` plus the polymorphic scope columns.
///
/// Scope encoding (gem `roles` table):
///
/// | scope | `resource_type` | `resource_id` |
/// |---|---|---|
/// | global | `None` | `None` |
/// | class | `Some(t)` | `None` |
/// | instance | `Some(t)` | `Some(id)` |
///
/// `#[non_exhaustive]` guards against literal construction outside this
/// crate (use the constructors), keeping the scope invariants enforceable.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RoleRecord {
    /// Role name (byte-exact).
    pub name: RoleName,
    /// Resource class name for class/instance scopes; `None` for global.
    pub resource_type: Option<String>,
    /// Stringified resource PK for the instance scope; `None` otherwise.
    pub resource_id: Option<ResourceId>,
}

impl RoleRecord {
    /// Construct from an explicit scope triple. Prefer [`RoleRecord::global`],
    /// [`RoleRecord::for_class`], [`RoleRecord::for_instance`] over juggling
    /// `Option`s by hand.
    #[must_use]
    pub fn new(
        name: RoleName,
        resource_type: Option<String>,
        resource_id: Option<ResourceId>,
    ) -> Self {
        Self { name, resource_type, resource_id }
    }

    /// A **global** role row (both scope columns `None`).
    #[must_use]
    pub fn global(name: impl Into<RoleName>) -> Self {
        Self::new(name.into(), None, None)
    }

    /// A **class-scoped** role row (`resource_type` set, `resource_id` `None`).
    #[must_use]
    pub fn for_class(name: impl Into<RoleName>, resource_type: impl Into<String>) -> Self {
        Self::new(name.into(), Some(resource_type.into()), None)
    }

    /// An **instance-scoped** role row (both scope columns set).
    #[must_use]
    pub fn for_instance(
        name: impl Into<RoleName>,
        resource_type: impl Into<String>,
        resource_id: impl Into<ResourceId>,
    ) -> Self {
        Self::new(name.into(), Some(resource_type.into()), Some(resource_id.into()))
    }

    /// `resource_type IS NULL AND resource_id IS NULL`.
    #[must_use]
    pub fn is_global(&self) -> bool {
        self.resource_type.is_none() && self.resource_id.is_none()
    }

    /// `resource_type = type_name AND resource_id IS NULL`.
    #[must_use]
    pub fn is_class_scoped_to(&self, type_name: &str) -> bool {
        self.resource_type.as_deref() == Some(type_name) && self.resource_id.is_none()
    }

    /// `resource_type = type_name AND resource_id = id`.
    #[must_use]
    pub fn is_instance_scoped_to(&self, type_name: &str, id: &ResourceId) -> bool {
        self.resource_type.as_deref() == Some(type_name) && self.resource_id.as_ref() == Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn role_name_is_byte_exact() {
        assert_eq!(RoleName::from("admin"), RoleName::from("admin"));
        assert_ne!(RoleName::from("Admin"), RoleName::from("admin"));
        assert_ne!(RoleName::from("admin"), RoleName::from("admin "));
    }

    #[test]
    fn resource_id_accepts_string_and_integer_keys() {
        // schema.rb precedent: teams.team_code is a string PK.
        let string_pk = ResourceId::from("T-42");
        assert_eq!(string_pk.as_str(), "T-42");
        assert_eq!(ResourceId::from(42_i64), ResourceId::from(42_u64));
        assert_eq!(ResourceId::from(42_i64).as_str(), "42");
    }

    #[test]
    fn role_record_scope_predicates_pin_the_encoding() {
        let global = RoleRecord::global("admin");
        assert!(global.is_global());
        assert!(!global.is_class_scoped_to("Forum"));
        assert!(!global.is_instance_scoped_to("Forum", &ResourceId::from(1_i64)));

        let class = RoleRecord::for_class("manager", "Forum");
        assert!(!class.is_global());
        assert!(class.is_class_scoped_to("Forum"));
        assert!(!class.is_class_scoped_to("Group"));
        assert!(!class.is_instance_scoped_to("Forum", &ResourceId::from(1_i64)));

        let instance = RoleRecord::for_instance("moderator", "Forum", 7_i64);
        assert!(!instance.is_global());
        // An instance row is NOT the class row (id column differs) - the
        // ladder's class disjunct is a separate match.
        assert!(!instance.is_class_scoped_to("Forum"));
        assert!(instance.is_instance_scoped_to("Forum", &ResourceId::from(7_i64)));
        assert!(!instance.is_instance_scoped_to("Forum", &ResourceId::from(8_i64)));
    }
}
