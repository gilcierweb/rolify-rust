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

/// ============================================================
/// DieselBackend — TestBackend implementation for rolify-diesel
/// ============================================================
///
/// Implements the full D-13 fixture matrix with query counting
/// instrumentation for TEST-05. One backend per holder/role-table pair.
#[cfg(feature = "suite")]
pub mod diesel_backend {
    use std::sync::{Arc, OnceLock};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use diesel::Connection;
    use diesel::connection::{Connection as _, InstrumentationEvent};
    use diesel_migrations::MigrationHarness;
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::config::RolifyConfig;
    use rolify_core::manager::Rolify;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::{ResourceKey, ResourceRef};
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::{RemovalOutcome, ResourceStore, RoleStore, Sealed, ScopeColumn, RemovalTarget};
    use rolify_core::user::RolifyUser;

    use rolify_test::fixtures::{DefaultUser, FixtureResource, FixtureUser as TestFixtureUser};

    use crate::support::*;
    use rolify_diesel::{DieselStore, MIGRATIONS};

    // Re-export types needed by the trait
    type Store = DieselStore;
    type Error = rolify_diesel::Error;

    // Per-engine connection type.
    #[cfg(feature = "postgres")]
    type Conn = diesel::pg::PgConnection;
    #[cfg(feature = "mysql")]
    type Conn = diesel::mysql::MysqlConnection;
    #[cfg(feature = "sqlite")]
    type Conn = diesel::sqlite::SqliteConnection;

    /// Holder fixture definition.
    #[derive(Clone, Debug)]
    struct HolderFixture {
        login: &'static str,
        table: &'static str,
        holder_type: &'static str,
        name: &'static str,
    }

    /// Resource fixture definition.
    #[derive(Clone, Debug)]
    struct ResourceFixture {
        which: FixtureResource,
        table: &'static str,
        type_name: &'static str,
        pk_column: &'static str,
    }

    /// The Diesel backend for the parity suite.
    pub struct DieselBackend {
        // The main subject user (default: "admin")
        subject: TestFixtureUser<DefaultUser, DieselStore>,
        // All registered holders
        holders: Vec<(&'static str, ResourceId)>,
        // All registered resources
        resources: Vec<(FixtureResource, ResourceKey)>,
        // Query counter (shared across connection wrappers)
        query_counter: Arc<AtomicUsize>,
        // Connection factory
        conn_factory: Box<dyn Fn() -> Conn + Send + Sync + 'static>,
    }

    impl Sealed for DieselBackend {}

    impl DieselBackend {
        /// Build a fresh backend with the full D-13 fixture set.
        pub async fn build() -> Result<Self, Error> {
            let config = test_config();
            let mut conn = Self::make_conn();

            // Run migrations
            conn.run_pending_migrations(MIGRATIONS).expect("migrations apply");

            // Setup fixture tables
            setup_fixtures(&mut conn);

            // Install query counter
            let query_counter = install_query_counter(&mut conn);

            // Create store
            let mut store = DieselStore::new(&config);

            // Register holder tables for all pairs (D-07: one store per pair)
            // Default pair: users / roles / users_roles
            store = store.for_holder_table("users");
            // Register additional pairs for the suite
            store = store.for_holder_table("customers");
            store = store.for_holder_table("admins"); // for Admin::Moderator

            // Register resource tables for class-scope expansion (resources_find)
            store = store.register_resource_table("Forum", "forums", "id");
            store = store.register_resource_table("Group", "groups", "id");
            store = store.register_resource_table("Team", "teams", "team_code");
            store = store.register_resource_table("Organization", "organizations", "id");
            store = store.register_resource_table("Company", "organizations", "id"); // STI
            store = store.register_resource_table("Right", "rights", "id");

            // Create the engine
            let engine = Rolify::new(store, (), config);

            // Register holders (the full fixture `users` table + other pairs)
            let holder_fixtures = [
                HolderFixture { login: "admin", table: "users", holder_type: "User", name: "Admin User" },
                HolderFixture { login: "moderator", table: "users", holder_type: "User", name: "Moderator User" },
                HolderFixture { login: "god", table: "users", holder_type: "User", name: "God User" },
                HolderFixture { login: "zombie", table: "users", holder_type: "User", name: "Zombie User" },
                // Customer pair
                HolderFixture { login: "customer1", table: "customers", holder_type: "Customer", name: "Customer One" },
                HolderFixture { login: "customer2", table: "customers", holder_type: "Customer", name: "Customer Two" },
                // Admin::Moderator pair
                HolderFixture { login: "admin_mod", table: "admins", holder_type: "Admin::Moderator", name: "Admin Moderator" },
            ];

            let mut holders = Vec::new();
            for fixture in &holder_fixtures {
                let id = insert_holder(&mut conn, fixture.table, fixture.holder_type, fixture.name);
                holders.push((fixture.login, id.clone()));
                // Register with store for all_holders / holders_where
                engine.store().register_holder(fixture.holder_type, id);
            }

            // Register resources (the full fixture resources)
            let resource_fixtures = [
                ResourceFixture { which: FixtureResource::ForumFirst, table: "forums", type_name: "Forum", pk_column: "id" },
                ResourceFixture { which: FixtureResource::ForumSecond, table: "forums", type_name: "Forum", pk_column: "id" },
                ResourceFixture { which: FixtureResource::ForumLast, table: "forums", type_name: "Forum", pk_column: "id" },
                ResourceFixture { which: FixtureResource::GroupFirst, table: "groups", type_name: "Group", pk_column: "id" },
                ResourceFixture { which: FixtureResource::GroupLast, table: "groups", type_name: "Group", pk_column: "id" },
                ResourceFixture { which: FixtureResource::TeamFirst, table: "teams", type_name: "Team", pk_column: "team_code" },
                ResourceFixture { which: FixtureResource::TeamLast, table: "teams", type_name: "Team", pk_column: "team_code" },
                ResourceFixture { which: FixtureResource::Organization, table: "organizations", type_name: "Organization", pk_column: "id" },
                ResourceFixture { which: FixtureResource::Company, table: "organizations", type_name: "Company", pk_column: "id" },
            ];

            let mut resources = Vec::new();
            for fixture in &resource_fixtures {
                let key = insert_resource(&mut conn, fixture.table, fixture.which.to_string().replace("_", " ").replace("first", "First").replace("second", "Second").replace("last", "Last"));
                // The key's resource_type is the singular (forum, group, team, organization)
                // We need to map it to the type_name for the registry
                let mapped_key = ResourceKey::new(fixture.type_name, key.resource_id);
                resources.push((fixture.which, mapped_key));
                engine.store().register_resource(mapped_key);
            }

            // Create subject (default to "admin" user)
            let admin_id = holders.iter().find(|(login, _)| *login == "admin").map(|(_, id)| id.clone()).expect("admin holder");
            let subject = TestFixtureUser::new("admin", admin_id, engine);

            // Build connection factory for new connections
            let conn_factory = Box::new(Self::make_conn);

            Ok(Self {
                subject,
                holders,
                resources,
                query_counter,
                conn_factory,
            })
        }

        #[cfg(feature = "postgres")]
        fn make_conn() -> Conn {
            let container = pg_container();
            let host_port = container.get_host_port_ipv4(5432).expect("Postgres port");
            let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
            PgConnection::establish(&url).expect("Postgres connection")
        }

        #[cfg(feature = "mysql")]
        fn make_conn() -> Conn {
            let container = mysql_container();
            let host_port = container.get_host_port_ipv4(3306).expect("MySQL port");
            let url = format!("mysql://root@127.0.0.1:{host_port}/test");
            MysqlConnection::establish(&url).expect("MySQL connection")
        }

        #[cfg(feature = "sqlite")]
        fn make_conn() -> Conn {
            let mut conn = SqliteConnection::establish(":memory:").expect("SQLite in-memory");
            diesel::sql_query("PRAGMA foreign_keys = ON").execute(&mut conn).expect("PRAGMA foreign_keys = ON");
            conn
        }

        fn make_counted_conn(&self) -> (Conn, Arc<AtomicUsize>) {
            let mut conn = (self.conn_factory)();
            let counter = Arc::new(AtomicUsize::new(0));
            let counting = Arc::clone(&counter);
            conn.set_instrumentation(Box::new(move |event: InstrumentationEvent<'_>| {
                if matches!(event, InstrumentationEvent::StartQuery { .. }) {
                    counting.fetch_add(1, Ordering::Relaxed);
                }
            }));
            (conn, counter)
        }

        fn holder_id(&self, login: &str) -> Option<ResourceId> {
            self.holders.iter().find(|(l, _)| *l == login).map(|(_, id)| id.clone())
        }

        fn resource_key(&self, which: FixtureResource) -> Option<ResourceKey> {
            self.resources.iter().find(|(w, _)| *w == which).map(|(_, k)| k.clone())
        }
    }

    // ============================================================
    // TestBackend implementation
    // ============================================================

    #[maybe_async::maybe_async(AFIT)]
    impl rolify_test::backend::TestBackend for DieselBackend {
        type Store = DieselStore;
        type Subject = TestFixtureUser<DefaultUser, DieselStore>;
        type Error = Error;

        async fn build() -> Result<Self, Self::Error>
        where
            Self: Sized,
        {
            Self::build().await
        }

        fn subject(&mut self, login: &str) -> &mut Self::Subject {
            let holder = self.holder_id(login).expect("unknown fixture login");
            self.subject.seat_as(login, holder);
            &mut self.subject
        }

        fn holder_id(&self, login: &str) -> Option<ResourceId> {
            self.holder_id(login)
        }

        fn resource(&self, which: FixtureResource) -> ResourceKey {
            self.resource_key(which).expect("unknown fixture resource")
        }

        async fn reset_roles(&mut self) -> Result<(), Self::Error> {
            // Need a fresh connection for reset
            let mut conn = (self.conn_factory)();
            reset_roles(&mut conn);
            // Also clear the in-memory store
            rolify_core::user::RolifyUser::store(&mut self.subject).clear();
            Ok(())
        }

        async fn create_role_row(&mut self, record: RoleRecord) -> Result<(), Self::Error> {
            let (mut conn, _counter) = self.make_counted_conn();
            rolify_core::user::RolifyUser::store(&mut self.subject).insert(record);
            Ok(())
        }

        async fn grant_to(&mut self, login: &str, name: &RoleName, scope: ResourceRef<'_>) -> Result<(), Self::Error> {
            let holder = self.holder_id(login).expect("unknown fixture login");
            let (mut conn, _counter) = self.make_counted_conn();
            let role = rolify_core::user::RolifyUser::store(&mut self.subject)
                .find_or_create_by(&mut conn, name, scope)
                .await?;
            rolify_core::user::RolifyUser::store(&mut self.subject)
                .add(&mut conn, &holder, &role)
                .await?;
            Ok(())
        }

        async fn role_row_count(&mut self) -> Result<usize, Self::Error> {
            let count = rolify_core::user::RolifyUser::store(&mut self.subject).rows().len();
            Ok(count)
        }

        fn engine(&mut self) -> &mut Rolify<Self::Store> {
            self.subject.engine()
        }

        fn reset_query_count(&mut self) {
            self.query_counter.store(0, Ordering::Relaxed);
        }

        fn query_count(&self) -> Option<usize> {
            Some(self.query_counter.load(Ordering::Relaxed))
        }
    }
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