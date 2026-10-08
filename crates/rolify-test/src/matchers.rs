//! [`RoleAssertions`] - consumer test assertions over [`RolifyUser`].
//!
//! RED: the test module below pins the tracer contract first; the trait,
//! the provided assert methods, and the shared panic-message builder land
//! in GREEN.

#[cfg(test)]
mod tests {
    use super::{RoleAssertions, build_assertion_message};
    use crate::InMemoryStore;
    use rolify_core::config::RolifyConfig;
    use rolify_core::query::ResourceFilter;
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
        let message = build_assertion_message(
            "assert_has_role",
            "Player",
            &holder,
            &expected,
            ResourceFilter::Global,
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
        let message = build_assertion_message(
            "assert_has_role",
            "Player",
            &holder,
            &expected,
            ResourceFilter::Any,
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
        let message = build_assertion_message(
            "assert_has_role",
            "Player",
            &holder,
            &expected,
            ResourceFilter::Instance("Forum", &forum_id),
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
