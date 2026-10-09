//! Executor uniformity acceptance tests in async mode (ROADMAP SC-5):
//! the `SQLx` store must behave identically across three connection
//! surfaces, mirroring the diesel executor legs (Phase 3) and the
//! diesel-async executor legs (04-07):
//!
//! 1. Bare `PgConnection` (autocommit)
//! 2. Pool checkout (a reborrow of the inner connection through
//!    `PoolConnection`'s `DerefMut`)
//! 3. Caller-owned `Transaction` with a mid-flight read INSIDE the
//!    transaction and cross-connection verification after commit
//! 4. Caller-owned transaction rollback: a deliberate `Err` returned
//!    from the scoped block forces the rollback, and zero role or
//!    join rows leak (cross-connection verify)
//!
//! sqlx 0.9 executor discipline (RESEARCH Pitfall 4): the 0.9 line
//! deleted the `Executor` impls for the wrapper handles (`Transaction`
//! and `PoolConnection`), so every store call below passes a REBORROW
//! of the inner connection (`&mut *tx` / `&mut *checkout`), never the
//! wrapper. The store's `Conn` type is the bare connection by
//! contract, which keeps the legs honest: a wrapper handle cannot leak
//! into a store call, it would not type-check.
//!
//! Runs on Postgres (cfg-gated like the tracer). `SQLite` is excluded
//! per D-11 (the locked non-gate posture): the single-writer engine
//! makes locking-sensitive executor acceptance meaningless, exactly as
//! in the diesel executor tests.
//!
//! Runs: `cargo test -p rolify-sqlx --features postgres,suite --test executor`

#![cfg(feature = "postgres")]

mod support;

use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_sqlx::SqlxStore;
use sqlx::Row as _;

use crate::support::{
    apply_migrations_pg, insert_holder_pg, pg_conn, pg_pool, reset_roles_pg, setup_fixtures_pg,
};

/// Count the role rows matching an exact triple on a verification
/// connection (the zero-leak probe; every value is a runtime bind).
async fn count_role_rows(
    conn: &mut sqlx::PgConnection,
    name: &str,
    resource_type: &str,
    resource_id: &str,
) -> i64 {
    sqlx::query(
        "SELECT COUNT(*) FROM roles WHERE name = $1 AND resource_type = $2 AND resource_id = $3",
    )
    .bind(name)
    .bind(resource_type)
    .bind(resource_id)
    .fetch_one(conn)
    .await
    .expect("count the role rows for the exact triple")
    .get::<i64, _>(0)
}

/// Count the join rows for one holder on a verification connection
/// (the zero-leak probe for the link table).
async fn count_holder_links(conn: &mut sqlx::PgConnection, holder: &str) -> i64 {
    sqlx::query("SELECT COUNT(*) FROM users_roles WHERE user_id = $1")
        .bind(
            holder
                .parse::<i64>()
                .expect("integer holder id under the default kind"),
        )
        .fetch_one(conn)
        .await
        .expect("count the holder's join rows")
        .get::<i64, _>(0)
}

#[allow(clippy::too_many_lines)] // linear 4-leg executor matrix
#[tokio::test]
async fn executor_uniformity_bare_checkout_tx_commit_rollback() {
    // Bootstrap: shared container (implicit readiness), migrations,
    // fixtures, and a clean role slate. Per-leg resets follow the
    // diesel executor test's discipline.
    let pool = pg_pool().await;
    apply_migrations_pg(&pool).await;
    setup_fixtures_pg(&pool).await;
    reset_roles_pg(&pool).await;

    let config = RolifyConfig::default();

    // --- LEG 1: bare connection (autocommit) ---
    let mut conn = pg_conn().await;
    let mut store = SqlxStore::new(&config);
    let user_id = insert_holder_pg(&mut conn, "users", "User", "executor_user_bare").await;

    let admin = store
        .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
        .await
        .expect("bare: find_or_create_by global admin");
    assert!(admin.is_global());
    let added = store
        .add(&mut conn, &user_id, &admin)
        .await
        .expect("bare: add global admin");
    assert!(added, "bare: first add creates the link");
    let added_again = store
        .add(&mut conn, &user_id, &admin)
        .await
        .expect("bare: add the same role again");
    assert!(!added_again, "bare: second add returns false (link dedupe)");

    // Cross-connection visibility: a FRESH direct connection sees the
    // autocommitted write.
    let mut verify_conn = pg_conn().await;
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
        .expect("bare: where_ on the verification connection");
    assert_eq!(roles.len(), 1, "bare: role visible cross-connection");
    assert!(roles[0].is_global());
    reset_roles_pg(&pool).await;

    // --- LEG 2: pool checkout (a reborrow of the inner connection) ---
    let mut checkout = pool.acquire().await.expect("pool checkout");
    let mut store_pooled = SqlxStore::new(&config);
    let user_id_pooled =
        insert_holder_pg(&mut checkout, "users", "User", "executor_user_pooled").await;

    // sqlx 0.9 deleted the Executor impls for the wrapper handles
    // (RESEARCH Pitfall 4): the store takes a REBORROW of the inner
    // connection (`&mut *checkout`), never the `PoolConnection` handle.
    let manager = store_pooled
        .find_or_create_by(
            &mut *checkout,
            &RoleName::from("manager"),
            ResourceRef::Class("Forum"),
        )
        .await
        .expect("pooled: find_or_create_by class manager");
    assert!(manager.is_class_scoped_to("Forum"));
    let added = store_pooled
        .add(&mut *checkout, &user_id_pooled, &manager)
        .await
        .expect("pooled: add class manager");
    assert!(added, "pooled: first add creates the link");

    // Cross-connection verify via a bare connection.
    let mut verify_conn = pg_conn().await;
    let roles = store_pooled
        .where_(
            &mut verify_conn,
            &user_id_pooled,
            &RoleQuery {
                name: &RoleName::from("manager"),
                filter: ResourceFilter::Class("Forum"),
            },
        )
        .await
        .expect("pooled: where_ on the verification connection");
    assert_eq!(roles.len(), 1, "pooled: role visible cross-connection");
    assert!(roles[0].is_class_scoped_to("Forum"));
    // Return the checkout to the pool before the transaction legs.
    drop(checkout);
    reset_roles_pg(&pool).await;

    // --- LEG 3: caller-owned transaction (commit, mid-flight read) ---
    let mut seed_conn = pg_conn().await;
    let user_id_tx = insert_holder_pg(&mut seed_conn, "users", "User", "executor_user_tx").await;
    let mut store_tx = SqlxStore::new(&config);

    let mut tx = pool.begin().await.expect("tx: begin on the pool");
    let moderator = store_tx
        .find_or_create_by(
            &mut *tx,
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &ResourceId::from("99")),
        )
        .await
        .expect("tx: find_or_create_by instance moderator");
    assert!(moderator.is_instance_scoped_to("Forum", &ResourceId::from("99")));
    let added = store_tx
        .add(&mut *tx, &user_id_tx, &moderator)
        .await
        .expect("tx: add instance moderator");
    assert!(
        added,
        "tx: first add creates the link inside the transaction"
    );

    // Read the granted role back INSIDE the same transaction: the
    // caller's connection sees the not-yet-committed write mid-flight.
    let roles_inside = store_tx
        .where_(
            &mut *tx,
            &user_id_tx,
            &RoleQuery {
                name: &RoleName::from("moderator"),
                filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
            },
        )
        .await
        .expect("tx: where_ inside the transaction");
    assert_eq!(
        roles_inside.len(),
        1,
        "tx: role visible inside the transaction (mid-flight read)"
    );
    assert!(
        roles_inside[0].is_instance_scoped_to("Forum", &ResourceId::from("99")),
        "tx: the mid-flight record carries the granted scope"
    );

    tx.commit().await.expect("tx: commit");

    // Cross-connection verify AFTER the commit: a second connection
    // outside the transaction sees the durable write.
    let mut verify_conn = pg_conn().await;
    let roles = store_tx
        .where_(
            &mut verify_conn,
            &user_id_tx,
            &RoleQuery {
                name: &RoleName::from("moderator"),
                filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
            },
        )
        .await
        .expect("tx: where_ on the verification connection after commit");
    assert_eq!(
        roles.len(),
        1,
        "tx: role visible cross-connection after commit"
    );
    assert!(roles[0].is_instance_scoped_to("Forum", &ResourceId::from("99")));
    reset_roles_pg(&pool).await;

    // --- LEG 4: caller-owned transaction (rollback absorption) ---
    let mut seed_conn = pg_conn().await;
    let user_id_rb = insert_holder_pg(&mut seed_conn, "users", "User", "executor_user_rb").await;
    let mut store_rb = SqlxStore::new(&config);

    let mut tx = pool.begin().await.expect("rb: begin on the pool");
    // The scoped grant block: a deliberate Err returned from this
    // block forces the caller-owned transaction to roll back, the
    // sqlx-shaped mirror of the diesel executor test's
    // Err-forces-rollback closure.
    let scoped_grant: Result<(), rolify_sqlx::Error> = async {
        let editor = store_rb
            .find_or_create_by(&mut *tx, &RoleName::from("editor"), ResourceRef::Global)
            .await
            .expect("rb: find_or_create_by global editor");
        assert!(editor.is_global());
        let added = store_rb
            .add(&mut *tx, &user_id_rb, &editor)
            .await
            .expect("rb: add global editor");
        assert!(
            added,
            "rb: the link exists inside the transaction before the rollback"
        );
        Err(rolify_sqlx::Error::Core(
            rolify_core::RolifyError::InvalidConfig {
                reason: "intentional rollback for the executor rollback leg".into(),
            },
        ))
    }
    .await;
    assert!(
        scoped_grant.is_err(),
        "rb: the scoped block returned the deliberate Err"
    );
    tx.rollback()
        .await
        .expect("rb: the Err forces the rollback");

    // Cross-connection verify: the rollback unwound EVERY write; zero
    // rows leaked from the caller's transaction.
    let mut verify_conn = pg_conn().await;
    let roles = store_rb
        .where_(
            &mut verify_conn,
            &user_id_rb,
            &RoleQuery {
                name: &RoleName::from("editor"),
                filter: ResourceFilter::Global,
            },
        )
        .await
        .expect("rb: where_ on the verification connection after rollback");
    assert_eq!(
        roles.len(),
        0,
        "rb: NO role links survive the rollback (full absorption)"
    );

    let role_rows = count_role_rows(&mut verify_conn, "editor", "", "").await;
    assert_eq!(
        role_rows, 0,
        "rb: NO role row in the roles table after rollback"
    );

    let link_rows = count_holder_links(&mut verify_conn, user_id_rb.as_str()).await;
    assert_eq!(link_rows, 0, "rb: NO join rows leaked after rollback");
}
