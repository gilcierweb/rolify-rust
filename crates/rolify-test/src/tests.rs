//! In-memory SPI grid tests - every assertion passes by kernel delegation
//! (TEST-03). Runs in BOTH modes via two cargo invocations:
//! `cargo test -p rolify-test` and
//! `cargo test --workspace --features rolify-core/is_sync` (the workspace
//! command forwards `is_sync` into this crate; a bare
//! `-p rolify-test --features rolify-core/is_sync` would leave this crate's
//! maybe-async in async shape against a sync core - run the workspace form).

use pretty_assertions::assert_eq;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord, RoleSet};
use rolify_core::store::{ResourceKey, ResourceStore, RoleStore, ScopeColumn};

use crate::InMemoryStore;

/// Shared seeded store: `alice` holds global `admin` + `manager`@Forum,
/// `bob` holds `moderator`@Forum#7. Also proves level-1 (triple) and
/// level-2 (link guard) dedupe. Flips sync/async with the `is_sync` mode.
#[maybe_async::maybe_async]
async fn seeded_store() -> (InMemoryStore, ResourceId, ResourceId, ResourceId) {
    let mut store = InMemoryStore::new();
    let alice = ResourceId::from(1_i64);
    let bob = ResourceId::from(2_i64);
    let forum_seven = ResourceId::from(7_i64);

    let admin = RoleName::from("admin");
    let manager = RoleName::from("manager");
    let moderator = RoleName::from("moderator");

    let admin_row = store
        .find_or_create_by(&mut (), &admin, ResourceRef::Global)
        .await
        .unwrap();
    let dup = store
        .find_or_create_by(&mut (), &admin, ResourceRef::Global)
        .await
        .unwrap();
    assert_eq!(admin_row, dup, "find_or_create_by dedupes on the triple");

    let manager_row = store
        .find_or_create_by(&mut (), &manager, ResourceRef::Class("Forum"))
        .await
        .unwrap();
    let moderator_row = store
        .find_or_create_by(
            &mut (),
            &moderator,
            ResourceRef::Instance("Forum", &forum_seven),
        )
        .await
        .unwrap();
    assert_eq!(store.assertion_len(), 3);

    let first_link = store.add(&mut (), &alice, &admin_row).await.unwrap();
    assert!(first_link);
    let second_link = store.add(&mut (), &alice, &admin_row).await.unwrap();
    assert!(!second_link, "link guard ignores the duplicate add");
    store.add(&mut (), &alice, &manager_row).await.unwrap();
    store.add(&mut (), &bob, &moderator_row).await.unwrap();
    assert_eq!(store.link_count(), 3);
    assert_eq!(store.link_count_for(&alice), 2);

    (store, alice, bob, forum_seven)
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn grid_non_strict_ladder() {
    let (store, alice, bob, forum_seven) = seeded_store().await;
    let admin = RoleName::from("admin");
    let manager = RoleName::from("manager");
    // global row satisfies every filter for alice (override ladders)
    for kind in [
        ResourceFilter::Global,
        ResourceFilter::Class("Forum"),
        ResourceFilter::Instance("Forum", &forum_seven),
        ResourceFilter::Any,
    ] {
        let query = RoleQuery {
            name: &admin,
            filter: kind,
        };
        let rows = store.where_(&mut (), &alice, &query).await.unwrap();
        let row_count = rows.len();
        assert_eq!(row_count, 1, "global admin must satisfy {kind:?}");
    }
    // not leaked to bob
    let query = RoleQuery {
        name: &admin,
        filter: ResourceFilter::Any,
    };
    let bob_rows = store.where_(&mut (), &bob, &query).await.unwrap();
    assert!(bob_rows.is_empty());
    // class covers instance
    let query = RoleQuery {
        name: &manager,
        filter: ResourceFilter::Instance("Forum", &forum_seven),
    };
    let covered = store.where_(&mut (), &alice, &query).await.unwrap();
    assert_eq!(covered.len(), 1);
    // reverse never holds
    let query = RoleQuery {
        name: &manager,
        filter: ResourceFilter::Global,
    };
    let reverse = store.where_(&mut (), &alice, &query).await.unwrap();
    assert!(reverse.is_empty());
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn grid_strict_predicates() {
    let (store, alice, bob, forum_seven) = seeded_store().await;
    let admin = RoleName::from("admin");
    let manager = RoleName::from("manager");
    let moderator = RoleName::from("moderator");

    // ---- strict predicates through the SPI ----
    let strict_class = RoleQuery {
        name: &manager,
        filter: ResourceFilter::Class("Forum"),
    };
    let strict_rows = store
        .where_strict(&mut (), &alice, &strict_class)
        .await
        .unwrap();
    assert_eq!(strict_rows.len(), 1);
    // strict: a global row does NOT satisfy a class query
    let strict_class_admin = RoleQuery {
        name: &admin,
        filter: ResourceFilter::Class("Forum"),
    };
    let strict_global = store
        .where_strict(&mut (), &alice, &strict_class_admin)
        .await
        .unwrap();
    assert!(strict_global.is_empty());
    // strict: exact instance only
    let strict_inst = RoleQuery {
        name: &moderator,
        filter: ResourceFilter::Instance("Forum", &forum_seven),
    };
    let strict_bob = store
        .where_strict(&mut (), &bob, &strict_inst)
        .await
        .unwrap();
    assert_eq!(strict_bob.len(), 1);

    // ---- where_any: ONE OR-folded path ----
    let any_name = RoleName::from("ghost");
    let queries = [
        RoleQuery {
            name: &any_name,
            filter: ResourceFilter::Any,
        },
        RoleQuery {
            name: &manager,
            filter: ResourceFilter::Class("Forum"),
        },
    ];
    let rows = store.where_any(&mut (), &alice, &queries).await.unwrap();
    let manager_class = RoleQuery {
        name: &manager,
        filter: ResourceFilter::Class("Forum"),
    };
    let expected = store
        .where_strict(&mut (), &alice, &manager_class)
        .await
        .unwrap();
    assert_eq!(rows, expected);

    // ---- exists / roles_of ----
    let alice_has_any_scope = store
        .exists(&mut (), &alice, ScopeColumn::ResourceType)
        .await
        .unwrap();
    assert!(alice_has_any_scope);
    let bob_has_instance_scope = store
        .exists(&mut (), &bob, ScopeColumn::ResourceId)
        .await
        .unwrap();
    assert!(bob_has_instance_scope);
    let alice_instance_scoped = store
        .exists(&mut (), &alice, ScopeColumn::ResourceId)
        .await
        .unwrap();
    assert!(!alice_instance_scoped);
    let alice_roles = store.roles_of(&mut (), &alice).await.unwrap();
    assert_eq!(alice_roles.len(), 2);
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn grid_removal_family_and_cleanup() {
    let (mut store, alice, bob, forum_seven) = seeded_store().await;
    let moderator = RoleName::from("moderator");

    // ---- removal family through the SPI ----
    // Exact sweep for bob
    let outcome = store
        .remove(
            &mut (),
            &bob,
            &moderator,
            RemovalTarget::Exact("Forum", &forum_seven),
            true,
        )
        .await
        .unwrap();
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(
        outcome.removed_roles.len(),
        1,
        "bob's last link empties the row"
    );
    assert_eq!(store.assertion_len(), 2);
    let bob_roles = store.roles_of(&mut (), &bob).await.unwrap();
    assert!(bob_roles.is_empty());

    // TypeSweep also sweeps instance rows: grant alice an instance row too
    let vip_row = store
        .find_or_create_by(
            &mut (),
            &RoleName::from("vip"),
            ResourceRef::Instance("Forum", &forum_seven),
        )
        .await
        .unwrap();
    store.add(&mut (), &alice, &vip_row).await.unwrap();
    let outcome = store
        .remove(
            &mut (),
            &alice,
            &RoleName::from("vip"),
            RemovalTarget::TypeSweep("Forum"),
            true,
        )
        .await
        .unwrap();
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(outcome.removed_roles.len(), 1);

    // remove without cleanup keeps the empty row
    let helper = RoleName::from("helper");
    let helper_row = store
        .find_or_create_by(&mut (), &helper, ResourceRef::Global)
        .await
        .unwrap();
    store.add(&mut (), &alice, &helper_row).await.unwrap();
    let outcome = store
        .remove(&mut (), &alice, &helper, RemovalTarget::NameOnly, false)
        .await
        .unwrap();
    assert_eq!(outcome.removed_links, 1);
    assert!(outcome.removed_roles.is_empty(), "flag off keeps the row");
    assert_eq!(store.assertion_len(), 3);
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn cached_and_queried_paths_agree() {
    let mut store = InMemoryStore::new();
    let holder = ResourceId::from(1_i64);
    let forum_id = ResourceId::from(7_i64);

    for row in [
        RoleRecord::global("admin"),
        RoleRecord::for_class("manager", "Forum"),
        RoleRecord::for_instance("moderator", "Forum", 7_i64),
    ] {
        let created = store
            .find_or_create_by(
                &mut (),
                &row.name,
                match (&row.resource_type, &row.resource_id) {
                    (None, None) => ResourceRef::Global,
                    (Some(type_name), None) => ResourceRef::Class(type_name),
                    (Some(type_name), Some(id)) => ResourceRef::Instance(type_name, id),
                    (None, Some(_)) => unreachable!("invalid triple in fixture"),
                },
            )
            .await
            .unwrap();
        store.add(&mut (), &holder, &created).await.unwrap();
    }

    let snapshot_rows = store.roles_of(&mut (), &holder).await.unwrap();
    let snapshot = RoleSet::new(&snapshot_rows);

    for name in ["admin", "manager", "moderator", "ghost"] {
        let name = RoleName::from(name);
        for kind in [
            ResourceFilter::Global,
            ResourceFilter::Class("Forum"),
            ResourceFilter::Instance("Forum", &forum_id),
            ResourceFilter::Any,
        ] {
            let query = RoleQuery {
                name: &name,
                filter: kind,
            };
            let queried_rows = store.where_(&mut (), &holder, &query).await.unwrap();
            let cached = snapshot.has_cached_role(&query);
            assert_eq!(
                !queried_rows.is_empty(),
                cached,
                "mismatch for {name} / {kind:?}"
            );
        }
    }
}

#[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
async fn resource_store_fixture_registry() {
    let mut store = InMemoryStore::new();
    let holder = ResourceId::from(1_i64);
    let forum_one = ResourceKey::new("Forum", 1_i64);
    let forum_two = ResourceKey::new("Forum", 2_i64);
    let group_nine = ResourceKey::new("Group", 9_i64);
    store.register_resource(forum_one.clone());
    store.register_resource(forum_two.clone());
    store.register_resource(group_nine.clone());

    let manager = RoleName::from("manager");
    let row_class = store
        .find_or_create_by(&mut (), &manager, ResourceRef::Class("Forum"))
        .await
        .unwrap();
    store.add(&mut (), &holder, &row_class).await.unwrap();

    // resources_find: class row covers both Forum instances, not Group
    let found = store
        .resources_find(&mut (), &["Forum"], &manager)
        .await
        .unwrap();
    assert_eq!(found.len(), 2);
    assert!(found.contains(&forum_one) && found.contains(&forum_two));

    // in_list: holder has manager at class scope -> both forums match
    let candidates = vec![forum_one.clone(), forum_two.clone(), group_nine];
    let found = store
        .in_list(
            &mut (),
            &candidates,
            &holder,
            std::slice::from_ref(&manager),
        )
        .await
        .unwrap();
    assert_eq!(
        found.len(),
        3,
        "gem `in` applies no resource_type condition"
    );
}
