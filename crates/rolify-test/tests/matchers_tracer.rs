//! Tracer proof for TOOL-02: the end-to-end consumer path through the
//! public surface only (`rolify_core::*` types plus `rolify_test::`
//! mock and assertions). Grants against [`InMemoryStore`], asserts through
//! [`RoleAssertions`](rolify_test::RoleAssertions), and pins the
//! full-context panic shape.
//!
//! Runs in both modes with zero Docker: `cargo test -p rolify-test` and
//! `cargo test -p rolify-test --features is_sync`.

use rolify_core::config::RolifyConfig;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_test::{InMemoryStore, RoleAssertions};

/// A player/account: the consumer-side holder. All traffic in these tests
/// flows through the `RolifyUser` provided methods plus `RoleAssertions`.
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

/// Grant at global plus class scope, assert both positives with a custom
/// context, assert the negative on an unheld name, and read the held names
/// back through the store.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn consumer_grant_then_assert_path() {
    let mut player = Player::fresh(7);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
        .await
        .unwrap();

    player
        .assert_has_role(
            &role_name("admin"),
            ResourceFilter::Global,
            "tracer global admin",
        )
        .await;
    player
        .assert_has_role(
            &role_name("moderator"),
            ResourceFilter::Class("Forum"),
            "tracer class moderator",
        )
        .await;
    player
        .assert_has_no_role(
            &role_name("ghost"),
            ResourceFilter::Any,
            "tracer never granted",
        )
        .await;

    let names = player.roles_name().await.unwrap();
    assert_eq!(names.len(), 2, "exactly the two granted names read back");
    assert!(
        names.contains(&role_name("admin")),
        "global admin reads back: {names:?}"
    );
    assert!(
        names.contains(&role_name("moderator")),
        "class moderator reads back: {names:?}"
    );
}

/// A missing role panics, and the message carries the holder id so the
/// failure points at the consumer row under test.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "holder_id = 7")]
async fn missing_role_panic_carries_holder_id() {
    let mut player = Player::fresh(7);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .assert_has_role(
            &role_name("ghost"),
            ResourceFilter::Global,
            "tracer panic shape",
        )
        .await;
}
