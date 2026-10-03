//! User-class finder assoc fns ([`RolifyUser`] statics): `with_role`
//! (strict-aware), `without_role`, `with_all_roles`, `with_any_roles`
//! (FIND-01..FIND-04, D-01..D-04).
//!
//! The statics themselves live on the consumer trait
//! ([`RolifyUser`](crate::user::RolifyUser), D-05); this module hosts
//! the pure composition they share, written ONCE (DRY):
//!
//! * [`finder_strictness`] - the finders.rb:4 strictness gate,
//!   consumed by `with_role`: `strict_rolify && resource && resource
//!   != :any`, resolved from the engine configuration (D-07) plus the
//!   query's filter through the kernel predicate. The `Any` filter
//!   never engages strict, even under a strict configuration (FIND-01).
//! * [`subtract_ids`] - `without_role` = `all_holders` minus the
//!   `with_role` matches (finders.rb:13, the `all_except` mirror).
//! * [`intersect_ids`] / [`union_dedup_into`] - the `with_all_roles`
//!   intersection (finders.rb:16-24, `&=` with early exit once the
//!   running intersection empties, finders.rb:21) and the
//!   `with_any_roles` union with dedup (finders.rb:26-32, `+` then
//!   `uniq`). The empty-input early returns live in the statics: an
//!   empty query list yields `[]` with ZERO store calls (finders.rb:
//!   37-48 over zero args - `parse_args` leaves `users = []`
//!   untouched), pinned by the `user_flow` probes.
//!
//! ## Parity notes (D-01..D-04, D-19)
//!
//! * **D-19 (return shape):** the gem returns the user-class relation;
//!   the port returns `Vec<ResourceId>` and the consumer filters its own
//!   table with `IN`. One round-trip per query through
//!   [`RoleStore::holders_where`] (D-01) - never a per-holder predicate
//!   walk.
//! * **D-02 (holder universe):** `all_holders` is the FULL holder table
//!   for the queried types - including holders that never received a
//!   role - NOT "holders that have roles". The store owns the table;
//!   `without_role` subtracts the matches from it.
//! * **D-03 (multi-class users):** every static passes
//!   `[Self::rolify_type()]` (the D-08 member) as the `holder_types`
//!   slice, so one store serves any number of user classes (`User` and
//!   `Customer` families alike); the store discriminates.
//! * **D-04 (ordering):** results are unordered sets. No ordering
//!   guarantee is implemented (zero ORDER BY burden on the adapters)
//!   and none is tested; the ported suite compares id lists as sets.
//!
//! [`RoleStore::holders_where`]: crate::store::RoleStore::holders_where

use crate::config::RolifyConfig;
use crate::query::RoleQuery;
use crate::role::ResourceId;

/// finders.rb:4 strictness for one query: `strict_rolify && resource &&
/// resource != :any` - delegated to the kernel predicate over the
/// configuration's `strict` flag (D-07: the engine owns the config).
/// `Global` (the gem's nil resource) and `Any` never engage strict.
#[must_use]
pub(crate) fn finder_strictness(config: &RolifyConfig, query: &RoleQuery<'_>) -> bool {
    config.strict_engages_for(&query.filter)
}

/// finders.rb:13 (`all_except`) set subtraction: the `universe` entries
/// absent from `remove` - `without_role` = `all_holders` minus the
/// `with_role` matches.
#[must_use]
pub(crate) fn subtract_ids(universe: &[ResourceId], remove: &[ResourceId]) -> Vec<ResourceId> {
    universe
        .iter()
        .filter(|holder| !remove.contains(*holder))
        .cloned()
        .collect()
}

/// finders.rb:16-24 (`&=`) set intersection: the `running` entries also
/// present in `incoming`. Callers early-exit once the running
/// intersection empties (finders.rb:21) before fetching further queries.
#[must_use]
pub(crate) fn intersect_ids(running: &[ResourceId], incoming: &[ResourceId]) -> Vec<ResourceId> {
    running
        .iter()
        .filter(|holder| incoming.contains(*holder))
        .cloned()
        .collect()
}

/// finders.rb:26-32 (`+` then `uniq`) set union: append every `incoming`
/// holder not already in `accumulated`, so one holder matching several
/// queries appears exactly once.
pub(crate) fn union_dedup_into(accumulated: &mut Vec<ResourceId>, incoming: &[ResourceId]) {
    for holder in incoming {
        if !accumulated.contains(holder) {
            accumulated.push(holder.clone());
        }
    }
}
