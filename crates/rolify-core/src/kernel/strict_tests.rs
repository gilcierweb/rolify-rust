//! Strict predicates + the narrow strict gate + ratified corner semantics.
//!
//! Sources: `role_adapter.rb` `where_strict` (lines 11-26),
//! `find_cached_strict` (lines 41-48); gating verified identical at
//! `role.rb:26`, `role.rb:48`, `finders.rb:4`.

use super::fixtures::{
    FilterKind, class_manager_rows, global_admin_rows, instance_moderator_rows, make_query,
};
use super::{strict_engages, where_, where_strict};
use crate::query::ResourceFilter;
use crate::role::{ResourceId, RoleName, RoleRecord};
use rstest::rstest;

/// Strict rows: exact scope only - no global/class override ladder.
/// Each case runs BOTH paths (`where_strict` and, inside `sc1_strict_row`,
/// the cached verdict via `find_cached_strict` equivalence through
/// `where_strict`).
#[rstest]
// strict Class: exact type+null-id rows match; global/instance rows do not
#[case::strict_class_direct(class_manager_rows(), "manager", FilterKind::Class, "Forum", 1, true)]
#[case::strict_class_misses_global_row(global_admin_rows(), "admin", FilterKind::Class, "Forum", 1, false)]
#[case::strict_class_misses_instance_row(instance_moderator_rows(), "moderator", FilterKind::Class, "Forum", 1, false)]
#[case::strict_class_wrong_type(class_manager_rows(), "manager", FilterKind::Class, "Group", 1, false)]
// strict Instance: exact triple only
#[case::strict_instance_direct(instance_moderator_rows(), "moderator", FilterKind::Instance, "Forum", 1, true)]
#[case::strict_instance_misses_class_row(class_manager_rows(), "manager", FilterKind::Instance, "Forum", 1, false)]
#[case::strict_instance_misses_global_row(global_admin_rows(), "admin", FilterKind::Instance, "Forum", 1, false)]
#[case::strict_instance_wrong_id(instance_moderator_rows(), "moderator", FilterKind::Instance, "Forum", 2, false)]
#[case::strict_instance_wrong_type(instance_moderator_rows(), "moderator", FilterKind::Instance, "Group", 1, false)]
// strict corner: strict(Global) = exactly-global (deliberate divergence -
// the gem's two strict paths contradict each other here; see parity matrix)
#[case::corner_strict_global_hits_global_row(global_admin_rows(), "admin", FilterKind::Global, "Forum", 1, true)]
#[case::corner_strict_global_misses_class_row(class_manager_rows(), "manager", FilterKind::Global, "Forum", 1, false)]
#[case::corner_strict_global_misses_instance_row(instance_moderator_rows(), "moderator", FilterKind::Global, "Forum", 1, false)]
// strict corner: strict(Any) = name-only (deliberate divergence - the gem
// raises/never reaches; see parity matrix)
#[case::corner_strict_any_global_row(global_admin_rows(), "admin", FilterKind::Any, "Forum", 1, true)]
#[case::corner_strict_any_class_row(class_manager_rows(), "manager", FilterKind::Any, "Group", 1, true)]
#[case::corner_strict_any_wrong_name(global_admin_rows(), "auditor", FilterKind::Any, "Forum", 1, false)]
fn sc1_strict_row(
    #[case] rows: Vec<RoleRecord>,
    #[case] name: &str,
    #[case] kind: FilterKind,
    #[case] type_name: &str,
    #[case] id: i64,
    #[case] expected: bool,
) {
    let name = RoleName::from(name);
    let rid = ResourceId::from(id);
    let query = make_query(&name, kind, type_name, &rid);

    let strict_query_path = !where_strict(&rows, &query).is_empty();
    assert_eq!(strict_query_path, expected, "strict query path (where_strict)");
    assert_eq!(
        super::find_cached_strict(&rows, &query),
        expected,
        "strict cached path (find_cached_strict)"
    );
}

/// The gate itself: strict engages ONLY for Class/Instance filters
/// (role.rb:26, role.rb:48, finders.rb:4).
#[rstest]
#[case::off_global(false, ResourceFilter::Global, false)]
#[case::on_global_gate_closed(true, ResourceFilter::Global, false)]
#[case::on_any_gate_closed(true, ResourceFilter::Any, false)]
#[case::off_any(false, ResourceFilter::Any, false)]
#[case::on_class_engages(true, ResourceFilter::Class("Forum"), true)]
#[case::off_class(false, ResourceFilter::Class("Forum"), false)]
fn strict_engages_only_on_class_or_instance(
    #[case] strict: bool,
    #[case] filter: ResourceFilter<'_>,
    #[case] expected: bool,
) {
    assert_eq!(strict_engages(strict, &filter), expected);
}

#[test]
fn strict_engages_instance_variant() {
    let id = ResourceId::from(7_i64);
    assert!(strict_engages(true, &ResourceFilter::Instance("Forum", &id)));
    assert!(!strict_engages(false, &ResourceFilter::Instance("Forum", &id)));
}

/// Gating behavior consequence: for `Global` and `Any` filters the strict
/// predicates MUST return the same verdict as the non-strict ladder (the
/// gate never engages there, and our ratified corners keep the predicates
/// aligned); for `Class`/`Instance` they CAN differ.
#[test]
fn strict_matches_non_strict_on_global_and_any_filters() {
    let name = RoleName::from("admin");
    let id = ResourceId::from(1_i64);
    for rows in [global_admin_rows(), class_manager_rows(), instance_moderator_rows()] {
        for kind in [FilterKind::Global, FilterKind::Any] {
            for type_name in ["Forum", "Group"] {
                let query = make_query(&name, kind, type_name, &id);
                assert_eq!(
                    !where_(&rows, &query).is_empty(),
                    !where_strict(&rows, &query).is_empty(),
                    "strict/non-strict diverged on a gate-closed filter ({kind:?}, {type_name})"
                );
            }
        }
    }
}

/// The difference probe: strict CAN change Class/Instance outcomes (a global
/// row satisfies non-strict Class/Instance queries but never strict ones).
#[test]
fn strict_differs_on_class_and_instance_when_only_global_held() {
    let rows = global_admin_rows();
    let name = RoleName::from("admin");
    let id = ResourceId::from(1_i64);

    let class_query = make_query(&name, FilterKind::Class, "Forum", &id);
    assert!(!where_(&rows, &class_query).is_empty());
    assert!(where_strict(&rows, &class_query).is_empty());

    let instance_query = make_query(&name, FilterKind::Instance, "Forum", &id);
    assert!(!where_(&rows, &instance_query).is_empty());
    assert!(where_strict(&rows, &instance_query).is_empty());
}

/// And for completeness: the class holder differs on Instance under strict
/// (class-covers-instance is a non-strict-ladder behavior only).
#[test]
fn strict_differs_on_instance_when_class_held() {
    let rows = class_manager_rows();
    let name = RoleName::from("manager");
    let id = ResourceId::from(1_i64);

    let query = make_query(&name, FilterKind::Instance, "Forum", &id);
    assert!(!where_(&rows, &query).is_empty());
    assert!(where_strict(&rows, &query).is_empty());
}
