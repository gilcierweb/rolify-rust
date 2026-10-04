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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::OnceCell;

use diesel::Connection;
use diesel::RunQueryDsl;
use diesel::connection::InstrumentationEvent;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel_migrations::MigrationHarness;
use rolify_core::role::ResourceId;
use rolify_diesel::MIGRATIONS;
use rolify_diesel::rows::IdRow;

#[cfg(feature = "postgres")]
use diesel::pg::PgConnection;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use testcontainers::{runners::SyncRunner, ImageExt};
#[cfg(feature = "postgres")]
use testcontainers::runners::AsyncRunner;
#[cfg(feature = "postgres")]
use testcontainers_modules::postgres;

#[cfg(feature = "mysql")]
use diesel::mysql::MysqlConnection;
#[cfg(feature = "mysql")]
use testcontainers_modules::mysql;

#[cfg(feature = "sqlite")]
use diesel::sqlite::SqliteConnection;

// Per-engine connection type for the monomorphic helpers below.
// Diesel 2.3 only executes `sql_query` against concrete backends
// (generic `C: Connection` fails the specialization bounds), so every
// helper takes `&mut Conn` instead of a generic connection.
#[cfg(feature = "postgres")]
type Conn = diesel::pg::PgConnection;
#[cfg(feature = "mysql")]
type Conn = diesel::mysql::MysqlConnection;
#[cfg(feature = "sqlite")]
type Conn = diesel::sqlite::SqliteConnection;

/// Process-wide serializer for tests sharing one database.
///
/// Every backend build truncates the shared `roles` tables, so two
/// suite cases running on neighboring threads wipe each other's rows
/// (the recorded Phase 3 gap 2 mechanism, proven on the race helpers
/// in `concurrency.rs`). Each `DieselBackend` holds this guard for its
/// whole lifetime. Acquisition spins on `yield_now`: timing-free,
/// correctness never depends on timing, only liveness.
/// `std::sync::Mutex` cannot serve here because its guard is `!Sync`
/// and the backend must stay `Sync` for the `TestBackend` bound.
static SUITE_SERIAL: AtomicBool = AtomicBool::new(false);

/// Held for one backend's whole lifetime; releases on drop.
pub struct SuiteGuard {
    flag: &'static AtomicBool,
}

impl SuiteGuard {
    /// Acquire the process-wide suite lock (spins until free).
    #[must_use]
    pub fn acquire() -> Self {
        while SUITE_SERIAL.swap(true, Ordering::Acquire) {
            std::thread::yield_now();
        }
        Self {
            flag: &SUITE_SERIAL,
        }
    }
}

impl Drop for SuiteGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// Shared Postgres container per test binary (SyncRunner).
#[cfg(feature = "postgres")]
static PG_CONTAINER: OnceLock<testcontainers::Container<postgres::Postgres>> = OnceLock::new();

/// Shared MySQL container per test binary (SyncRunner).
#[cfg(feature = "mysql")]
static MYSQL_CONTAINER: OnceLock<testcontainers::Container<mysql::Mysql>> = OnceLock::new();

/// Get or start the shared Postgres container (SyncRunner).
#[cfg(feature = "postgres")]
pub fn pg_container() -> &'static testcontainers::Container<postgres::Postgres> {
    PG_CONTAINER.get_or_init(|| {
        testcontainers::runners::SyncRunner::start(
            postgres::Postgres::default().with_tag("17"),
        )
        .expect("Docker must be available for Postgres parity leg; postgres:17 image will be pulled")
    })
}

/// Get or start the shared Postgres container (AsyncRunner).
#[cfg(feature = "postgres")]
static PG_CONTAINER_ASYNC: OnceCell<testcontainers::ContainerAsync<postgres::Postgres>> = OnceCell::const_new();

#[cfg(feature = "postgres")]
pub async fn pg_container_async() -> &'static testcontainers::ContainerAsync<postgres::Postgres> {
    PG_CONTAINER_ASYNC
        .get_or_init(|| async {
            testcontainers::runners::AsyncRunner::start(
                postgres::Postgres::default().with_tag("17"),
            )
            .await
            .expect("Docker must be available for Postgres parity leg; postgres:17 image will be pulled")
        })
        .await
}

/// Get the Postgres container port (async).
#[cfg(feature = "postgres")]
pub async fn pg_container_port() -> u16 {
    let container = pg_container_async().await;
    container.get_host_port_ipv4(5432).await.expect("Postgres port mapping")
}

/// Get or start the shared MySQL container.
#[cfg(feature = "mysql")]
pub fn mysql_container() -> &'static testcontainers::Container<mysql::Mysql> {
    MYSQL_CONTAINER.get_or_init(|| {
        testcontainers::runners::SyncRunner::start(
            mysql::Mysql::default().with_tag("8.4"),
        )
        .expect("Docker must be available for MySQL parity leg; mysql:8.4 image will be pulled")
    })
}

/// Build a fresh Diesel connection to the Postgres container.
#[cfg(feature = "postgres")]
pub fn pg_conn() -> PgConnection {
    let container = pg_container();
    let host_port = container
        .get_host_port_ipv4(5432)
        .expect("Postgres port mapping");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
    PgConnection::establish(&url).expect("Postgres connection")
}

/// Build a fresh Diesel connection to the MySQL container.
#[cfg(feature = "mysql")]
pub fn mysql_conn() -> MysqlConnection {
    let container = mysql_container();
    let host_port = container
        .get_host_port_ipv4(3306)
        .expect("MySQL port mapping");
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
pub fn run_migrations<Conn, Backend>(conn: &mut Conn)
where
    Conn: MigrationHarness<Backend>,
    Backend: diesel::backend::Backend,
{
    conn.run_pending_migrations(MIGRATIONS)
        .expect("embedded migrations apply cleanly");
}

/// Reset role state (truncate roles + users_roles) for test isolation.
///
/// Uses TRUNCATE ... CASCADE on Postgres/MySQL, DELETE on SQLite.
/// Does NOT touch fixture tables (users, customers, forums, etc.).
pub fn reset_roles(conn: &mut Conn) {
    #[cfg(feature = "postgres")]
    {
        diesel::sql_query("TRUNCATE TABLE users_roles, roles RESTART IDENTITY CASCADE")
            .execute(conn)
            .expect("truncate roles");
    }
    #[cfg(feature = "mysql")]
    {
        // MySQL forbids TRUNCATE on a table referenced by a foreign key
        // (error 1701: users_roles.role_id references roles.id), so the
        // reset is the FK-safe DELETE pair, child links before parent
        // rows (the landed rolify-sqlx helper documents the same rule).
        diesel::sql_query("DELETE FROM users_roles")
            .execute(conn)
            .expect("delete users_roles");
        diesel::sql_query("DELETE FROM roles")
            .execute(conn)
            .expect("delete roles");
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
/// D-06: the statement text lives in the shared suite
/// (`rolify_test::ddl`); this helper only executes the array for the
/// compiled engine, one `sql_query` execution per statement with the
/// same error message as the inlined blocks it replaces. Function name
/// and signature are unchanged, so every call site keeps working.
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
pub fn setup_fixtures(conn: &mut Conn) {
    #[cfg(feature = "postgres")]
    for statement in rolify_test::ddl::POSTGRES {
        diesel::sql_query(*statement)
            .execute(conn)
            .expect("fixture tables");
    }
    #[cfg(feature = "mysql")]
    for statement in rolify_test::ddl::MYSQL {
        diesel::sql_query(*statement)
            .execute(conn)
            .expect("fixture tables");
    }
    #[cfg(feature = "sqlite")]
    for statement in rolify_test::ddl::SQLITE {
        diesel::sql_query(*statement)
            .execute(conn)
            .expect("fixture tables");
    }
}

/// Reset consumer fixture tables for test isolation.
///
/// TRUNCATEs (Postgres/MySQL) or DELETEs (SQLite) the nine fixture
/// tables `setup_fixtures` creates. Role state is NOT touched (use
/// `reset_roles` for that). Fixture tables carry no foreign keys, so
/// no CASCADE is needed. Identity sequences restart where the engine
/// allows it, so the first inserted rows deterministically take the
/// canonical fixture ids again. Tests asserting over fixture-table
/// contents (class expansion, holder universes) call this for
/// hermetic per-test universes; role/link-scoped tests do not need it.
pub fn reset_fixtures(conn: &mut Conn) {
    #[cfg(feature = "postgres")]
    {
        diesel::sql_query(
            "TRUNCATE TABLE users, customers, forums, groups, teams, organizations, rights, moderators_rights, admin_rights RESTART IDENTITY",
        )
        .execute(conn)
        .expect("truncate fixtures");
    }
    #[cfg(feature = "mysql")]
    {
        // No foreign key points at a fixture table, so TRUNCATE is
        // safe here (unlike the roles reset above) and restarts the
        // AUTO_INCREMENT counters.
        diesel::sql_query(
            "TRUNCATE TABLE users, customers, forums, groups, teams, organizations, rights, moderators_rights, admin_rights",
        )
        .execute(conn)
        .expect("truncate fixtures");
    }
    #[cfg(feature = "sqlite")]
    {
        for table in [
            "users",
            "customers",
            "forums",
            "groups",
            "teams",
            "organizations",
            "rights",
            "moderators_rights",
            "admin_rights",
        ] {
            diesel::sql_query(format!("DELETE FROM {table}"))
                .execute(conn)
                .unwrap_or_else(|error| panic!("delete fixtures from {table}: {error}"));
        }
        // Restart the rowid aliases so reinserted rows take the
        // canonical ids again (best effort: only tables declared with
        // the auto-increment keyword track here).
        diesel::sql_query(
            "DELETE FROM sqlite_sequence WHERE name IN ('users', 'customers', 'forums', 'groups', 'organizations', 'rights')",
        )
        .execute(conn)
        .ok();
    }
}

/// Insert a fixture holder (user/customer/admin/etc.) and return its ID.
pub fn insert_holder(conn: &mut Conn, table: &str, holder_type: &str, name: &str) -> ResourceId {
    #[cfg(feature = "postgres")]
    {
        // Use RETURNING id in the INSERT (works for integer PK tables like forums, groups, etc.)
        let row: IdRow = diesel::sql_query(&format!(
            "INSERT INTO {} (rolify_type, name) VALUES ($1, $2) RETURNING id",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(holder_type)
        .bind::<diesel::sql_types::Text, _>(name)
        .get_result(conn)
        .expect("insert holder");
        ResourceId::from(row.id)
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
        // MySQL: get last insert id (aliased for by-name decoding)
        let row: IdRow = diesel::sql_query("SELECT LAST_INSERT_ID() AS id")
            .get_result(conn)
            .expect("last insert id");
        ResourceId::from(row.id)
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
        // SQLite is untyped: decode through a row struct, never a bare primitive.
        let row: IdRow = diesel::sql_query("SELECT last_insert_rowid() AS id")
            .get_result(conn)
            .expect("last insert rowid");
        ResourceId::from(row.id)
    }
}

/// Insert a fixture resource (forum/group/team) and return its key.
pub fn insert_resource(
    conn: &mut Conn,
    table: &str,
    name: &str,
) -> rolify_core::store::ResourceKey {
    #[cfg(feature = "postgres")]
    {
        // Use RETURNING id in the INSERT (works for integer PK tables like forums, groups, etc.)
        let row: IdRow = diesel::sql_query(&format!(
            "INSERT INTO {} (name) VALUES ($1) RETURNING id",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(name)
        .get_result(conn)
        .expect("insert resource");
        rolify_core::store::ResourceKey::new(table.trim_end_matches('s'), row.id.to_string())
    }
    #[cfg(feature = "mysql")]
    {
        diesel::sql_query(&format!("INSERT INTO {} (name) VALUES (?)", table))
            .bind::<diesel::sql_types::Text, _>(name)
            .execute(conn)
            .expect("insert resource");
        let row: IdRow = diesel::sql_query("SELECT LAST_INSERT_ID() AS id")
            .get_result(conn)
            .expect("last insert id");
        rolify_core::store::ResourceKey::new(table.trim_end_matches('s'), row.id.to_string())
    }
    #[cfg(feature = "sqlite")]
    {
        diesel::sql_query(&format!("INSERT INTO {} (name) VALUES (?)", table))
            .bind::<diesel::sql_types::Text, _>(name)
            .execute(conn)
            .expect("insert resource");
        // SQLite is untyped: decode through a row struct, never a bare primitive.
        let row: IdRow = diesel::sql_query("SELECT last_insert_rowid() AS id")
            .get_result(conn)
            .expect("last insert rowid");
        rolify_core::store::ResourceKey::new(table.trim_end_matches('s'), row.id.to_string())
    }
}

/// Query counting instrumentation for TEST-05.
///
/// Installs a counter on a raw connection that counts every
/// `InstrumentationEvent::StartQuery` event the connection emits.
/// That includes transaction control statements (BEGIN/COMMIT reach
/// the hook as queries on diesel 2.3): the counter measures executed
/// statements, and the query-guard cases pin statement counts on
/// paths without transaction plumbing (cached predicates at zero,
/// single round-trip `where_any` at one).
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
    Pool::builder()
        .max_size(4)
        .build(manager)
        .expect("mysql pool")
}

/// Default test configuration for the default role/join table pair.
pub fn test_config() -> rolify_core::config::RolifyConfig {
    rolify_core::config::RolifyConfig::builder()
        .build()
        .unwrap()
}

/// ============================================================
/// DieselBackend — TestBackend implementation for rolify-diesel
/// ============================================================
///
/// The database-backed `TestBackend` the ported parity suite binds to
/// (`parity_suite!` over `DieselBackend`, one line per engine file).
/// One backend owns one engine (store plus its live connection, D-06)
/// over the shared container database; every suite operation flows
/// through that engine connection, so the installed query counter sees
/// every query.
///
/// Two structural facts shape this module:
/// - Diesel connections are `Send` but not `Sync`, while `RolifyUser`
///   (and through it `TestBackend`) requires `Sync`. The engine lives
///   behind a `Mutex`; every method reaches it through `get_mut`
///   (exclusive `&mut self` access, never blocking), which is `Sync`
///   exactly when the connection is `Send`.
/// - Fixture rows carry the suite's canonical ids (holders 1-4,
///   forums 1-3, groups 1-2, teams "1"-"2", organization/company 1,
///   per `rolify-test/src/fixtures.rs`): seeded once with explicit ids
///   plus conflict-ignore, so every backend in the binary converges on
///   the identical rows the `InMemoryBackend` owns by construction.
#[cfg(feature = "suite")]
pub mod diesel_backend {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use diesel::Connection;
    use diesel::RunQueryDsl;
    use diesel_migrations::MigrationHarness;
    use rolify_core::config::RolifyConfig;
    use rolify_core::manager::Rolify;
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::{ResourceKey, RoleStore, Sealed};
    use rolify_core::user::RolifyUser;

    use rolify_test::fixtures::{DefaultUser, FixtureResource, UserClass, fixture_holders};

    use crate::support::*;
    use rolify_diesel::rows::CountRow;
    use rolify_diesel::{DieselStore, MIGRATIONS};

    // Re-export types needed by the trait
    type Store = DieselStore;
    type Error = rolify_diesel::Error;

    // The per-engine connection type (`Conn`) comes from the parent
    // module: every helper is monomorphic over the compiled engine.

    /// The suite subject: a holder identity over the single engine.
    ///
    /// `RolifyUser` requires `Sync`; the engine cell is `Sync` through
    /// the `Mutex` (see the module docs). All access is through
    /// `engine_mut` on `&mut self`, so the mutex never blocks.
    pub struct DieselSubject {
        login: String,
        holder: ResourceId,
        engine_cell: Mutex<Rolify<DieselStore>>,
        config: RolifyConfig,
    }

    impl DieselSubject {
        fn engine_mut(&mut self) -> &mut Rolify<DieselStore> {
            self.engine_cell
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    #[maybe_async::maybe_async(AFIT)]
    impl RolifyUser for DieselSubject {
        type Store = DieselStore;

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

    /// The Diesel backend for the parity suite.
    pub struct DieselBackend {
        // Process-wide suite lock, held for the backend's whole lifetime.
        serial: SuiteGuard,
        // The seated subject (default: "admin").
        subject: DieselSubject,
        // All registered holders (canonical fixture ids).
        holders: Vec<(&'static str, ResourceId)>,
        // All registered resources (canonical fixture keys).
        resources: Vec<(FixtureResource, ResourceKey)>,
        // Query counter installed on the engine connection (TEST-05).
        query_counter: Arc<AtomicUsize>,
    }

    impl Sealed for DieselBackend {}

    impl DieselBackend {
        #[cfg(feature = "postgres")]
        fn make_conn() -> Conn {
            let container = pg_container();
            let host_port = container.get_host_port_ipv4(5432).expect("Postgres port");
            let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
            diesel::pg::PgConnection::establish(&url).expect("Postgres connection")
        }

        #[cfg(feature = "mysql")]
        fn make_conn() -> Conn {
            let container = mysql_container();
            let host_port = container.get_host_port_ipv4(3306).expect("MySQL port");
            let url = format!("mysql://root@127.0.0.1:{host_port}/test");
            diesel::mysql::MysqlConnection::establish(&url).expect("MySQL connection")
        }

        #[cfg(feature = "sqlite")]
        fn make_conn() -> Conn {
            let mut conn =
                diesel::sqlite::SqliteConnection::establish(":memory:").expect("SQLite in-memory");
            diesel::sql_query("PRAGMA foreign_keys = ON")
                .execute(&mut conn)
                .expect("PRAGMA foreign_keys = ON");
            conn
        }

        /// Seed the canonical fixture rows with explicit ids.
        ///
        /// The suite's fixture identities are fixed (`fixtures.rs`):
        /// holders 1-4, forums 1-3, groups 1-2, teams "1"-"2",
        /// organization 1. Explicit ids plus conflict-ignore make every
        /// backend in the binary converge on the identical rows (tables
        /// persist across backends on server engines). The suite never
        /// auto-inserts fixture rows, so the sequences stay consistent.
        #[cfg(feature = "postgres")]
        fn seed_canonical_rows(conn: &mut Conn) {
            for statement in [
                "INSERT INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie') ON CONFLICT (id) DO NOTHING",
                "INSERT INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3') ON CONFLICT (id) DO NOTHING",
                "INSERT INTO groups (id, name) VALUES (1, 'Group 1'), (2, 'Group 2') ON CONFLICT (id) DO NOTHING",
                "INSERT INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2') ON CONFLICT (team_code) DO NOTHING",
                "INSERT INTO organizations (id, type) VALUES (1, 'Organization') ON CONFLICT (id) DO NOTHING",
            ] {
                diesel::sql_query(statement)
                    .execute(conn)
                    .expect("seed canonical fixture row");
            }
        }

        /// Seed the canonical fixture rows with explicit ids (MySQL:
        /// `INSERT IGNORE` skips the duplicate key on repeat builds).
        #[cfg(feature = "mysql")]
        fn seed_canonical_rows(conn: &mut Conn) {
            for statement in [
                "INSERT IGNORE INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie')",
                "INSERT IGNORE INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3')",
                "INSERT IGNORE INTO `groups` (id, name) VALUES (1, 'Group 1'), (2, 'Group 2')",
                "INSERT IGNORE INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2')",
                "INSERT IGNORE INTO organizations (id, type) VALUES (1, 'Organization')",
            ] {
                diesel::sql_query(statement)
                    .execute(conn)
                    .expect("seed canonical fixture row");
            }
        }

        /// Seed the canonical fixture rows with explicit ids (SQLite:
        /// `INSERT OR IGNORE` skips the duplicate key on repeat builds).
        #[cfg(feature = "sqlite")]
        fn seed_canonical_rows(conn: &mut Conn) {
            for statement in [
                "INSERT OR IGNORE INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie')",
                "INSERT OR IGNORE INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3')",
                "INSERT OR IGNORE INTO groups (id, name) VALUES (1, 'Group 1'), (2, 'Group 2')",
                "INSERT OR IGNORE INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2')",
                "INSERT OR IGNORE INTO organizations (id, type) VALUES (1, 'Organization')",
            ] {
                diesel::sql_query(statement)
                    .execute(conn)
                    .expect("seed canonical fixture row");
            }
        }

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
    impl rolify_test::backend::TestBackend for DieselBackend {
        type Store = DieselStore;
        type Subject = DieselSubject;
        type Error = Error;

        fn build() -> impl Future<Output = Result<Self, Self::Error>> + Send
        where
            Self: Sized,
        {
            async move {
                // Serialize backends sharing one database (see SuiteGuard).
                let serial = SuiteGuard::acquire();
                let config = test_config();
                let mut conn = Self::make_conn();

                conn.run_pending_migrations(MIGRATIONS)
                    .expect("migrations apply");
                setup_fixtures(&mut conn);
                Self::seed_canonical_rows(&mut conn);

                // The counter lives on the engine connection, so every
                // suite operation through the subject or the engine counts.
                let query_counter = install_query_counter(&mut conn);

                let store = DieselStore::new(&config)
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
                let subject = DieselSubject {
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
                    query_counter,
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
                reset_roles(conn);
                Ok(())
            }
        }

        fn create_role_row(
            &mut self,
            record: RoleRecord,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send {
            // Own the scope data before the future: the write scope
            // borrows it, and both move into the future together.
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
                let row: CountRow =
                    diesel::sql_query(format!("SELECT COUNT(*) AS count FROM {table}"))
                        .get_result(&mut *conn)
                        .await?;
                Ok(usize::try_from(row.count).expect("role row count is never negative"))
            }
        }

        fn engine(&mut self) -> &mut Rolify<Self::Store> {
            self.subject.engine_mut()
        }

        fn reset_query_count(&mut self) {
            self.query_counter.store(0, Ordering::Relaxed);
        }

        fn query_count(&self) -> Option<usize> {
            Some(self.query_counter.load(Ordering::Relaxed))
        }
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use diesel::deserialize::QueryableByName;

    #[derive(QueryableByName)]
    struct ForeignKeysRow {
        #[diesel(sql_type = diesel::sql_types::Integer)]
        foreign_keys: i32,
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_conn_enables_fk() {
        let mut conn = sqlite_conn();
        // Verify FK pragma is on (SQLite is untyped: decode via a row struct)
        let row: ForeignKeysRow = diesel::sql_query("PRAGMA foreign_keys")
            .get_result(&mut conn)
            .expect("pragma");
        assert_eq!(row.foreign_keys, 1);
    }
}

/// Async test support for the diesel-async rider (D-08/D-09).
///
/// Provides:
/// - bb8 pool helper for AsyncPgConnection (canonical gate pool)
/// - Async migration application via AsyncMigrationHarness (A3)
/// - Async fixture application over rolify_test::ddl
/// - Async reset helpers for role/fixture state
#[cfg(all(feature = "async", feature = "postgres"))]
pub mod async_support {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use bb8::Pool;
    use diesel_async::pooled_connection::AsyncDieselConnectionManager;
    use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, AsyncMigrationHarness};
    use diesel_migrations::MigrationHarness;
    use rolify_core::role::ResourceId;
    use rolify_diesel::MIGRATIONS;

    use crate::support::{pg_container_async, pg_container_port};

    /// Get or build a bb8 pool for AsyncPgConnection.
    pub async fn pg_pool_async() -> Pool<AsyncDieselConnectionManager<AsyncPgConnection>> {
        let host_port = pg_container_port().await;
        let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
        let pool: Pool<AsyncDieselConnectionManager<AsyncPgConnection>> = Pool::builder().max_size(10).build(manager).await.expect("async pg pool");
        pool
    }

    /// Run embedded migrations on a direct async connection (not pooled).
    /// Uses AsyncMigrationHarness which takes ownership of the connection.
    pub async fn run_migrations_async_direct() {
        let host_port = pg_container_port().await;
        let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
        let conn = AsyncPgConnection::establish(&url).await.expect("async pg connection for migrations");
        let mut harness = AsyncMigrationHarness::new(conn);
        harness.run_pending_migrations(MIGRATIONS)
            .expect("async embedded migrations apply cleanly");
    }

    /// Apply fixture DDLs on an async connection (consumes rolify_test::ddl).
    pub async fn setup_fixtures_async(conn: &mut AsyncPgConnection) {
        for statement in rolify_test::ddl::POSTGRES {
            diesel::sql_query(*statement)
                .execute(conn)
                .await
                .expect("async fixture tables");
        }
    }

    /// Reset role state (truncate roles + users_roles) on an async connection.
    pub async fn reset_roles_async(conn: &mut AsyncPgConnection) {
        diesel::sql_query("TRUNCATE TABLE users_roles, roles RESTART IDENTITY CASCADE")
            .execute(conn)
            .await
            .expect("async truncate roles");
    }

    /// Reset consumer fixture tables on an async connection.
    pub async fn reset_fixtures_async(conn: &mut AsyncPgConnection) {
        diesel::sql_query(
            "TRUNCATE TABLE users, customers, forums, groups, teams, organizations, rights, moderators_rights, admin_rights RESTART IDENTITY",
        )
        .execute(conn)
        .await
        .expect("async truncate fixtures");
    }

    /// Insert a fixture holder on an async connection and return its ID.
    pub async fn insert_holder_async(conn: &mut AsyncPgConnection, table: &str, holder_type: &str, name: &str) -> ResourceId {
        let row: rolify_diesel::rows::IdRow = diesel::sql_query(&format!(
            "INSERT INTO {} (rolify_type, name) VALUES ($1, $2) RETURNING id",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(holder_type)
        .bind::<diesel::sql_types::Text, _>(name)
        .get_result(conn)
        .await
        .expect("async insert holder");
        ResourceId::from(row.id)
    }

    /// Insert a fixture resource on an async connection and return its key.
    pub async fn insert_resource_async(
        conn: &mut AsyncPgConnection,
        table: &str,
        name: &str,
    ) -> rolify_core::store::ResourceKey {
        let row: rolify_diesel::rows::IdRow = diesel::sql_query(&format!(
            "INSERT INTO {} (name) VALUES ($1) RETURNING id",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(name)
        .get_result(conn)
        .await
        .expect("async insert resource");
        rolify_core::store::ResourceKey::new(table.trim_end_matches('s'), row.id.to_string())
    }

    /// Query counting instrumentation for async TEST-05.
    pub fn install_query_counter_async(conn: &mut AsyncPgConnection) -> Arc<AtomicUsize> {
        let counter = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&counter);
        conn.set_instrumentation(Box::new(move |event: diesel::connection::InstrumentationEvent<'_>| {
            if matches!(event, diesel::connection::InstrumentationEvent::StartQuery { .. }) {
                counting.fetch_add(1, Ordering::Relaxed);
            }
        }));
        counter
    }
}

/// Async DieselBackend for the parity suite (async mode).
#[cfg(all(feature = "async", feature = "postgres", feature = "suite"))]
pub mod diesel_async_backend {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use bb8::Pool;
    use diesel_async::pooled_connection::bb8::AsyncDieselConnectionManager;
    use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
    use rolify_core::config::RolifyConfig;
    use rolify_core::manager::Rolify;
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::{ResourceKey, RoleStore, Sealed};
    use rolify_core::user::RolifyUser;

    use rolify_test::fixtures::{DefaultUser, FixtureResource, UserClass, fixture_holders};

    use crate::support::async_support::*;
    use rolify_diesel::rows::CountRow;
    use rolify_diesel::{DieselStore, MIGRATIONS};

    // Re-export types needed by the trait
    type Store = DieselStore;
    type Error = rolify_diesel::Error;

    /// The suite subject: a holder identity over the single async engine.
    pub struct DieselAsyncSubject {
        login: String,
        holder: ResourceId,
        engine_cell: Mutex<Rolify<DieselStore>>,
        config: RolifyConfig,
    }

    impl DieselAsyncSubject {
        fn engine_mut(&mut self) -> &mut Rolify<DieselStore> {
            self.engine_cell
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    #[maybe_async::maybe_async(AFIT)]
    impl RolifyUser for DieselAsyncSubject {
        type Store = DieselStore;

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

    /// The async Diesel backend for the parity suite.
    pub struct DieselAsyncBackend {
        // Process-wide suite lock, held for the backend's whole lifetime.
        serial: crate::support::SuiteGuard,
        // The seated subject (default: "admin").
        subject: DieselAsyncSubject,
        // All registered holders (canonical fixture ids).
        holders: Vec<(&'static str, ResourceId)>,
        // All registered resources (canonical fixture keys).
        resources: Vec<(FixtureResource, ResourceKey)>,
        // Query counter installed on the engine connection (TEST-05).
        query_counter: Arc<AtomicUsize>,
        // bb8 pool for executor tests (SC-5).
        pool: Pool<AsyncDieselConnectionManager<AsyncPgConnection>>,
    }

    impl Sealed for DieselAsyncBackend {}

    impl DieselAsyncBackend {
        /// Get a checkout from the bb8 pool (DerefMut -> &mut AsyncPgConnection).
        pub async fn checkout(&self) -> bb8::PooledConnection<'_, AsyncDieselConnectionManager<AsyncPgConnection>> {
            self.pool.get().await.expect("async pool checkout")
        }

        /// Seed the canonical fixture rows with explicit ids.
        async fn seed_canonical_rows(conn: &mut AsyncPgConnection) {
            for statement in [
                "INSERT INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie') ON CONFLICT (id) DO NOTHING",
                "INSERT INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3') ON CONFLICT (id) DO NOTHING",
                "INSERT INTO groups (id, name) VALUES (1, 'Group 1'), (2, 'Group 2') ON CONFLICT (id) DO NOTHING",
                "INSERT INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2') ON CONFLICT (team_code) DO NOTHING",
                "INSERT INTO organizations (id, type) VALUES (1, 'Organization') ON CONFLICT (id) DO NOTHING",
            ] {
                diesel::sql_query(statement)
                    .execute(conn)
                    .await
                    .expect("seed async canonical fixture row");
            }
        }

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
    // TestBackend implementation (async)
    // ============================================================

    #[maybe_async::maybe_async(AFIT)]
    impl rolify_test::backend::TestBackend for DieselAsyncBackend {
        type Store = DieselStore;
        type Subject = DieselAsyncSubject;
        type Error = Error;

        fn build() -> impl Future<Output = Result<Self, Self::Error>> + Send
        where
            Self: Sized,
        {
            async move {
                // Serialize backends sharing one database (see SuiteGuard).
                let serial = crate::support::SuiteGuard::acquire();
                let config = crate::support::test_config();
                
                // Run migrations on a direct connection first (AsyncMigrationHarness needs ownership)
                run_migrations_async_direct().await;
                
                let pool = pg_pool_async().await;
                let mut conn = pool.get().await.expect("initial async pool checkout");

                setup_fixtures_async(&mut conn).await;
                Self::seed_canonical_rows(&mut conn).await;

                // The counter lives on the engine connection, so every
                // suite operation through the subject or the engine counts.
                let query_counter = install_query_counter_async(&mut conn);

                let store = DieselStore::new(&config)
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
                let subject = DieselAsyncSubject {
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
                    query_counter,
                    pool,
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
            let pool = self.pool.clone();
            async move {
                let mut conn = pool.get().await.expect("async pool checkout for reset");
                reset_roles_async(&mut conn).await;
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
            let pool = self.pool.clone();
            async move {
                let mut conn = pool.get().await.expect("async pool checkout for count");
                let table = "roles";
                let row: CountRow =
                    diesel::sql_query(format!("SELECT COUNT(*) AS count FROM {table}"))
                        .get_result(&mut *conn)
                        .await?;
                Ok(usize::try_from(row.count).expect("role row count is never negative"))
            }
        }

        fn engine(&mut self) -> &mut Rolify<Self::Store> {
            self.subject.engine_mut()
        }

        fn reset_query_count(&mut self) {
            self.query_counter.store(0, Ordering::Relaxed);
        }

        fn query_count(&self) -> Option<usize> {
            Some(self.query_counter.load(Ordering::Relaxed))
        }
    }
}
