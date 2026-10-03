//! Adapter SPI - the soft-sealed [`RoleStore`] / [`ResourceStore`] traits.
//!
//! Port of the gem's abstract adapter contract
//! (`rolify/lib/rolify/adapters/base.rb`, `RoleAdapterBase` /
//! `ResourceAdapterBase`) into sealed per-backend traits. The surface is
//! traced - never invented: `where`, `where_strict`, `find_or_create_by`,
//! `add`, `remove`, `exists?` (`RoleAdapterBase` + the concrete adapters'
//! `where_strict`), plus `resources_find` and `in` (here `in_list`, since
//! `in` is a Rust keyword) from `ResourceAdapterBase`. The gem defines no
//! user-ids lookup or resource-find operation, so none exists here either -
//! anything a Phase-2 finder needs gets its own parity-matrix entry.
//!
//! Contract notes:
//!
//! * **Not dyn-compatible, on purpose.** `type Conn` / `type Error` make
//!   these traits static-dispatch only (E0038) - adapters are generic
//!   parameters; a boxed dyn form is intentionally impossible.
//! * **Soft seal.** `Sealed` is a doc-hidden public marker (a hard
//!   `sealed`-crate seal cannot span crates; `embedded-hal` 1.0 precedent).
//!   Implementations exist only inside this workspace's crates
//!   (`rolify-test`, then Phase-3+ adapters). The orphan rule (E0117) means
//!   adapters impl the SPI for **local wrapper types**, never for foreign
//!   types like `sqlx::PgPool`.
//! * **One mode per build graph.** A dependent crate picks async (default)
//!   or `is_sync` via feature unification; within-crate `compile_error!`
//!   guards for mutually-exclusive adapter features land with
//!   `rolify-diesel` (Pitfall 7).

// `Future` is named in the trait signatures in async mode only; maybe-async
// strips the `impl Future` return type in `is_sync` mode.
#[cfg(not(feature = "is_sync"))]
use core::future::Future;

use crate::catalog::RoleCatalogQuery;
use crate::error::RolifyError;
use crate::kernel::RemovalTarget;
use crate::query::RoleQuery;
use crate::resource::ResourceRef;
use crate::role::{ResourceId, RoleName, RoleRecord};

#[doc(hidden)]
pub mod seal {
    /// Marker supertrait gating SPI impls (soft seal - see module docs).
    pub trait Sealed {}
}
pub use seal::Sealed;

/// Which scope column [`RoleStore::exists`] inspects (the gem's `column`
/// argument in `exists?(relation, column)` - `role_adapter.rb`).
///
/// # Example
///
/// ```
/// use rolify_core::store::ScopeColumn;
///
/// assert_eq!(ScopeColumn::ResourceType, ScopeColumn::ResourceType);
/// assert_ne!(ScopeColumn::ResourceType, ScopeColumn::ResourceId);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeColumn {
    /// `resource_type IS NOT NULL` - any scoped (non-global) row.
    ResourceType,
    /// `resource_id IS NOT NULL` - any instance-scoped row.
    ResourceId,
}

/// Report of a [`RoleStore::remove`] call.
///
/// Mirrors what the gem's `remove` returns conceptually (the affected
/// roles) plus the `remove_role_if_empty` cleanup it performs inline when
/// the flag is set (role_adapter.rb:58-70: after deleting the join rows,
/// each role whose last membership vanished is destroyed). Plain data -
/// adapters construct it, so it is intentionally NOT `#[non_exhaustive]`.
///
/// # Example
///
/// ```
/// use rolify_core::store::RemovalOutcome;
///
/// let outcome = RemovalOutcome::default();
/// assert_eq!(outcome.removed_links, 0);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovalOutcome {
    /// How many user-to-role links were deleted.
    pub removed_links: usize,
    /// Role rows deleted by the `remove_role_if_empty` sweep (empty only
    /// when the flag was false or nothing became empty).
    pub removed_roles: Vec<RoleRecord>,
}

/// The role-row storage SPI that backend adapters implement.
///
/// # Example
///
/// The reference implementation is [`rolify_test::InMemoryStore`]. All
/// examples that call the dual-mode SPI live in the phase-1 integration
/// suites (`tests/user_flow.rs`, `rolify-test/tests/spi_integration.rs`),
/// which run identically in both modes.
///
/// [`rolify_test::InMemoryStore`]: https://docs.rs/rolify-test
#[maybe_async::maybe_async(AFIT)]
pub trait RoleStore: Sealed + Send + Sync + 'static {
    /// Backend connection/pool handle. `Send` is required because provided
    /// methods hold `&mut Conn` across `.await` points (SPI futures are
    /// `Send`-bound by contract) - every targeted backend connection type
    /// (diesel/sqlx/sea-orm/mongodb) is `Send`.
    type Conn: Send;

    /// Backend error - one `thiserror` enum per adapter, converting from
    /// [`RolifyError`] so core-level failures (callback veto, invalid config)
    /// flow through adapter errors uniformly.
    type Error: core::error::Error + Send + Sync + From<RolifyError> + 'static;

    /// Gem `where` (role_adapter.rb:106-121 via `build_query`) - the
    /// non-strict three-disjunct ladder; semantics fixed by
    /// [`crate::kernel::where_`], scoped to `holder`'s role rows.
    fn where_(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send;

    /// Gem `where_strict` (role_adapter.rb:11-26) - exact scope, no
    /// overrides; semantics fixed by [`crate::kernel::where_strict`].
    fn where_strict(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send;

    /// Gem `where` with several conditions OR-joined
    /// (`build_conditions`, role_adapter.rb:88-104 - `join(' OR ')`): ONE
    /// round-trip for "any of these queries" - the counterpart of
    /// `has_any_role?`'s persisted path, never N sequential checks.
    fn where_any(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        queries: &[RoleQuery<'_>],
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send;

    /// Gem `find_or_create_by` (`role_adapter.rb`) - role-row dedupe on the
    /// exact `(name, resource_type, resource_id)` triple (idempotent-add
    /// level 1; the link-guard level 2 lives in [`RoleStore::add`]).
    fn find_or_create_by(
        &mut self,
        conn: &mut Self::Conn,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send;

    /// Gem `add` (role_adapter.rb:52-54):
    /// `relation.roles << role unless relation.roles.include?(role)` -
    /// line-idempotent link creation (level-2 dedupe).
    fn add(
        &mut self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        role: &RoleRecord,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;

    /// Gem `remove` (role_adapter.rb:58-70): delete the holder's links whose
    /// role matches `name` + `target`'s conjunctive sweep
    /// ([`crate::kernel::removal_match`]); when `remove_role_if_empty` is
    /// set, also delete each role row whose last link just vanished
    /// (`role.destroy if ... limit(1).empty?`).
    fn remove(
        &mut self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        name: &RoleName,
        target: RemovalTarget<'_>,
        remove_role_if_empty: bool,
    ) -> impl Future<Output = Result<RemovalOutcome, Self::Error>> + Send;

    /// Gem `exists?` (role_adapter.rb:72-74):
    /// `relation.where("<column> IS NOT NULL")` over the holder's rows.
    fn exists(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        column: ScopeColumn,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;

    /// All role rows linked to `holder` - the port of reading the
    /// `user.roles` association (the gem reaches it directly; adapters own
    /// the join table, so the surface must expose it). Feeds `roles_name`
    /// / `only_has_role?` (role.rb:77-90).
    fn roles_of(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send;

    /// User-class finder read (D-01, `adapter.scope` behind
    /// `finders.rb:5-9`): the holder ids whose LINKED role rows match
    /// `query` - ONE round-trip per call (the store composes the join
    /// itself; there is never one store call per holder).
    ///
    /// * `holder_types` - the holder-side type registry filter (D-03,
    ///   the `types: &[&str]` precedent of
    ///   [`ResourceStore::resources_find`]): only holders registered
    ///   under one of these types can appear; an empty slice matches
    ///   nothing. One store serves several user classes (`User` and
    ///   `Customer` alike) through this slice.
    /// * `strict` - selects the ladder the matching rows must satisfy:
    ///   the SAME kernel predicates behind [`RoleStore::where_`] and
    ///   [`RoleStore::where_strict`] (`where_` semantics when false,
    ///   `where_strict` semantics when true). The caller resolves the
    ///   flag from its configuration plus the query's filter
    ///   (finders.rb:4); the store never re-derives it.
    ///
    /// The gem returns the user-class relation; the SPI returns ids
    /// (D-19, see [`crate::finders`]): the consumer filters its own
    /// table with `IN`. Results are unordered sets (D-04) - each holder
    /// id appears at most once no matter how many rows matched.
    fn holders_where(
        &self,
        conn: &mut Self::Conn,
        holder_types: &[&str],
        query: &RoleQuery<'_>,
        strict: bool,
    ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send;

    /// The FULL holder table for the given types - the holder universe
    /// D-02 pins: INCLUDING never-rolificated holders, NOT "holders
    /// that have roles". The gem reads `User.all` behind `all_except`
    /// (`finders.rb:13`); `without_role` subtracts the
    /// [`RoleStore::holders_where`] matches from this list.
    ///
    /// Same `holder_types` registry filter (D-03) as
    /// [`RoleStore::holders_where`]; results are unordered (D-04).
    fn all_holders(
        &self,
        conn: &mut Self::Conn,
        holder_types: &[&str],
    ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send;

    /// Gem `find_roles` catalog branch (`resource_adapter.rb:6-11`) - the
    /// ONE catalog read per adapter (D-15). Filter semantics, documented
    /// once here and mirrored by every adapter:
    ///
    /// * `types` - `resource_type IN types` (empty slice matches nothing).
    ///   Global rows (`resource_type: None`) NEVER match: the gem's join
    ///   constrains the type column, so resource-side reads structurally
    ///   exclude globals (`resource_adapter.rb:8` asymmetry).
    /// * `name` - byte-exact match when `Some`, no name constraint when
    ///   `None` (the gem's `role_name != :any` branch).
    /// * `scope` - `ClassAndInstance` accepts class and instance rows
    ///   within `types`; `ClassOnly` accepts only `resource_id IS NULL`
    ///   rows; `InstanceOnly` accepts only rows whose `resource_id` equals
    ///   the payload id within `types`.
    /// * `holder` - when `Some`, only rows LINKED to that holder (the
    ///   join); mirrors the `user.roles` branch at `resource_adapter.rb:7`.
    ///   When `None`, linkage is ignored (the `role_class` branch).
    fn roles_matching(
        &self,
        conn: &mut Self::Conn,
        query: &RoleCatalogQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send;

    /// Resource-scoped role deletion (OQ1 / SC-3 / D-10): delete exactly the
    /// role rows matching `(resource_type, resource_id)` — i.e. instance-bound
    /// roles for a specific resource — and return the count of deleted rows.
    /// Join rows vanish via FK `ON DELETE CASCADE` (D-10). Class-scoped rows
    /// of the same type (where `resource_id = ''`) are NOT touched — the WHERE
    /// is the exact scope pair, never a type-only sweep (that semantics lives
    /// in `RemovalTarget::TypeSweep`, a different path).
    ///
    /// Mirrors the gem's `dependent: :destroy` behavior on the resource side
    /// (`rolify/spec/rolify/resource_spec.rb:507-510`: `expect { subject.destroy }.to change { Role.count }.by(-2)`).
    fn remove_roles_for_scope(
        &mut self,
        conn: &mut Self::Conn,
        resource_type: &str,
        resource_id: &ResourceId,
    ) -> impl Future<Output = Result<usize, Self::Error>> + Send;
}

/// Identity of a persisted consumer resource (its type name plus its
/// stringified primary key). Resource-side finders never materialize
/// consumer rows through the store - they return keys the consumer
/// resolves through its own ORM (Phase 2 finder semantics).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ResourceKey {
    /// Resource class name (`resource_type` column value).
    pub resource_type: String,
    /// Stringified resource primary key.
    pub resource_id: ResourceId,
}

impl ResourceKey {
    /// Identify a persisted resource by type name + stringified PK.
    #[must_use]
    pub fn new(resource_type: impl Into<String>, resource_id: impl Into<ResourceId>) -> Self {
        Self {
            resource_type: resource_type.into(),
            resource_id: resource_id.into(),
        }
    }
}

/// The resource-side finder SPI (gem `ResourceAdapterBase`, base.rb).
///
/// Members are contract-only for Phase 1 (finder semantics land in Phase 2,
/// RSRC-*); rolify-test's `InMemoryStore` implements them over its fixture
/// registry so the signatures cannot drift away from a real implementation.
///
/// # Example
///
/// ```
/// use rolify_core::store::ResourceKey;
///
/// let key = ResourceKey::new("Forum", 7_i64);
/// assert_eq!(key.resource_type, "Forum");
/// ```
#[maybe_async::maybe_async(AFIT)]
pub trait ResourceStore: Sealed + Send + Sync + 'static {
    /// Backend connection/pool handle - `Send` for the same reason as
    /// [`RoleStore::Conn`].
    type Conn: Send;

    /// Backend error - same contract as [`RoleStore::Error`].
    type Error: core::error::Error + Send + Sync + From<RolifyError> + 'static;

    /// Gem `resources_find` (`resource_adapter.rb`): resources of the given
    /// STI type family (`types` includes descendants, per
    /// `relation_types_for`, base.rb:27-28) that hold `name` at class scope
    /// or at their own instance scope.
    fn resources_find(
        &self,
        conn: &mut Self::Conn,
        types: &[&str],
        name: &RoleName,
    ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send;

    /// Gem `in` (`resource_adapter.rb` - `in` is a Rust keyword, hence
    /// `in_list`): among `candidates`, the resources where `holder` has any
    /// of `names` at the resource's class scope or instance scope.
    fn in_list(
        &self,
        conn: &mut Self::Conn,
        candidates: &[ResourceKey],
        holder: &ResourceId,
        names: &[RoleName],
    ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send;
}
