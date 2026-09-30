//! Pure semantics kernel - the gem's match ladder as zero-I/O predicates.
//!
//! Port of `rolify/lib/rolify/adapters/active_record/role_adapter.rb`:
//! `build_query` (lines 106-121) for the query path and `find_cached`
//! (lines 28-39 - byte-identical in the Mongoid adapter, the purity proof).
//!
//! ## D-02 divergence (deliberate, documented)
//!
//! [`ResourceFilter::Any`] (the gem's `:any`) matches by name **including
//! global-role rows**: that is the gem's *database* path - `build_query:107`
//! short-circuits to `name = ?` before any scope condition. The gem's
//! *other* path (`role.rb`'s `new_record?`, in-memory) requires a resource
//! for `:any` and therefore excludes globals. The kernel follows the DB path
//! on purpose; the divergence is recorded in the parity matrix.

use crate::query::{ResourceFilter, RoleQuery};
use crate::role::RoleRecord;

/// Non-strict three-disjunct ladder over in-memory rows - the semantics of
/// `RoleStore::where_` (every adapter translates it mechanically; the gem's
/// cached path is the same predicate, see `find_cached`).
///
/// Disjuncts per filter (every disjunct is name-gated, exact byte match):
///
/// | filter | matches |
/// |---|---|
/// | `Global` | name AND `resource_type` IS NULL AND `resource_id` IS NULL |
/// | `Class(t)` | name AND (global OR (`resource_type` = t AND `resource_id` IS NULL)) |
/// | `Instance(t, id)` | name AND (global OR class(t) OR (`resource_type` = t AND `resource_id` = id)) |
/// | `Any` | name only - **including** global rows (D-02) |
#[must_use]
pub fn where_<'a>(rows: &'a [RoleRecord], query: &RoleQuery<'_>) -> Vec<&'a RoleRecord> {
    rows.iter()
        .filter(|record| record_matches(record, query))
        .collect()
}

/// One row against the ladder - every disjunct is name-gated.
fn record_matches(record: &RoleRecord, query: &RoleQuery<'_>) -> bool {
    let name_ok = record.name == *query.name;
    match &query.filter {
        // gem: resource == nil -> global disjunct only
        ResourceFilter::Global => name_ok && record.is_global(),
        // gem: :any -> short-circuit to name-only (build_query:107) - D-02
        ResourceFilter::Any => name_ok,
        // gem: Class -> global OR (type AND id NULL)
        ResourceFilter::Class(type_name) => {
            name_ok && (record.is_global() || record.is_class_scoped_to(type_name))
        }
        // gem: instance -> global OR class(t) OR exact instance (3 disjuncts)
        ResourceFilter::Instance(type_name, id) => {
            name_ok
                && (record.is_global()
                    || record.is_class_scoped_to(type_name)
                    || record.is_instance_scoped_to(type_name, id))
        }
    }
}

/// Strict-mode gate - verified identical at the gem's three call sites
/// (`role.rb:26`, `role.rb:48`, `finders.rb:4`:
/// `strict_rolify && resource && resource != :any`).
///
/// Strict engages ONLY when a concrete `Class`/`Instance` resource is given;
/// never for `Global` (nil is falsy in Ruby), never for `Any`.
#[must_use]
pub fn strict_engages(strict: bool, filter: &ResourceFilter<'_>) -> bool {
    strict && matches!(filter, ResourceFilter::Class(_) | ResourceFilter::Instance(..))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::{ResourceId, RoleName};
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    /// Test-local filter encoding so rstest cases stay plain data.
    #[derive(Clone, Copy)]
    enum FilterKind {
        Global,
        Class,
        Instance,
        Any,
    }

    fn admin(name: &str) -> RoleName {
        RoleName::from(name)
    }

    fn matches(rows: &[RoleRecord], name: &RoleName, kind: FilterKind, type_name: &str, id: i64) -> bool {
        let rid = ResourceId::from(id);
        let filter = match kind {
            FilterKind::Global => ResourceFilter::Global,
            FilterKind::Class => ResourceFilter::Class(type_name),
            FilterKind::Instance => ResourceFilter::Instance(type_name, &rid),
            FilterKind::Any => ResourceFilter::Any,
        };
        let query = RoleQuery { name, filter };
        !where_(rows, &query).is_empty()
    }

    /// SC-1 minimal rows (full matrix lands in 01-02, both paths):
    /// holder rows x query filter x expected.
    #[rstest]
    // global-direct: a global row satisfies a global query
    #[case::global_direct(&[RoleRecord::global("admin")], FilterKind::Global, "Forum", 1, true)]
    // global override, class query (the gem's headline behavior)
    #[case::global_overrides_class(&[RoleRecord::global("admin")], FilterKind::Class, "Forum", 1, true)]
    // global override, instance query
    #[case::global_overrides_instance(&[RoleRecord::global("admin")], FilterKind::Instance, "Forum", 1, true)]
    // Any includes global rows (D-02 ratification)
    #[case::any_includes_global(&[RoleRecord::global("admin")], FilterKind::Any, "Forum", 1, true)]
    // class row covers an instance query of the same type
    #[case::class_covers_instance(&[RoleRecord::for_class("admin", "Forum")], FilterKind::Instance, "Forum", 1, true)]
    // reverse never holds: a class-scoped role does NOT satisfy a global query
    #[case::class_role_misses_global(&[RoleRecord::for_class("admin", "Forum")], FilterKind::Global, "Forum", 1, false)]
    #[case::class_role_misses_other_class(&[RoleRecord::for_class("admin", "Forum")], FilterKind::Class, "Group", 1, false)]
    // exact instance only
    #[case::instance_exact(&[RoleRecord::for_instance("admin", "Forum", 1_i64)], FilterKind::Instance, "Forum", 1, true)]
    #[case::instance_wrong_id(&[RoleRecord::for_instance("admin", "Forum", 1_i64)], FilterKind::Instance, "Forum", 2, false)]
    // wrong-name negative at every scope
    #[case::wrong_name_global(&[RoleRecord::global("moderator")], FilterKind::Global, "Forum", 1, false)]
    #[case::wrong_name_any(&[RoleRecord::global("moderator")], FilterKind::Any, "Forum", 1, false)]
    fn ladder_rows(
        #[case] rows: &[RoleRecord],
        #[case] kind: FilterKind,
        #[case] type_name: &str,
        #[case] id: i64,
        #[case] expected: bool,
    ) {
        assert_eq!(matches(rows, &admin("admin"), kind, type_name, id), expected);
    }

    #[test]
    fn empty_rows_never_match() {
        assert!(!matches(&[], &admin("admin"), FilterKind::Any, "Forum", 1));
    }

    /// The gate itself is the contract: strict engages ONLY on Class/Instance
    /// filters (verified identical at role.rb:26, role.rb:48, finders.rb:4).
    #[rstest]
    #[case::off_global(false, ResourceFilter::Global, false)]
    #[case::on_global(true, ResourceFilter::Global, false)]
    #[case::on_any(true, ResourceFilter::Any, false)]
    #[case::off_any(false, ResourceFilter::Any, false)]
    #[case::on_class(true, ResourceFilter::Class("Forum"), true)]
    #[case::off_class(false, ResourceFilter::Class("Forum"), false)]
    fn strict_engages_only_on_class_or_instance(
        #[case] strict: bool,
        #[case] filter: ResourceFilter<'_>,
        #[case] expected: bool,
    ) {
        assert_eq!(strict_engages(strict, &filter), expected);
    }

    #[test]
    fn strict_engages_on_instance() {
        let id = ResourceId::from(1_i64);
        assert!(strict_engages(true, &ResourceFilter::Instance("Forum", &id)));
        assert!(!strict_engages(false, &ResourceFilter::Instance("Forum", &id)));
    }
}
