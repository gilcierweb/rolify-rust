//! rolify-test - test-support crate for the rolify workspace.
//!
//! [`InMemoryStore`] is the reference [`RoleStore`]/[`ResourceStore`]
//! implementation (TEST-03): role rows live in a `Vec`, the join table is a
//! `Vec<(holder, role)>` link list, and a resource registry backs the finder
//! contract. **Every** matching decision delegates to the pure kernel, so
//! cached-vs-queried consistency is provable by construction. No Docker, no
//! I/O.
//!
//! Orphan-rule note: the `Sealed` + SPI impls below are legal because
//! `InMemoryStore` is local to this crate - the same pattern every backend
//! adapter crate follows (Pitfall 4: never impl core traits for foreign
//! types).
//!
//! ## The `suite` feature (TEST-01, D-11)
//!
//! The ported parity suite lives behind the non-default `suite` feature
//! (`dep:rstest` + `dep:faker-rust`): adapters enable it through
//! `dev-dependency rolify-test = { features = ["suite"] }` and bind their
//! backend with one `parity_suite!` line. `faker-rust` is an optional NORMAL
//! dependency (not a dev-dependency) because dev-dependencies do not
//! propagate: the fixture seeding code must compile inside the adapters'
//! own test binaries (manifests carry no comments, so the rationale lives
//! here).
//!
//! ## Consumer mock quickstart
//!
//! [`InMemoryStore`] doubles as the published consumer mock: `Conn` is
//! `()`, so a test spins it up with no handle, seeds it through
//! [`InMemoryStore::grant`] / [`InMemoryStore::insert`] /
//! [`InMemoryStore::register_resource`] / [`InMemoryStore::register_holder`],
//! and asserts through [`RoleAssertions`]. The surface ships as-is with no
//! storage redesign: the same rows, links, and registries the reference
//! implementation validates.
//!
//! ```rust
//! use rolify_core::role::{ResourceId, RoleRecord};
//! use rolify_core::store::RoleStore;
//! use rolify_test::InMemoryStore;
//!
//! # #[cfg(not(feature = "is_sync"))]
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() { usage().await; }
//! # #[cfg(feature = "is_sync")]
//! # fn main() { usage(); }
//! #
//! #[maybe_async::maybe_async]
//! async fn usage() {
//!     let mut store = InMemoryStore::new();
//!     let holder = ResourceId::from(1_i64);
//!     store.grant(&holder, RoleRecord::global("admin"));
//!     let held = store.roles_of(&mut (), &holder).await.unwrap();
//!     assert_eq!(held, vec![RoleRecord::global("admin")]);
//! }
//! ```

// `Future` is named in the impl signatures in async mode only; maybe-async
// strips the `impl Future` return type in `is_sync` mode. The cfg gate can
// only see this crate's own feature, while the macro keys on the unified
// `maybe-async/is_sync` package feature: in graphs where another member
// (for example rolify-diesel's default `sync`) unifies sync on, the import
// is present yet unused, so the allow keeps those builds warning-free.
#[cfg(not(feature = "is_sync"))]
#[allow(unused_imports)]
use core::future::Future;

use maybe_async::maybe_async;
use rolify_core::RolifyError;
use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
use rolify_core::kernel::{self, RemovalTarget};
use rolify_core::query::RoleQuery;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::{
    RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn, Sealed,
};

#[cfg(feature = "suite")]
pub mod backend;
#[cfg(feature = "suite")]
pub mod ddl;
#[cfg(feature = "suite")]
pub mod fixtures;
#[cfg(feature = "suite")]
pub mod suite;

// Always compiled (D-07): the consumer assertion surface ships with the
// mock, no feature flag.
pub mod matchers;
pub use matchers::RoleAssertions;

// Always compiled (D-12): the deterministic fixture builders ship with
// the mock, no feature flag (the faker generators inside stay opt-in).
// Bare declaration only for now; the crate-root re-export lands with the
// proof wiring once the builder functions exist.
pub mod builders;

/// In-memory [`RoleStore`] + [`ResourceStore`] - the workspace's reference
/// implementation, validation target, and published consumer mock.
///
/// Mock framing: `Conn` is `()` (no external connection to manage), so a
/// test builds the mock with [`InMemoryStore::new`] and seeds it through
/// [`InMemoryStore::grant`] (insert plus link in one step),
/// [`InMemoryStore::insert`] (verbatim row, no dedupe, no links),
/// [`InMemoryStore::register_resource`] (finder targets), and
/// [`InMemoryStore::register_holder`] (holder-table rows feeding the finder
/// reads). Rows follow the gem `roles` table shape (`name`,
/// `resource_type`, `resource_id` as [`RoleRecord`]). The surface ships
/// as-is with no storage redesign: the same rows, links, and registries
/// the reference implementation validates. Pair it with
/// [`RoleAssertions`] for the assertion half.
///
/// * `rows` - the role rows table (`find_or_create_by`'s dedupe target).
/// * `links` - the join table: `(holder, role-row)` pairs (the gem's
///   `users_roles` HABTM join).
/// * `registry` - fixture resources for the `ResourceStore` finder contract.
/// * `holders` - the fixture holder registry (the in-memory `users`
///   table): `(rolify_type, holder id)` pairs feeding the 02-07 finder
///   reads [`RoleStore::holders_where`] / [`RoleStore::all_holders`].
///
/// `Conn` is `()`: there is no external connection to manage.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    rows: Vec<RoleRecord>,
    links: Vec<(ResourceId, RoleRecord)>,
    registry: Vec<ResourceKey>,
    holders: Vec<(String, ResourceId)>,
}

impl InMemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a role row verbatim (test-setup primitive - no dedupe, no
    /// links; prefer `RoleStore::find_or_create_by` + `RoleStore::add` (or a
    /// consumer's `add_role`) for behavior-representative setup).
    pub fn insert(&mut self, record: RoleRecord) {
        self.rows.push(record);
    }

    /// Setup shortcut: insert the row if new AND link it to `holder`
    /// (sync by design - mode-agnostic fixture code, not part of the SPI).
    pub fn grant(&mut self, holder: &ResourceId, record: RoleRecord) {
        if !self.rows.contains(&record) {
            self.rows.push(record.clone());
        }
        let linked = self
            .links
            .iter()
            .any(|(owner, row)| owner == holder && row == &record);
        if !linked {
            self.links.push((holder.clone(), record));
        }
    }

    /// All role rows, in insertion order.
    #[must_use]
    pub fn rows(&self) -> &[RoleRecord] {
        &self.rows
    }

    /// Empty the role rows and links, keeping the fixture registries
    /// intact (resources and holders are schema-shaped fixtures, not
    /// role state) - the port of the suite preamble
    /// `role_class.destroy_all` plus `roles = []` (`shared_contexts.rb:14-15`).
    pub fn clear(&mut self) {
        self.rows.clear();
        self.links.clear();
    }

    /// Number of role rows (assertion helper for tests).
    #[must_use]
    pub fn assertion_len(&self) -> usize {
        self.rows.len()
    }

    /// Number of links in the join table (all holders).
    #[must_use]
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    /// Number of links for one holder.
    #[must_use]
    pub fn link_count_for(&self, holder: &ResourceId) -> usize {
        self.links
            .iter()
            .filter(|(owner, _)| owner == holder)
            .count()
    }

    /// Register a fixture resource ([`ResourceStore`] finder target).
    pub fn register_resource(&mut self, key: ResourceKey) {
        if !self.registry.contains(&key) {
            self.registry.push(key);
        }
    }

    /// Register a fixture holder: a `users`-table row for the 02-07
    /// finder reads - the holder's type discriminator (the D-08
    /// `rolify_type` literal) plus its stringified primary key. Feeds
    /// [`RoleStore::all_holders`] and the `holder_types` filter of
    /// [`RoleStore::holders_where`]; idempotent per `(type, id)` pair.
    pub fn register_holder(
        &mut self,
        holder_type: impl Into<String>,
        holder_id: impl Into<ResourceId>,
    ) {
        let holder_type = holder_type.into();
        let holder_id = holder_id.into();
        if !self
            .holders
            .iter()
            .any(|(known_type, known_id)| known_type == &holder_type && known_id == &holder_id)
        {
            self.holders.push((holder_type, holder_id));
        }
    }

    /// Role rows linked to `holder` (one entry per link; the link guard
    /// makes duplicates impossible through the SPI).
    fn holder_rows(&self, holder: &ResourceId) -> Vec<RoleRecord> {
        self.links
            .iter()
            .filter(|(owner, _)| owner == holder)
            .map(|(_, row)| row.clone())
            .collect()
    }
}

impl Sealed for InMemoryStore {}

#[maybe_async(AFIT)]
impl RoleStore for InMemoryStore {
    type Conn = ();
    type Error = RolifyError;

    /// Non-strict ladder over the holder's linked rows - delegates the
    /// decision to the pure kernel (`kernel::where_`).
    fn where_(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let held = self.holder_rows(holder);
        let rows: Vec<RoleRecord> = kernel::where_(&held, query).into_iter().cloned().collect();
        async move { Ok(rows) }
    }

    /// Strict exact-scope predicate - `kernel::where_strict`.
    fn where_strict(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let held = self.holder_rows(holder);
        let rows: Vec<RoleRecord> = kernel::where_strict(&held, query)
            .into_iter()
            .cloned()
            .collect();
        async move { Ok(rows) }
    }

    /// OR-joined multi-query (the gem's `build_conditions` `join(' OR ')`) -
    /// ONE pass over the holder's rows, deduped. Never N sequential checks.
    fn where_any(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        queries: &[RoleQuery<'_>],
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let held = self.holder_rows(holder);
        let mut matched: Vec<RoleRecord> = Vec::new();
        for query in queries {
            for row in kernel::where_(&held, query) {
                if !matched.contains(row) {
                    matched.push(row.clone());
                }
            }
        }
        async move { Ok(matched) }
    }

    /// Gem `find_or_create_by` (`role_adapter.rb`) - level-1 dedupe on the
    /// exact `(name, resource_type, resource_id)` triple.
    fn find_or_create_by(
        &mut self,
        _conn: &mut Self::Conn,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send {
        let (resource_type, resource_id) = match scope {
            ResourceRef::Global => (None, None),
            ResourceRef::Class(type_name) => (Some(type_name.to_owned()), None),
            ResourceRef::Instance(type_name, id) => (Some(type_name.to_owned()), Some(id.clone())),
        };
        let record = if let Some(existing) = self.rows.iter().find(|record| {
            record.name == *name
                && record.resource_type == resource_type
                && record.resource_id == resource_id
        }) {
            existing.clone()
        } else {
            let record = RoleRecord::new(name.clone(), resource_type, resource_id);
            self.rows.push(record.clone());
            record
        };
        async move { Ok(record) }
    }

    /// Gem `add` (`relation.roles << role unless include?`) - level-2
    /// link-guard dedupe. Returns `true` when a new link was created.
    fn add(
        &mut self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        role: &RoleRecord,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let already_linked = self
            .links
            .iter()
            .any(|(owner, row)| owner == holder && row == role);
        if !already_linked {
            self.links.push((holder.clone(), role.clone()));
        }
        async move { Ok(!already_linked) }
    }

    /// Gem `remove` (`role_adapter.rb:58-70`): delete the holder's links
    /// matching name+target (kernel `removal_match` - the conjunctive sweep
    /// family), then the `remove_role_if_empty` cleanup: rows whose last
    /// link (across ALL holders) vanished get deleted.
    fn remove(
        &mut self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        name: &RoleName,
        target: RemovalTarget<'_>,
        remove_role_if_empty: bool,
    ) -> impl Future<Output = Result<RemovalOutcome, Self::Error>> + Send {
        let affected: Vec<RoleRecord> = self
            .links
            .iter()
            .filter(|(owner, row)| owner == holder && kernel::removal_match(row, name, &target))
            .map(|(_, row)| row.clone())
            .collect();

        let before = self.links.len();
        self.links
            .retain(|(owner, row)| !(owner == holder && kernel::removal_match(row, name, &target)));
        let removed_links = before - self.links.len();

        let mut removed_roles = Vec::new();
        if remove_role_if_empty {
            for record in affected {
                let still_linked = self.links.iter().any(|(_, row)| row == &record);
                if !still_linked && self.rows.contains(&record) {
                    self.rows.retain(|row| row != &record);
                    removed_roles.push(record);
                }
            }
        }

        async move {
            Ok(RemovalOutcome {
                removed_links,
                removed_roles,
            })
        }
    }

    /// Gem `exists?` (`relation.where("<column> IS NOT NULL")`) over the
    /// holder's linked rows.
    fn exists(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        column: ScopeColumn,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let found = self
            .links
            .iter()
            .filter(|(owner, _)| owner == holder)
            .any(|(_, row)| match column {
                ScopeColumn::ResourceType => row.resource_type.is_some(),
                ScopeColumn::ResourceId => row.resource_id.is_some(),
            });
        async move { Ok(found) }
    }

    /// All role rows linked to `holder` (the `user.roles` association read).
    fn roles_of(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let rows = self.holder_rows(holder);
        async move { Ok(rows) }
    }

    /// The D-15 catalog read over the role rows plus the join table, with
    /// the filter semantics documented on the SPI member.
    fn roles_matching(
        &self,
        _conn: &mut Self::Conn,
        query: &RoleCatalogQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let found = self
            .rows
            .iter()
            .filter(|row| {
                let type_hit = row
                    .resource_type
                    .as_deref()
                    .is_some_and(|row_type| query.types.contains(&row_type));
                if !type_hit {
                    return false;
                }
                if let Some(wanted) = query.name {
                    if row.name != *wanted {
                        return false;
                    }
                }
                match &query.scope {
                    CatalogScope::ClassAndInstance => {}
                    CatalogScope::ClassOnly => {
                        if row.resource_id.is_some() {
                            return false;
                        }
                    }
                    CatalogScope::InstanceOnly { resource_id } => {
                        match (&row.resource_id, resource_id) {
                            (Some(row_id), Some(wanted)) if row_id == *wanted => {}
                            // `None` is the documented "every instance
                            // row in types" query: any instance row
                            // matches, class rows fall through to the
                            // reject arm below.
                            (Some(_), None) => {}
                            _ => return false,
                        }
                    }
                }
                if let Some(holder) = query.holder {
                    let linked = self
                        .links
                        .iter()
                        .any(|(owner, linked_row)| owner == holder && linked_row == *row);
                    if !linked {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        async move { Ok(found) }
    }

    /// D-01 finder read over the join table: holder ids whose linked
    /// rows satisfy the kernel ladder (`where_strict` semantics when
    /// `strict`, `where_` semantics otherwise), restricted to holders
    /// registered under one of `holder_types`. A linked holder absent
    /// from the registry is invisible - the users-table INNER JOIN
    /// drops dangling join rows. Holder ids are deduped: one entry per
    /// holder no matter how many rows matched.
    fn holders_where(
        &self,
        _conn: &mut Self::Conn,
        holder_types: &[&str],
        query: &RoleQuery<'_>,
        strict: bool,
    ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
        let mut matched: Vec<ResourceId> = Vec::new();
        for (holder, row) in &self.links {
            if matched.contains(holder) {
                continue;
            }
            let registered = self.holders.iter().any(|(known_type, known_id)| {
                known_id == holder && holder_types.contains(&known_type.as_str())
            });
            if !registered {
                continue;
            }
            let held = core::slice::from_ref(row);
            let hit = if strict {
                !kernel::where_strict(held, query).is_empty()
            } else {
                !kernel::where_(held, query).is_empty()
            };
            if hit {
                matched.push(holder.clone());
            }
        }
        async move { Ok(matched) }
    }

    /// D-02 finder read: every holder id registered under one of
    /// `holder_types` - the FULL fixture `users` table, including
    /// never-rolificated holders (ids with no links return too).
    fn all_holders(
        &self,
        _conn: &mut Self::Conn,
        holder_types: &[&str],
    ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
        let found = self
            .holders
            .iter()
            .filter(|(known_type, _)| holder_types.contains(&known_type.as_str()))
            .map(|(_, holder_id)| holder_id.clone())
            .collect();
        async move { Ok(found) }
    }

    /// Resource-scoped role deletion (OQ1 / SC-3 / D-10): delete exactly the
    /// role rows matching `(resource_type, resource_id)` and return the count.
    /// Join rows for those roles are also removed (cascade simulation).
    fn remove_roles_for_scope(
        &mut self,
        _conn: &mut Self::Conn,
        resource_type: &str,
        resource_id: &ResourceId,
    ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
        let rt = Some(resource_type.to_owned());
        let rid = Some(resource_id.clone());

        // Find matching role records
        let matching: Vec<RoleRecord> = self
            .rows
            .iter()
            .filter(|row| row.resource_type == rt && row.resource_id == rid)
            .cloned()
            .collect();

        let count = matching.len();

        // Remove those role rows
        self.rows
            .retain(|row| !(row.resource_type == rt && row.resource_id == rid));

        // Remove associated links (cascade)
        for role in &matching {
            self.links.retain(|(_, row)| row != role);
        }

        async move { Ok(count) }
    }
}

#[maybe_async(AFIT)]
impl ResourceStore for InMemoryStore {
    type Conn = ();
    type Error = RolifyError;

    /// Gem `resources_find`: registered resources of the STI family `types`
    /// that hold `name` at class scope or at their own instance scope.
    fn resources_find(
        &self,
        _conn: &mut Self::Conn,
        types: &[&str],
        name: &RoleName,
    ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
        let found = self
            .registry
            .iter()
            .filter(|key| types.contains(&key.resource_type.as_str()))
            .filter(|key| {
                self.rows.iter().any(|row| {
                    row.name == *name
                        && row.resource_type.as_deref() == Some(key.resource_type.as_str())
                        && (row.resource_id.is_none()
                            || row.resource_id.as_ref() == Some(&key.resource_id))
                })
            })
            .cloned()
            .collect();
        async move { Ok(found) }
    }

    /// Gem `in` (`resource_adapter.rb`): among `candidates`, resources where
    /// `holder` has any of `names` with `resource_id` NULL (class/global
    /// scope) or equal to the resource's id. Mirrors the gem's SQL, which
    /// carries no `resource_type` condition on this path (documented
    /// here; Phase-2 finder work owns any tightening).
    fn in_list(
        &self,
        _conn: &mut Self::Conn,
        candidates: &[ResourceKey],
        holder: &ResourceId,
        names: &[RoleName],
    ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
        let found = candidates
            .iter()
            .filter(|key| {
                self.links.iter().any(|(owner, row)| {
                    owner == holder
                        && names.contains(&row.name)
                        && (row.resource_id.is_none()
                            || row.resource_id.as_ref() == Some(&key.resource_id))
                })
            })
            .cloned()
            .collect();
        async move { Ok(found) }
    }
}

#[cfg(test)]
mod tests;
