//! Async-only posture proofs (SC-3) for `rolify-sqlx`.
//!
//! This crate has no sync mode to arm (Phase 1 D-05: it declares no sync
//! feature of any name and never forwards `rolify-core/is_sync`), so every
//! test here is a plain `#[tokio::test]` - never a dual-arm `maybe_async`
//! wrapper. The legs prove:
//! - `SqlxStore<DB>` and its connection are `Send + Sync` (the `const fn`
//!   helpers ported from `rolify-core/tests/compile_smoke.rs`).
//! - A store call crosses a `tokio::spawn` boundary: the SPI futures hold
//!   `&mut Conn` across `.await` points and stay `Send` (if the future
//!   were not `Send`, `tokio::spawn` would fail to compile - the spawn IS
//!   the proof).
//! - Pool checkouts pass a reborrow of the inner connection (`&mut
//!   *pooled`), never the wrapper handle (RESEARCH Pitfall 4: sqlx 0.9
//!   deleted the `Transaction`/`PoolConnection` `Executor` impls).
//!
//! The SPI stays non-dyn by design (E0038): `RoleStore` carries `type Conn`
//! and `type Error`, so static dispatch is the only form. `Box<dyn>`
//! assertions apply to consumer traits only, never the store - there is
//! deliberately no `Box<dyn RoleStore>` here.

#![cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]

const fn assert_send_sync<T: Send + Sync>() {}

// The shared support module ships helpers for several test binaries;
// this binary uses the bootstrap arms per engine, so the unused arms are
// allowed to sit idle here (same precedent as tests/migrations.rs).
#[allow(dead_code)]
mod support;

#[cfg(feature = "postgres")]
mod postgres {
    use rolify_core::config::RolifyConfig;
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::RoleName;
    use rolify_core::store::RoleStore;
    use rolify_sqlx::SqlxStore;

    use super::assert_send_sync;
    use crate::support::{
        apply_migrations_pg, pg_conn, pg_pool, reset_roles_pg, setup_fixtures_pg,
    };

    #[test]
    fn store_and_connection_are_send_sync() {
        assert_send_sync::<SqlxStore<sqlx::Postgres>>();
        assert_send_sync::<sqlx::PgConnection>();
    }

    #[tokio::test]
    async fn spawned_store_call_crosses_await_points() {
        let pool = pg_pool().await;
        apply_migrations_pg(&pool).await;
        setup_fixtures_pg(&pool).await;
        reset_roles_pg(&pool).await;

        // Pitfall-4 discipline, pinned behaviorally: the checkout passes
        // a reborrow of the inner connection, never the wrapper handle.
        let mut pooled = pool.acquire().await.expect("pool checkout");
        sqlx::query("SELECT 1")
            .fetch_one(&mut *pooled)
            .await
            .expect("checkout executes through the deref");
        drop(pooled);

        // The Send proof: the whole store call moves into the spawned
        // task and its result comes back over the JoinHandle.
        let holder_store = SqlxStore::<sqlx::Postgres>::new(&RolifyConfig::default());
        let holder_conn = pg_conn().await;
        let handle = tokio::spawn(async move {
            let mut task_store = holder_store;
            let mut task_conn = holder_conn;
            task_store
                .find_or_create_by(
                    &mut task_conn,
                    &RoleName::from("smoke"),
                    ResourceRef::Global,
                )
                .await
                .expect("spawned find_or_create_by")
        });
        let record = handle.await.expect("spawned task joins");
        assert!(record.is_global());
        assert_eq!(record.name.as_str(), "smoke");

        reset_roles_pg(&pool).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql {
    use rolify_core::config::RolifyConfig;
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::RoleName;
    use rolify_core::store::RoleStore;
    use rolify_sqlx::SqlxStore;

    use super::assert_send_sync;
    use crate::support::{
        apply_migrations_mysql, mysql_conn, mysql_pool, reset_roles_mysql, setup_fixtures_mysql,
    };

    #[test]
    fn store_and_connection_are_send_sync() {
        assert_send_sync::<SqlxStore<sqlx::MySql>>();
        assert_send_sync::<sqlx::MySqlConnection>();
    }

    #[tokio::test]
    async fn spawned_store_call_crosses_await_points() {
        let pool = mysql_pool().await;
        apply_migrations_mysql(&pool).await;
        setup_fixtures_mysql(&pool).await;
        reset_roles_mysql(&pool).await;

        // Pitfall-4 discipline, pinned behaviorally: the checkout passes
        // a reborrow of the inner connection, never the wrapper handle.
        let mut pooled = pool.acquire().await.expect("pool checkout");
        sqlx::query("SELECT 1")
            .fetch_one(&mut *pooled)
            .await
            .expect("checkout executes through the deref");
        drop(pooled);

        // The Send proof: the whole store call moves into the spawned
        // task and its result comes back over the JoinHandle.
        let holder_store = SqlxStore::<sqlx::MySql>::new(&RolifyConfig::default());
        let holder_conn = mysql_conn().await;
        let handle = tokio::spawn(async move {
            let mut task_store = holder_store;
            let mut task_conn = holder_conn;
            task_store
                .find_or_create_by(
                    &mut task_conn,
                    &RoleName::from("smoke"),
                    ResourceRef::Global,
                )
                .await
                .expect("spawned find_or_create_by")
        });
        let record = handle.await.expect("spawned task joins");
        assert!(record.is_global());
        assert_eq!(record.name.as_str(), "smoke");

        reset_roles_mysql(&pool).await;
    }
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use rolify_core::config::RolifyConfig;
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::RoleName;
    use rolify_core::store::RoleStore;
    use rolify_sqlx::SqlxStore;

    use super::assert_send_sync;
    use crate::support::{apply_migrations_sqlite_conn, sqlite_file_conn};

    #[test]
    fn store_and_connection_are_send_sync() {
        assert_send_sync::<SqlxStore<sqlx::Sqlite>>();
        assert_send_sync::<sqlx::SqliteConnection>();
    }

    #[tokio::test]
    async fn spawned_store_call_crosses_await_points() {
        // File-backed, like the sqlite tracer: the spawned task owns its
        // connection to the shared file.
        let db_path = std::env::temp_dir().join(format!(
            "rolify_sqlx_smoke_sqlite_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&db_path);
        {
            let mut setup_conn = sqlite_file_conn(&db_path).await;
            apply_migrations_sqlite_conn(&mut setup_conn).await;
        }

        // The Send proof: the whole store call moves into the spawned
        // task and its result comes back over the JoinHandle.
        let holder_store = SqlxStore::<sqlx::Sqlite>::new(&RolifyConfig::default());
        let holder_conn = sqlite_file_conn(&db_path).await;
        let handle = tokio::spawn(async move {
            let mut task_store = holder_store;
            let mut task_conn = holder_conn;
            task_store
                .find_or_create_by(
                    &mut task_conn,
                    &RoleName::from("smoke"),
                    ResourceRef::Global,
                )
                .await
                .expect("spawned find_or_create_by")
        });
        let record = handle.await.expect("spawned task joins");
        assert!(record.is_global());
        assert_eq!(record.name.as_str(), "smoke");

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-journal"));
    }
}
