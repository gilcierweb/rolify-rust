//! Full SC-1 matrix for the NON-strict ladder (query path `where_` AND
//! cached path `find_cached` in the same assertion body, per the plan).
//!
//! Every row traces to `rolify/spec/rolify/shared_examples/shared_examples_for_has_role.rb`
//! (205 lines) via the RESEARCH.md SC-1 table (L337-357). Kernel-level, the
//! gem's holder-relation (`self.roles`) is modeled as the input slice; the
//! ":any matches the HELD row when an unassigned same-name role exists
//! elsewhere" case therefore reduces to "Any matches held rows" plus
//! "Class(Group) misses" (documented inline).

use super::fixtures::{
    FilterKind, class_manager_rows, global_admin_rows, instance_moderator_rows, make_query,
};
use super::{find_cached, where_};
use crate::role::{ResourceId, RoleName, RoleRecord};
use rstest::rstest;

/// One matrix row, both paths: `where_` emptiness and `find_cached` must
/// agree with the expected membership. Byte-exact names throughout.
#[rstest]
// ---- holder: global `admin` (spec L8-16) ----
#[case::global_direct(global_admin_rows(), "admin", FilterKind::Global, "Forum", 1, true)]
#[case::global_overrides_class(global_admin_rows(), "admin", FilterKind::Class, "Forum", 1, true)]
#[case::global_overrides_instance(
    global_admin_rows(),
    "admin",
    FilterKind::Instance,
    "Forum",
    1,
    true
)]
#[case::global_seen_by_any(global_admin_rows(), "admin", FilterKind::Any, "Forum", 1, true)]
#[case::global_holder_wrong_name_any(
    global_admin_rows(),
    "global",
    FilterKind::Any,
    "Forum",
    1,
    false
)]
#[case::global_holder_other_role_name(
    global_admin_rows(),
    "moderator",
    FilterKind::Global,
    "Forum",
    1,
    false
)]
#[case::global_holder_class_query_wrong_name(
    global_admin_rows(),
    "manager",
    FilterKind::Class,
    "Forum",
    1,
    false
)]
// ---- holder: class `manager` on Forum (spec L50-64) ----
#[case::class_direct(class_manager_rows(), "manager", FilterKind::Class, "Forum", 1, true)]
#[case::class_covers_instance(
    class_manager_rows(),
    "manager",
    FilterKind::Instance,
    "Forum",
    1,
    true
)]
#[case::class_seen_by_any(class_manager_rows(), "manager", FilterKind::Any, "Forum", 1, true)]
#[case::class_reverse_never_holds_global(
    class_manager_rows(),
    "manager",
    FilterKind::Global,
    "Forum",
    1,
    false
)]
#[case::class_wrong_type(class_manager_rows(), "manager", FilterKind::Class, "Group", 1, false)]
#[case::any_matches_held_row(class_manager_rows(), "manager", FilterKind::Any, "Group", 1, true)]
// ---- holder: instance `moderator` on Forum #1 (spec L84-92 + instance ctx) ----
#[case::instance_direct(
    instance_moderator_rows(),
    "moderator",
    FilterKind::Instance,
    "Forum",
    1,
    true
)]
#[case::instance_seen_by_any(
    instance_moderator_rows(),
    "moderator",
    FilterKind::Any,
    "Forum",
    1,
    true
)]
#[case::instance_reverse_never_holds_global(
    instance_moderator_rows(),
    "moderator",
    FilterKind::Global,
    "Forum",
    1,
    false
)]
#[case::instance_is_not_class(
    instance_moderator_rows(),
    "moderator",
    FilterKind::Class,
    "Forum",
    1,
    false
)]
#[case::instance_wrong_id(
    instance_moderator_rows(),
    "moderator",
    FilterKind::Instance,
    "Forum",
    2,
    false
)]
#[case::instance_wrong_type(
    instance_moderator_rows(),
    "moderator",
    FilterKind::Instance,
    "Group",
    1,
    false
)]
// ---- nonexistent names, any holder / empty ----
#[case::nonexistent_name_any(global_admin_rows(), "dummy", FilterKind::Any, "Forum", 1, false)]
#[case::nonexistent_name_global(
    instance_moderator_rows(),
    "dumber",
    FilterKind::Global,
    "Forum",
    1,
    false
)]
#[case::empty_rows_never_match(Vec::new(), "admin", FilterKind::Any, "Forum", 1, false)]
// ---- byte-exact matching (no case-folding, no trimming) ----
#[case::byte_exact_capital_misses(
    global_admin_rows(),
    "Admin",
    FilterKind::Global,
    "Forum",
    1,
    false
)]
#[case::byte_exact_capital_misses_any(
    global_admin_rows(),
    "Admin",
    FilterKind::Any,
    "Forum",
    1,
    false
)]
#[case::byte_exact_trailing_space(
    global_admin_rows(),
    "admin ",
    FilterKind::Any,
    "Forum",
    1,
    false
)]
fn sc1_row(
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

    let query_path = !where_(&rows, &query).is_empty();
    assert_eq!(query_path, expected, "query path (where_)");

    let cached_path = find_cached(&rows, &query);
    assert_eq!(cached_path, expected, "cached path (find_cached)");
}

/// Cross-path agreement on a mixed row set: for every holder row
/// combination and every name of interest, `where_` and `find_cached` must
/// return the same verdict (they are one predicate by construction; the gem
/// relies on this for cached-vs-queried consistency).
#[test]
fn query_and_cached_paths_agree_on_mixed_rows() {
    let forum_id = ResourceId::from(faker_rust::number::between(1, 1_000));
    let rows = vec![
        RoleRecord::global("admin"),
        RoleRecord::for_class("manager", "Forum"),
        RoleRecord::for_instance("moderator", "Forum", forum_id.clone()),
        RoleRecord::for_instance("vip", "Group", 9_i64),
    ];
    for name in ["admin", "manager", "moderator", "vip", "ghost"] {
        let name = RoleName::from(name);
        for kind in [
            FilterKind::Global,
            FilterKind::Class,
            FilterKind::Instance,
            FilterKind::Any,
        ] {
            for type_name in ["Forum", "Group"] {
                let query = make_query(&name, kind, type_name, &forum_id);
                assert_eq!(
                    !where_(&rows, &query).is_empty(),
                    find_cached(&rows, &query),
                    "paths diverged for name={name} kind={kind:?} type={type_name}",
                );
            }
        }
    }
}
