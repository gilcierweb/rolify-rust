//! Tracer test: end-to-end "grant global then class then instance role, read it back"
//! on real Postgres — one path through every layer.
//
//! This is the Phase 3 tracer slice (Task 1): it exercises the complete
//! grant/check/remove lifecycle through the public `RoleStore` SPI against
//! a real Postgres container. It validates:
//! - Migrations apply cleanly
//! - `find_or_create_by` SELECT-first + catch-re-read (D-04)
//! - `add` idempotent link creation with UNIQUE catch-and-ignore (D-05)
//! - `remove` transactional with orphan sweep (PITFALL 2 / SC-3)
//! - Ladder queries match gem semantics 1:1 (D-06)
//! - Sentinel `''` storage, never NULL (D-01/D-02)
//! - Byte-exact role name comparison
//! - No interpolated VALUES in SQL (only quoted identifiers + ordered binds)

#![cfg(all(feature = "sync", feature = "postgres"))]

mod support;

use diesel::Connection;
use diesel::RunQueryDsl;
use diesel_migrations::MigrationHarness;
use rolify_core::config::RolifyConfig;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::RoleStore;
use rolify_diesel::rows::CountRow;
use rolify_diesel::{DieselStore, MIGRATIONS};

use crate::support::{pg_conn, pg_container, reset_roles, setup_fixtures};

#[test]
fn tracer_grant_check_revoke_lifecycle() {
    // 1. Start container and get connection
    let _container = pg_container(); // ensure started
    let mut conn = pg_conn();

    // Serialized with the other tests sharing this database.
    let _serial = crate::support::SuiteGuard::acquire();
    // 2. Apply migrations
    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations apply");

    // 3. Setup fixture tables (users, forums, etc.)
    setup_fixtures(&mut conn);

    // 4. Create store (default tables: roles / users_roles)
    let config = RolifyConfig::builder().build().unwrap();
    let mut store = DieselStore::new(&config);

    // 5. Create a test holder (user)
    let user_id = crate::support::insert_holder(&mut conn, "users", "User", "test_user");

    // ============================================================
    // GRANT PHASE: add global, then class, then instance roles
    // ============================================================

    // 5a. Grant GLOBAL role "admin"
    let admin_global = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by global admin");
    assert!(admin_global.is_global());
    assert_eq!(admin_global.name.as_str(), "admin");

    let added = store
        .add(&mut conn, &user_id, &admin_global)
        .expect("add global admin");
    assert!(added, "first add creates link");

    // Second add is idempotent (D-05: UNIQUE catch-and-ignore)
    let added_again = store
        .add(&mut conn, &user_id, &admin_global)
        .expect("add global admin again");
    assert!(!added_again, "second add returns false (already linked)");

    // 5b. Grant CLASS role "manager" on Forum
    let manager_class = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("manager"),
            rolify_core::resource::ResourceRef::Class("Forum"),
        )
        .expect("find_or_create_by class manager");
    assert!(manager_class.is_class_scoped_to("Forum"));

    let added = store
        .add(&mut conn, &user_id, &manager_class)
        .expect("add class manager");
    assert!(added);

    // 5c. Grant INSTANCE role "moderator" on Forum#42
    let forum_42 = ResourceId::from("42");
    let moderator_inst = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("moderator"),
            rolify_core::resource::ResourceRef::Instance("Forum", &forum_42),
        )
        .expect("find_or_create_by instance moderator");
    assert!(moderator_inst.is_instance_scoped_to("Forum", &forum_42));

    let added = store
        .add(&mut conn, &user_id, &moderator_inst)
        .expect("add instance moderator");
    assert!(added);

    // ============================================================
    // CHECK PHASE: verify roles via ladder queries (where_, where_strict, where_any)
    // ============================================================

    // where_ (non-strict ladder): global overrides class/instance
    let admin_query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Global,
    };
    let roles = store
        .where_(&mut conn, &user_id, &admin_query)
        .expect("where_ global admin");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_global());

    // Global role visible through class query (override ladder)
    let class_query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_(&mut conn, &user_id, &class_query)
        .expect("where_ class sees global");
    assert_eq!(
        roles.len(),
        1,
        "global admin visible through class query (ladder override)"
    );
    assert!(roles[0].is_global());

    // Global role visible through instance query (override ladder)
    let inst_query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_(&mut conn, &user_id, &inst_query)
        .expect("where_ instance sees global");
    assert_eq!(
        roles.len(),
        1,
        "global admin visible through instance query (ladder override)"
    );
    assert!(roles[0].is_global());

    // Class role visible through class query
    let class_query = RoleQuery {
        name: &RoleName::from("manager"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_(&mut conn, &user_id, &class_query)
        .expect("where_ class manager");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_class_scoped_to("Forum"));

    // Class role visible through instance query (class covers instance)
    let inst_query = RoleQuery {
        name: &RoleName::from("manager"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_(&mut conn, &user_id, &inst_query)
        .expect("where_ instance sees class");
    assert_eq!(
        roles.len(),
        1,
        "class manager visible through instance query (ladder override)"
    );
    assert!(roles[0].is_class_scoped_to("Forum"));

    // Instance role only visible through exact instance query
    let inst_query = RoleQuery {
        name: &RoleName::from("moderator"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_(&mut conn, &user_id, &inst_query)
        .expect("where_ instance moderator");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_instance_scoped_to("Forum", &forum_42));

    // Instance role NOT visible through class query (reverse never holds)
    let class_query = RoleQuery {
        name: &RoleName::from("moderator"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_(&mut conn, &user_id, &class_query)
        .expect("where_ class no instance");
    assert_eq!(
        roles.len(),
        0,
        "instance role not visible through class query"
    );

    // where_strict (exact scope, no ladder)
    let strict_global = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Global,
    };
    let roles = store
        .where_strict(&mut conn, &user_id, &strict_global)
        .expect("where_strict global");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_global());

    // Strict class query does NOT see global role
    let strict_class = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_strict(&mut conn, &user_id, &strict_class)
        .expect("where_strict class");
    assert_eq!(roles.len(), 0, "strict class does not see global role");

    // Strict instance query does NOT see class role
    let strict_inst = RoleQuery {
        name: &RoleName::from("manager"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_strict(&mut conn, &user_id, &strict_inst)
        .expect("where_strict instance");
    assert_eq!(roles.len(), 0, "strict instance does not see class role");

    // where_any (OR-joined multi-query, single round-trip)
    let any_queries = [
        RoleQuery {
            name: &RoleName::from("ghost"),
            filter: ResourceFilter::Global,
        },
        RoleQuery {
            name: &RoleName::from("admin"),
            filter: ResourceFilter::Global,
        },
    ];
    let roles = store
        .where_any(&mut conn, &user_id, &any_queries)
        .expect("where_any");
    assert_eq!(roles.len(), 1, "where_any finds admin, not ghost");
    assert!(roles[0].is_global());

    // ============================================================
    // EXISTS PHASE
    // ============================================================

    // Has any scoped role (resource_type != '')
    let has_scoped = store
        .exists(
            &mut conn,
            &user_id,
            rolify_core::store::ScopeColumn::ResourceType,
        )
        .expect("exists resource_type");
    assert!(has_scoped, "user has class/instance roles");

    // Has any instance-scoped role (resource_id != '')
    let has_instance = store
        .exists(
            &mut conn,
            &user_id,
            rolify_core::store::ScopeColumn::ResourceId,
        )
        .expect("exists resource_id");
    assert!(has_instance, "user has instance role");

    // ============================================================
    // ROLES_OF PHASE
    // ============================================================

    let all_roles = store.roles_of(&mut conn, &user_id).expect("roles_of");
    assert_eq!(
        all_roles.len(),
        3,
        "user has 3 roles: global admin, class manager, instance moderator"
    );
    let names: Vec<&str> = all_roles.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"admin"));
    assert!(names.contains(&"manager"));
    assert!(names.contains(&"moderator"));

    // ============================================================
    // FIND_OR_CREATE_BY IDEMPOTENCE (D-04: SELECT-first + catch-re-read)
    // ============================================================

    // Second find_or_create_by for same triple returns same record, no duplicate row
    let admin_global_2 = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by global admin again");
    assert_eq!(
        admin_global, admin_global_2,
        "find_or_create_by is idempotent on triple"
    );

    // Verify only ONE role row exists for (admin, '', '')
    let count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM roles WHERE name = 'admin' AND resource_type = '' AND resource_id = ''")
        .get_result(&mut conn)
        .expect("count admin rows");
    assert_eq!(
        count_row.count, 1,
        "exactly one global admin role row (no duplicates)"
    );

    // ============================================================
    // ADD IDEMPOTENCE (D-05: UNIQUE catch-and-ignore on join)
    // ============================================================

    // Third add of same link returns false, link count stays 1
    let added_third = store
        .add(&mut conn, &user_id, &admin_global)
        .expect("add admin third time");
    assert!(!added_third);

    let link_count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM users_roles WHERE user_id = $1 AND role_id = (SELECT id FROM roles WHERE name = 'admin' AND resource_type = '' AND resource_id = '')")
        .bind::<diesel::sql_types::Text, _>(user_id.as_str())
        .get_result(&mut conn)
        .expect("count links");
    assert_eq!(link_count_row.count, 1, "exactly one link for admin (no duplicates)");

    // ============================================================
    // REMOVE PHASE: transactional with orphan sweep (PITFALL 2 / SC-3)
    // ============================================================

    // Remove instance role (Exact target) — role row should be deleted (orphan sweep)
    let outcome = store
        .remove(
            &mut conn,
            &user_id,
            &RoleName::from("moderator"),
            RemovalTarget::Exact("Forum", &forum_42),
            true, // remove_role_if_empty = true
        )
        .expect("remove instance moderator");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(
        outcome.removed_roles.len(),
        1,
        "orphan instance role row deleted"
    );
    assert_eq!(outcome.removed_roles[0].name.as_str(), "moderator");

    // Verify role row is gone
    let count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM roles WHERE name = 'moderator' AND resource_type = 'Forum' AND resource_id = '42'")
        .get_result(&mut conn)
        .expect("count moderator rows");
    assert_eq!(count_row.count, 0, "moderator role row deleted by orphan sweep");

    // Remove class role (TypeSweep) — class role has no other holders, so it should be deleted
    let outcome = store
        .remove(
            &mut conn,
            &user_id,
            &RoleName::from("manager"),
            RemovalTarget::TypeSweep("Forum"),
            true,
        )
        .expect("remove class manager");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(
        outcome.removed_roles.len(),
        1,
        "orphan class role row deleted"
    );
    assert_eq!(outcome.removed_roles[0].name.as_str(), "manager");

    // Remove global role (NameOnly) — global role has no other holders, so it should be deleted
    let outcome = store
        .remove(
            &mut conn,
            &user_id,
            &RoleName::from("admin"),
            RemovalTarget::NameOnly,
            true,
        )
        .expect("remove global admin");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(
        outcome.removed_roles.len(),
        1,
        "orphan global role row deleted"
    );
    assert_eq!(outcome.removed_roles[0].name.as_str(), "admin");

    // All roles gone
    let all_roles = store
        .roles_of(&mut conn, &user_id)
        .expect("roles_of after remove all");
    assert_eq!(all_roles.len(), 0);

    // ============================================================
    // ORPHAN SWEEP EDGE CASE: role with surviving holder NOT deleted
    // ============================================================

    // Reset and create two users with same role
    reset_roles(&mut conn);
    let user1 = crate::support::insert_holder(&mut conn, "users", "User", "user1");
    let user2 = crate::support::insert_holder(&mut conn, "users", "User", "user2");

    let editor = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("editor"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by editor");
    store
        .add(&mut conn, &user1, &editor)
        .expect("add editor to user1");
    store
        .add(&mut conn, &user2, &editor)
        .expect("add editor to user2");

    // Remove from user1 only — role row should survive (user2 still has it)
    let outcome = store
        .remove(
            &mut conn,
            &user1,
            &RoleName::from("editor"),
            RemovalTarget::NameOnly,
            true,
        )
        .expect("remove editor from user1");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(
        outcome.removed_roles.len(),
        0,
        "role row NOT deleted — user2 still holds it"
    );

    // Verify role row still exists
    let count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM roles WHERE name = 'editor' AND resource_type = '' AND resource_id = ''")
        .get_result(&mut conn)
        .expect("count editor rows");
    assert_eq!(count_row.count, 1, "editor role row survives");

    // Remove from user2 — now role row should be deleted
    let outcome = store
        .remove(
            &mut conn,
            &user2,
            &RoleName::from("editor"),
            RemovalTarget::NameOnly,
            true,
        )
        .expect("remove editor from user2");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(
        outcome.removed_roles.len(),
        1,
        "orphan role row deleted after last holder"
    );
    assert_eq!(outcome.removed_roles[0].name.as_str(), "editor");

    // ============================================================
    // BYTE-EXACT ROLE NAME SEMANTICS
    // ============================================================

    reset_roles(&mut conn);
    let user = crate::support::insert_holder(&mut conn, "users", "User", "user");

    // Add "Admin" (capital A)
    let admin_cap = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("Admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by Admin");
    store.add(&mut conn, &user, &admin_cap).expect("add Admin");

    // Query for "admin" (lowercase) — should NOT match (byte-exact)
    let query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Global,
    };
    let roles = store
        .where_(&mut conn, &user, &query)
        .expect("where_ lowercase admin");
    assert_eq!(roles.len(), 0, "byte-exact: 'admin' != 'Admin'");

    // Add "admin" — creates SEPARATE row (two rows total)
    let admin_low = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by admin");
    store.add(&mut conn, &user, &admin_low).expect("add admin");

    let count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM roles WHERE name IN ('Admin', 'admin') AND resource_type = '' AND resource_id = ''")
        .get_result(&mut conn)
        .expect("count admin rows");
    assert_eq!(
        count_row.count, 2,
        "byte-exact: 'Admin' and 'admin' are two distinct rows"
    );

    // Clean up
    reset_roles(&mut conn);
}

#[test]
fn tracer_concurrent_find_or_create_by_race() {
    // Concurrency test: two threads racing find_or_create_by on same triple
    // → exactly ONE role row created (D-04 catch-re-read + DB UNIQUE)
    // This test is PostgreSQL-only (SQLite single-writer, MySQL in CI)
    use std::sync::Arc;
    use std::thread;

    // Serialized with the other tests sharing this database.
    let _serial = crate::support::SuiteGuard::acquire();
    let _container = pg_container();
    let config = RolifyConfig::builder().build().unwrap();

    // Pre-create the role table by running one find_or_create_by
    {
        let mut conn = pg_conn();
        conn.run_pending_migrations(MIGRATIONS).expect("migrations");
        setup_fixtures(&mut conn);
        let mut store = DieselStore::new(&config);
        store
            .find_or_create_by(
                &mut conn,
                &RoleName::from("racer"),
                rolify_core::resource::ResourceRef::Global,
            )
            .expect("seed");
    }

    let config = Arc::new(config);
    let mut handles = vec![];

    for _ in 0..2 {
        let config = Arc::clone(&config);
        let handle = thread::spawn(move || {
            let mut conn = pg_conn();
            let mut store = DieselStore::new(&config);
            store
                .find_or_create_by(
                    &mut conn,
                    &RoleName::from("racer"),
                    rolify_core::resource::ResourceRef::Global,
                )
                .expect("concurrent find_or_create_by")
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    // Verify exactly ONE role row exists
    let mut conn = pg_conn();
    let count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM roles WHERE name = 'racer' AND resource_type = '' AND resource_id = ''")
        .get_result(&mut conn)
        .expect("count racer rows");
    assert_eq!(
        count_row.count, 1,
        "concurrent find_or_create_by created exactly one row"
    );
}
