//! Builders proof for TOOL-02 (D-12, D-13, D-14, D-15): the standard
//! preset seeds [`InMemoryStore`] through the public builder surface only
//! (`standard_preset` plus `apply_preset`), and the seeded state proves
//! through [`RoleAssertions`](rolify_test::RoleAssertions) positives plus
//! an unseeded-holder negative. The faker-gated submodule pins the
//! generator output shape without ever using generated values as lookup
//! keys (T-7-07).
//!
//! Runs in all three legs with zero Docker: `cargo test -p rolify-test
//! --test builders_proof`, plus `--features faker`, plus `--features
//! is_sync`.

use rolify_core::config::RolifyConfig;
use rolify_core::query::ResourceFilter;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_test::{InMemoryStore, RoleAssertions, apply_preset, standard_preset};

/// A preset-seeded account: the consumer-side holder. Seeding flows
/// through the builder free functions on the owned store; assertions
/// flow through `RoleAssertions`. The `id` field selects which preset
/// holder the assertions speak for.
struct PresetPlayer {
    id: i64,
    store: InMemoryStore,
    conn: (),
    config: RolifyConfig,
}

impl PresetPlayer {
    fn unseeded(player_id: i64) -> Self {
        Self {
            id: player_id,
            store: InMemoryStore::new(),
            conn: (),
            config: RolifyConfig::default(),
        }
    }
}

impl RolifyUser for PresetPlayer {
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
        "PresetPlayer"
    }
    fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
        (&mut self.store, &mut self.conn)
    }
}

/// The preset carries the deterministic holder graph: preset-admin 9001
/// with global admin plus class manager on Forum, preset-moderator 9002
/// with the Forum instance moderator grant.
#[test]
fn standard_preset_carries_deterministic_holder_graph() {
    let preset = standard_preset();
    assert_eq!(preset.len(), 2, "exactly the admin plus moderator holders");
    assert_eq!(preset[0].login, "preset-admin");
    assert_eq!(preset[0].holder_id, ResourceId::from(9001_i64));
    assert_eq!(preset[0].grants.len(), 2, "admin carries two grants");
    assert_eq!(preset[1].login, "preset-moderator");
    assert_eq!(preset[1].holder_id, ResourceId::from(9002_i64));
    assert_eq!(preset[1].grants.len(), 1, "moderator carries one grant");
}

/// Apply the preset to one shared store, then assert the seeded state
/// through the matcher family: admin positives, moderator instance
/// positive plus admin negative, and an unseeded holder with nothing.
#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn preset_seeds_store_and_asserts_through_matchers() {
    let mut player = PresetPlayer::unseeded(9001);
    let preset = standard_preset();
    {
        let (store, conn) = player.store_with_conn();
        let records = apply_preset(store, conn, &preset).await.unwrap();
        assert_eq!(
            records.len(),
            3,
            "two admin grants plus one moderator grant"
        );
    }

    player
        .assert_has_role(
            &RoleName::from("admin"),
            ResourceFilter::Global,
            "preset global admin",
        )
        .await;
    player
        .assert_has_role(
            &RoleName::from("manager"),
            ResourceFilter::Class("Forum"),
            "preset class manager",
        )
        .await;

    player.id = 9002;
    let forum_id = ResourceId::from(3_i64);
    player
        .assert_has_role(
            &RoleName::from("moderator"),
            ResourceFilter::Instance("Forum", &forum_id),
            "preset instance moderator",
        )
        .await;
    player
        .assert_has_no_role(
            &RoleName::from("admin"),
            ResourceFilter::Any,
            "moderator never granted admin",
        )
        .await;

    player.id = 7777;
    player
        .assert_has_no_role(
            &RoleName::from("admin"),
            ResourceFilter::Any,
            "never seeded",
        )
        .await;
    player
        .assert_has_no_role(
            &RoleName::from("moderator"),
            ResourceFilter::Any,
            "never seeded",
        )
        .await;
}

/// Faker generator shape (D-14, T-7-07): prefixed names plus distinct
/// ids, asserted as shape only. Generated values never serve as lookup
/// keys: every lookup above pins a literal.
#[cfg(feature = "faker")]
mod faker_shape {
    use rolify_test::builders::{fake_forum_id, fake_holder_id, fake_role_name};

    #[test]
    fn generators_pin_output_shape() {
        let first_id = fake_holder_id();
        let second_id = fake_holder_id();
        assert_ne!(
            first_id, second_id,
            "holder ids vary across calls: {first_id:?} vs {second_id:?}"
        );
        let generated_name = fake_role_name("reviewer");
        assert!(
            generated_name.as_str().starts_with("reviewer-"),
            "generated name keeps the caller prefix: {generated_name:?}"
        );
        assert!(
            !fake_forum_id().as_str().is_empty(),
            "forum id is a usable identifier"
        );
    }
}
