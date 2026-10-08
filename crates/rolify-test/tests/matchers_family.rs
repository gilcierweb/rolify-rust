//! Family proof for TOOL-02 (D-04, D-05, D-06, D-10): ladder spot checks
//! (global override, class-covers-instance, strict exactness, Any includes
//! global), every negative mirror, cached-vs-queried agreement on one
//! holder, and names-set order-insensitivity, through the public surface
//! only (`rolify_core::*` types plus `rolify_test::` mock and assertions).
//!
//! No resource-side matchers exist by design (D-10): resource reads stay
//! plain `assert!` on query results, so this file pins the user-side
//! surface only.
//!
//! Runs in both modes with zero Docker: `cargo test -p rolify-test` and
//! `cargo test -p rolify-test --features is_sync`.

use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord, RoleSet};
use rolify_core::store::RoleStore;
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

    fn strict(holder_id: i64) -> Self {
        Self {
            id: holder_id,
            store: InMemoryStore::new(),
            conn: (),
            config: RolifyConfig::builder()
                .strict(true)
                .build()
                .expect("the strict proof configuration always passes validation"),
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

/// Live snapshot of one holder's linked rows for the cached-vs-queried
/// agreement checks.
#[maybe_async::maybe_async]
async fn live_snapshot(player: &mut Player) -> Vec<RoleRecord> {
    let holder = player.rolify_id();
    let (store, conn) = player.store_with_conn();
    store.roles_of(&mut *conn, &holder).await.unwrap()
}

/// Global override through the non-strict asserts: a global admin answers
/// the class ask and the any-folded ask.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn global_override_answers_non_strict_asserts() {
    let mut player = Player::fresh(40);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .assert_has_role(
            &role_name("admin"),
            ResourceFilter::Class("Forum"),
            "global override",
        )
        .await;
    let admin = role_name("admin");
    let queries = [RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Class("Forum"),
    )];
    player
        .assert_has_any_roles(&queries, "any includes the override")
        .await;
}

/// The strict assert rejects the global override the non-strict twin
/// accepts.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "expected_name = admin")]
async fn strict_assert_rejects_global_override() {
    let mut player = Player::fresh(40);
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

/// Class-covers-instance through the assert: a class grant answers the
/// instance ask.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn class_grant_covers_instance_assert() {
    let mut player = Player::fresh(41);
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
}

/// Strict exactness: a class grant satisfies the strict class assert.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn strict_assert_passes_on_exact_class_grant() {
    let mut player = Player::fresh(41);
    player
        .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
        .await
        .unwrap();
    player
        .assert_has_strict_role(
            &role_name("moderator"),
            ResourceFilter::Class("Forum"),
            "exact class row",
        )
        .await;
}

/// Any includes global: the `Any` filter answers a global grant through
/// both the single-role and the any-folded asserts.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn any_filter_includes_global_grant() {
    let mut player = Player::fresh(42);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .assert_has_role(
            &role_name("admin"),
            ResourceFilter::Any,
            "any includes global",
        )
        .await;
    let admin = role_name("admin");
    let queries = [RoleQuery::with_role(&admin)];
    player
        .assert_has_any_roles(&queries, "any fold includes global")
        .await;
}

/// The any assert stays non-strict even under a strict config: a global
/// grant answers the class-folded query.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn any_assert_stays_non_strict_under_strict_config() {
    let mut player = Player::strict(42);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    let admin = role_name("admin");
    let ghost = role_name("ghost");
    let queries = [
        RoleQuery::with_role(&ghost),
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum")),
    ];
    player
        .assert_has_any_roles(&queries, "any never engages strict")
        .await;
}

/// Only passes on a solo grant.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn only_assert_passes_on_solo_grant() {
    let mut player = Player::fresh(43);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .assert_only_has_role(&role_name("admin"), ResourceFilter::Global, "solo grant")
        .await;
}

/// Only panics once a second role lands.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "assert_only_has_role")]
async fn only_assert_panics_on_second_grant() {
    let mut player = Player::fresh(43);
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
            "second grant breaks only",
        )
        .await;
}

/// Names match regardless of grant order on both the store and the cached
/// paths.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn names_assert_matches_regardless_of_grant_order() {
    let mut player = Player::fresh(44);
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
        .assert_role_names(&expected, "order-insensitive store set")
        .await;
    let held = live_snapshot(&mut player).await;
    let snapshot = RoleSet::new(&held);
    player.assert_cached_role_names(&snapshot, &expected, "order-insensitive cached set");
}

/// Store asserts and cached asserts agree on one holder for has, strict,
/// all, and any (T-7-04: the twins delegate to the same kernel-backed
/// predicates, so a stale snapshot would fail here instead of passing).
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn store_and_cached_asserts_agree_on_one_holder() {
    let mut player = Player::fresh(45);
    let admin = role_name("admin");
    let moderator = role_name("moderator");
    let ghost = role_name("ghost");
    let forum_id = ResourceId::from(7_i64);
    player.add_role(&admin, ResourceRef::Global).await.unwrap();
    player
        .add_role(&moderator, ResourceRef::Class("Forum"))
        .await
        .unwrap();

    player
        .assert_has_role(
            &admin,
            ResourceFilter::Instance("Forum", &forum_id),
            "global override via store",
        )
        .await;
    player
        .assert_has_strict_role(
            &moderator,
            ResourceFilter::Class("Forum"),
            "exact class via store",
        )
        .await;
    let both = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
    ];
    player.assert_has_all_roles(&both, "both via store").await;
    let either = [RoleQuery::with_role(&ghost), RoleQuery::with_role(&admin)];
    player
        .assert_has_any_roles(&either, "one hit via store")
        .await;

    let held = live_snapshot(&mut player).await;
    let snapshot = RoleSet::new(&held);
    player.assert_has_cached_role(
        &snapshot,
        &admin,
        ResourceFilter::Instance("Forum", &forum_id),
        "global override via cache",
    );
    player.assert_has_strict_cached_role(
        &snapshot,
        &moderator,
        ResourceFilter::Class("Forum"),
        "exact class via cache",
    );
    player.assert_has_all_cached(&snapshot, &both, "both via cache");
    player.assert_has_any_cached(&snapshot, &either, "one hit via cache");
}

/// Only agrees between store and cached paths on a solo holder.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn only_agrees_between_store_and_cached_on_solo_holder() {
    let mut player = Player::fresh(46);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .assert_only_has_role(
            &role_name("admin"),
            ResourceFilter::Global,
            "solo via store",
        )
        .await;
    let held = live_snapshot(&mut player).await;
    let snapshot = RoleSet::new(&held);
    player.assert_only_has_cached(
        &snapshot,
        &role_name("admin"),
        ResourceFilter::Global,
        "solo via cache",
    );
}

/// Every store negative passes on an empty holder with context attached.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn every_store_negative_passes_on_empty_holder() {
    let mut player = Player::fresh(47);
    let admin = role_name("admin");
    let class_query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"));
    player
        .assert_has_no_role(&admin, ResourceFilter::Any, "nothing granted")
        .await;
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

/// Each store negative panics once the matching state is granted.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "assert_has_no_role")]
async fn store_negative_panics_on_granted_role() {
    let mut player = Player::fresh(48);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .assert_has_no_role(&role_name("admin"), ResourceFilter::Global, "granted admin")
        .await;
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "assert_has_no_strict_role")]
async fn store_negative_panics_on_strict_grant() {
    let mut player = Player::fresh(48);
    player
        .add_role(&role_name("moderator"), ResourceRef::Class("Forum"))
        .await
        .unwrap();
    player
        .assert_has_no_strict_role(
            &role_name("moderator"),
            ResourceFilter::Class("Forum"),
            "strictly granted",
        )
        .await;
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "assert_has_no_all_roles")]
async fn store_negative_panics_when_all_queries_hold() {
    let mut player = Player::fresh(49);
    let admin = role_name("admin");
    player.add_role(&admin, ResourceRef::Global).await.unwrap();
    let queries = [RoleQuery::with_role(&admin)];
    player
        .assert_has_no_all_roles(&queries, "every query holds")
        .await;
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "assert_has_no_any_roles")]
async fn store_negative_panics_when_any_query_hits() {
    let mut player = Player::fresh(49);
    let admin = role_name("admin");
    player.add_role(&admin, ResourceRef::Global).await.unwrap();
    let queries = [RoleQuery::with_role(&admin)];
    player
        .assert_has_no_any_roles(&queries, "one query hits")
        .await;
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "assert_has_no_only_role")]
async fn store_negative_panics_on_solo_grant() {
    let mut player = Player::fresh(50);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    player
        .assert_has_no_only_role(
            &role_name("admin"),
            ResourceFilter::Global,
            "solo grant is only",
        )
        .await;
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
#[should_panic(expected = "assert_has_no_role_names")]
async fn store_negative_panics_on_exact_name_set() {
    let mut player = Player::fresh(50);
    player
        .add_role(&role_name("admin"), ResourceRef::Global)
        .await
        .unwrap();
    let expected = [role_name("admin")];
    player
        .assert_has_no_role_names(&expected, "exact set unexpectedly held")
        .await;
}

/// Every cached negative passes on an empty snapshot.
#[test]
fn every_cached_negative_passes_on_empty_snapshot() {
    let player = Player::fresh(51);
    let admin = role_name("admin");
    let rows: [RoleRecord; 0] = [];
    let snapshot = RoleSet::new(&rows);
    let class_query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"));
    player.assert_has_no_cached_role(&snapshot, &admin, ResourceFilter::Global, "empty snapshot");
    player.assert_has_no_strict_cached_role(
        &snapshot,
        &admin,
        ResourceFilter::Class("Forum"),
        "empty snapshot",
    );
    player.assert_has_no_all_cached(
        &snapshot,
        core::slice::from_ref(&class_query),
        "empty snapshot",
    );
    player.assert_has_no_any_cached(
        &snapshot,
        core::slice::from_ref(&class_query),
        "empty snapshot",
    );
    player.assert_has_no_only_cached(&snapshot, &admin, ResourceFilter::Global, "empty snapshot");
    let expected = [role_name("admin")];
    player.assert_has_no_cached_role_names(&snapshot, &expected, "empty set differs");
}

/// Each cached negative panics once the snapshot holds the matching state.
#[test]
#[should_panic(expected = "assert_has_no_cached_role")]
fn cached_negative_panics_on_snapshot_hit() {
    let player = Player::fresh(52);
    let rows = [RoleRecord::global("admin")];
    let snapshot = RoleSet::new(&rows);
    player.assert_has_no_cached_role(
        &snapshot,
        &role_name("admin"),
        ResourceFilter::Global,
        "cached admin",
    );
}

#[test]
#[should_panic(expected = "assert_has_no_strict_cached_role")]
fn cached_negative_panics_on_strict_snapshot_hit() {
    let player = Player::fresh(52);
    let rows = [RoleRecord::for_class("moderator", "Forum")];
    let snapshot = RoleSet::new(&rows);
    player.assert_has_no_strict_cached_role(
        &snapshot,
        &role_name("moderator"),
        ResourceFilter::Class("Forum"),
        "strictly cached",
    );
}

#[test]
#[should_panic(expected = "assert_has_no_all_cached")]
fn cached_negative_panics_when_all_queries_hit_snapshot() {
    let player = Player::fresh(53);
    let admin = role_name("admin");
    let rows = [RoleRecord::global("admin")];
    let snapshot = RoleSet::new(&rows);
    let queries = [RoleQuery::with_role(&admin)];
    player.assert_has_no_all_cached(&snapshot, &queries, "every query cached");
}

#[test]
#[should_panic(expected = "assert_has_no_any_cached")]
fn cached_negative_panics_when_any_query_hits_snapshot() {
    let player = Player::fresh(53);
    let admin = role_name("admin");
    let ghost = role_name("ghost");
    let rows = [RoleRecord::global("admin")];
    let snapshot = RoleSet::new(&rows);
    let queries = [RoleQuery::with_role(&ghost), RoleQuery::with_role(&admin)];
    player.assert_has_no_any_cached(&snapshot, &queries, "one cached hit");
}

#[test]
#[should_panic(expected = "assert_has_no_only_cached")]
fn cached_negative_panics_on_solo_snapshot() {
    let player = Player::fresh(54);
    let rows = [RoleRecord::global("admin")];
    let snapshot = RoleSet::new(&rows);
    player.assert_has_no_only_cached(
        &snapshot,
        &role_name("admin"),
        ResourceFilter::Global,
        "solo cached role",
    );
}

#[test]
#[should_panic(expected = "assert_has_no_cached_role_names")]
fn cached_negative_panics_on_exact_cached_set() {
    let player = Player::fresh(54);
    let rows = [RoleRecord::global("admin")];
    let snapshot = RoleSet::new(&rows);
    let expected = [role_name("admin")];
    player.assert_has_no_cached_role_names(&snapshot, &expected, "exact cached set");
}
