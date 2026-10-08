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
    let held_text = held
        .iter()
        .map(render_role_record)
        .collect::<Vec<String>>()
        .join(", ");
    let error_line = store_error
        .map(|error_text| format!("store_error = {error_text}\n"))
        .unwrap_or_default();
    format!(
        "operation = {operation}\nholder_type = {holder_type}\nholder_id = {holder_id}\nexpected_name = {}\nexpected_scope = {}\nheld_roles = [{held_text}]\n{error_line}context = {context}",
        expected.name,
        render_scope(expected.filter),
    )
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
            .assert_has_no_role(&role_name("ghost"), ResourceFilter::Any, "empty holder holds nothing")
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
}
