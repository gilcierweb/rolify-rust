//! End-to-end SPI integration: the ENTIRE grant/query/revoke flow through
//! the public surface only (`rolify_core::*` public types + `rolify_test::
//! InMemoryStore` - deliberately no kernel-module imports; kernel parity is
//! proven transitively because the reference store delegates to it).
//!
//! Runs in both modes: `cargo test -p rolify-test` and
//! `cargo test -p rolify-test --features is_sync` (crate-native feature,
//! which forwards to `rolify-core/is_sync`).

use std::sync::{Arc, Mutex};

use rolify_core::RemovalTarget;
use rolify_core::config::RolifyConfig;
use rolify_core::error::RolifyError;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::{Resource, ResourceRef};
use rolify_core::role::{ResourceId, RoleName, RoleRecord, RoleSet};
use rolify_core::store::RoleStore;
use rolify_core::user::RolifyUser;
use rolify_test::InMemoryStore;

/// A player/account: the consumer-side holder. All grant/revoke traffic in
/// these tests flows through the `RolifyUser` provided methods.
struct Player {
    id: i64,
    store: InMemoryStore,
    conn: (),
    config: RolifyConfig,
}

impl Player {
    fn new(id: i64, config: RolifyConfig) -> Self {
        Self {
            id,
            store: InMemoryStore::new(),
            conn: (),
            config,
        }
    }
}

impl Resource for Player {
    fn type_name() -> &'static str {
        "Player"
    }
    fn resource_id(&self) -> ResourceId {
        ResourceId::from(self.id)
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
    fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
        (&mut self.store, &mut self.conn)
    }
}

fn role(name: &str) -> RoleName {
    RoleName::from(name)
}

/// SC-1 sample rows, end-to-end: grant via `add_role`, verify via the
/// query path (`has_role`) AND the cached path (`RoleSet` delegation) -
/// both must agree on every row.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn grant_then_query_and_cached_paths_agree() {
    let mut player = Player::new(1, RolifyConfig::default());
    let forum_seven = ResourceId::from(7_i64);

    player
        .add_role(&role("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .add_role(&role("manager"), ResourceRef::Class("Forum"))
        .await
        .unwrap();
    player
        .add_role(
            &role("moderator"),
            ResourceRef::Instance("Forum", &forum_seven),
        )
        .await
        .unwrap();

    for (name, filter, expected) in [
        // global override: satisfies every scope
        ("admin", ResourceFilter::Global, true),
        ("admin", ResourceFilter::Class("Forum"), true),
        (
            "admin",
            ResourceFilter::Instance("Forum", &forum_seven),
            true,
        ),
        ("admin", ResourceFilter::Any, true),
        // class covers instance, reverse never holds
        ("manager", ResourceFilter::Class("Forum"), true),
        (
            "manager",
            ResourceFilter::Instance("Forum", &forum_seven),
            true,
        ),
        ("manager", ResourceFilter::Global, false),
        ("manager", ResourceFilter::Class("Group"), false),
        // instance is exact
        (
            "moderator",
            ResourceFilter::Instance("Forum", &forum_seven),
            true,
        ),
        ("moderator", ResourceFilter::Class("Forum"), false),
        (
            "moderator",
            ResourceFilter::Instance("Forum", &ResourceId::from(8_i64)),
            false,
        ),
        // never granted
        ("ghost", ResourceFilter::Any, false),
    ] {
        let via_query = player.has_role(&role(name), filter).await.unwrap();

        let rows = {
            let holder = player.rolify_id();
            let (store, conn) = player.store_with_conn();
            store.roles_of(&mut *conn, &holder).await.unwrap()
        };
        let snapshot = RoleSet::new(&rows);
        let via_cache = player.has_cached_role(
            &snapshot,
            &RoleQuery {
                name: &role(name),
                filter,
            },
        );

        assert_eq!(via_query, expected, "query path: {name} / {filter:?}");
        assert_eq!(via_cache, expected, "cached path: {name} / {filter:?}");
    }
}

/// Strict mode flips outcomes ONLY on Class/Instance filters - Global and
/// Any are gate-immune (role.rb:26 gating, observable end-to-end).
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn strict_mode_flip_observable_only_on_scoped_filters() {
    let mut strict_player = Player::new(1, RolifyConfig::builder().strict(true).build().unwrap());
    strict_player
        .add_role(&role("admin"), ResourceRef::Global)
        .await
        .unwrap();

    let mut relaxed_player = Player::new(2, RolifyConfig::default());
    relaxed_player
        .add_role(&role("admin"), ResourceRef::Global)
        .await
        .unwrap();

    // Class/Instance: strict loses the global override.
    let strict_class = strict_player
        .has_role(&role("admin"), ResourceFilter::Class("Forum"))
        .await
        .unwrap();
    let relaxed_class = relaxed_player
        .has_role(&role("admin"), ResourceFilter::Class("Forum"))
        .await
        .unwrap();
    assert!(!strict_class);
    assert!(relaxed_class);

    // Global/Any: identical outcomes under both configurations.
    let strict_global = strict_player
        .has_role(&role("admin"), ResourceFilter::Global)
        .await
        .unwrap();
    let strict_any = strict_player
        .has_role(&role("admin"), ResourceFilter::Any)
        .await
        .unwrap();
    assert!(strict_global);
    assert!(strict_any);
}

/// CONF-05 end-to-end: a vetoing `before_add` returns the veto error,
/// leaves the store untouched (no row, no link), and `after_add` never runs.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn veto_end_to_end_blocks_the_grant() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let writer = Arc::clone(&events);
    let config = RolifyConfig::builder()
        .before_add(Arc::new(move |_record: &RoleRecord| {
            writer
                .lock()
                .expect("probe lock poisoned")
                .push("before_add");
            Err(RolifyError::CallbackVeto {
                callback: "before_add",
                reason: "policy".into(),
            })
        }))
        .after_add({
            let writer = Arc::clone(&events);
            Arc::new(move |_record: &RoleRecord| {
                writer
                    .lock()
                    .expect("probe lock poisoned")
                    .push("after_add");
            })
        })
        .build()
        .unwrap();
    let mut player = Player::new(1, config);

    let outcome = player.add_role(&role("admin"), ResourceRef::Global).await;

    assert!(matches!(outcome, Err(RolifyError::CallbackVeto { .. })));
    let check = player
        .has_role(&role("admin"), ResourceFilter::Any)
        .await
        .unwrap();
    assert!(!check, "nothing may become visible after a veto");
    assert_eq!(
        events.lock().expect("probe lock poisoned").as_slice(),
        ["before_add"]
    );

    // A non-vetoed second role succeeds normally (hooks not sticky).
    let mut clean = Player::new(2, RolifyConfig::default());
    clean
        .add_role(&role("admin"), ResourceRef::Global)
        .await
        .unwrap();
    let granted = clean
        .has_role(&role("admin"), ResourceFilter::Global)
        .await
        .unwrap();
    assert!(granted);
}

/// The subtle sweep, end-to-end: `TypeSweep("Forum")` removes the class row
/// AND the instance rows of that type - and nothing else.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn type_sweep_removes_class_and_instance_rows_of_that_type_only() {
    let mut player = Player::new(1, RolifyConfig::default());
    let forum_one = ResourceId::from(1_i64);

    player
        .add_role(&role("manager"), ResourceRef::Class("Forum"))
        .await
        .unwrap();
    player
        .add_role(&role("manager"), ResourceRef::Instance("Forum", &forum_one))
        .await
        .unwrap();
    player
        .add_role(&role("manager"), ResourceRef::Class("Group"))
        .await
        .unwrap();
    player
        .add_role(&role("admin"), ResourceRef::Global)
        .await
        .unwrap();

    let outcome = player
        .remove_role(&role("manager"), RemovalTarget::TypeSweep("Forum"))
        .await
        .unwrap();
    assert_eq!(outcome.removed_links, 2, "class row AND instance row swept");

    let remaining = player
        .has_role(&role("manager"), ResourceFilter::Any)
        .await
        .unwrap();
    let group_kept = player
        .has_role(&role("manager"), ResourceFilter::Class("Group"))
        .await
        .unwrap();
    let admin_kept = player
        .has_role(&role("admin"), ResourceFilter::Global)
        .await
        .unwrap();
    assert!(remaining, "the Group-class manager row still matches :any");
    assert!(group_kept, "other types are untouched");
    assert!(admin_kept, "other names are untouched");

    let forum_class_left = player
        .has_role(&role("manager"), ResourceFilter::Class("Forum"))
        .await
        .unwrap();
    assert!(!forum_class_left);
}
