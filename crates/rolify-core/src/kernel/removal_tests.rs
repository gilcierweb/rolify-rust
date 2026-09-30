//! Removal-condition family - the gem's `remove` conditions
//! (role_adapter.rb:58-70) as conjunctive matches (NOT the query ladder,
//! Pitfall 8).
//!
//! | removal target | deletes rows matching |
//! |---|---|
//! | `NameOnly` | name at ALL scopes (the `-2` cross-scope sweep) |
//! | `TypeSweep(T)` | name AND `resource_type = T`, id UNCONSTRAINED (class rows AND instance rows of T) |
//! | `Exact(T, id)` | exact triple only |

use super::{RemovalTarget, removal_match};
use crate::role::{ResourceId, RoleName, RoleRecord};

fn sample_rows() -> Vec<RoleRecord> {
    vec![
        RoleRecord::global("manager"),
        RoleRecord::for_class("manager", "Forum"),
        RoleRecord::for_instance("manager", "Forum", 1_i64),
        RoleRecord::for_instance("manager", "Forum", 2_i64),
        RoleRecord::for_class("manager", "Group"),
        RoleRecord::for_instance("manager", "Group", 3_i64),
        RoleRecord::global("auditor"),
        RoleRecord::for_instance("auditor", "Forum", 1_i64),
    ]
}

fn surviving(rows: &[RoleRecord], name: &str, target: RemovalTarget<'_>) -> Vec<RoleRecord> {
    let name = RoleName::from(name);
    rows.iter()
        .filter(|record| !removal_match(record, &name, &target))
        .cloned()
        .collect()
}

/// `NameOnly` sweeps every scope row of that name, nothing else.
#[test]
fn name_only_sweeps_all_scopes_of_that_name() {
    let left = surviving(&sample_rows(), "manager", RemovalTarget::NameOnly);
    assert_eq!(left.len(), 2);
    assert!(left.iter().all(|record| record.name == RoleName::from("auditor")));
}

/// The subtle sweep (plan-pinned): a Class argument removes the class row
/// AND every instance row of that type - the gem never sets
/// `cond[:resource_id]` for a Class (role_adapter.rb:56-68).
#[test]
fn type_sweep_removes_class_and_instance_rows_of_the_type() {
    let left = surviving(&sample_rows(), "manager", RemovalTarget::TypeSweep("Forum"));

    let forum_rows_left = left
        .iter()
        .filter(|record| record.resource_type.as_deref() == Some("Forum"))
        .count();
    assert_eq!(forum_rows_left, 1, "only the unrelated-auditor Forum row may survive");

    // The Group-scoped manager rows survive untouched.
    assert!(left.iter().any(|record| record.is_class_scoped_to("Group")));
}

/// Plan-mandated pin: an instance row (`Some("Forum")`, `Some(1)`) IS hit
/// by `TypeSweep("Forum")`; a `Some("Group")` row is NOT.
#[test]
fn type_sweep_hits_instance_row_same_type_and_misses_other_type() {
    let name = RoleName::from("manager");
    let forum_instance = RoleRecord::for_instance("manager", "Forum", 1_i64);
    let group_class = RoleRecord::for_class("manager", "Group");

    assert!(removal_match(&forum_instance, &name, &RemovalTarget::TypeSweep("Forum")));
    assert!(!removal_match(&group_class, &name, &RemovalTarget::TypeSweep("Forum")));
}

/// `Exact` removes exactly one triple.
#[test]
fn exact_sweeps_the_exact_triple_only() {
    let forum_one = ResourceId::from(1_i64);
    let left = surviving(
        &sample_rows(),
        "manager",
        RemovalTarget::Exact("Forum", &forum_one),
    );
    assert_eq!(left.len(), 7);
    assert!(
        !left.iter().any(|record| record.name == RoleName::from("manager")
            && record.is_instance_scoped_to("Forum", &forum_one)),
        "the exact manager/Forum#1 row must be swept"
    );
    // Class row of the same type survives (Exact is not TypeSweep).
    assert!(left.iter().any(|record| record.is_class_scoped_to("Forum")));
    // Instance row with another id survives.
    assert!(
        left.iter()
            .any(|record| record.is_instance_scoped_to("Forum", &ResourceId::from(2_i64)))
    );
}

/// Byte-exact names here too: "Manager" removes nothing of "manager".
#[test]
fn removal_is_name_byte_exact() {
    let left = surviving(&sample_rows(), "Manager", RemovalTarget::NameOnly);
    assert_eq!(left.len(), sample_rows().len());
}
