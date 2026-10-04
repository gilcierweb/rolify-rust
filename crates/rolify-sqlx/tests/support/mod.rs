//! Test support for `rolify-sqlx` integration tests.
//!
//! Provides:
//! - Postgres testcontainer bootstrap (postgres:17 pinned, `AsyncRunner`)
//! - MySQL testcontainer bootstrap (mysql:8.4 pinned, `AsyncRunner`)
//! - SQLite in-memory pool with foreign keys forced on
//! - One shared container per test binary (`tokio::sync::OnceCell`
//!   statics, the async-native equivalent of rolify-diesel's `OnceLock`
//!   bootstrap from Phase 3)
//! - Migration appliers (`MIGRATIONS_<ENGINE>.run(&pool)`) and per-engine
//!   reset helpers mirroring the rolify-diesel support shape
//!
//! RESEARCH A2: container startup uses the `AsyncRunner` `.start().await`
//! path. The Phase-3-verified `SyncRunner` + blocking fallback stays
//! available if the async runner misbehaves; switch inside the container
//! helpers and add the `blocking` feature to the `testcontainers-modules`
//! dev-dependency if that becomes necessary.

#[cfg(feature = "mysql")]
use sqlx::MySqlPool;
#[cfg(feature = "postgres")]
use sqlx::PgPool;
#[cfg(feature = "sqlite")]
use sqlx::SqlitePool;
#[cfg(feature = "sqlite")]
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

#[cfg(any(feature = "postgres", feature = "mysql"))]
use testcontainers::runners::AsyncRunner;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use testcontainers::{ContainerAsync, ImageExt};
#[cfg(feature = "mysql")]
use testcontainers_modules::mysql;
#[cfg(feature = "postgres")]
use testcontainers_modules::postgres;

/// Get or start the shared Postgres container (postgres:17, Phase 3 D-14 pin).
#[cfg(feature = "postgres")]
pub async fn pg_container() -> &'static ContainerAsync<postgres::Postgres> {
    static PG_CONTAINER: tokio::sync::OnceCell<ContainerAsync<postgres::Postgres>> =
        tokio::sync::OnceCell::const_new();
    PG_CONTAINER
        .get_or_init(|| async {
            postgres::Postgres::default()
                .with_tag("17")
                .start()
                .await
                .expect(
                    "Docker must be available for Postgres parity leg; postgres:17 image will be pulled",
                )
        })
        .await
}

/// Get or start the shared MySQL container (mysql:8.4, Phase 3 D-14 pin).
#[cfg(feature = "mysql")]
pub async fn mysql_container() -> &'static ContainerAsync<mysql::Mysql> {
    static MYSQL_CONTAINER: tokio::sync::OnceCell<ContainerAsync<mysql::Mysql>> =
        tokio::sync::OnceCell::const_new();
    MYSQL_CONTAINER
        .get_or_init(|| async {
            mysql::Mysql::default()
                .with_tag("8.4")
                .start()
                .await
                .expect(
                    "Docker must be available for MySQL parity leg; mysql:8.4 image will be pulled",
                )
        })
        .await
}

/// Build a sqlx pool connected to the shared Postgres container.
///
/// Connection facts mirror the diesel bootstrap: module defaults (user
/// `postgres`, password `postgres`, database `postgres`).
#[cfg(feature = "postgres")]
pub async fn pg_pool() -> PgPool {
    let container = pg_container().await;
    let host_port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Postgres port mapping");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
    PgPool::connect(&url).await.expect("Postgres pool")
}

/// Build a sqlx pool connected to the shared MySQL container.
///
/// Connection facts mirror the diesel bootstrap: module defaults (root
/// with empty password, database `test`).
#[cfg(feature = "mysql")]
pub async fn mysql_pool() -> MySqlPool {
    let container = mysql_container().await;
    let host_port = container
        .get_host_port_ipv4(3306)
        .await
        .expect("MySQL port mapping");
    let url = format!("mysql://root@127.0.0.1:{host_port}/test");
    MySqlPool::connect(&url).await.expect("MySQL pool")
}

/// Build a SQLite in-memory pool with FK enforcement on.
///
/// RESEARCH Pitfall 14: SQLite ships FKs off and enforcement is
/// per-connection; `foreign_keys(true)` runs the pragma on every
/// connection the pool opens. `max_connections(1)` pins every checkout to
/// the SAME private in-memory database (each new in-memory connection
/// would otherwise start from an empty database).
#[cfg(feature = "sqlite")]
pub async fn sqlite_memory_pool() -> SqlitePool {
    let connect_options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options)
        .await
        .expect("SQLite in-memory pool")
}

/// Apply the vendored Postgres migrations.
///
/// D-02: the runtime crate never auto-migrates; this helper is the test
/// consumer that invokes `MIGRATIONS_POSTGRES.run` explicitly.
#[cfg(feature = "postgres")]
pub async fn apply_migrations_pg(pool: &PgPool) {
    rolify_sqlx::MIGRATIONS_POSTGRES
        .run(pool)
        .await
        .expect("Postgres migrations apply cleanly");
}

/// Apply the vendored MySQL migrations (D-02 consumer-invoked posture).
#[cfg(feature = "mysql")]
pub async fn apply_migrations_mysql(pool: &MySqlPool) {
    rolify_sqlx::MIGRATIONS_MYSQL
        .run(pool)
        .await
        .expect("MySQL migrations apply cleanly");
}

/// Apply the vendored SQLite migrations (D-02 consumer-invoked posture).
#[cfg(feature = "sqlite")]
pub async fn apply_migrations_sqlite(pool: &SqlitePool) {
    rolify_sqlx::MIGRATIONS_SQLITE
        .run(pool)
        .await
        .expect("SQLite migrations apply cleanly");
}

/// Reset role state (truncate roles + users_roles) for test isolation.
///
/// Mirrors the diesel support shape: TRUNCATE ... RESTART IDENTITY CASCADE
/// on Postgres. Does NOT touch consumer fixture tables.
#[cfg(feature = "postgres")]
pub async fn reset_roles_pg(pool: &PgPool) {
    sqlx::query("TRUNCATE TABLE users_roles, roles RESTART IDENTITY CASCADE")
        .execute(pool)
        .await
        .expect("truncate roles on Postgres");
}

/// Reset role state (truncate roles + users_roles) for test isolation.
///
/// Mirrors the diesel support shape: plain TRUNCATE on MySQL.
#[cfg(feature = "mysql")]
pub async fn reset_roles_mysql(pool: &MySqlPool) {
    sqlx::query("TRUNCATE TABLE users_roles, roles")
        .execute(pool)
        .await
        .expect("truncate roles on MySQL");
}

/// Reset role state (delete roles + users_roles rows) for test isolation.
///
/// Mirrors the diesel support shape: DELETE on SQLite (its rowid-alias
/// `roles.id` restarts numbering once the table is empty). The
/// `sqlite_sequence` sweep is best effort: only AUTOINCREMENT tables have
/// rows there, and the rolify schema deliberately does not use
/// AUTOINCREMENT.
#[cfg(feature = "sqlite")]
pub async fn reset_roles_sqlite(pool: &SqlitePool) {
    sqlx::query("DELETE FROM users_roles")
        .execute(pool)
        .await
        .expect("delete users_roles on SQLite");
    sqlx::query("DELETE FROM roles")
        .execute(pool)
        .await
        .expect("delete roles on SQLite");
    let _ = sqlx::query("DELETE FROM sqlite_sequence WHERE name IN ('roles')")
        .execute(pool)
        .await;
}
