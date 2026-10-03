//! Test support for `rolify-diesel` integration tests.
//!
//! Provides:
//! - Postgres testcontainer bootstrap (postgres:17, SyncRunner, built-in readiness)
//! - MySQL testcontainer bootstrap (mysql:8.4, SyncRunner, built-in readiness)
//! - SQLite in-memory connection helper
//! - Shared OnceLock container per test binary
//! - Schema bootstrap (run migrations + fixture tables)
//! - Query counting instrumentation hook (TEST-05)
//! - Multi-pair fixture setup (mirrors `rolify/spec/support/schema.rb`)

use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};

use diesel::Connection;
use diesel::connection::{Connection as _, InstrumentationEvent};
use diesel::r2d2::{ConnectionManager, Pool};
use diesel_migrations::MigrationHarness;
use rolify_core::role::ResourceId;
use rolify_diesel::{DieselStore, MIGRATIONS};

#[cfg(feature = "postgres")]
use diesel::pg::PgConnection;
#[cfg(feature = "postgres")]
use testcontainers_modules::{postgres, testcontainers::runners::SyncRunner};

#[cfg(feature = "mysql")]
use diesel::mysql::MysqlConnection;
#[cfg(feature = "mysql")]
use testcontainers_modules::{mysql, testcontainers::runners::SyncRunner};

#[cfg(feature = "sqlite")]
use diesel::sqlite::SqliteConnection;

/// Shared Postgres container per test binary (SyncRunner).
#[cfg(feature = "postgres")]
static PG_CONTAINER: OnceLock<testcontainers::Container<SyncRunner, postgres::Postgres>> = OnceLock::new();

/// Shared MySQL container per test binary (SyncRunner).
#[cfg(feature = "mysql")]
static MYSQL_CONTAINER: OnceLock<testcontainers::Container<SyncRunner, mysql::Mysql>> = OnceLock::new();

/// Get or start the shared Postgres container.
#[cfg(feature = "postgres")]
pub fn pg_container() -> &'static testcontainers::Container<SyncRunner, postgres::Postgres> {
    PG_CONTAINER.get_or_init(|| {
        postgres::Postgres::default()
            .with_tag("17")
            .start()
            .expect("Docker must be available for Postgres parity leg; postgres:17 image will be pulled")
    })
}

/// Get or start the shared MySQL container.
#[cfg(feature = "mysql")]
pub fn mysql_container() -> &'static testcontainers::Container<SyncRunner, mysql::Mysql> {
    MYSQL_CONTAINER.get_or_init(|| {
        mysql::Mysql::default()
            .with_tag("8.4")
            .start()
            .expect("Docker must be available for MySQL parity leg; mysql:8.4 image will be pulled")
    })
}

/// Build a fresh Diesel connection to the Postgres container.
#[cfg(feature = "postgres")]
pub fn pg_conn() -> PgConnection {
    let container = pg_container();
    let host_port = container.get_host_port_ipv4(5432).expect("Postgres port mapping");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
    PgConnection::establish(&url).expect("Postgres connection")
}

/// Build a fresh Diesel connection to the MySQL container.
#[cfg(feature = "mysql")]
pub fn mysql_conn() -> MysqlConnection {
    let container = mysql_container();
    let host_port = container.get_host_port_ipv4(3306).expect("MySQL port mapping");
    // Module default: root with EMPTY password, db "test"
    let url = format!("mysql://root@127.0.0.1:{host_port}/test");
    MysqlConnection::establish(&url).expect("MySQL connection")
}

/// Build a fresh Diesel SQLite in-memory connection.
#[cfg(feature = "sqlite")]
pub fn sqlite_conn() -> SqliteConnection {
    let mut conn = SqliteConnection::establish(":memory:").expect("SQLite in-memory");
    // SQLite ships with FKs off; D-10 cascade sweep needs them on
    diesel::sql_query("PRAGMA foreign_keys = ON")
        .execute(&mut conn)
        .expect("PRAGMA foreign_keys = ON");
    conn
}

/// Run the embedded migrations on a connection.
pub fn run_migrations<C>(conn: &mut C)
where
    C: MigrationHarness<diesel::backend::Backend> + Connection,
{
    conn.run_pending_migrations(MIGRATIONS)
        .expect("embedded migrations apply cleanly");
}

/// Reset role state (truncate roles + users_roles) for test isolation.
///
/// Uses TRUNCATE ... CASCADE on Postgres/MySQL, DELETE on SQLite.
/// Does NOT touch fixture tables (users, customers, forums, etc.).
pub fn reset_roles<C>(conn: &mut C)
where
    C: Connection,
{
    #[cfg(feature = "postgres")]
    {
        diesel::sql_query("TRUNCATE TABLE users_roles, roles RESTART IDENTITY CASCADE")
            .execute(conn)
            .expect("truncate roles");
    }
    #[cfg(feature = "mysql")]
    {
        diesel::sql_query("TRUNCATE TABLE users_roles, roles")
            .execute(conn)
            .expect("truncate roles");
    }
    #[cfg(feature = "sqlite")]
    {
        diesel::sql_query("DELETE FROM users_roles")
            .execute(conn)
            .expect("delete users_roles");
        diesel::sql_query("DELETE FROM roles")
            .execute(conn)
            .expect("delete roles");
        // Reset autoincrement
        diesel::sql_query("DELETE FROM sqlite_sequence WHERE name IN ('roles')")
            .execute(conn)
            .ok(); // best effort
    }
}

/// Setup fixture tables mirroring `rolify/spec/support/schema.rb`.
///
/// Creates:
/// - `users` (id BIGSERIAL/INTEGER PK, rolify_type VARCHAR, name VARCHAR)
/// - `customers` (id BIGSERIAL/INTEGER PK, rolify_type VARCHAR, name VARCHAR)
/// - `forums` (id BIGSERIAL/INTEGER PK, name VARCHAR)
/// - `groups` (id BIGSERIAL/INTEGER PK, name VARCHAR)
/// - `teams` (team_code VARCHAR PK — string PK per schema.rb)
/// - `organizations` (id BIGSERIAL/INTEGER PK, type VARCHAR — STI family)
/// - `rights` (id BIGSERIAL/INTEGER PK, name VARCHAR — for custom pairs)
/// - `moderators_rights` (custom join: moderator_id + right_id)
/// - `admin_rights` (custom join: admin_id + right_id)
///
/// These are the consumer tables the suite expects. The store's
/// `holder_table` for the default pair is `users`.
pub fn setup_fixtures<C>(conn: &mut C)
where
    C: Connection,
{
    #[cfg(feature = "postgres")]
    {
        diesel::sql_query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id BIGSERIAL PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'User',
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS customers (
                id BIGSERIAL PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'Customer',
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS forums (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS groups (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS teams (
                team_code VARCHAR(191) PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS organizations (
                id BIGSERIAL PRIMARY KEY,
                type VARCHAR(191) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS rights (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            );
            CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            );
            "#
        )
        .execute(conn)
        .expect("fixture tables");
    }
    #[cfg(feature = "mysql")]
    {
        diesel::sql_query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'User',
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS customers (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'Customer',
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS forums (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS groups (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS teams (
                team_code VARCHAR(191) PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS organizations (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                type VARCHAR(191) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS rights (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            "#
        )
        .execute(conn)
        .expect("fixture tables");
    }
    #[cfg(feature = "sqlite")]
    {
        diesel::sql_query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rolify_type TEXT NOT NULL DEFAULT 'User',
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS customers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rolify_type TEXT NOT NULL DEFAULT 'Customer',
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS forums (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS groups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS teams (
                team_code TEXT PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS organizations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                type TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS rights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id TEXT NOT NULL,
                right_id INTEGER NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            );
            CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id TEXT NOT NULL,
                right_id INTEGER NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            );
            "#
        )
        .execute(conn)
        .expect("fixture tables");
    }
}

/// Insert a fixture holder (user/customer/admin/etc.) and return its ID.
pub fn insert_holder<C>(conn: &mut C, table: &str, holder_type: &str, name: &str) -> ResourceId
where
    C: Connection,
{
    #[cfg(feature = "postgres")]
    {
        let id: i64 = diesel::sql_query(&format!(
            "INSERT INTO {} (rolify_type, name) VALUES ($1, $2) RETURNING id",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(holder_type)
        .bind::<diesel::sql_types::Text, _>(name)
        .get_result(conn)
        .expect("insert holder");
        ResourceId::from(id)
    }
    #[cfg(feature = "mysql")]
    {
        diesel::sql_query(&format!(
            "INSERT INTO {} (rolify_type, name) VALUES (?, ?)",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(holder_type)
        .bind::<diesel::sql_types::Text, _>(name)
        .execute(conn)
        .expect("insert holder");
        // MySQL: get last insert id
        let id: i64 = diesel::sql_query("SELECT LAST_INSERT_ID()")
            .get_result(conn)
            .expect("last insert id");
        ResourceId::from(id)
    }
    #[cfg(feature = "sqlite")]
    {
        diesel::sql_query(&format!(
            "INSERT INTO {} (rolify_type, name) VALUES (?, ?)",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(holder_type)
        .bind::<diesel::sql_types::Text, _>(name)
        .execute(conn)
        .expect("insert holder");
        let id: i64 = diesel::sql_query("SELECT last_insert_rowid()")
            .get_result(conn)
            .expect("last insert rowid");
        ResourceId::from(id)
    }
}

/// Insert a fixture resource (forum/group/team) and return its key.
pub fn insert_resource<C>(conn: &mut C, table: &str, name: &str) -> rolify_core::store::ResourceKey
where
    C: Connection,
{
    #[cfg(feature = "postgres")]
    {
        let row: (String, String) = diesel::sql_query(&format!(
            "INSERT INTO {} (name) VALUES ($1) RETURNING id, name",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(name)
        .get_result(conn)
        .expect("insert resource");
        let (id, _name) = row;
        rolify_core::store::ResourceKey::new(table.trim_end_matches('s'), id) // forums -> forum
    }
    #[cfg(feature = "mysql")]
    {
        diesel::sql_query(&format!(
            "INSERT INTO {} (name) VALUES (?)",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(name)
        .execute(conn)
        .expect("insert resource");
        let id: i64 = diesel::sql_query("SELECT LAST_INSERT_ID()")
            .get_result(conn)
            .expect("last insert id");
        rolify_core::store::ResourceKey::new(table.trim_end_matches('s'), id.to_string())
    }
    #[cfg(feature = "sqlite")]
    {
        diesel::sql_query(&format!(
            "INSERT INTO {} (name) VALUES (?)",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(name)
        .execute(conn)
        .expect("insert resource");
        let id: i64 = diesel::sql_query("SELECT last_insert_rowid()")
            .get_result(conn)
            .expect("last insert rowid");
        rolify_core::store::ResourceKey::new(table.trim_end_matches('s'), id.to_string())
    }
}

/// Query counting instrumentation for TEST-05.
///
/// Wraps a connection and counts `InstrumentationEvent::StartQuery` events.
/// Transaction control events (Begin/Commit/Rollback) are NOT counted.
pub struct CountingConn<C> {
    conn: C,
    counter: Arc<AtomicUsize>,
}

impl<C> CountingConn<C>
where
    C: Connection,
{
    pub fn new(conn: C) -> (Self, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let counting = CountingConn {
            conn,
            counter: Arc::clone(&counter),
        };
        (counting, counter)
    }

    pub fn into_inner(self) -> C {
        self.conn
    }
}

impl<C> std::ops::Deref for CountingConn<C> {
    type Target = C;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl<C> std::ops::DerefMut for CountingConn<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

impl<C> Connection for CountingConn<C>
where
    C: Connection,
{
    type Backend = C::Backend;
    type TransactionManager = C::TransactionManager;

    fn establish(database_url: &str) -> Result<Self, diesel::ConnectionError>
    where
        Self: Sized,
    {
        C::establish(database_url).map(|conn| {
            let counter = Arc::new(AtomicUsize::new(0));
            CountingConn { conn, counter }
        })
    }

    fn execute(&mut self, query: &str) -> Result<usize, diesel::result::Error> {
        self.counter.fetch_add(1, Ordering::Relaxed);
        self.conn.execute(query)
    }

    fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: From<diesel::result::Error>,
    {
        self.conn.transaction(|conn| {
            let mut wrapped = CountingConn {
                conn,
                counter: Arc::clone(&self.counter),
            };
            f(&mut wrapped)
        })
    }

    fn begin_test_transaction(&mut self) -> Result<(), diesel::result::Error> {
        self.conn.begin_test_transaction()
    }

    fn rollback_test_transaction(&mut self) -> Result<(), diesel::result::Error> {
        self.conn.rollback_test_transaction()
    }

    fn get_instrumentation(&mut self) -> &mut dyn diesel::connection::Instrumentation {
        self.conn.get_instrumentation()
    }

    fn set_instrumentation(&mut self, instrumentation: Box<dyn diesel::connection::Instrumentation>) {
        self.conn.set_instrumentation(instrumentation)
    }
}

/// Helper to install a query counter on a raw connection.
pub fn install_query_counter<C>(conn: &mut C) -> Arc<AtomicUsize>
where
    C: Connection,
{
    let counter = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&counter);
    conn.set_instrumentation(Box::new(move |event: InstrumentationEvent<'_>| {
        if matches!(event, InstrumentationEvent::StartQuery { .. }) {
            counting.fetch_add(1, Ordering::Relaxed);
        }
    }));
    counter
}

/// r2d2 pool helper for executor acceptance tests (SC-5).
#[cfg(feature = "postgres")]
pub fn pg_pool() -> Pool<ConnectionManager<PgConnection>> {
    let container = pg_container();
    let host_port = container.get_host_port_ipv4(5432).expect("Postgres port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
    let manager = ConnectionManager::<PgConnection>::new(url);
    Pool::builder().max_size(4).build(manager).expect("pg pool")
}

#[cfg(feature = "mysql")]
pub fn mysql_pool() -> Pool<ConnectionManager<MysqlConnection>> {
    let container = mysql_container();
    let host_port = container.get_host_port_ipv4(3306).expect("MySQL port");
    let url = format!("mysql://root@127.0.0.1:{host_port}/test");
    let manager = ConnectionManager::<MysqlConnection>::new(url);
    Pool::builder().max_size(4).build(manager).expect("mysql pool")
}

/// Default test configuration for the default role/join table pair.
pub fn test_config() -> rolify_core::config::RolifyConfig {
    rolify_core::config::RolifyConfig::builder().build().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_conn_enables_fk() {
        let mut conn = sqlite_conn();
        // Verify FK pragma is on
        let fk: i64 = diesel::sql_query("PRAGMA foreign_keys")
            .get_result(&mut conn)
            .expect("pragma");
        assert_eq!(fk, 1);
    }
}