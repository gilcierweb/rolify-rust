//! Tracer test: end-to-end grant/check/revoke lifecycle on SQLite (local convenience leg).
//!
//! SQLite is a non-parity-gate leg (D-14): it runs locally without Docker for fast feedback.
//! Concurrency test is excluded (SQLite single-writer, SQLITE_BUSY makes it meaningless).

#![cfg(all(feature = "sync", feature = "sqlite"))]

use diesel::Connection;
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::RemovalTarget;
use rolify_diesel::{DieselStore, MIGRATIONS};

use crate::support::{reset_roles, setup_fixtures, sqlite_conn};

#[test]
fn tracer_grant_check_revoke_lifecycle() {
    let mut conn = sqlite_conn();

    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations apply");
    setup_fixtures(&mut conn);

    let config = RolifyConfig::builder().build().unwrap();
    let mut store = DieselStore::new(&config);

    let user_id = crate::support::insert_holder(&mut conn, "users", "User", "test_user");

    // Grant GLOBAL role "admin"
    let admin_global = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by global admin");
    assert!(admin_global.is_global());
    let added = store
        .add(&mut conn, &user_id, &admin_global)
        .expect("add global admin");
    assert!(added);
    let added_again = store
        .add(&mut conn, &user_id, &admin_global)
        .expect("add global admin again");
    assert!(!added_again);

    // Grant CLASS role "manager" on Forum
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

    // Grant INSTANCE role "moderator" on Forum#42
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

    // CHECK: non-strict ladder queries
    let admin_query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Global,
    };
    let roles = store
        .where_(&mut conn, &user_id, &admin_query)
        .expect("where_ global admin");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_global());

    let class_query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_(&mut conn, &user_id, &class_query)
        .expect("where_ class sees global");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_global());

    let inst_query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_(&mut conn, &user_id, &inst_query)
        .expect("where_ instance sees global");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_global());

    let class_query = RoleQuery {
        name: &RoleName::from("manager"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_(&mut conn, &user_id, &class_query)
        .expect("where_ class manager");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_class_scoped_to("Forum"));

    let inst_query = RoleQuery {
        name: &RoleName::from("manager"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_(&mut conn, &user_id, &inst_query)
        .expect("where_ instance sees class");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_class_scoped_to("Forum"));

    let inst_query = RoleQuery {
        name: &RoleName::from("moderator"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_(&mut conn, &user_id, &inst_query)
        .expect("where_ instance moderator");
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_instance_scoped_to("Forum", &forum_42));

    let class_query = RoleQuery {
        name: &RoleName::from("moderator"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_(&mut conn, &user_id, &class_query)
        .expect("where_ class no instance");
    assert_eq!(roles.len(), 0);

    // CHECK: strict queries
    let strict_global = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Global,
    };
    let roles = store
        .where_strict(&mut conn, &user_id, &strict_global)
        .expect("where_strict global");
    assert_eq!(roles.len(), 1);

    let strict_class = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Class("Forum"),
    };
    let roles = store
        .where_strict(&mut conn, &user_id, &strict_class)
        .expect("where_strict class");
    assert_eq!(roles.len(), 0);

    let strict_inst = RoleQuery {
        name: &RoleName::from("manager"),
        filter: ResourceFilter::Instance("Forum", &forum_42),
    };
    let roles = store
        .where_strict(&mut conn, &user_id, &strict_inst)
        .expect("where_strict instance");
    assert_eq!(roles.len(), 0);

    // CHECK: where_any
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
    assert_eq!(roles.len(), 1);

    // CHECK: exists
    let has_scoped = store
        .exists(
            &mut conn,
            &user_id,
            rolify_core::store::ScopeColumn::ResourceType,
        )
        .expect("exists resource_type");
    assert!(has_scoped);
    let has_instance = store
        .exists(
            &mut conn,
            &user_id,
            rolify_core::store::ScopeColumn::ResourceId,
        )
        .expect("exists resource_id");
    assert!(has_instance);

    // CHECK: roles_of
    let all_roles = store.roles_of(&mut conn, &user_id).expect("roles_of");
    assert_eq!(all_roles.len(), 3);

    // IDEMPOTENCE: find_or_create_by
    let admin_global_2 = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by global admin again");
    assert_eq!(admin_global, admin_global_2);

    let count: i64 = diesel::sql_query("SELECT COUNT(*) FROM roles WHERE name = 'admin' AND resource_type = '' AND resource_id = ''")
        .get_result(&mut conn)
        .expect("count admin rows");
    assert_eq!(count, 1);

    // IDEMPOTENCE: add
    let added_third = store
        .add(&mut conn, &user_id, &admin_global)
        .expect("add admin third time");
    assert!(!added_third);

    let link_count: i64 = diesel::sql_query("SELECT COUNT(*) FROM users_roles WHERE user_id = ? AND role_id = (SELECT id FROM roles WHERE name = 'admin' AND resource_type = '' AND resource_id = '')")
        .bind::<diesel::sql_types::Text, _>(user_id.as_str())
        .get_result(&mut conn)
        .expect("count links");
    assert_eq!(link_count, 1);

    // REMOVE: transactional with orphan sweep
    let outcome = store
        .remove(
            &mut conn,
            &user_id,
            &RoleName::from("moderator"),
            RemovalTarget::Exact("Forum", &forum_42),
            true,
        )
        .expect("remove instance moderator");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(outcome.removed_roles.len(), 1);
    assert_eq!(outcome.removed_roles[0].name.as_str(), "moderator");

    let count: i64 = diesel::sql_query("SELECT COUNT(*) FROM roles WHERE name = 'moderator' AND resource_type = 'Forum' AND resource_id = '42'")
        .get_result(&mut conn)
        .expect("count moderator rows");
    assert_eq!(count, 0);

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
    assert_eq!(outcome.removed_roles.len(), 1);
    assert_eq!(outcome.removed_roles[0].name.as_str(), "manager");

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
    assert_eq!(outcome.removed_roles.len(), 1);
    assert_eq!(outcome.removed_roles[0].name.as_str(), "admin");

    let all_roles = store
        .roles_of(&mut conn, &user_id)
        .expect("roles_of after remove all");
    assert_eq!(all_roles.len(), 0);

    // ORPHAN SWEEP EDGE CASE
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
    assert_eq!(outcome.removed_roles.len(), 0);

    let count: i64 = diesel::sql_query("SELECT COUNT(*) FROM roles WHERE name = 'editor' AND resource_type = '' AND resource_id = ''")
        .get_result(&mut conn)
        .expect("count editor rows");
    assert_eq!(count, 1);

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
    assert_eq!(outcome.removed_roles.len(), 1);
    assert_eq!(outcome.removed_roles[0].name.as_str(), "editor");

    // BYTE-EXACT ROLE NAME SEMANTICS
    reset_roles(&mut conn);
    let user = crate::support::insert_holder(&mut conn, "users", "User", "user");

    let admin_cap = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("Admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by Admin");
    store.add(&mut conn, &user, &admin_cap).expect("add Admin");

    let query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Global,
    };
    let roles = store
        .where_(&mut conn, &user, &query)
        .expect("where_ lowercase admin");
    assert_eq!(roles.len(), 0, "byte-exact: 'admin' != 'Admin'");

    let admin_low = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by admin");
    store.add(&mut conn, &user, &admin_low).expect("add admin");

    let count: i64 = diesel::sql_query("SELECT COUNT(*) FROM roles WHERE name IN ('Admin', 'admin') AND resource_type = '' AND resource_id = ''")
        .get_result(&mut conn)
        .expect("count admin rows");
    assert_eq!(
        count, 2,
        "byte-exact: 'Admin' and 'admin' are two distinct rows"
    );

    reset_roles(&mut conn);
}

// Note: Concurrency test is EXCLUDED for SQLite (D-14)
// SQLite is single-writer; SQLITE_BUSY makes concurrent tests meaningless.
// The parity gate is Postgres + MySQL only.
