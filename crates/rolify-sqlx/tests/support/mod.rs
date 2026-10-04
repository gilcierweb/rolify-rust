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
//! - Direct per-engine connections for the `FixtureUser` holders
//! - Fixture DDL execution from the shared suite arrays (D-06:
//!   `rolify_test::ddl`, byte-identical to the diesel setup fixtures)
//! - Holder/resource seeding (`insert_holder_*` / `insert_resource_*`)
//! - `FixtureUser<DB>`: a `RolifyUser` over `SqlxStore<DB>` so tracers
//!   drive grant/check/revoke through the PUBLIC provided methods
//!
//! RESEARCH A2: container startup uses the `AsyncRunner` `.start().await`
//! path. The Phase-3-verified `SyncRunner` + blocking fallback stays
//! available if the async runner misbehaves; switch inside the container
//! helpers and add the `blocking` feature to the `testcontainers-modules`
//! dev-dependency if that becomes necessary.

use rolify_core::config::RolifyConfig;
use rolify_core::role::ResourceId;
use rolify_core::store::{ResourceKey, RoleStore};
use rolify_core::user::RolifyUser;
use sqlx::database::Database;
use sqlx::{Connection as _, Row as _};

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

// ============================================================
// Direct connections (FixtureUser holders own one each)
// ============================================================

/// Open a direct Postgres connection to the shared container.
#[cfg(feature = "postgres")]
pub async fn pg_conn() -> sqlx::PgConnection {
    let container = pg_container().await;
    let host_port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Postgres port mapping");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
    sqlx::PgConnection::connect(&url)
        .await
        .expect("Postgres connection")
}

/// Open a direct MySQL connection to the shared container (module
/// defaults: root with empty password, database `test`).
#[cfg(feature = "mysql")]
pub async fn mysql_conn() -> sqlx::MySqlConnection {
    let container = mysql_container().await;
    let host_port = container
        .get_host_port_ipv4(3306)
        .await
        .expect("MySQL port mapping");
    let url = format!("mysql://root@127.0.0.1:{host_port}/test");
    sqlx::MySqlConnection::connect(&url)
        .await
        .expect("MySQL connection")
}

/// Open a direct SQLite in-memory connection with FK enforcement on.
///
/// Each in-memory connection is a private database; tests that use it
/// must run migrations, fixtures, and store operations on the SAME
/// connection (the shared pool in [`sqlite_memory_pool`] is the
/// multi-checkout alternative).
#[cfg(feature = "sqlite")]
pub async fn sqlite_conn() -> sqlx::SqliteConnection {
    let connect_options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    sqlx::SqliteConnection::connect_with(&connect_options)
        .await
        .expect("SQLite in-memory connection")
}

// ============================================================
// Fixture tables (D-06: the shared suite DDL arrays)
// ============================================================

/// Create the suite fixture tables on Postgres (users, customers,
/// forums, groups, teams, organizations, rights, and the custom-pair
/// join tables) by executing every statement of
/// `rolify_test::ddl::POSTGRES` on the pool.
#[cfg(feature = "postgres")]
pub async fn setup_fixtures_pg(pool: &PgPool) {
    for statement in rolify_test::ddl::POSTGRES {
        sqlx::query(*statement)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("fixture DDL failed on Postgres: {error}\n{statement}"));
    }
}

/// Create the suite fixture tables on MySQL (D-06 suite arrays).
#[cfg(feature = "mysql")]
pub async fn setup_fixtures_mysql(pool: &MySqlPool) {
    for statement in rolify_test::ddl::MYSQL {
        sqlx::query(*statement)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("fixture DDL failed on MySQL: {error}\n{statement}"));
    }
}

/// Create the suite fixture tables on SQLite (D-06 suite arrays).
#[cfg(feature = "sqlite")]
pub async fn setup_fixtures_sqlite(pool: &SqlitePool) {
    for statement in rolify_test::ddl::SQLITE {
        sqlx::query(*statement)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("fixture DDL failed on SQLite: {error}\n{statement}"));
    }
}

// ============================================================
// Holder / resource seeding (suite fixture tables)
// ============================================================

/// Insert a fixture holder row and return its stringified id.
///
/// AUDIT (sqlx 0.9 `SqlSafeStr` gate): `table` is a test-literal fixture
/// table name (`"users"`, `"customers"`), never consumer input; every
/// value travels as a runtime bind.
#[cfg(feature = "postgres")]
pub async fn insert_holder_pg(
    conn: &mut sqlx::PgConnection,
    table: &str,
    holder_type: &str,
    name: &str,
) -> ResourceId {
    let sql_text = format!("INSERT INTO {table} (rolify_type, name) VALUES ($1, $2) RETURNING id");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql_text.as_str()))
        .bind(holder_type)
        .bind(name)
        .fetch_one(conn)
        .await
        .expect("insert holder on Postgres");
    ResourceId::from(row.get::<i64, _>(0))
}

/// Insert a fixture holder row on MySQL (LAST_INSERT_ID on the same
/// connection).
#[cfg(feature = "mysql")]
pub async fn insert_holder_mysql(
    conn: &mut sqlx::MySqlConnection,
    table: &str,
    holder_type: &str,
    name: &str,
) -> ResourceId {
    let sql_text = format!("INSERT INTO {table} (rolify_type, name) VALUES (?, ?)");
    sqlx::query(sqlx::AssertSqlSafe(sql_text.as_str()))
        .bind(holder_type)
        .bind(name)
        .execute(conn)
        .await
        .expect("insert holder on MySQL");
    let row = sqlx::query("SELECT LAST_INSERT_ID()")
        .fetch_one(conn)
        .await
        .expect("last insert id on MySQL");
    ResourceId::from(row.get::<i64, _>(0))
}

/// Insert a fixture holder row on SQLite (last_insert_rowid on the same
/// connection).
#[cfg(feature = "sqlite")]
pub async fn insert_holder_sqlite(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    holder_type: &str,
    name: &str,
) -> ResourceId {
    let sql_text = format!("INSERT INTO {table} (rolify_type, name) VALUES (?, ?)");
    sqlx::query(sqlx::AssertSqlSafe(sql_text.as_str()))
        .bind(holder_type)
        .bind(name)
        .execute(conn)
        .await
        .expect("insert holder on SQLite");
    let row = sqlx::query("SELECT last_insert_rowid()")
        .fetch_one(conn)
        .await
        .expect("last insert rowid on SQLite");
    ResourceId::from(row.get::<i64, _>(0))
}

/// Insert a fixture resource row (integer autoincrement PK) and return
/// its resource key.
///
/// AUDIT: `table` is a test-literal fixture table name; the value
/// travels as a bind.
#[cfg(feature = "postgres")]
pub async fn insert_resource_pg(
    conn: &mut sqlx::PgConnection,
    table: &str,
    type_name: &str,
    name: &str,
) -> ResourceKey {
    let sql_text = format!("INSERT INTO {table} (name) VALUES ($1) RETURNING id");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql_text.as_str()))
        .bind(name)
        .fetch_one(conn)
        .await
        .expect("insert resource on Postgres");
    ResourceKey::new(type_name, row.get::<i64, _>(0).to_string())
}

/// Insert a fixture resource row on MySQL (LAST_INSERT_ID).
#[cfg(feature = "mysql")]
pub async fn insert_resource_mysql(
    conn: &mut sqlx::MySqlConnection,
    table: &str,
    type_name: &str,
    name: &str,
) -> ResourceKey {
    let sql_text = format!("INSERT INTO {table} (name) VALUES (?)");
    sqlx::query(sqlx::AssertSqlSafe(sql_text.as_str()))
        .bind(name)
        .execute(conn)
        .await
        .expect("insert resource on MySQL");
    let row = sqlx::query("SELECT LAST_INSERT_ID()")
        .fetch_one(conn)
        .await
        .expect("last insert id on MySQL");
    ResourceKey::new(type_name, row.get::<i64, _>(0).to_string())
}

/// Insert a fixture resource row on SQLite (last_insert_rowid).
#[cfg(feature = "sqlite")]
pub async fn insert_resource_sqlite(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    type_name: &str,
    name: &str,
) -> ResourceKey {
    let sql_text = format!("INSERT INTO {table} (name) VALUES (?)");
    sqlx::query(sqlx::AssertSqlSafe(sql_text.as_str()))
        .bind(name)
        .execute(conn)
        .await
        .expect("insert resource on SQLite");
    let row = sqlx::query("SELECT last_insert_rowid()")
        .fetch_one(conn)
        .await
        .expect("last insert rowid on SQLite");
    ResourceKey::new(type_name, row.get::<i64, _>(0).to_string())
}

// ============================================================
// FixtureUser: a RolifyUser over SqlxStore<DB>
// ============================================================

/// A `RolifyUser` fixture over [`rolify_sqlx::SqlxStore`]: owns the
/// store, ONE direct connection, and the config, so the tracer tests
/// drive grant/check/revoke through the PUBLIC provided methods
/// (`add_role`, `has_role`, `has_strict_role`, `has_any_roles`,
/// `only_has_role`, `roles_name`, `remove_role`/`revoke`).
///
/// The connection ownership is the SC-5 "one connection" story in
/// miniature: every store call the provided methods make lands on the
/// same connection the fixture owns, exactly like a consumer's pool
/// checkout held across a request.
pub struct FixtureUser<DB>
where
    DB: sqlx::Database,
{
    holder_id: ResourceId,
    store: rolify_sqlx::SqlxStore<DB>,
    conn: <DB as sqlx::Database>::Connection,
    config: RolifyConfig,
}

impl<DB: sqlx::Database> FixtureUser<DB> {
    /// Bundle a holder identity, its store, its connection, and the
    /// shared configuration.
    #[must_use]
    pub fn new(
        holder_id: ResourceId,
        store: rolify_sqlx::SqlxStore<DB>,
        conn: <DB as sqlx::Database>::Connection,
        config: RolifyConfig,
    ) -> Self {
        Self {
            holder_id,
            store,
            conn,
            config,
        }
    }
}

impl<DB> RolifyUser for FixtureUser<DB>
where
    DB: sqlx::Database,
    <DB as Database>::Connection: Sync,
    <DB as Database>::Arguments: sqlx::IntoArguments<DB>,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
    for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    type Store = rolify_sqlx::SqlxStore<DB>;

    fn store(&mut self) -> &mut Self::Store {
        &mut self.store
    }

    fn rolify_config(&self) -> &RolifyConfig {
        &self.config
    }

    fn rolify_id(&self) -> ResourceId {
        self.holder_id.clone()
    }

    fn rolify_type() -> &'static str {
        "User"
    }

    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut <Self::Store as RoleStore>::Conn) {
        (&mut self.store, &mut self.conn)
    }
}
