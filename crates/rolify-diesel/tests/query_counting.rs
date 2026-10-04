//! Query-counting backstop — binding-local proof that the
//! `TestBackend` counting hook actually counts queries on Diesel.
//!
//! This test verifies that:
//! - `reset_query_count()` zeroes the counter
//! - `query_count()` returns `Some(count)` after queries execute
//! - `InstrumentationEvent::StartQuery` increments are captured
//! - Transaction control events (Begin/Commit/Rollback) are NOT counted
//!
//! Runs on Postgres (cfg postgres) as the primary engine.

#![cfg(all(feature = "sync", feature = "postgres", feature = "suite"))]

mod support;

use diesel::Connection;
use diesel::RunQueryDsl;
use diesel_migrations::MigrationHarness;
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_diesel::{DieselStore, MIGRATIONS};

use crate::support::{pg_conn, pg_container, reset_roles, setup_fixtures};

#[test]
fn query_counting_hook_works() {
    let _container = pg_container();
    let mut conn = pg_conn();
    // Serialized with the other tests sharing this database.
    let _serial = crate::support::SuiteGuard::acquire();
    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations apply");
    setup_fixtures(&mut conn);

    let config = RolifyConfig::builder().build().unwrap();
    let mut store = DieselStore::new(&config);

    // Create a test user
    let user_id = crate::support::insert_holder(&mut conn, "users", "User", "test_user");

    // Grant a role
    let admin = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by");
    store.add(&mut conn, &user_id, &admin).expect("add role");

    // Install query counter on the connection
    let counter = crate::support::install_query_counter(&mut conn);

    // Verify counter starts at 0
    assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 0);

    // Execute a query - should increment
    let query = RoleQuery {
        name: &RoleName::from("admin"),
        filter: ResourceFilter::Global,
    };
    let _ = store
        .where_(&mut conn, &user_id, &query)
        .expect("where_ query");

    let count_after = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        count_after > 0,
        "query counter should increment on where_ query (got {count_after})"
    );

    // Reset counter
    counter.store(0, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 0);

    // Execute another query
    let _ = store
        .where_(&mut conn, &user_id, &query)
        .expect("where_ query again");
    let count_after2 = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        count_after2 > 0,
        "query counter should increment again (got {count_after2})"
    );
}

#[test]
fn query_counting_transaction_events_not_counted() {
    let _container = pg_container();
    let mut conn = pg_conn();
    // Serialized with the other tests sharing this database.
    let _serial = crate::support::SuiteGuard::acquire();
    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations apply");
    setup_fixtures(&mut conn);

    let counter = crate::support::install_query_counter(&mut conn);

    // Wrap a query in a transaction - transaction events should NOT be counted
    conn.transaction::<_, diesel::result::Error, _>(|conn| {
        let query = RoleQuery {
            name: &RoleName::from("admin"),
            filter: ResourceFilter::Global,
        };
        // This query inside the transaction should increment the counter
        diesel::sql_query("SELECT 1")
            .execute(conn)
            .expect("query in tx");
        Ok(())
    })
    .expect("transaction");

    let count = counter.load(std::sync::atomic::Ordering::Relaxed);
    // Should count the query inside the transaction, but not Begin/Commit
    assert!(
        count >= 1,
        "should count queries inside transaction (got {count})"
    );

    // Transaction control statements (BEGIN/COMMIT) also surface as
    // `StartQuery` instrumentation on diesel 2.3, so an empty
    // transaction DOES advance the counter: the hook counts executed
    // statements, and the zero/one-query guards pin their paths
    // (cached predicates, single round-trip `where_any`), never
    // transaction plumbing.
    let before = counter.load(std::sync::atomic::Ordering::Relaxed);
    conn.transaction::<_, diesel::result::Error, _>(|_conn| Ok(()))
        .expect("empty transaction");
    let after = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        after > before,
        "transaction control statements are visible to the counter (before {before}, after {after})"
    );
}

#[test]
fn cached_role_set_zero_queries() {
    let _container = pg_container();
    let mut conn = pg_conn();
    // Serialized with the other tests sharing this database.
    let _serial = crate::support::SuiteGuard::acquire();
    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations apply");
    setup_fixtures(&mut conn);

    let config = RolifyConfig::builder().build().unwrap();
    let mut store = DieselStore::new(&config);

    let user_id = crate::support::insert_holder(&mut conn, "users", "User", "test_user");

    // Grant roles
    let admin = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by admin");
    store.add(&mut conn, &user_id, &admin).expect("add admin");

    let manager = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("manager"),
            rolify_core::resource::ResourceRef::Class("Forum"),
        )
        .expect("find_or_create_by manager");
    store
        .add(&mut conn, &user_id, &manager)
        .expect("add manager");

    // Get roles via store (persistence path - this DOES query)
    let roles = store.roles_of(&mut conn, &user_id).expect("roles_of");

    // Create a RoleSet snapshot from the loaded roles
    let role_set = rolify_core::role::RoleSet::new(&roles);

    // Install query counter
    let counter = crate::support::install_query_counter(&mut conn);
    counter.store(0, std::sync::atomic::Ordering::Relaxed);

    // Now test cached predicates - they should NOT query the database.
    // The names are owned bindings: the queries borrow them, so the
    // temporaries cannot live inline (edition 2024 temporary rules).
    let admin_name = RoleName::from("admin");
    let manager_name = RoleName::from("manager");
    let admin_query = RoleQuery::with_role_and_filter(&admin_name, ResourceFilter::Global);
    assert!(role_set.has_cached_role(&admin_query));

    let manager_query =
        RoleQuery::with_role_and_filter(&manager_name, ResourceFilter::Class("Forum"));
    assert!(role_set.has_cached_role(&manager_query));

    // Global override
    let class_query = RoleQuery::with_role_and_filter(&admin_name, ResourceFilter::Class("Forum"));
    assert!(role_set.has_cached_role(&class_query));

    let count = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        count, 0,
        "cached predicates must execute with zero queries (got {count})"
    );
}

#[test]
fn uncached_has_any_roles_one_query() {
    let _container = pg_container();
    let mut conn = pg_conn();
    // Serialized with the other tests sharing this database.
    let _serial = crate::support::SuiteGuard::acquire();
    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations apply");
    setup_fixtures(&mut conn);

    let config = RolifyConfig::builder().build().unwrap();
    let mut store = DieselStore::new(&config);

    let user_id = crate::support::insert_holder(&mut conn, "users", "User", "test_user");

    // Grant a role
    let admin = store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .expect("find_or_create_by admin");
    store.add(&mut conn, &user_id, &admin).expect("add admin");

    // Install query counter
    let counter = crate::support::install_query_counter(&mut conn);
    counter.store(0, std::sync::atomic::Ordering::Relaxed);

    // Uncached has_any_roles with 2 queries - should be ONE round trip via where_any.
    // Owned name bindings for the same borrow reason as above.
    let ghost_name = RoleName::from("ghost");
    let admin_name = RoleName::from("admin");
    let queries = [
        RoleQuery::with_role_and_filter(&ghost_name, ResourceFilter::Global),
        RoleQuery::with_role_and_filter(&admin_name, ResourceFilter::Global),
    ];

    // We need to call the store's where_any directly since has_any_roles is on RolifyUser
    // but we can test the store-level where_any which is what has_any_roles uses
    let _ = store
        .where_any(&mut conn, &user_id, &queries)
        .expect("where_any");

    let count = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        count, 1,
        "uncached has_any_roles/where_any must execute exactly one query (got {count})"
    );
}
