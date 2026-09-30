//! Pure semantics kernel - the gem's match ladder as zero-I/O predicates.
//!
//! Port of `rolify/lib/rolify/adapters/active_record/role_adapter.rb`:
//! `build_query` (lines 106-121) for the query path, `find_cached` /
//! `find_cached_strict` (lines 28-48 - byte-identical in the Mongoid
//! adapter, the purity proof), `where_strict` (lines 11-26), and `remove`
//! (lines 58-70) for the removal family.
//!
//! ## D-02 divergence (deliberate, documented)
//!
//! [`ResourceFilter::Any`] (the gem's `:any`) matches by name **including
//! global-role rows**: that is the gem's *database* path - `build_query:107`
//! short-circuits to `name = ?` before any scope condition. The gem's
//! *other* path (`role.rb`'s `new_record?`, in-memory) requires a resource
//! for `:any` and therefore excludes globals. The kernel follows the DB path
//! on purpose; the divergence is recorded in `.planning/parity-matrix.md`.

use crate::query::{ResourceFilter, RoleQuery};
use crate::role::{ResourceId, RoleName, RoleRecord};

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod ladder_tests;
#[cfg(test)]
mod strict_tests;
#[cfg(test)]
mod removal_tests;

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
///
/// # Example
///
/// ```
/// use rolify_core::kernel::where_;
/// use rolify_core::query::{ResourceFilter, RoleQuery};
/// use rolify_core::role::{ResourceId, RoleName, RoleRecord};
///
/// let rows = [
///     RoleRecord::global("admin"),
///     RoleRecord::for_class("manager", "Forum"),
/// ];
/// let name = RoleName::from("admin");
/// let id = ResourceId::from(1_i64);
/// // the global override: a class query matches the global row
/// let query = RoleQuery { name: &name, filter: ResourceFilter::Instance("Forum", &id) };
/// assert_eq!(where_(&rows, &query).len(), 1);
/// // reverse never holds
/// let wrong = RoleName::from("manager");
/// let query = RoleQuery { name: &wrong, filter: ResourceFilter::Global };
/// assert!(where_(&rows, &query).is_empty());
/// ```
#[must_use]
pub fn where_<'a>(rows: &'a [RoleRecord], query: &RoleQuery<'_>) -> Vec<&'a RoleRecord> {
    rows.iter()
        .filter(|record| record_matches(record, query))
        .collect()
}

/// One row against the non-strict ladder - every disjunct is name-gated.
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

/// Strict predicates (`where_strict`, role_adapter.rb:11-26): exact scope,
/// NO override ladder - a global row never satisfies a strict class query,
/// a class row never satisfies a strict instance query.
///
/// Ratified corner semantics (both unreachable through the gem's own gating,
/// which is why the gem contradicts itself on them - see
/// `.planning/parity-matrix.md`):
///
/// * `strict(Global)` = **exactly-global** (name AND both columns NULL).
///   The gem's AR `where_strict(nil)` degrades to name-only-any-scope
///   (lines 18-19 `else {}`) while `find_cached_strict(nil)` computes
///   `resource_type = "NilClass"` and matches nothing: internally
///   contradictory. We pin the semantically sane choice.
/// * `strict(Any)` = **name-only**. The gem's `where_strict(:any)` would
///   raise (`:any.id`) - dead code - and the strict gate never engages for
///   `Any` anyway (`resource != :any` at role.rb:26/48, finders.rb:4).
///
/// # Example
///
/// ```
/// use rolify_core::kernel::where_strict;
/// use rolify_core::query::{ResourceFilter, RoleQuery};
/// use rolify_core::role::{RoleName, RoleRecord};
///
/// let rows = [RoleRecord::global("admin"), RoleRecord::for_class("manager", "Forum")];
/// let manager = RoleName::from("manager");
/// let query = RoleQuery { name: &manager, filter: ResourceFilter::Class("Forum") };
/// assert_eq!(where_strict(&rows, &query).len(), 1);
///
/// // exact scope only: the global row is NOT seen by a strict class query
/// let admin = RoleName::from("admin");
/// let query = RoleQuery { name: &admin, filter: ResourceFilter::Class("Forum") };
/// assert!(where_strict(&rows, &query).is_empty());
/// ```
#[must_use]
pub fn where_strict<'a>(rows: &'a [RoleRecord], query: &RoleQuery<'_>) -> Vec<&'a RoleRecord> {
    rows.iter()
        .filter(|record| strict_record_matches(record, query))
        .collect()
}

/// One row against the strict predicates - exact scope, no overrides.
fn strict_record_matches(record: &RoleRecord, query: &RoleQuery<'_>) -> bool {
    let name_ok = record.name == *query.name;
    match &query.filter {
        ResourceFilter::Class(type_name) => name_ok && record.is_class_scoped_to(type_name),
        ResourceFilter::Instance(type_name, id) => {
            name_ok && record.is_instance_scoped_to(type_name, id)
        }
        // Deliberate corner divergence (parity-matrix entry): exactly-global.
        ResourceFilter::Global => name_ok && record.is_global(),
        // Deliberate corner divergence (parity-matrix entry): name-only.
        ResourceFilter::Any => name_ok,
    }
}

/// Cached (zero-I/O) membership predicate - the gem's `find_cached`
/// (role_adapter.rb:28-39), byte-identical across AR/Mongoid, which is the
/// proof this logic is pure kernel.
///
/// By construction `find_cached` agrees with [`where_`] on any row set
/// (the gem guarantees the cached and queried paths cannot drift apart);
/// the SC-1 test matrix pins that agreement row by row.
///
/// # Example
///
/// ```
/// use rolify_core::kernel::find_cached;
/// use rolify_core::query::RoleQuery;
/// use rolify_core::role::{RoleName, RoleRecord};
///
/// let rows = [RoleRecord::global("admin")];
/// let name = RoleName::from("admin");
/// assert!(find_cached(&rows, &RoleQuery::with_role(&name)));
/// ```
#[must_use]
pub fn find_cached(rows: &[RoleRecord], query: &RoleQuery<'_>) -> bool {
    !where_(rows, query).is_empty()
}

/// Cached strict membership predicate - `find_cached_strict`
/// (`role_adapter.rb:41-48`), same corners as [`where_strict`].
///
/// # Example
///
/// ```
/// use rolify_core::kernel::find_cached_strict;
/// use rolify_core::query::{ResourceFilter, RoleQuery};
/// use rolify_core::role::{RoleName, RoleRecord};
///
/// let rows = [RoleRecord::for_class("manager", "Forum")];
/// let name = RoleName::from("manager");
/// let query = RoleQuery { name: &name, filter: ResourceFilter::Class("Forum") };
/// assert!(find_cached_strict(&rows, &query));
/// ```
#[must_use]
pub fn find_cached_strict(rows: &[RoleRecord], query: &RoleQuery<'_>) -> bool {
    !where_strict(rows, query).is_empty()
}

/// Strict-mode gate - verified identical at the gem's three call sites
/// (`role.rb:26`, `role.rb:48`, `finders.rb:4`:
/// `strict_rolify && resource && resource != :any`).
///
/// Strict engages ONLY when a concrete `Class`/`Instance` resource is given;
/// never for `Global` (nil is falsy in Ruby), never for `Any`.
///
/// # Example
///
/// ```
/// use rolify_core::kernel::strict_engages;
/// use rolify_core::query::ResourceFilter;
///
/// assert!(strict_engages(true, &ResourceFilter::Class("Forum")));
/// assert!(!strict_engages(true, &ResourceFilter::Global));
/// assert!(!strict_engages(true, &ResourceFilter::Any));
/// assert!(!strict_engages(false, &ResourceFilter::Class("Forum")));
/// ```
#[must_use]
pub fn strict_engages(strict: bool, filter: &ResourceFilter<'_>) -> bool {
    strict && matches!(filter, ResourceFilter::Class(_) | ResourceFilter::Instance(..))
}

/// What a `remove_role(name, resource)` call sweeps (the gem's `remove`
/// conditions, role_adapter.rb:58-70).
///
/// This is a DISTINCT predicate family from the query ladder: removal
/// conditions are conjunctive (`name` / `name+type` / `name+type+id`),
/// never the override OR-ladder (Pitfall 8).
///
/// # Example
///
/// ```
/// use rolify_core::kernel::{RemovalTarget, removal_match};
/// use rolify_core::role::{ResourceId, RoleName, RoleRecord};
///
/// let name = RoleName::from("manager");
/// let class_row = RoleRecord::for_class("manager", "Forum");
/// let inst_row = RoleRecord::for_instance("manager", "Forum", 7_i64);
///
/// // TypeSweep hits class rows AND instance rows of that type
/// assert!(removal_match(&class_row, &name, &RemovalTarget::TypeSweep("Forum")));
/// assert!(removal_match(&inst_row, &name, &RemovalTarget::TypeSweep("Forum")));
/// // Exact hits the triple only
/// let id = ResourceId::from(7_i64);
/// assert!(removal_match(&inst_row, &name, &RemovalTarget::Exact("Forum", &id)));
/// assert!(!removal_match(&class_row, &name, &RemovalTarget::Exact("Forum", &id)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalTarget<'a> {
    /// `remove_role(name)` - deletes rows with this name at ALL scopes.
    NameOnly,
    /// `remove_role(name, Forum)` (a Class) - deletes rows with this name
    /// AND `resource_type = Forum`, **id unconstrained**: class rows AND
    /// instance rows of that type alike (the subtle sweep -
    /// `cond[:resource_id]` is simply not set for a Class argument).
    TypeSweep(&'a str),
    /// `remove_role(name, forum)` (an instance) - exact triple only.
    Exact(&'a str, &'a ResourceId),
}

/// Removal-family predicate: does `record` fall under the sweep of
/// `target` for role `name`? See [`RemovalTarget`] for the sweep rules.
///
/// # Example
///
/// ```
/// use rolify_core::kernel::{RemovalTarget, removal_match};
/// use rolify_core::role::{RoleName, RoleRecord};
///
/// let name = RoleName::from("manager");
/// let row = RoleRecord::for_instance("manager", "Forum", 1_i64);
/// assert!(removal_match(&row, &name, &RemovalTarget::NameOnly));
/// assert!(removal_match(&row, &name, &RemovalTarget::TypeSweep("Forum")));
/// assert!(!removal_match(&row, &name, &RemovalTarget::TypeSweep("Group")));
/// ```
#[must_use]
pub fn removal_match(record: &RoleRecord, name: &RoleName, target: &RemovalTarget<'_>) -> bool {
    let name_ok = record.name == *name;
    match target {
        RemovalTarget::NameOnly => name_ok,
        RemovalTarget::TypeSweep(type_name) => {
            name_ok && record.resource_type.as_deref() == Some(*type_name)
        }
        RemovalTarget::Exact(type_name, id) => {
            name_ok && record.is_instance_scoped_to(type_name, id)
        }
    }
}
