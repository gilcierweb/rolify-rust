//! Executor uniformity acceptance tests in async mode (ROADMAP SC-5).
//!
//! Proves the same async `DieselStore` calls work identically across three
//! connection surfaces, mirroring the sync executor test leg for leg:
//! 1. Bare `AsyncPgConnection` (autocommit)
//! 2. `bb8` pool checkout (a reborrow of the inner connection)
//! 3. Caller-owned `AsyncConnection::transaction(async move |conn| ..)`
//!    with a mid-flight read INSIDE the transaction and cross-connection
//!    verification after commit
//! 4. Caller-owned transaction rollback: an `Err` inside the closure is
//!    fully absorbed, zero role or join rows leak (cross-connection verify)
//!
//! LEG 5 proves the async store surface is `Send` across await points
//! (SC-3 smoke for the rider): a full grant-then-read round trip runs
//! inside `tokio::spawn` and the result crosses the `JoinHandle`.
//!
//! Runs on Postgres (cfg-gated like the tracer). `SQLite` is excluded per
//! D-14: locking-sensitive executor acceptance is not meaningful on a
//! single-writer engine.
//!
//! Runs: `cargo test -p rolify-diesel --no-default-features --features async,bb8,postgres --test executor_async`

#![cfg(all(feature = "async", feature = "bb8", feature = "postgres"))]

mod support;

use diesel_async::{AsyncConnection, RunQueryDsl};
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_diesel::{DieselStore, rows::CountRow};

use crate::support::async_support::{
    insert_holder_async, pg_conn_async, pg_pool_async, reset_roles_async,
    run_migrations_async_direct, setup_fixtures_async,
};

#[allow(clippy::too_many_lines)] // linear 4-leg executor matrix
#[tokio::test(flavor = "multi_thread")]
async fn executor_uniformity_bare_pool_tx_commit_rollback() {
    run_migrations_async_direct().await;

    let config = RolifyConfig::builder().build().unwrap();

    // --- LEG 1: Bare async connection (autocommit) ---
    let mut conn = pg_conn_async().await;
    setup_fixtures_async(&mut conn).await;
    reset_roles_async(&mut conn).await;

    let mut store = DieselStore::new(&config);
    let user_id = insert_holder_async(&mut conn, "users", "User", "executor_user").await;

    let admin = store
        .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
        .await
        .expect("bare: find_or_create_by global admin");
    assert!(admin.is_global());
    let added = store
        .add(&mut conn, &user_id, &admin)
        .await
        .expect("bare: add global admin");
    assert!(added, "bare: first add creates link");
    let added_again = store
        .add(&mut conn, &user_id, &admin)
        .await
        .expect("bare: add again");
    assert!(!added_again, "bare: second add returns false");

    // Verify via a fresh connection (cross-connection visibility).
    let mut verify_conn = pg_conn_async().await;
    let roles = store
        .where_(
            &mut verify_conn,
            &user_id,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            },
        )
        .await
        .expect("bare: where_ verify");
    assert_eq!(roles.len(), 1, "bare: role visible cross-connection");
    assert!(roles[0].is_global());
    reset_roles_async(&mut conn).await;

    // --- LEG 2: bb8 pool checkout (reborrow of the inner connection) ---
    let pool = pg_pool_async().await;
    let mut pooled = pool.get().await.expect("pool checkout");
    setup_fixtures_async(&mut pooled).await;
    reset_roles_async(&mut pooled).await;

    let mut store_pooled = DieselStore::new(&config);
    let user_id_pooled =
        insert_holder_async(&mut pooled, "users", "User", "executor_user_pooled").await;

    let manager = store_pooled
        .find_or_create_by(
            &mut pooled,
            &RoleName::from("manager"),
            ResourceRef::Class("Forum"),
        )
        .await
        .expect("pooled: find_or_create_by class manager");
    assert!(manager.is_class_scoped_to("Forum"));
    let added = store_pooled
        .add(&mut pooled, &user_id_pooled, &manager)
        .await
        .expect("pooled: add class manager");
    assert!(added, "pooled: first add creates link");

    // Cross-connection verify via a bare connection.
    let mut verify_conn = pg_conn_async().await;
    let roles = store
        .where_(
            &mut verify_conn,
            &user_id_pooled,
            &RoleQuery {
                name: &RoleName::from("manager"),
                filter: ResourceFilter::Class("Forum"),
            },
        )
        .await
        .expect("pooled: where_ verify");
    assert_eq!(roles.len(), 1, "pooled: role visible cross-connection");
    assert!(roles[0].is_class_scoped_to("Forum"));
    reset_roles_async(&mut pooled).await;

    // --- LEG 3: Caller-owned transaction (commit, mid-flight read) ---
    let mut tx_conn = pg_conn_async().await;
    setup_fixtures_async(&mut tx_conn).await;
    reset_roles_async(&mut tx_conn).await;

    let mut store_tx = DieselStore::new(&config);
    let user_id_tx = insert_holder_async(&mut tx_conn, "users", "User", "executor_user_tx").await;
    let user_id_tx_verify = user_id_tx.clone();

    tx_conn
        .transaction::<_, rolify_diesel::Error, _>(async move |conn| {
            let moderator = store_tx
                .find_or_create_by(
                    conn,
                    &RoleName::from("moderator"),
                    ResourceRef::Instance("Forum", &ResourceId::from("99")),
                )
                .await
                .expect("tx: find_or_create_by instance moderator");
            assert!(moderator.is_instance_scoped_to("Forum", &ResourceId::from("99")));
            let added = store_tx
                .add(conn, &user_id_tx, &moderator)
                .await
                .expect("tx: add instance moderator");
            assert!(added, "tx: first add creates link");

            // Read the granted role back INSIDE the same transaction: the
            // caller's connection sees the mid-flight write.
            let roles = store_tx
                .where_(
                    conn,
                    &user_id_tx,
                    &RoleQuery {
                        name: &RoleName::from("moderator"),
                        filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
                    },
                )
                .await
                .expect("tx: where_ inside transaction");
            assert_eq!(roles.len(), 1, "tx: role visible inside transaction");
            assert!(roles[0].is_instance_scoped_to("Forum", &ResourceId::from("99")));

            Ok(())
        })
        .await
        .expect("tx: commit");

    // Cross-connection verify after commit.
    let mut verify_conn = pg_conn_async().await;
    let roles = store
        .where_(
            &mut verify_conn,
            &user_id_tx_verify,
            &RoleQuery {
                name: &RoleName::from("moderator"),
                filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
            },
        )
        .await
        .expect("tx: where_ verify after commit");
    assert_eq!(
        roles.len(),
        1,
        "tx: role visible cross-connection after commit"
    );
    assert!(roles[0].is_instance_scoped_to("Forum", &ResourceId::from("99")));
    reset_roles_async(&mut verify_conn).await;

    // --- LEG 4: Caller-owned transaction (rollback absorption) ---
    let mut rb_conn = pg_conn_async().await;
    setup_fixtures_async(&mut rb_conn).await;
    reset_roles_async(&mut rb_conn).await;

    let mut store_rb = DieselStore::new(&config);
    let user_id_rb = insert_holder_async(&mut rb_conn, "users", "User", "executor_user_rb").await;
    let user_id_rb_verify = user_id_rb.clone();

    let rb_result: Result<(), rolify_diesel::Error> = rb_conn
        .transaction(async move |conn| {
            let editor = store_rb
                .find_or_create_by(conn, &RoleName::from("editor"), ResourceRef::Global)
                .await
                .expect("rb: find_or_create_by global editor");
            assert!(editor.is_global());
            let added = store_rb
                .add(conn, &user_id_rb, &editor)
                .await
                .expect("rb: add global editor");
            assert!(added, "rb: first add creates link");

            // Force the rollback.
            Err(rolify_diesel::Error::Core(
                rolify_core::RolifyError::InvalidConfig {
                    reason: "intentional rollback for test".into(),
                },
            ))
        })
        .await;

    assert!(rb_result.is_err(), "rb: transaction rolled back");

    // Cross-connection verify: NO rows survive the rollback.
    let mut verify_conn = pg_conn_async().await;
    let roles = store
        .where_(
            &mut verify_conn,
            &user_id_rb_verify,
            &RoleQuery {
                name: &RoleName::from("editor"),
                filter: ResourceFilter::Global,
            },
        )
        .await
        .expect("rb: where_ verify after rollback");
    assert_eq!(
        roles.len(),
        0,
        "rb: NO role rows leaked after rollback (full absorption)"
    );

    let count_row: CountRow = diesel::sql_query(
        "SELECT COUNT(*) AS count FROM roles WHERE name = 'editor' AND resource_type = '' AND resource_id = ''",
    )
    .get_result(&mut verify_conn)
    .await
    .expect("rb: count editor rows");
    assert_eq!(
        count_row.count, 0,
        "rb: NO role row in roles table after rollback"
    );

    let link_count_row: CountRow =
        diesel::sql_query("SELECT COUNT(*) AS count FROM users_roles WHERE user_id = $1")
            .bind::<diesel::sql_types::Text, _>(user_id_rb_verify.as_str())
            .get_result(&mut verify_conn)
            .await
            .expect("rb: count links");
    assert_eq!(
        link_count_row.count, 0,
        "rb: NO join rows leaked after rollback"
    );

    // --- LEG 5: tokio::spawn Send-across-await proof (SC-3 smoke) ---
    let mut spawn_conn = pg_conn_async().await;
    setup_fixtures_async(&mut spawn_conn).await;
    reset_roles_async(&mut spawn_conn).await;

    let mut store_spawn = DieselStore::new(&config);
    let user_id_spawn =
        insert_holder_async(&mut spawn_conn, "users", "User", "executor_user_spawn").await;
    let user_id_spawn_verify = user_id_spawn.clone();

    // The store calls move into a spawned task with the connection and
    // holder: the future must be Send across every await point to be
    // spawned at all, and the result must cross the JoinHandle.
    let handle = tokio::spawn(async move {
        let viewer = store_spawn
            .find_or_create_by(
                &mut spawn_conn,
                &RoleName::from("viewer"),
                ResourceRef::Global,
            )
            .await
            .expect("spawn: find_or_create_by global viewer");
        let added = store_spawn
            .add(&mut spawn_conn, &user_id_spawn, &viewer)
            .await
            .expect("spawn: add global viewer");
        assert!(added, "spawn: add creates link inside spawned task");

        let spawned_name = RoleName::from("viewer");
        let roles = store_spawn
            .where_(
                &mut spawn_conn,
                &user_id_spawn,
                &RoleQuery {
                    name: &spawned_name,
                    filter: ResourceFilter::Global,
                },
            )
            .await
            .expect("spawn: where_ readback");
        assert_eq!(roles.len(), 1, "spawn: role readable inside task");
        roles
    });

    let roles = handle.await.expect("spawned task must not panic");
    assert_eq!(roles.len(), 1, "spawn: result crossed the JoinHandle");
    assert!(roles[0].is_global(), "spawn: global role round trip");

    // Cross-connection verify: the spawned task's writes committed.
    let mut verify_conn = pg_conn_async().await;
    let roles = store
        .where_(
            &mut verify_conn,
            &user_id_spawn_verify,
            &RoleQuery {
                name: &RoleName::from("viewer"),
                filter: ResourceFilter::Global,
            },
        )
        .await
        .expect("spawn: where_ verify after task join");
    assert_eq!(
        roles.len(),
        1,
        "spawn: role visible cross-connection after task join"
    );
    assert!(roles[0].is_global());
}
