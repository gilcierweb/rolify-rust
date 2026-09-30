//! Role value types: [`RoleName`], [`ResourceId`], [`RoleRecord`].
//!
//! These mirror the gem's `roles` table row (name, `resource_type`,
//! `resource_id`) as plain data - the kernel predicates operate on them
//! without any I/O.

use core::fmt;

use crate::query::RoleQuery;

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

/// Zero-I/O cached snapshot of one user's roles (CORE-04).
///
/// `RoleSet` borrows a **pre-fetched** role list and answers membership
/// questions purely, via the kernel. Passing it anything that required a
/// round-trip would defeat the gem's cached path: the `role.rb`
/// `new_record?`-vs-persisted distinction maps to **which** slice you hand
/// it (loaded association rows vs. a fresh hit), never to extra queries on
/// the kernel side. The gem's `:any` `new_record?` divergence lives OUTSIDE
/// the kernel (D-2 parity-matrix entry).
///
/// Zero-I/O is enforced statically: no constructor or method takes any
/// backend handle at all (Pitfall 2 - the signature itself is the proof).
#[derive(Clone, Copy, Debug)]
pub struct RoleSet<'a> {
    rows: &'a [RoleRecord],
}

impl<'a> RoleSet<'a> {
    /// Snapshot a slice of already-loaded role rows.
    #[must_use]
    pub fn new(rows: &'a [RoleRecord]) -> Self {
        Self { rows }
    }

    /// The borrowed rows.
    #[must_use]
    pub fn rows(&self) -> &'a [RoleRecord] {
        self.rows
    }

    /// Non-strict cached membership - the gem's `has_cached_role?`
    /// (`role.rb:47-49`), delegating to [`crate::kernel::find_cached`].
    #[must_use]
    pub fn has_cached_role(&self, query: &RoleQuery<'_>) -> bool {
        crate::kernel::find_cached(self.rows, query)
    }

    /// Strict cached membership - the gem's `has_strict_cached_role?`
    /// (`role.rb:51-54`), delegating to [`crate::kernel::find_cached_strict`].
    /// Callers apply the strict gate themselves (`kernel::strict_engages`);
    /// this predicate does not re-decide it.
    #[must_use]
    pub fn has_strict_cached_role(&self, query: &RoleQuery<'_>) -> bool {
        crate::kernel::find_cached_strict(self.rows, query)
    }
}

#[cfg(test)]
mod role_set {
    use super::*;
    use crate::kernel::{fixtures, where_};
    use crate::query::{ResourceFilter, RoleQuery};
    use rstest::rstest;

    fn customer_roles() -> Vec<RoleRecord> {
        vec![
            RoleRecord::global("admin"),
            RoleRecord::for_class("manager", "Forum"),
            RoleRecord::for_instance("moderator", "Forum", 7_i64),
        ]
    }

    #[test]
    fn new_and_rows_are_zero_io_borrows() {
        let rows = customer_roles();
        let set = RoleSet::new(&rows);
        assert_eq!(set.rows().len(), 3);
    }

    #[test]
    fn cached_role_agrees_with_the_query_path() {
        let rows = customer_roles();
        let set = RoleSet::new(&rows);
        let admin = RoleName::from("admin");
        let manager = RoleName::from("manager");
        let moderator = RoleName::from("moderator");
        let ghost = RoleName::from("ghost");
        let forum_seven = ResourceId::from(7_i64);

        // global override visible through the cache
        assert!(set.has_cached_role(&RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Instance("Forum", &forum_seven),
        )));
        // class covers instance
        assert!(set.has_cached_role(&RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Instance("Forum", &forum_seven),
        )));
        // reverse never holds
        assert!(!set.has_cached_role(&RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Global,
        )));
        // instance is not class
        assert!(!set.has_cached_role(&RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Class("Forum"),
        )));
        // any includes global (D-2)
        assert!(set.has_cached_role(&RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Any,
        )));
        // unknown
        assert!(!set.has_cached_role(&RoleQuery::with_role(&ghost)));
    }

    #[test]
    fn strict_cached_role_matches_exact_scope_only() {
        let rows = customer_roles();
        let set = RoleSet::new(&rows);
        let manager = RoleName::from("manager");
        let moderator = RoleName::from("moderator");
        let admin = RoleName::from("admin");
        let forum_seven = ResourceId::from(7_i64);

        assert!(set.has_strict_cached_role(&RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Class("Forum"),
        )));
        assert!(set.has_strict_cached_role(&RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_seven),
        )));
        // no overrides under strict
        assert!(!set.has_strict_cached_role(&RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Class("Forum"),
        )));
        assert!(!set.has_strict_cached_role(&RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Instance("Forum", &forum_seven),
        )));
    }

    /// SC-1 cross-path agreement sweep: for every holder scenario and every
    /// name x kind x type x id combination, `RoleSet::has_cached_role` must
    /// agree with the `where_` query path. Combinatorial by construction, so
    /// every SC-1 row is covered (the labeled headline rows above are the
    /// spec-traceable anchors).
    #[rstest]
    #[case::global_admin(vec![RoleRecord::global("admin")], "admin")]
    #[case::class_manager(vec![RoleRecord::for_class("manager", "Forum")], "manager")]
    #[case::instance_moderator(vec![RoleRecord::for_instance("moderator", "Forum", 1_i64)], "moderator")]
    #[case::mixed(customer_roles(), "admin")]
    fn sc1_cached_vs_query_agreement(#[case] rows: Vec<RoleRecord>, #[case] held: &str) {
        let set = RoleSet::new(&rows);
        let names = [held, "admin", "manager", "ghost", "Admin"];
        for name in names {
            let name = RoleName::from(name);
            for id_raw in [1_i64, 7, 42] {
                let id = ResourceId::from(id_raw);
                for kind in [
                    fixtures::FilterKind::Global,
                    fixtures::FilterKind::Class,
                    fixtures::FilterKind::Instance,
                    fixtures::FilterKind::Any,
                ] {
                    for type_name in ["Forum", "Group"] {
                        let query =
                            fixtures::make_query(&name, kind, type_name, &id);
                        let query_path = !where_(&rows, &query).is_empty();
                        let cached_path = set.has_cached_role(&query);
                        assert_eq!(
                            query_path, cached_path,
                            "where_/has_cached_role divergence: name={name} kind={kind:?} type={type_name} id={id}"
                        );
                        let strict_agree = set.has_strict_cached_role(&query)
                            != crate::kernel::where_strict(&rows, &query).is_empty();
                        assert!(strict_agree);
                    }
                }
            }
        }
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
