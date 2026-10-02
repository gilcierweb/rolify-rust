//! [`RolifyUser`] - the consumer trait carrying the gem's `Role` concern
//! logic (`rolify/lib/rolify/role.rb`) as provided methods, written once.
//!
//! Storage steps delegate to the adapter SPI ([`crate::store::RoleStore`]);
//! every behavioral knob (strict mode, callbacks, `remove_role_if_empty`,
//! table names) flows through the required [`RolifyUser::rolify_config`]
//! seam. The `method_missing` shortcuts of `role.rb` are intentionally NOT
//! ported; call `has_role(&RoleName::from("admin"), filter)`.

// `Future` is named in the provided signatures in async mode only;
// maybe-async strips the `impl Future` return type in `is_sync` mode.
#[cfg(not(feature = "is_sync"))]
use core::future::Future;

use crate::config::RolifyConfig;
use crate::kernel::{RemovalTarget, strict_engages};
use crate::query::{ResourceFilter, RoleQuery};
use crate::resource::ResourceRef;
use crate::role::{ResourceId, RoleName, RoleRecord, RoleSet};
use crate::store::{RemovalOutcome, RoleStore};

/// Implement on your user/account type to give it roles.
///
/// Required members: the store accessor, the pinned config seam, the
/// holder identity used by the join table, the type discriminator feeding
/// the user-class finders, and a split borrow handing the
/// SPI `(&mut store, &mut conn)` at once (two independent `&mut self`
/// accessors could never do that in safe Rust - one method must split the
/// disjoint fields).
///
/// # Example
///
/// Consumer wiring plus a full grant/check/revoke pass against the
/// in-memory reference store - live in BOTH modes (the `maybe_async`
/// attribute rewrites the example's own `await`s when `is_sync` is active):
///
/// ```rust
/// use rolify_core::config::RolifyConfig;
/// use rolify_core::query::{ResourceFilter, RoleQuery};
/// use rolify_core::resource::ResourceRef;
/// use rolify_core::role::{ResourceId, RoleName, RoleSet};
/// use rolify_core::user::RolifyUser;
/// use rolify_test::InMemoryStore;
///
/// struct Player { id: i64, store: InMemoryStore, conn: (), config: RolifyConfig }
///
/// impl RolifyUser for Player {
///     type Store = InMemoryStore;
///     fn store(&mut self) -> &mut InMemoryStore { &mut self.store }
///     fn rolify_config(&self) -> &RolifyConfig { &self.config }
///     fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
///     fn rolify_type() -> &'static str { "Player" }
///     fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
///         (&mut self.store, &mut self.conn)
///     }
/// }
///
/// # #[cfg(not(feature = "is_sync"))]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() { usage().await; }
/// # #[cfg(feature = "is_sync")]
/// # fn main() { usage(); }
/// #
/// #[maybe_async::maybe_async]
/// async fn usage() {
///     let mut player = Player { id: 1, store: InMemoryStore::new(), conn: (), config: RolifyConfig::default() };
///     player.add_role(&RoleName::from("admin"), ResourceRef::Global).await.unwrap();
///     let has_admin = player.has_role(&RoleName::from("admin"), ResourceFilter::Global).await.unwrap();
///     assert!(has_admin);
///     let names = player.roles_name().await.unwrap();
///     assert_eq!(names, vec![RoleName::from("admin")]);
/// }
/// ```
#[maybe_async::maybe_async(AFIT)]
pub trait RolifyUser: Send + Sync + 'static {
    /// The store owning this user's role rows (static dispatch - the SPI is
    /// not dyn-compatible by design).
    type Store: RoleStore;

    /// Borrow the store.
    fn store(&mut self) -> &mut Self::Store;

    /// Borrow the resolved configuration (pinned seam - all strict/callback
    /// routing keys off this).
    fn rolify_config(&self) -> &RolifyConfig;

    /// The holder's stable identity for the join table (the gem's user
    /// primary key, stringified - same precedent as [`ResourceId`]).
    fn rolify_id(&self) -> ResourceId;

    /// The holder's type discriminator - the `RolifyUser` counterpart of
    /// [`Resource::type_name`](crate::resource::Resource::type_name),
    /// feeding the `holder_types` argument of the user-class finders
    /// (02-07).
    ///
    /// Required with NO default body (D-08): every consumer names its own
    /// type. No `where Self: Sized` bound: unlike `Resource` (where the
    /// bound preserves dyn-compatibility per D-09), `RolifyUser` is
    /// static-dispatch-only by design (`AFIT` plus the `type Store`
    /// associated type make `dyn` impossible), so the bound is unnecessary.
    fn rolify_type() -> &'static str;

    /// Borrow store and connection as one disjoint split - the SPI's
    /// `(&Store, &mut Conn)` call shape requires both at once.
    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut <Self::Store as RoleStore>::Conn);

    /// `add_role(name, resource = nil)` - the gem's two-level idempotence
    /// (`role.rb:12-22`): level-1 role-row dedupe via
    /// `find_or_create_by`, level-2 link guard inside `store.add`.
    ///
    /// CONF-05 choreography: `before_add` receives the would-be record and
    /// runs BEFORE any store mutation, so a veto leaves the store
    /// untouched; `after_add` runs only after a successful add.
    fn add_role(
        &mut self,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            let (resource_type, resource_id) = match scope {
                ResourceRef::Global => (None, None),
                ResourceRef::Class(type_name) => (Some(type_name.to_owned()), None),
                ResourceRef::Instance(type_name, id) => {
                    (Some(type_name.to_owned()), Some(id.clone()))
                }
            };
            let candidate = RoleRecord::new(name.clone(), resource_type, resource_id);
            self.rolify_config()
                .run_before_add(&candidate)
                .map_err(<Self::Store as RoleStore>::Error::from)?;

            let holder = self.rolify_id();
            let (store, conn) = self.store_with_conn();
            let role = store.find_or_create_by(&mut *conn, name, scope).await?;
            // gem: `relation.roles << role unless relation.roles.include?(role)`
            store.add(&mut *conn, &holder, &role).await?;

            self.rolify_config().run_after_add(&role);
            Ok(role)
        }
    }

    /// `grant` - thin alias of [`RolifyUser::add_role`] (`role.rb:23`),
    /// the only add-side alias kept (the gem's other alias families are
    /// out of scope).
    ///
    /// # Errors
    ///
    /// Propagates the store errors of `add_role` (including callback vetoes
    /// converted into the store error).
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::kernel::RemovalTarget;
    /// use rolify_core::query::ResourceFilter;
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::InMemoryStore;
    ///
    /// struct Player { id: i64, store: InMemoryStore, conn: (), config: RolifyConfig }
    ///
    /// impl RolifyUser for Player {
    ///     type Store = InMemoryStore;
    ///     fn store(&mut self) -> &mut InMemoryStore { &mut self.store }
    ///     fn rolify_config(&self) -> &RolifyConfig { &self.config }
    ///     fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    ///     fn rolify_type() -> &'static str { "Player" }
    ///     fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
    ///         (&mut self.store, &mut self.conn)
    ///     }
    /// }
    ///
    /// # #[cfg(not(feature = "is_sync"))]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() { usage().await; }
    /// # #[cfg(feature = "is_sync")]
    /// # fn main() { usage(); }
    /// #
    /// #[maybe_async::maybe_async]
    /// async fn usage() {
    ///     let mut player = Player { id: 1, store: InMemoryStore::new(), conn: (), config: RolifyConfig::default() };
    ///     let name = RoleName::from("moderator");
    ///     player.grant(&name, ResourceRef::Class("Forum")).await.unwrap();
    ///     let strict_here = player.has_strict_role(&name, ResourceFilter::Class("Forum")).await.unwrap();
    ///     assert!(strict_here);
    ///     let strict_elsewhere = player.has_strict_role(&name, ResourceFilter::Class("Group")).await.unwrap();
    ///     assert!(!strict_elsewhere);
    ///     player.revoke(&name, RemovalTarget::TypeSweep("Forum")).await.unwrap();
    ///     let gone = player.has_role(&name, ResourceFilter::Class("Forum")).await.unwrap();
    ///     assert!(!gone);
    /// }
    /// ```
    fn grant(
        &mut self,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, <Self::Store as RoleStore>::Error>> + Send {
        async move { self.add_role(name, scope).await }
    }

    /// `remove_role(name, resource = nil)` (role.rb:27-30) - deletes this
    /// holder's links matching the conjunctive sweep `target`
    /// ([`RemovalTarget`]), honoring `remove_role_if_empty` (when the last
    /// membership of a role row vanished, the row itself is destroyed -
    /// role_adapter.rb:58-70).
    ///
    /// CONF-05: `before_remove` vetoes before any mutation; `after_remove`
    /// runs only after a successful remove.
    fn remove_role(
        &mut self,
        name: &RoleName,
        target: RemovalTarget<'_>,
    ) -> impl Future<Output = Result<RemovalOutcome, <Self::Store as RoleStore>::Error>> + Send
    {
        async move {
            let candidate = match target {
                RemovalTarget::NameOnly => RoleRecord::global(name.clone()),
                RemovalTarget::TypeSweep(type_name) => {
                    RoleRecord::for_class(name.clone(), type_name)
                }
                RemovalTarget::Exact(type_name, id) => {
                    RoleRecord::for_instance(name.clone(), type_name, id.clone())
                }
            };
            self.rolify_config()
                .run_before_remove(&candidate)
                .map_err(<Self::Store as RoleStore>::Error::from)?;

            let remove_if_empty = self.rolify_config().remove_role_if_empty();
            let holder = self.rolify_id();
            let (store, conn) = self.store_with_conn();
            let outcome = store
                .remove(&mut *conn, &holder, name, target, remove_if_empty)
                .await?;

            self.rolify_config().run_after_remove(&candidate);
            Ok(outcome)
        }
    }

    /// `revoke` - thin alias of [`RolifyUser::remove_role`] (`role.rb:85`),
    /// the only remove-side alias kept.
    ///
    /// # Errors
    ///
    /// Propagates the store errors of `remove_role` (including callback
    /// vetoes converted into the store error).
    fn revoke(
        &mut self,
        name: &RoleName,
        target: RemovalTarget<'_>,
    ) -> impl Future<Output = Result<RemovalOutcome, <Self::Store as RoleStore>::Error>> + Send
    {
        async move { self.remove_role(name, target).await }
    }

    /// `has_role?(name, resource = nil)` (role.rb:25-41) - routes through
    /// the strict predicates ONLY when `strict` is configured AND the filter
    /// is Class/Instance (the gem's narrow gate at role.rb:26), via the
    /// pinned [`RolifyUser::rolify_config`] seam.
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::ResourceFilter;
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::InMemoryStore;
    ///
    /// struct Player { id: i64, store: InMemoryStore, conn: (), config: RolifyConfig }
    ///
    /// impl RolifyUser for Player {
    ///     type Store = InMemoryStore;
    ///     fn store(&mut self) -> &mut InMemoryStore { &mut self.store }
    ///     fn rolify_config(&self) -> &RolifyConfig { &self.config }
    ///     fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    ///     fn rolify_type() -> &'static str { "Player" }
    ///     fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
    ///         (&mut self.store, &mut self.conn)
    ///     }
    /// }
    ///
    /// # #[cfg(not(feature = "is_sync"))]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() { usage().await; }
    /// # #[cfg(feature = "is_sync")]
    /// # fn main() { usage(); }
    /// #
    /// #[maybe_async::maybe_async]
    /// async fn usage() {
    ///     let mut player = Player {
    ///         id: 1,
    ///         store: InMemoryStore::new(),
    ///         conn: (),
    ///         config: RolifyConfig::default(),
    ///     };
    ///     player
    ///         .add_role(&RoleName::from("admin"), ResourceRef::Global)
    ///         .await
    ///         .unwrap();
    ///     let has = player
    ///         .has_role(&RoleName::from("admin"), ResourceFilter::Global)
    ///         .await
    ///         .unwrap();
    ///     assert!(has);
    /// }
    /// ```
    fn has_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
    ) -> impl Future<Output = Result<bool, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            let strict = self.rolify_config().strict_engages_for(&filter);
            let holder = self.rolify_id();
            let query = RoleQuery { name, filter };
            let (store, conn) = self.store_with_conn();
            let rows = if strict {
                store.where_strict(&mut *conn, &holder, &query).await?
            } else {
                store.where_(&mut *conn, &holder, &query).await?
            };
            Ok(!rows.is_empty())
        }
    }

    /// `has_strict_role?(name, resource)` (`role.rb:43-45`) - direct strict
    /// membership with NO gate: the strict check at `role.rb:26` lives in
    /// [`RolifyUser::has_role`]; callers of this path opted in.
    ///
    /// Two reachable corners by design (both unreachable through
    /// `has_role`'s gate, both expressible here): `ResourceFilter::Global`
    /// matches exactly-global rows, `ResourceFilter::Any` matches by name
    /// alone.
    ///
    /// # Errors
    ///
    /// Propagates the store errors of `where_strict`.
    fn has_strict_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
    ) -> impl Future<Output = Result<bool, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            let holder = self.rolify_id();
            let query = RoleQuery { name, filter };
            let (store, conn) = self.store_with_conn();
            let rows = store.where_strict(&mut *conn, &holder, &query).await?;
            Ok(!rows.is_empty())
        }
    }

    /// `has_cached_role?` (role.rb:47-49) over a caller-supplied [zeroed-IO]
    /// snapshot - strict routing mirrors [`RolifyUser::has_role`].
    ///
    /// [zeroed-IO]: crate::role::RoleSet
    fn has_cached_role(&self, snapshot: &RoleSet<'_>, query: &RoleQuery<'_>) -> bool {
        if self.rolify_config().strict_engages_for(&query.filter) {
            snapshot.has_strict_cached_role(query)
        } else {
            snapshot.has_cached_role(query)
        }
    }

    /// `has_strict_cached_role?` (role.rb:51-54) - direct strict cached
    /// membership, no gating (callers of the strict path opted in).
    fn has_strict_cached_role(&self, snapshot: &RoleSet<'_>, query: &RoleQuery<'_>) -> bool {
        snapshot.has_strict_cached_role(query)
    }

    /// `has_all_roles?` (*args) (role.rb:56-67) - sequential per-query
    /// checks with early exit on the first miss.
    fn has_all_roles(
        &mut self,
        queries: &[RoleQuery<'_>],
    ) -> impl Future<Output = Result<bool, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            let strict = self.rolify_config().strict();
            for query in queries {
                let engages = strict_engages(strict, &query.filter);
                let holder = self.rolify_id();
                let (store, conn) = self.store_with_conn();
                let rows = if engages {
                    store.where_strict(&mut *conn, &holder, query).await?
                } else {
                    store.where_(&mut *conn, &holder, query).await?
                };
                if rows.is_empty() {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }

    /// `has_any_role?` (*args) (role.rb:69-75) - ONE OR-folded store round
    /// (`build_conditions`' `join(' OR ')`), never N sequential checks. The
    /// gem's persisted path is always non-strict here (no strict branch in
    /// `has_any_role?`), so this is too - parity pinned in
    /// `strict_tests`/parity-matrix notes.
    fn has_any_roles(
        &mut self,
        queries: &[RoleQuery<'_>],
    ) -> impl Future<Output = Result<bool, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            if queries.is_empty() {
                return Ok(false);
            }
            let holder = self.rolify_id();
            let (store, conn) = self.store_with_conn();
            let rows = store.where_any(&mut *conn, &holder, queries).await?;
            Ok(!rows.is_empty())
        }
    }

    /// `only_has_role?` (role.rb:77-79) - the holder has the asked role AND
    /// exactly one role in total (counts ALL linked roles, not matches).
    fn only_has_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
    ) -> impl Future<Output = Result<bool, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            if !self.has_role(name, filter).await? {
                return Ok(false);
            }
            let holder = self.rolify_id();
            let (store, conn) = self.store_with_conn();
            let roles = store.roles_of(&mut *conn, &holder).await?;
            Ok(roles.len() == 1)
        }
    }

    /// `roles_name` (role.rb:88-90) - all role names linked to this holder.
    fn roles_name(
        &mut self,
    ) -> impl Future<Output = Result<Vec<RoleName>, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            let holder = self.rolify_id();
            let (store, conn) = self.store_with_conn();
            let roles = store.roles_of(&mut *conn, &holder).await?;
            Ok(roles.into_iter().map(|record| record.name).collect())
        }
    }
}
