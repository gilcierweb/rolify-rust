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

/// Truncate the suite fixture tables and restart their identity
/// sequences (Postgres arm of the diesel support's `reset_fixtures`; no
/// CASCADE needed because no foreign key points at a fixture table).
/// The resource-side acceptance cases share one container database, so
/// every case starts from a clean fixture slate.
#[cfg(feature = "postgres")]
pub async fn reset_fixtures_pg(pool: &PgPool) {
    sqlx::query(
        "TRUNCATE TABLE users, customers, forums, groups, teams, organizations, rights, moderators_rights, admin_rights RESTART IDENTITY",
    )
    .execute(pool)
    .await
    .expect("truncate fixtures on Postgres");
}

/// Process-wide serializer for test cases sharing one container
/// database: the async counterpart of the diesel support's
/// `SuiteGuard`. Every resource-side acceptance case truncates the
/// shared tables, so concurrent cases on neighboring test threads must
/// not interleave; the guard is held for a whole case body.
#[cfg(feature = "postgres")]
pub async fn suite_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static SUITE_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    SUITE_MUTEX.lock().await
}

/// Reset role state for test isolation.
///
/// MySQL forbids `TRUNCATE` on a table referenced by a foreign key
/// (`users_roles.role_id` references `roles.id`, error 1701), so the reset
/// is the FK-safe `DELETE` pair, child links before parent rows (the
/// generated ids are stringified holder/role keys, never asserted, so no
/// `AUTO_INCREMENT` reset is needed). Does NOT touch consumer fixture
/// tables.
#[cfg(feature = "mysql")]
pub async fn reset_roles_mysql(pool: &MySqlPool) {
    sqlx::query("DELETE FROM users_roles")
        .execute(pool)
        .await
        .expect("delete users_roles on MySQL");
    sqlx::query("DELETE FROM roles")
        .execute(pool)
        .await
        .expect("delete roles on MySQL");
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
        .execute(&mut *conn)
        .await
        .expect("insert holder on MySQL");
    let row = sqlx::query("SELECT LAST_INSERT_ID()")
        .fetch_one(&mut *conn)
        .await
        .expect("last insert id on MySQL");
    // `LAST_INSERT_ID()` reports `BIGINT UNSIGNED`: decode as `u64`
    // (the `i64` decode rejects unsigned columns on this driver).
    ResourceId::from(row.get::<u64, _>(0))
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
        .execute(&mut *conn)
        .await
        .expect("insert holder on SQLite");
    let row = sqlx::query("SELECT last_insert_rowid()")
        .fetch_one(&mut *conn)
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
        .execute(&mut *conn)
        .await
        .expect("insert resource on MySQL");
    let row = sqlx::query("SELECT LAST_INSERT_ID()")
        .fetch_one(&mut *conn)
        .await
        .expect("last insert id on MySQL");
    // `LAST_INSERT_ID()` reports `BIGINT UNSIGNED`: decode as `u64`.
    ResourceKey::new(type_name, row.get::<u64, _>(0).to_string())
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
        .execute(&mut *conn)
        .await
        .expect("insert resource on SQLite");
    let row = sqlx::query("SELECT last_insert_rowid()")
        .fetch_one(&mut *conn)
        .await
        .expect("last insert rowid on SQLite");
    ResourceKey::new(type_name, row.get::<i64, _>(0).to_string())
}

/// Insert a `teams` row keyed by its string `team_code` primary key (the
/// gem's non-integer PK fixture, `spec/support/schema.rb`). Static
/// statement: no interpolation, plain binds for both values.
#[cfg(feature = "postgres")]
pub async fn insert_team_pg(conn: &mut sqlx::PgConnection, team_code: &str, name: &str) {
    sqlx::query("INSERT INTO teams (team_code, name) VALUES ($1, $2)")
        .bind(team_code)
        .bind(name)
        .execute(conn)
        .await
        .expect("insert team on Postgres");
}

/// Insert an STI `organizations` row of the given type
/// (`"Organization"`, `"Company"`) and return its resource key with the
/// stringified generated id. Static statement (the `type` column name
/// is quoted per Postgres rules); the STI type travels as a bind.
#[cfg(feature = "postgres")]
pub async fn insert_organization_pg(conn: &mut sqlx::PgConnection, type_name: &str) -> ResourceKey {
    let row = sqlx::query("INSERT INTO organizations (\"type\") VALUES ($1) RETURNING id")
        .bind(type_name)
        .fetch_one(conn)
        .await
        .expect("insert organization on Postgres");
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

// ============================================================
// File-backed SQLite (the sqlite tracer's shared database)
// ============================================================

/// Connect to a file-backed SQLite database (creating it when missing)
/// with FK enforcement on every connection.
///
/// Rationale: each `:memory:` connection owns a private database, so a
/// mirror tracer with two holders on two connections plus a shared
/// migration run needs a shared file. The path lives under
/// `std::env::temp_dir()` (honoring `TMPDIR`); the caller owns cleanup.
#[cfg(feature = "sqlite")]
pub async fn sqlite_file_conn(path: &std::path::Path) -> sqlx::SqliteConnection {
    let connect_options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    sqlx::SqliteConnection::connect_with(&connect_options)
        .await
        .expect("SQLite file connection")
}

/// Apply the vendored SQLite migrations on a single connection (the
/// file-backed tracer's shared database; `Migrator::run` accepts a
/// reborrowed connection through the per-backend `Acquire` impl).
#[cfg(feature = "sqlite")]
pub async fn apply_migrations_sqlite_conn(conn: &mut sqlx::SqliteConnection) {
    rolify_sqlx::MIGRATIONS_SQLITE
        .run(&mut *conn)
        .await
        .expect("SQLite migrations apply cleanly");
}

/// Create the suite fixture tables on a single SQLite connection
/// (the D-06 `rolify_test::ddl::SQLITE` arrays, same statements as the
/// pool helper).
#[cfg(feature = "sqlite")]
pub async fn setup_fixtures_sqlite_conn(conn: &mut sqlx::SqliteConnection) {
    for statement in rolify_test::ddl::SQLITE {
        sqlx::query(*statement)
            .execute(&mut *conn)
            .await
            .unwrap_or_else(|error| panic!("fixture DDL failed on SQLite: {error}\n{statement}"));
    }
}

/// Reset role state on a single SQLite connection (the DELETE pair from
/// the pool helper, plus the best-effort `sqlite_sequence` sweep).
#[cfg(feature = "sqlite")]
pub async fn reset_roles_sqlite_conn(conn: &mut sqlx::SqliteConnection) {
    sqlx::query("DELETE FROM users_roles")
        .execute(&mut *conn)
        .await
        .expect("delete users_roles on SQLite");
    sqlx::query("DELETE FROM roles")
        .execute(&mut *conn)
        .await
        .expect("delete roles on SQLite");
    let _ = sqlx::query("DELETE FROM sqlite_sequence WHERE name IN ('roles')")
        .execute(&mut *conn)
        .await;
}

// ============================================================
// SqlxBackend — TestBackend implementation for rolify-sqlx
// ============================================================
//
// The database-backed `TestBackend` the ported parity suite binds to
// (`parity_suite!` over `SqlxBackend`, one line per engine file).
// One backend owns one engine (store plus its live connection, D-06)
// over the shared container database; every suite operation flows
// through that engine connection, so the query counter sees every query.
//
// The generic backend type (one impl serves all three engines - D-13)
// mirrors the diesel_backend module structure: fixture structs,
// async build() flow, the ten members, and the hook members.
// reset_query_count delegates to the store's accessor, query_count
// returns Some(store.query_count()).
#[cfg(feature = "suite")]
pub mod sqlx_backend {
    use std::sync::Mutex;

    use rolify_core::config::RolifyConfig;
    use rolify_core::manager::Rolify;
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::{ResourceKey, RoleStore, Sealed};
    use rolify_core::user::RolifyUser;

    use rolify_test::fixtures::{DefaultUser, FixtureResource, UserClass, fixture_holders};

    use sqlx::Connection;

    #[cfg(feature = "mysql")]
    use super::{mysql_container, mysql_pool, setup_fixtures_mysql};
    #[cfg(feature = "postgres")]
    use super::{pg_container, pg_pool, setup_fixtures_pg};
    #[cfg(feature = "sqlite")]
    use super::{setup_fixtures_sqlite_conn, sqlite_memory_pool};
    use rolify_sqlx::SqlxStore;
    use rolify_sqlx::rows::CountRow;
    use sqlx::ConnectOptions;

    /// Process-wide serializer for tests sharing one database.
    ///
    /// Every backend build truncates the shared `roles` tables, so two
    /// suite cases running on neighboring tasks wipe each other's rows.
    /// Each `SqlxBackend` holds this guard for its whole lifetime.
    /// Acquisition spins on `yield_now`: timing-free, correctness never
    /// depends on timing, only liveness.
    pub struct SuiteGuard {
        flag: &'static std::sync::atomic::AtomicBool,
    }

    impl SuiteGuard {
        /// Acquire the process-wide suite lock (spins until free).
        #[must_use]
        pub fn acquire() -> Self {
            static SUITE_SERIAL: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            while SUITE_SERIAL.swap(true, std::sync::atomic::Ordering::Acquire) {
                std::thread::yield_now();
            }
            Self {
                flag: &SUITE_SERIAL,
            }
        }
    }

    impl Drop for SuiteGuard {
        fn drop(&mut self) {
            self.flag.store(false, std::sync::atomic::Ordering::Release);
        }
    }

    /// Default test configuration for the default role/join table pair.
    pub fn test_config() -> rolify_core::config::RolifyConfig {
        rolify_core::config::RolifyConfig::builder()
            .build()
            .unwrap()
    }

    // Re-export types needed by the trait
    type Store<DB> = SqlxStore<DB>;
    type Error = rolify_sqlx::Error;

    /// The suite subject: a holder identity over the single engine.
    ///
    /// `RolifyUser` requires `Sync`; the engine cell is `Sync` through
    /// the `Mutex`; all access is through `engine_mut` on `&mut self`,
    /// so the mutex never blocks.
    pub struct SqlxSubject<DB>
    where
        DB: sqlx::Database,
        <DB as sqlx::Database>::Arguments: sqlx::IntoArguments<DB>,
        for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
    {
        login: String,
        holder: ResourceId,
        engine_cell: Mutex<Rolify<SqlxStore<DB>>>,
        config: RolifyConfig,
    }

    impl<DB> SqlxSubject<DB>
    where
        DB: sqlx::Database,
        <DB as sqlx::Database>::Arguments: sqlx::IntoArguments<DB>,
        for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
    {
        fn engine_mut(&mut self) -> &mut Rolify<SqlxStore<DB>> {
            self.engine_cell
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    #[maybe_async::maybe_async(AFIT)]
    impl<DB> RolifyUser for SqlxSubject<DB>
    where
        DB: sqlx::Database,
        <DB as sqlx::Database>::Arguments: sqlx::IntoArguments<DB>,
        for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
    {
        type Store = SqlxStore<DB>;

        fn store(&mut self) -> &mut Self::Store {
            self.engine_mut().store_with_conn().0
        }

        fn rolify_config(&self) -> &RolifyConfig {
            &self.config
        }

        fn rolify_id(&self) -> ResourceId {
            self.holder.clone()
        }

        fn rolify_type() -> &'static str {
            DefaultUser::rolify_type()
        }

        fn store_with_conn(&mut self) -> (&mut Self::Store, &mut <Self::Store as RoleStore>::Conn) {
            self.engine_mut().store_with_conn()
        }
    }

    /// Trait for engine-specific test operations.
    /// Implemented per concrete engine to provide the operations that differ
    /// between Postgres, MySQL, and SQLite.
    trait SqlxTestEngine<DB>
    where
        DB: sqlx::Database,
    {
        /// Create a new connection to the test database.
        fn make_conn()
        -> impl std::future::Future<Output = <DB as sqlx::Database>::Connection> + Send;

        /// Run the embedded migrations on the connection.
        fn run_migrations(
            conn: &mut <DB as sqlx::Database>::Connection,
        ) -> impl std::future::Future<Output = ()> + Send;

        /// Set up the fixture tables on the connection.
        fn setup_fixtures(
            conn: &mut <DB as sqlx::Database>::Connection,
        ) -> impl std::future::Future<Output = ()> + Send;

        /// Seed the canonical fixture rows.
        fn seed_canonical_rows(
            conn: &mut <DB as sqlx::Database>::Connection,
        ) -> impl std::future::Future<Output = ()> + Send;

        /// Reset the role state on the connection.
        fn reset_roles(
            conn: &mut <DB as sqlx::Database>::Connection,
        ) -> impl std::future::Future<Output = ()> + Send;

        /// Get the pool for this engine (for executor tests).
        #[allow(dead_code)]
        fn get_pool() -> impl std::future::Future<Output = Option<sqlx::Pool<DB>>> + Send;
    }

    // Postgres implementation
    #[cfg(feature = "postgres")]
    impl SqlxTestEngine<sqlx::Postgres> for () {
        fn make_conn() -> impl std::future::Future<Output = sqlx::PgConnection> + Send {
            async {
                let container = pg_container().await;
                let host_port = container
                    .get_host_port_ipv4(5432)
                    .await
                    .expect("Postgres port");
                let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
                sqlx::PgConnection::connect(&url)
                    .await
                    .expect("Postgres connection")
            }
        }

        fn run_migrations(
            conn: &mut sqlx::PgConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                // Use a pool for migrations (pool implements Acquire)
                let container = pg_container().await;
                let host_port = container
                    .get_host_port_ipv4(5432)
                    .await
                    .expect("Postgres port");
                let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
                let pool = sqlx::PgPool::connect(&url)
                    .await
                    .expect("Postgres pool for migrations");
                rolify_sqlx::MIGRATIONS_POSTGRES
                    .run(&pool)
                    .await
                    .expect("Postgres migrations apply");
            }
        }

        fn setup_fixtures(
            _conn: &mut sqlx::PgConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                // Use a pool for fixtures (setup_fixtures_pg expects a pool)
                let container = pg_container().await;
                let host_port = container
                    .get_host_port_ipv4(5432)
                    .await
                    .expect("Postgres port");
                let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
                let pool = sqlx::PgPool::connect(&url)
                    .await
                    .expect("Postgres pool for fixtures");
                setup_fixtures_pg(&pool).await;
            }
        }

        fn seed_canonical_rows(
            conn: &mut sqlx::PgConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                for statement in [
                    "INSERT INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie') ON CONFLICT (id) DO NOTHING",
                    "INSERT INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3') ON CONFLICT (id) DO NOTHING",
                    "INSERT INTO groups (id, name) VALUES (1, 'Group 1'), (2, 'Group 2') ON CONFLICT (id) DO NOTHING",
                    "INSERT INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2') ON CONFLICT (team_code) DO NOTHING",
                    "INSERT INTO organizations (id, type) VALUES (1, 'Organization') ON CONFLICT (id) DO NOTHING",
                ] {
                    sqlx::query(statement)
                        .execute(&mut *conn)
                        .await
                        .expect("seed canonical fixture row");
                }
            }
        }

        fn reset_roles(
            conn: &mut sqlx::PgConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                sqlx::query("TRUNCATE TABLE users_roles, roles RESTART IDENTITY CASCADE")
                    .execute(&mut *conn)
                    .await
                    .expect("truncate roles on Postgres");
            }
        }

        fn get_pool() -> impl std::future::Future<Output = Option<sqlx::Pool<sqlx::Postgres>>> + Send
        {
            async { Some(pg_pool().await) }
        }
    }

    // MySQL implementation
    #[cfg(feature = "mysql")]
    impl SqlxTestEngine<sqlx::MySql> for () {
        fn make_conn() -> impl std::future::Future<Output = sqlx::MySqlConnection> + Send {
            async {
                let container = mysql_container().await;
                let host_port = container
                    .get_host_port_ipv4(3306)
                    .await
                    .expect("MySQL port");
                let url = format!("mysql://root@127.0.0.1:{host_port}/test");
                sqlx::MySqlConnection::connect(&url)
                    .await
                    .expect("MySQL connection")
            }
        }

        fn run_migrations(
            conn: &mut sqlx::MySqlConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                // Use a pool for migrations (pool implements Acquire)
                let container = mysql_container().await;
                let host_port = container
                    .get_host_port_ipv4(3306)
                    .await
                    .expect("MySQL port");
                let url = format!("mysql://root@127.0.0.1:{host_port}/test");
                let pool = sqlx::MySqlPool::connect(&url)
                    .await
                    .expect("MySQL pool for migrations");
                rolify_sqlx::MIGRATIONS_MYSQL
                    .run(&pool)
                    .await
                    .expect("MySQL migrations apply");
            }
        }

        fn setup_fixtures(
            _conn: &mut sqlx::MySqlConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                // Use a pool for fixtures (setup_fixtures_mysql expects a pool)
                let container = mysql_container().await;
                let host_port = container
                    .get_host_port_ipv4(3306)
                    .await
                    .expect("MySQL port");
                let url = format!("mysql://root@127.0.0.1:{host_port}/test");
                let pool = sqlx::MySqlPool::connect(&url)
                    .await
                    .expect("MySQL pool for fixtures");
                setup_fixtures_mysql(&pool).await;
            }
        }

        fn seed_canonical_rows(
            conn: &mut sqlx::MySqlConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                for statement in [
                    "INSERT IGNORE INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie')",
                    "INSERT IGNORE INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3')",
                    "INSERT IGNORE INTO `groups` (id, name) VALUES (1, 'Group 1'), (2, 'Group 2')",
                    "INSERT IGNORE INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2')",
                    "INSERT IGNORE INTO organizations (id, type) VALUES (1, 'Organization')",
                ] {
                    sqlx::query(statement)
                        .execute(&mut *conn)
                        .await
                        .expect("seed canonical fixture row");
                }
            }
        }

        fn reset_roles(
            conn: &mut sqlx::MySqlConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                sqlx::query("DELETE FROM users_roles")
                    .execute(&mut *conn)
                    .await
                    .expect("delete users_roles on MySQL");
                sqlx::query("DELETE FROM roles")
                    .execute(&mut *conn)
                    .await
                    .expect("delete roles on MySQL");
            }
        }

        fn get_pool() -> impl std::future::Future<Output = Option<sqlx::Pool<sqlx::MySql>>> + Send {
            async { Some(mysql_pool().await) }
        }
    }

    // SQLite implementation
    #[cfg(feature = "sqlite")]
    impl SqlxTestEngine<sqlx::Sqlite> for () {
        fn make_conn() -> impl std::future::Future<Output = sqlx::SqliteConnection> + Send {
            async {
                // Use a file-backed SQLite database in the current working directory (writable)
                let cwd = std::env::current_dir().expect("current directory");
                let db_path = cwd.join(format!("rolify_test_{}.db", std::process::id()));
                let connect_options = sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&db_path)
                    .create_if_missing(true)
                    .foreign_keys(true);
                // First, create a pool to run migrations (pool implements Acquire)
                let pool = sqlx::SqlitePool::connect_with(connect_options.clone())
                    .await
                    .expect("SQLite pool for migrations");
                // Run migrations on the pool
                rolify_sqlx::MIGRATIONS_SQLITE
                    .run(&pool)
                    .await
                    .expect("SQLite migrations apply");
                // Now create a direct connection to the same file (migrations already applied)
                sqlx::SqliteConnection::connect(connect_options.to_url_lossy().as_str())
                    .await
                    .expect("SQLite file connection")
            }
        }

        fn run_migrations(
            _conn: &mut sqlx::SqliteConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async { /* Migrations already run in make_conn */ }
        }

        fn setup_fixtures(
            conn: &mut sqlx::SqliteConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                setup_fixtures_sqlite_conn(conn).await;
            }
        }

        fn seed_canonical_rows(
            conn: &mut sqlx::SqliteConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                for statement in [
                    "INSERT OR IGNORE INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie')",
                    "INSERT OR IGNORE INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3')",
                    "INSERT OR IGNORE INTO groups (id, name) VALUES (1, 'Group 1'), (2, 'Group 2')",
                    "INSERT OR IGNORE INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2')",
                    "INSERT OR IGNORE INTO organizations (id, type) VALUES (1, 'Organization')",
                ] {
                    sqlx::query(statement)
                        .execute(&mut *conn)
                        .await
                        .expect("seed canonical fixture row");
                }
            }
        }

        fn reset_roles(
            conn: &mut sqlx::SqliteConnection,
        ) -> impl std::future::Future<Output = ()> + Send {
            async {
                sqlx::query("DELETE FROM users_roles")
                    .execute(&mut *conn)
                    .await
                    .expect("delete users_roles on SQLite");
                sqlx::query("DELETE FROM roles")
                    .execute(&mut *conn)
                    .await
                    .expect("delete roles on SQLite");
                let _ = sqlx::query("DELETE FROM sqlite_sequence WHERE name IN ('roles')")
                    .execute(&mut *conn)
                    .await;
            }
        }

        fn get_pool() -> impl std::future::Future<Output = Option<sqlx::Pool<sqlx::Sqlite>>> + Send
        {
            async { Some(sqlite_memory_pool().await) }
        }
    }

    /// The Sqlx backend for the parity suite.
    pub struct SqlxBackend<DB>
    where
        DB: sqlx::Database,
        <DB as sqlx::Database>::Arguments: sqlx::IntoArguments<DB>,
        for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        (): SqlxTestEngine<DB>,
    {
        // Process-wide suite lock, held for the backend's whole lifetime.
        serial: SuiteGuard,
        // The seated subject (default: "admin").
        subject: SqlxSubject<DB>,
        // All registered holders (canonical fixture ids).
        holders: Vec<(&'static str, ResourceId)>,
        // All registered resources (canonical fixture keys).
        resources: Vec<(FixtureResource, ResourceKey)>,
    }

    impl<DB> Sealed for SqlxBackend<DB>
    where
        DB: sqlx::Database,
        <DB as sqlx::Database>::Arguments: sqlx::IntoArguments<DB>,
        for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        (): SqlxTestEngine<DB>,
    {
    }

    impl<DB> SqlxBackend<DB>
    where
        DB: sqlx::Database,
        <DB as sqlx::Database>::Arguments: sqlx::IntoArguments<DB>,
        for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        (): SqlxTestEngine<DB>,
    {
        /// The canonical resource keys (identity with the seeded rows).
        fn canonical_resources() -> Vec<(FixtureResource, ResourceKey)> {
            vec![
                (FixtureResource::ForumFirst, ResourceKey::new("Forum", "1")),
                (FixtureResource::ForumSecond, ResourceKey::new("Forum", "2")),
                (FixtureResource::ForumLast, ResourceKey::new("Forum", "3")),
                (FixtureResource::GroupFirst, ResourceKey::new("Group", "1")),
                (FixtureResource::GroupLast, ResourceKey::new("Group", "2")),
                (FixtureResource::TeamFirst, ResourceKey::new("Team", "1")),
                (FixtureResource::TeamLast, ResourceKey::new("Team", "2")),
                (
                    FixtureResource::Organization,
                    ResourceKey::new("Organization", "1"),
                ),
                (FixtureResource::Company, ResourceKey::new("Company", "1")),
            ]
        }

        fn holder_id(&self, login: &str) -> Option<ResourceId> {
            self.holders
                .iter()
                .find(|(known, _)| *known == login)
                .map(|(_, holder)| holder.clone())
        }

        fn resource_key(&self, which: FixtureResource) -> Option<ResourceKey> {
            self.resources
                .iter()
                .find(|(known, _)| *known == which)
                .map(|(_, key)| key.clone())
        }
    }

    // ============================================================
    // TestBackend implementation
    // ============================================================

    #[maybe_async::maybe_async(AFIT)]
    impl<DB> rolify_test::backend::TestBackend for SqlxBackend<DB>
    where
        DB: sqlx::Database,
        <DB as sqlx::Database>::Arguments: sqlx::IntoArguments<DB>,
        for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> &'r str: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        for<'r> usize: sqlx::ColumnIndex<<DB as sqlx::Database>::Row>,
        (): SqlxTestEngine<DB>,
    {
        type Store = SqlxStore<DB>;
        type Subject = SqlxSubject<DB>;
        type Error = Error;

        fn build() -> impl Future<Output = Result<Self, Self::Error>> + Send
        where
            Self: Sized,
        {
            async move {
                // Serialize backends sharing one database (see SuiteGuard).
                let serial = SuiteGuard::acquire();
                let config = test_config();
                let mut conn = <() as SqlxTestEngine<DB>>::make_conn().await;

                // Run migrations
                <() as SqlxTestEngine<DB>>::run_migrations(&mut conn).await;

                // Setup fixtures from shared suite (D-06)
                <() as SqlxTestEngine<DB>>::setup_fixtures(&mut conn).await;

                // Seed canonical fixture rows
                <() as SqlxTestEngine<DB>>::seed_canonical_rows(&mut conn).await;

                let store = SqlxStore::new(&config)
                    .for_holder_table("users")
                    .register_resource_table("Forum", "forums", "id")
                    .register_resource_table("Group", "groups", "id")
                    .register_resource_table("Team", "teams", "team_code")
                    .register_resource_table("Organization", "organizations", "id")
                    .register_resource_table("Company", "organizations", "id")
                    .register_resource_table("Right", "rights", "id");
                let engine = Rolify::new(store, conn, config.clone());

                let holders = fixture_holders();
                let admin_holder = holders
                    .iter()
                    .find(|(login, _)| *login == "admin")
                    .map(|(_, holder)| holder.clone())
                    .expect("the admin fixture login is always seated");
                let subject = SqlxSubject {
                    login: "admin".to_owned(),
                    holder: admin_holder,
                    engine_cell: Mutex::new(engine),
                    config,
                };
                let resources = Self::canonical_resources();

                Ok(Self {
                    serial,
                    subject,
                    holders,
                    resources,
                })
            }
        }

        fn subject(&mut self, login: &str) -> &mut Self::Subject {
            let holder = self.holder_id(login).expect("unknown fixture login");
            self.subject.login = login.to_owned();
            self.subject.holder = holder;
            &mut self.subject
        }

        fn holder_id(&self, login: &str) -> Option<ResourceId> {
            self.holder_id(login)
        }

        fn resource(&self, which: FixtureResource) -> ResourceKey {
            self.resource_key(which).expect("unknown fixture resource")
        }

        fn reset_roles(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
            let (_, conn) = self.subject.store_with_conn();
            async move {
                <() as SqlxTestEngine<DB>>::reset_roles(conn).await;
                Ok(())
            }
        }

        fn create_role_row(
            &mut self,
            record: RoleRecord,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send {
            let scope_owned = (record.resource_type.clone(), record.resource_id.clone());
            let name = record.name.clone();
            let (store, conn) = self.subject.store_with_conn();
            async move {
                let scope = match (&scope_owned.0, &scope_owned.1) {
                    (None, None) => ResourceRef::Global,
                    (Some(type_name), None) => ResourceRef::Class(type_name),
                    (Some(type_name), Some(resource_id)) => {
                        ResourceRef::Instance(type_name, resource_id)
                    }
                    (None, Some(_)) => panic!(
                        "a role row with an id but no type is not constructible through the public constructors"
                    ),
                };
                store.find_or_create_by(&mut *conn, &name, scope).await?;
                Ok(())
            }
        }

        fn grant_to(
            &mut self,
            login: &str,
            name: &RoleName,
            scope: ResourceRef<'_>,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send {
            let holder = self.holder_id(login).expect("unknown fixture login");
            let (store, conn) = self.subject.store_with_conn();
            async move {
                let role = store.find_or_create_by(&mut *conn, name, scope).await?;
                store.add(&mut *conn, &holder, &role).await?;
                Ok(())
            }
        }

        fn role_row_count(&mut self) -> impl Future<Output = Result<usize, Self::Error>> + Send {
            let (store, conn) = self.subject.store_with_conn();
            let table = store.role_table().to_owned();
            async move {
                let sql_text = format!("SELECT COUNT(*) AS count FROM {table}");
                let row = sqlx::query(sqlx::AssertSqlSafe(sql_text.as_str()))
                    .fetch_one(&mut *conn)
                    .await?;
                let count_row = CountRow::from_row::<DB>(&row)?;
                Ok(usize::try_from(count_row.count).expect("role row count is never negative"))
            }
        }

        fn engine(&mut self) -> &mut Rolify<Self::Store> {
            self.subject.engine_mut()
        }

        fn reset_query_count(&mut self) {
            self.subject.store().reset_query_count();
        }

        fn query_count(&self) -> Option<usize> {
            // Access the store through the Mutex to get the query count
            // without requiring &mut self (query_count on store takes &self)
            Some(
                self.subject
                    .engine_cell
                    .lock()
                    .unwrap()
                    .store_with_conn()
                    .0
                    .query_count(),
            )
        }
    }
}
