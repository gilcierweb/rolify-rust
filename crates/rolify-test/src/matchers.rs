//! [`RoleAssertions`] - consumer test assertions over [`RolifyUser`].
//!
//! Blanket trait (D-01): every holder implementing [`RolifyUser`] gains
//! `assert_has_role` plus `assert_has_no_role` with no extra wiring. Both
//! methods delegate every decision to the existing `has_role` predicate, so
//! the kernel ladder (global override, strict gate, class-covers-instance)
//! is inherited, never reimplemented.
//!
//! Failure shape (D-02, D-11): mismatches panic (no `Result` return) with
//! the full context on one `key = value` line each: holder type plus id,
//! expected name plus scope, the complete held-role list in insertion
//! order, and the trailing caller context (D-08, passed as `format!`
//! output the way `assert_eq!` takes custom messages). Store failures panic
//! through the same builder with a `store_error` line; the held list then
//! renders empty because it could not be read.
//!
//! Argument shape (D-09): borrowed [`RoleName`] plus [`ResourceFilter`],
//! exactly the `has_role` query halves, so callers reuse the queries they
//! already build. User-side only (D-10): no resource-side matchers.
//!
//! Dual-mode: the trait carries `maybe_async` AFIT exactly like
//! [`RolifyUser`]; it compiles in both default async and `is_sync` builds
//! with no `block_on` anywhere.
//!
//! Test-output note (T-7-01): failure messages list role names, so CI logs
//! of failing tests contain them. Names render through `Display` only and
//! are never interpreted (T-7-02); the byte-exact comparison stays inside
//! `has_role`, untouched by formatting.

// `Future` is named in the provided signatures in async mode only;
// maybe-async strips the `impl Future` return type in `is_sync` mode. See
// the matching gate in `lib.rs` for why the allow rides along.
#[cfg(not(feature = "is_sync"))]
#[allow(unused_imports)]
use core::future::Future;

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::RoleStore;
use rolify_core::user::RolifyUser;

/// Consumer assertions over [`RolifyUser`] (TOOL-02, D-01).
///
/// Blanket-implemented for every holder type: implement [`RolifyUser`] and
/// the asserts resolve with no further wiring. Every decision delegates to
/// [`RolifyUser::has_role`]; the methods only add the panic-on-mismatch
/// shell plus the full-context message.
///
/// # Example
///
/// Grant plus assert through the published mock, live in BOTH modes (the
/// `maybe_async` attribute rewrites the example's own `await`s when
/// `is_sync` is active):
///
/// ```rust
/// use rolify_core::config::RolifyConfig;
/// use rolify_core::query::ResourceFilter;
/// use rolify_core::resource::ResourceRef;
/// use rolify_core::role::{ResourceId, RoleName};
/// use rolify_core::user::RolifyUser;
/// use rolify_test::{InMemoryStore, RoleAssertions};
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
///     player.assert_has_role(&RoleName::from("admin"), ResourceFilter::Global, "seeded admin").await;
///     player.assert_has_no_role(&RoleName::from("ghost"), ResourceFilter::Any, "never granted").await;
/// }
/// ```
#[maybe_async::maybe_async(AFIT)]
pub trait RoleAssertions: RolifyUser {
    /// Positive assertion (D-03 `assert_has_role` naming leg): passes when
    /// [`RolifyUser::has_role`] holds, else panics with the full context
    /// (holder type plus id, expected name plus scope, held-role list, and
    /// `context`). Store failures panic through the same message with a
    /// `store_error` line instead of returning.
    ///
    /// # Panics
    ///
    /// Panics when the holder lacks the role at the queried scope, or when
    /// the underlying store read fails.
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
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     let forum_id = ResourceId::from(7_i64);
    ///     player.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await.unwrap();
    ///     player.assert_has_role(&RoleName::from("moderator"), ResourceFilter::Instance("Forum", &forum_id), "class covers instance").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let expected = RoleQuery { name, filter };
            let satisfied = match self.has_role(expected.name, expected.filter).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_assertion_message(
                        "assert_has_role",
                        Self::rolify_type(),
                        &holder,
                        &expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if !satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_assertion_message(
                    "assert_has_role",
                    Self::rolify_type(),
                    &holder,
                    &expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Negative assertion (D-05 symmetry leg): passes when
    /// [`RolifyUser::has_role`] is false, else panics with the same
    /// full-context message shape as [`RoleAssertions::assert_has_role`].
    /// Store failures panic through the same message with a `store_error`
    /// line instead of returning.
    ///
    /// # Panics
    ///
    /// Panics when the holder unexpectedly holds the role at the queried
    /// scope, or when the underlying store read fails.
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::ResourceFilter;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     player.assert_has_no_role(&RoleName::from("ghost"), ResourceFilter::Any, "nothing granted yet").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_no_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let expected = RoleQuery { name, filter };
            let satisfied = match self.has_role(expected.name, expected.filter).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_assertion_message(
                        "assert_has_no_role",
                        Self::rolify_type(),
                        &holder,
                        &expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_assertion_message(
                    "assert_has_no_role",
                    Self::rolify_type(),
                    &holder,
                    &expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Strict positive assertion (D-04 family leg): passes when
    /// [`RolifyUser::has_strict_role`] holds, else panics with the full
    /// context. Exact scope, no ladder: a global grant never satisfies a
    /// class or instance ask here (the override lives only in the
    /// non-strict [`RoleAssertions::assert_has_role`]).
    ///
    /// # Panics
    ///
    /// Panics when the holder lacks the role at the exact queried scope,
    /// or when the underlying store read fails.
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
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     player.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await.unwrap();
    ///     player.assert_has_strict_role(&RoleName::from("moderator"), ResourceFilter::Class("Forum"), "exact class row").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_strict_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let expected = RoleQuery { name, filter };
            let satisfied = match self.has_strict_role(expected.name, expected.filter).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_assertion_message(
                        "assert_has_strict_role",
                        Self::rolify_type(),
                        &holder,
                        &expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if !satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_assertion_message(
                    "assert_has_strict_role",
                    Self::rolify_type(),
                    &holder,
                    &expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// All-roles positive assertion (D-04 family leg): passes when
    /// [`RolifyUser::has_all_roles`] holds for every query in the slice,
    /// else panics naming the full query list. Sequential checks with
    /// early exit on the first miss, exactly like the predicate.
    ///
    /// # Panics
    ///
    /// Panics when any query misses, or when the underlying store read
    /// fails.
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::{ResourceFilter, RoleQuery};
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     let admin = RoleName::from("admin");
    ///     player.add_role(&admin, ResourceRef::Global).await.unwrap();
    ///     let queries = [RoleQuery::with_role(&admin)];
    ///     player.assert_has_all_roles(&queries, "every query held").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_all_roles(
        &mut self,
        queries: &[RoleQuery<'_>],
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let satisfied = match self.has_all_roles(queries).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_multi_assertion_message(
                        "assert_has_all_roles",
                        Self::rolify_type(),
                        &holder,
                        queries,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if !satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_multi_assertion_message(
                    "assert_has_all_roles",
                    Self::rolify_type(),
                    &holder,
                    queries,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Any-roles positive assertion (D-04 family leg): passes when
    /// [`RolifyUser::has_any_roles`] holds for at least one query, else
    /// panics naming the full query list. ONE OR-folded store round, and
    /// always non-strict: the gem's `has_any_role?` carries no strict
    /// branch, so this stays non-strict even under a strict config.
    ///
    /// # Panics
    ///
    /// Panics when every query misses (an empty slice always panics, it
    /// matches nothing), or when the underlying store read fails.
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::{ResourceFilter, RoleQuery};
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     let admin = RoleName::from("admin");
    ///     let ghost = RoleName::from("ghost");
    ///     player.add_role(&admin, ResourceRef::Global).await.unwrap();
    ///     let queries = [RoleQuery::with_role(&ghost), RoleQuery::with_role(&admin)];
    ///     player.assert_has_any_roles(&queries, "one hit suffices").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_any_roles(
        &mut self,
        queries: &[RoleQuery<'_>],
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let satisfied = match self.has_any_roles(queries).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_multi_assertion_message(
                        "assert_has_any_roles",
                        Self::rolify_type(),
                        &holder,
                        queries,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if !satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_multi_assertion_message(
                    "assert_has_any_roles",
                    Self::rolify_type(),
                    &holder,
                    queries,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Only-role positive assertion (D-04 family leg): passes when
    /// [`RolifyUser::only_has_role`] holds (the asked role matches AND the
    /// holder carries exactly one role in total), else panics with the
    /// full held list showing what else the holder carries.
    ///
    /// # Panics
    ///
    /// Panics when the asked role is missing or the holder carries any
    /// additional role, or when the underlying store read fails.
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
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     player.assert_only_has_role(&RoleName::from("admin"), ResourceFilter::Global, "solo admin").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_only_has_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let expected = RoleQuery { name, filter };
            let satisfied = match self.only_has_role(expected.name, expected.filter).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_assertion_message(
                        "assert_only_has_role",
                        Self::rolify_type(),
                        &holder,
                        &expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if !satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_assertion_message(
                    "assert_only_has_role",
                    Self::rolify_type(),
                    &holder,
                    &expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Role-names positive assertion (D-04 family leg): passes when the
    /// sorted name set from [`RolifyUser::roles_name`] equals the expected
    /// sorted set, regardless of grant order. The panic shows the missing
    /// plus unexpected names side by side with the full held list.
    ///
    /// Test-output note (T-7-05): the failure message lists role names, so
    /// CI logs of failing tests contain them; treat the output like any
    /// other assertion diff.
    ///
    /// # Panics
    ///
    /// Panics when the held name set differs from the expected set, or
    /// when the underlying store read fails.
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     let expected = [RoleName::from("admin")];
    ///     player.assert_role_names(&expected, "exact name set").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_role_names(
        &mut self,
        expected: &[RoleName],
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let held_names = match self.roles_name().await {
                Ok(held_names) => held_names,
                Err(store_error) => {
                    let message = build_names_assertion_message(
                        "assert_role_names",
                        Self::rolify_type(),
                        &holder,
                        expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            let expected_texts = sorted_unique_texts(expected.iter().map(RoleName::as_str));
            let held_texts = sorted_unique_texts(held_names.iter().map(RoleName::as_str));
            if expected_texts != held_texts {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_names_assertion_message(
                    "assert_role_names",
                    Self::rolify_type(),
                    &holder,
                    expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Strict negative assertion (D-05 symmetry leg): passes exactly when
    /// [`RolifyUser::has_strict_role`] is false, else panics with the same
    /// full-context shape as
    /// [`RoleAssertions::assert_has_strict_role`]. This method is the
    /// negation: no manual `!` around the positive is needed.
    ///
    /// # Panics
    ///
    /// Panics when the holder holds the role at the exact queried scope,
    /// or when the underlying store read fails.
    ///
    /// # Example
    ///
    /// Grant then deny: the negative panics on the strictly held role.
    /// Runs live in BOTH modes:
    ///
    /// ```rust,should_panic
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::ResourceFilter;
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     player.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await.unwrap();
    ///     player.assert_has_no_strict_role(&RoleName::from("moderator"), ResourceFilter::Class("Forum"), "strictly held, so this panics").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_no_strict_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let expected = RoleQuery { name, filter };
            let satisfied = match self.has_strict_role(expected.name, expected.filter).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_assertion_message(
                        "assert_has_no_strict_role",
                        Self::rolify_type(),
                        &holder,
                        &expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_assertion_message(
                    "assert_has_no_strict_role",
                    Self::rolify_type(),
                    &holder,
                    &expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// All-roles negative assertion (D-05 symmetry leg): passes exactly
    /// when [`RolifyUser::has_all_roles`] is false (at least one query
    /// misses), else panics naming the full query list through the same
    /// multi-query message as
    /// [`RoleAssertions::assert_has_all_roles`].
    ///
    /// # Panics
    ///
    /// Panics when every query holds, or when the underlying store read
    /// fails.
    ///
    /// # Example
    ///
    /// Grant both then deny: the negative panics. Runs live in BOTH modes:
    ///
    /// ```rust,should_panic
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::RoleQuery;
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     let admin = RoleName::from("admin");
    ///     player.add_role(&admin, ResourceRef::Global).await.unwrap();
    ///     let queries = [RoleQuery::with_role(&admin)];
    ///     player.assert_has_no_all_roles(&queries, "held, so this panics").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_no_all_roles(
        &mut self,
        queries: &[RoleQuery<'_>],
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let satisfied = match self.has_all_roles(queries).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_multi_assertion_message(
                        "assert_has_no_all_roles",
                        Self::rolify_type(),
                        &holder,
                        queries,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_multi_assertion_message(
                    "assert_has_no_all_roles",
                    Self::rolify_type(),
                    &holder,
                    queries,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Any-roles negative assertion (D-05 symmetry leg): passes exactly
    /// when [`RolifyUser::has_any_roles`] is false (no query hits,
    /// including the empty slice which matches nothing), else panics
    /// through the same multi-query message as
    /// [`RoleAssertions::assert_has_any_roles`].
    ///
    /// # Panics
    ///
    /// Panics when at least one query hits, or when the underlying store
    /// read fails.
    ///
    /// # Example
    ///
    /// Grant then deny: the negative panics. Runs live in BOTH modes:
    ///
    /// ```rust,should_panic
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::RoleQuery;
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     let admin = RoleName::from("admin");
    ///     player.add_role(&admin, ResourceRef::Global).await.unwrap();
    ///     let queries = [RoleQuery::with_role(&admin)];
    ///     player.assert_has_no_any_roles(&queries, "held, so this panics").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_no_any_roles(
        &mut self,
        queries: &[RoleQuery<'_>],
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let satisfied = match self.has_any_roles(queries).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_multi_assertion_message(
                        "assert_has_no_any_roles",
                        Self::rolify_type(),
                        &holder,
                        queries,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_multi_assertion_message(
                    "assert_has_no_any_roles",
                    Self::rolify_type(),
                    &holder,
                    queries,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Only-role negative assertion (D-05 symmetry leg): passes exactly
    /// when [`RolifyUser::only_has_role`] is false (the asked role is
    /// missing or the holder carries more than one role), else panics
    /// with the same full-context shape as
    /// [`RoleAssertions::assert_only_has_role`].
    ///
    /// # Panics
    ///
    /// Panics when the holder holds exactly the asked role and nothing
    /// else, or when the underlying store read fails.
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::query::ResourceFilter;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     player.assert_has_no_only_role(&RoleName::from("admin"), ResourceFilter::Global, "nothing granted yet").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_no_only_role(
        &mut self,
        name: &RoleName,
        filter: ResourceFilter<'_>,
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let expected = RoleQuery { name, filter };
            let satisfied = match self.only_has_role(expected.name, expected.filter).await {
                Ok(satisfied) => satisfied,
                Err(store_error) => {
                    let message = build_assertion_message(
                        "assert_has_no_only_role",
                        Self::rolify_type(),
                        &holder,
                        &expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            if satisfied {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_assertion_message(
                    "assert_has_no_only_role",
                    Self::rolify_type(),
                    &holder,
                    &expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }

    /// Role-names negative assertion (D-05 symmetry leg): passes exactly
    /// when the held name set differs from the expected set, else panics
    /// through the same names-set message as
    /// [`RoleAssertions::assert_role_names`].
    ///
    /// # Panics
    ///
    /// Panics when the held name set equals the expected set, or when the
    /// underlying store read fails.
    ///
    /// # Example
    ///
    /// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
    /// example's own `await`s when `is_sync` is active):
    ///
    /// ```rust
    /// use rolify_core::config::RolifyConfig;
    /// use rolify_core::resource::ResourceRef;
    /// use rolify_core::role::{ResourceId, RoleName};
    /// use rolify_core::user::RolifyUser;
    /// use rolify_test::{InMemoryStore, RoleAssertions};
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
    ///     let expected = [RoleName::from("admin"), RoleName::from("moderator")];
    ///     player.assert_has_no_role_names(&expected, "moderator is missing").await;
    /// }
    /// ```
    // The `Output = ()` reads as an unneeded unit return once maybe-async
    // rewrites the signature for `is_sync` builds.
    #[allow(clippy::unused_unit)]
    fn assert_has_no_role_names(
        &mut self,
        expected: &[RoleName],
        context: impl Into<String> + Send,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let context_text: String = context.into();
            let holder = self.rolify_id();
            let held_names = match self.roles_name().await {
                Ok(held_names) => held_names,
                Err(store_error) => {
                    let message = build_names_assertion_message(
                        "assert_has_no_role_names",
                        Self::rolify_type(),
                        &holder,
                        expected,
                        &[],
                        &context_text,
                        Some(store_error.to_string()),
                    );
                    panic!("{message}");
                }
            };
            let expected_texts = sorted_unique_texts(expected.iter().map(RoleName::as_str));
            let held_texts = sorted_unique_texts(held_names.iter().map(RoleName::as_str));
            if expected_texts == held_texts {
                let (store, conn) = self.store_with_conn();
                let held_read = store.roles_of(&mut *conn, &holder).await;
                let (held, read_error) = match held_read {
                    Ok(held) => (held, None),
                    Err(store_error) => (Vec::new(), Some(store_error.to_string())),
                };
                let message = build_names_assertion_message(
                    "assert_has_no_role_names",
                    Self::rolify_type(),
                    &holder,
                    expected,
                    &held,
                    &context_text,
                    read_error,
                );
                panic!("{message}");
            }
        }
    }
}

impl<Holder> RoleAssertions for Holder where Holder: RolifyUser {}

/// Shared panic-message builder (D-11): one `key = value` line each for the
/// assert operation, the holder type plus id, the expected name plus scope,
/// the complete held-role list in insertion order, the optional store
/// failure, and the caller context. ASCII only. Names render through
/// `Display` and are never interpreted (T-7-02).
#[must_use]
fn build_assertion_message(
    operation: &str,
    holder_type: &str,
    holder_id: &ResourceId,
    expected: &RoleQuery<'_>,
    held: &[RoleRecord],
    context: &str,
    store_error: Option<String>,
) -> String {
    let held_text = render_held_list(held);
    let error_line = render_error_line(store_error);
    format!(
        "operation = {operation}\nholder_type = {holder_type}\nholder_id = {holder_id}\nexpected_name = {}\nexpected_scope = {}\nheld_roles = [{held_text}]\n{error_line}context = {context}",
        expected.name,
        render_scope(expected.filter),
    )
}

/// Shared multi-query panic-message builder (D-04 all plus any legs): the
/// same `key = value` lines as [`build_assertion_message`], with the
/// expected side rendered as the full query list instead of one query.
#[must_use]
fn build_multi_assertion_message(
    operation: &str,
    holder_type: &str,
    holder_id: &ResourceId,
    queries: &[RoleQuery<'_>],
    held: &[RoleRecord],
    context: &str,
    store_error: Option<String>,
) -> String {
    let expected_text = queries
        .iter()
        .map(render_expected_query)
        .collect::<Vec<String>>()
        .join(", ");
    let held_text = render_held_list(held);
    let error_line = render_error_line(store_error);
    format!(
        "operation = {operation}\nholder_type = {holder_type}\nholder_id = {holder_id}\nexpected_queries = [{expected_text}]\nheld_roles = [{held_text}]\n{error_line}context = {context}",
    )
}

/// Shared names-set panic-message builder (D-04 names leg): the same holder
/// plus held plus context lines, with the expected side rendered as the
/// sorted name set and the mismatch split into missing plus unexpected
/// names (T-7-05: role names travel inside the consumer test binary only).
#[must_use]
fn build_names_assertion_message(
    operation: &str,
    holder_type: &str,
    holder_id: &ResourceId,
    expected: &[RoleName],
    held: &[RoleRecord],
    context: &str,
    store_error: Option<String>,
) -> String {
    let expected_texts = sorted_unique_texts(expected.iter().map(RoleName::as_str));
    let held_name_texts = sorted_unique_texts(held.iter().map(|record| record.name.as_str()));
    let missing = names_difference(&expected_texts, &held_name_texts);
    let unexpected = names_difference(&held_name_texts, &expected_texts);
    let held_text = render_held_list(held);
    let error_line = render_error_line(store_error);
    format!(
        "operation = {operation}\nholder_type = {holder_type}\nholder_id = {holder_id}\nexpected_names = [{}]\nmissing_names = [{}]\nunexpected_names = [{}]\nheld_roles = [{held_text}]\n{error_line}context = {context}",
        expected_texts.join(", "),
        missing.join(", "),
        unexpected.join(", "),
    )
}

/// One expected query rendered for the multi-query line: `name (scope)`
/// with the same scope words as [`render_scope`].
#[must_use]
fn render_expected_query(query: &RoleQuery<'_>) -> String {
    format!("{} ({})", query.name, render_scope(query.filter))
}

/// The held-role list shared by every builder: rows in insertion order,
/// comma-joined, empty when the store read failed.
#[must_use]
fn render_held_list(held: &[RoleRecord]) -> String {
    held.iter()
        .map(render_role_record)
        .collect::<Vec<String>>()
        .join(", ")
}

/// The optional store-failure line shared by every builder: the
/// `store_error` line when present, nothing otherwise.
#[must_use]
fn render_error_line(store_error: Option<String>) -> String {
    store_error
        .map(|error_text| format!("store_error = {error_text}\n"))
        .unwrap_or_default()
}

/// Sorted unique name texts for set comparison: byte-exact names sorted
/// as strings with duplicates removed, so grant order never matters.
#[must_use]
fn sorted_unique_texts<'texts>(names: impl IntoIterator<Item = &'texts str>) -> Vec<String> {
    let mut texts: Vec<String> = names.into_iter().map(str::to_owned).collect();
    texts.sort();
    texts.dedup();
    texts
}

/// Entries of `first` absent from `second`, preserving sorted order.
#[must_use]
fn names_difference(first: &[String], second: &[String]) -> Vec<String> {
    first
        .iter()
        .filter(|name| !second.contains(name))
        .cloned()
        .collect()
}

/// Human-readable scope half of the expected query: `Global`, `Any`,
/// `Class(Type)`, or `Instance(Type, id)`.
#[must_use]
fn render_scope(filter: ResourceFilter<'_>) -> String {
    match filter {
        ResourceFilter::Global => "Global".to_owned(),
        ResourceFilter::Any => "Any".to_owned(),
        ResourceFilter::Class(type_name) => format!("Class({type_name})"),
        ResourceFilter::Instance(type_name, resource_id) => {
            format!("Instance({type_name}, {resource_id})")
        }
    }
}

/// Human-readable held row: `name (global)`, `name (class Type)`, or
/// `name (instance Type#id)`.
#[must_use]
fn render_role_record(record: &RoleRecord) -> String {
    match (&record.resource_type, &record.resource_id) {
        (None, None) => format!("{} (global)", record.name),
        (Some(type_name), None) => format!("{} (class {type_name})", record.name),
        (Some(type_name), Some(resource_id)) => {
            format!("{} (instance {type_name}#{resource_id})", record.name)
        }
        (None, Some(resource_id)) => format!("{} (instance #{resource_id})", record.name),
    }
}

#[cfg(test)]
mod tests {
    use super::{RoleAssertions, build_assertion_message};
    use crate::InMemoryStore;
    use rolify_core::config::RolifyConfig;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::user::RolifyUser;

    struct Player {
        id: i64,
        store: InMemoryStore,
        conn: (),
        config: RolifyConfig,
    }

    impl Player {
        fn fresh(holder_id: i64) -> Self {
            Self {
                id: holder_id,
                store: InMemoryStore::new(),
                conn: (),
                config: RolifyConfig::default(),
            }
        }

        fn strict(holder_id: i64) -> Self {
            Self {
                id: holder_id,
                store: InMemoryStore::new(),
                conn: (),
                config: RolifyConfig::builder()
                    .strict(true)
                    .build()
                    .expect("the strict test configuration always passes validation"),
            }
        }
    }

    impl RolifyUser for Player {
        type Store = InMemoryStore;

        fn store(&mut self) -> &mut InMemoryStore {
            &mut self.store
        }
        fn rolify_config(&self) -> &RolifyConfig {
            &self.config
        }
        fn rolify_id(&self) -> ResourceId {
            ResourceId::from(self.id)
        }
        fn rolify_type() -> &'static str {
            "Player"
        }
        fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
            (&mut self.store, &mut self.conn)
        }
    }

    fn role_name(raw: &str) -> RoleName {
        RoleName::from(raw)
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn granted_global_admin_satisfies_assert_has_role() {
        let mut player = Player::fresh(1);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .assert_has_role(
                &role_name("admin"),
                ResourceFilter::Global,
                "seeded global admin",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "holder_id = 1")]
    async fn missing_role_panics_naming_holder_and_expectation() {
        let mut player = Player::fresh(1);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .assert_has_role(
                &role_name("ghost"),
                ResourceFilter::Global,
                "missing role probe",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn assert_has_no_role_passes_on_empty_holder() {
        let mut player = Player::fresh(2);
        player
            .assert_has_no_role(
                &role_name("ghost"),
                ResourceFilter::Any,
                "empty holder holds nothing",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "expected_name = admin")]
    async fn assert_has_no_role_panics_once_the_role_is_granted() {
        let mut player = Player::fresh(2);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .assert_has_no_role(
                &role_name("admin"),
                ResourceFilter::Global,
                "granted role probe",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn class_grant_satisfies_instance_filtered_assert() {
        let mut player = Player::fresh(3);
        let forum_id = ResourceId::from(7_i64);
        player
            .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        player
            .assert_has_role(
                &role_name("moderator"),
                ResourceFilter::Instance("Forum", &forum_id),
                "class covers instance",
            )
            .await;
        player
            .assert_has_no_role(
                &role_name("moderator"),
                ResourceFilter::Class("Group"),
                "other class untouched",
            )
            .await;
    }

    #[test]
    fn panic_message_carries_holder_expected_held_and_context() {
        let held = vec![
            RoleRecord::global("admin"),
            RoleRecord::for_class("moderator", "Forum"),
        ];
        let expected = role_name("ghost");
        let holder = ResourceId::from(9_i64);
        let query = RoleQuery {
            name: &expected,
            filter: ResourceFilter::Global,
        };
        let message = build_assertion_message(
            "assert_has_role",
            "Player",
            &holder,
            &query,
            &held,
            "probe context",
            None,
        );
        assert!(
            message.contains("holder_type = Player"),
            "names the holder type: {message}"
        );
        assert!(
            message.contains("holder_id = 9"),
            "names the holder id: {message}"
        );
        assert!(
            message.contains("expected_name = ghost"),
            "names the expected role: {message}"
        );
        assert!(
            message.contains("expected_scope = Global"),
            "names the expected scope: {message}"
        );
        assert!(
            message.contains("admin (global)"),
            "lists the held global role: {message}"
        );
        assert!(
            message.contains("moderator (class Forum)"),
            "lists the held class role: {message}"
        );
        assert!(
            message.contains("context = probe context"),
            "carries the caller context: {message}"
        );
        assert!(
            !message.contains("store_error"),
            "no error line without a store failure: {message}"
        );
    }

    #[test]
    fn panic_message_includes_store_error_when_present() {
        let expected = role_name("ghost");
        let holder = ResourceId::from(9_i64);
        let query = RoleQuery {
            name: &expected,
            filter: ResourceFilter::Any,
        };
        let message = build_assertion_message(
            "assert_has_role",
            "Player",
            &holder,
            &query,
            &[],
            "probe context",
            Some("connection refused".to_owned()),
        );
        assert!(
            message.contains("store_error = connection refused"),
            "renders the store failure: {message}"
        );
        assert!(
            message.contains("held_roles = []"),
            "renders the unreadable held list as empty: {message}"
        );
    }

    #[test]
    fn panic_message_renders_instance_held_rows() {
        let forum_id = ResourceId::from(7_i64);
        let held = vec![RoleRecord::for_instance(
            "moderator",
            "Forum",
            forum_id.clone(),
        )];
        let expected = role_name("moderator");
        let holder = ResourceId::from(9_i64);
        let query = RoleQuery {
            name: &expected,
            filter: ResourceFilter::Instance("Forum", &forum_id),
        };
        let message = build_assertion_message(
            "assert_has_role",
            "Player",
            &holder,
            &query,
            &held,
            "probe context",
            None,
        );
        assert!(
            message.contains("expected_scope = Instance(Forum, 7)"),
            "renders the instance scope: {message}"
        );
        assert!(
            message.contains("moderator (instance Forum#7)"),
            "renders the held instance row: {message}"
        );
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn strict_assert_passes_on_exact_class_grant() {
        let mut player = Player::fresh(11);
        player
            .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        player
            .assert_has_strict_role(
                &role_name("moderator"),
                ResourceFilter::Class("Forum"),
                "exact class grant",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "expected_name = admin")]
    async fn strict_assert_panics_when_only_global_override_exists() {
        let mut player = Player::fresh(11);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .assert_has_strict_role(
                &role_name("admin"),
                ResourceFilter::Class("Forum"),
                "global is not the class row",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn all_assert_passes_on_two_held_roles() {
        let mut player = Player::fresh(12);
        let admin = role_name("admin");
        let moderator = role_name("moderator");
        player.add_role(&admin, ResourceRef::Global).await.unwrap();
        player
            .add_role(&moderator, ResourceRef::Class("Forum"))
            .await
            .unwrap();
        let queries = [
            RoleQuery::with_role(&admin),
            RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
        ];
        player
            .assert_has_all_roles(&queries, "both grants held")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "ghost")]
    async fn all_assert_panics_naming_the_missing_query() {
        let mut player = Player::fresh(12);
        let admin = role_name("admin");
        let ghost = role_name("ghost");
        player.add_role(&admin, ResourceRef::Global).await.unwrap();
        let queries = [RoleQuery::with_role(&admin), RoleQuery::with_role(&ghost)];
        player
            .assert_has_all_roles(&queries, "one query misses")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn any_assert_passes_on_one_hit_of_two_queries() {
        let mut player = Player::fresh(13);
        let admin = role_name("admin");
        let ghost = role_name("ghost");
        player.add_role(&admin, ResourceRef::Global).await.unwrap();
        let queries = [RoleQuery::with_role(&ghost), RoleQuery::with_role(&admin)];
        player
            .assert_has_any_roles(&queries, "one hit suffices")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn any_assert_stays_non_strict_under_strict_config() {
        let mut player = Player::strict(13);
        let admin = role_name("admin");
        let ghost = role_name("ghost");
        player.add_role(&admin, ResourceRef::Global).await.unwrap();
        let queries = [
            RoleQuery::with_role(&ghost),
            RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum")),
        ];
        player
            .assert_has_any_roles(&queries, "global override answers inside any")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn only_assert_passes_on_solo_role() {
        let mut player = Player::fresh(14);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .assert_only_has_role(&role_name("admin"), ResourceFilter::Global, "solo role")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "assert_only_has_role")]
    async fn only_assert_panics_on_two_role_holder() {
        let mut player = Player::fresh(14);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        player
            .assert_only_has_role(
                &role_name("admin"),
                ResourceFilter::Global,
                "second role breaks only",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn names_assert_passes_regardless_of_grant_order() {
        let mut player = Player::fresh(15);
        player
            .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        let expected = [role_name("admin"), role_name("moderator")];
        player
            .assert_role_names(&expected, "order-insensitive set match")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "unexpected_names")]
    async fn names_assert_panics_showing_the_unexpected_entry() {
        let mut player = Player::fresh(15);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .add_role(&role_name("ghost"), ResourceRef::Global)
            .await
            .unwrap();
        let expected = [role_name("admin"), role_name("moderator")];
        player
            .assert_role_names(&expected, "ghost is unexpected")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn every_negative_passes_on_empty_holder() {
        let mut player = Player::fresh(21);
        let admin = role_name("admin");
        let class_query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"));
        player
            .assert_has_no_strict_role(&admin, ResourceFilter::Class("Forum"), "nothing granted")
            .await;
        player
            .assert_has_no_all_roles(core::slice::from_ref(&class_query), "nothing granted")
            .await;
        player
            .assert_has_no_any_roles(core::slice::from_ref(&class_query), "nothing granted")
            .await;
        player
            .assert_has_no_only_role(&admin, ResourceFilter::Global, "nothing granted")
            .await;
        let expected = [role_name("admin")];
        player
            .assert_has_no_role_names(&expected, "empty set differs")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "assert_has_no_strict_role")]
    async fn no_strict_panics_once_the_strict_role_is_granted() {
        let mut player = Player::fresh(22);
        player
            .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
            .await
            .unwrap();
        player
            .assert_has_no_strict_role(
                &role_name("moderator"),
                ResourceFilter::Class("Forum"),
                "strictly held",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn no_all_passes_when_one_query_misses() {
        let mut player = Player::fresh(23);
        let admin = role_name("admin");
        let ghost = role_name("ghost");
        player.add_role(&admin, ResourceRef::Global).await.unwrap();
        let queries = [RoleQuery::with_role(&admin), RoleQuery::with_role(&ghost)];
        player
            .assert_has_no_all_roles(&queries, "one query misses")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "assert_has_no_all_roles")]
    async fn no_all_panics_when_both_queries_hold() {
        let mut player = Player::fresh(23);
        let admin = role_name("admin");
        let moderator = role_name("moderator");
        player.add_role(&admin, ResourceRef::Global).await.unwrap();
        player
            .add_role(&moderator, ResourceRef::Class("Forum"))
            .await
            .unwrap();
        let queries = [
            RoleQuery::with_role(&admin),
            RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
        ];
        player
            .assert_has_no_all_roles(&queries, "both queries hold")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn no_any_passes_on_empty_query_slice() {
        let mut player = Player::fresh(24);
        player
            .assert_has_no_any_roles(&[], "empty slice matches nothing")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "assert_has_no_any_roles")]
    async fn no_any_panics_once_a_query_hits() {
        let mut player = Player::fresh(24);
        let admin = role_name("admin");
        let ghost = role_name("ghost");
        player.add_role(&admin, ResourceRef::Global).await.unwrap();
        let queries = [RoleQuery::with_role(&ghost), RoleQuery::with_role(&admin)];
        player
            .assert_has_no_any_roles(&queries, "one query hits")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "assert_has_no_only_role")]
    async fn no_only_panics_on_solo_role() {
        let mut player = Player::fresh(25);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        player
            .assert_has_no_only_role(
                &role_name("admin"),
                ResourceFilter::Global,
                "solo role is only",
            )
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn no_names_passes_when_sets_differ() {
        let mut player = Player::fresh(26);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        let expected = [role_name("admin"), role_name("moderator")];
        player
            .assert_has_no_role_names(&expected, "moderator missing")
            .await;
    }

    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    #[should_panic(expected = "assert_has_no_role_names")]
    async fn no_names_panics_when_sets_match() {
        let mut player = Player::fresh(26);
        player
            .add_role(&role_name("admin"), ResourceRef::Global)
            .await
            .unwrap();
        let expected = [role_name("admin")];
        player
            .assert_has_no_role_names(&expected, "exact set unexpectedly held")
            .await;
    }
}
