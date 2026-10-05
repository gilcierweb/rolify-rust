//! Test bootstrap for `rolify-seaorm`: testcontainers engines (Postgres
//! 17 / `MySQL` 8.4, pinned images, runner readiness waits), `Migrator::up`,
//! shared fixture DDL from `rolify-test::ddl`, canonical fixture rows, and
//! the `SeaormBackend` [`TestBackend`] implementation with the explicit
//! query-count seam (TEST-05).
//!
//! Reset deletes only `users_roles` and `roles` rows; fixture tables are
//! seeded once per container and never truncated.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;

use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::{ResourceKey, RoleStore, Sealed};
use rolify_core::user::RolifyUser;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

use rolify_seaorm::{Error, Migrator, SeaormStore};
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::{FixtureResource, FixtureResources, UserClass, fixture_holders};

/// The store type used by the suite bindings: a plain pooled
/// `DatabaseConnection` executor.
pub type SuiteStore = SeaormStore<DatabaseConnection>;

#[cfg(feature = "postgres")]
pub mod pg {
    use super::*;
    use testcontainers_modules::postgres;

    static PG_CONTAINER: OnceLock<ContainerAsync<postgres::Postgres>> = OnceLock::new();

    /// Shared `postgres:17` container for this test binary.
    pub async fn container() -> &'static ContainerAsync<postgres::Postgres> {
        if let Some(container) = PG_CONTAINER.get() {
            return container;
        }
        let container = postgres::Postgres::default()
            .with_tag("17")
            .start()
            .await
            .expect("Docker must be available for Postgres; postgres:17 image will be pulled");
        let _ = PG_CONTAINER.set(container);
        PG_CONTAINER.get().expect("container initialized")
    }

    /// Fresh `DatabaseConnection` against the shared container.
    pub async fn connect() -> DatabaseConnection {
        let container = container().await;
        let host_port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("Postgres port mapping");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
        Database::connect(&url).await.expect("connect to Postgres")
    }
}

#[cfg(feature = "mysql")]
#[allow(dead_code)] // dual-feature builds route `connect()` to Postgres; the mysql arm is compiled for the mysql-only and dual gates
mod mysql {
    use super::*;
    use testcontainers_modules::mysql;

    static MYSQL_CONTAINER: OnceLock<ContainerAsync<mysql::Mysql>> = OnceLock::new();

    /// Shared `mysql:8.4` container for this test binary.
    pub async fn container() -> &'static ContainerAsync<mysql::Mysql> {
        if let Some(container) = MYSQL_CONTAINER.get() {
            return container;
        }
        let container = mysql::Mysql::default()
            .with_tag("8.4")
            .start()
            .await
            .expect("Docker must be available for MySQL; mysql:8.4 image will be pulled");
        let _ = MYSQL_CONTAINER.set(container);
        MYSQL_CONTAINER.get().expect("container initialized")
    }

    /// Fresh `DatabaseConnection` against the shared container.
    pub async fn connect() -> DatabaseConnection {
        let container = container().await;
        let host_port = container
            .get_host_port_ipv4(3306)
            .await
            .expect("MySQL port mapping");
        let url = format!("mysql://root@127.0.0.1:{host_port}/test");
        Database::connect(&url).await.expect("connect to MySQL")
    }
}

#[cfg(feature = "postgres")]
async fn connect() -> DatabaseConnection {
    pg::connect().await
}

#[cfg(all(feature = "mysql", not(feature = "postgres")))]
async fn connect() -> DatabaseConnection {
    mysql::connect().await
}

#[cfg(feature = "postgres")]
fn fixture_ddl() -> &'static [&'static str] {
    rolify_test::ddl::POSTGRES
}

#[cfg(all(feature = "mysql", not(feature = "postgres")))]
fn fixture_ddl() -> &'static [&'static str] {
    rolify_test::ddl::MYSQL
}

/// Apply the Migrator plus the shared fixture DDL and canonical rows
/// (mirroring `data.rb`, identical across engines here).
async fn setup_schema(conn: &DatabaseConnection) {
    Migrator::up(conn, None).await.expect("Migrator up");
    for statement in fixture_ddl() {
        conn.execute_unprepared(statement)
            .await
            .expect("fixture DDL statement");
    }
    for statement in [
        "INSERT INTO users (id, rolify_type, name) VALUES (1, 'User', 'admin'), (2, 'User', 'moderator'), (3, 'User', 'god'), (4, 'User', 'zombie')",
        "INSERT INTO forums (id, name) VALUES (1, 'Forum 1'), (2, 'Forum 2'), (3, 'Forum 3')",
        if cfg!(feature = "mysql") && !cfg!(feature = "postgres") {
            // `groups` is a reserved word on MySQL 8+ (backtick-quoted).
            "INSERT INTO `groups` (id, name) VALUES (1, 'Group 1'), (2, 'Group 2')"
        } else {
            "INSERT INTO groups (id, name) VALUES (1, 'Group 1'), (2, 'Group 2')"
        },
        "INSERT INTO teams (team_code, name) VALUES ('1', 'Team 1'), ('2', 'Team 2')",
        "INSERT INTO organizations (id, type) VALUES (1, 'Organization')",
    ] {
        conn.execute_unprepared(statement)
            .await
            .expect("seed canonical fixture row");
    }
}

/// Delete all link and role rows (fixture tables are never truncated).
async fn reset_role_rows(conn: &DatabaseConnection) {
    conn.execute_unprepared("DELETE FROM users_roles")
        .await
        .expect("reset users_roles");
    conn.execute_unprepared("DELETE FROM roles")
        .await
        .expect("reset roles");
}

/// The suite subject: a holder identity over the single engine
/// (mirrors the diesel suite's local subject wrapper).
pub struct SeaormSubject<C: UserClass> {
    holder: ResourceId,
    engine: Rolify<SuiteStore>,
    _class: PhantomData<C>,
}

#[maybe_async::maybe_async(AFIT)]
impl<C: UserClass> RolifyUser for SeaormSubject<C> {
    type Store = SuiteStore;

    fn store(&mut self) -> &mut Self::Store {
        self.engine.store_with_conn().0
    }

    fn rolify_config(&self) -> &RolifyConfig {
        self.engine.config()
    }

    fn rolify_id(&self) -> ResourceId {
        self.holder.clone()
    }

    fn rolify_type() -> &'static str {
        C::rolify_type()
    }

    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut <Self::Store as RoleStore>::Conn) {
        self.engine.store_with_conn()
    }
}

/// `TestBackend` over the `SeaORM` store and a pooled connection.
pub struct SeaormBackend<C: UserClass> {
    subject: SeaormSubject<C>,
    resources: FixtureResources,
    /// Shared counter handle (`query_count` takes &self via this seam).
    counter: Arc<AtomicUsize>,
}

impl<C: UserClass> Sealed for SeaormBackend<C> {}

#[maybe_async::maybe_async(AFIT)]
impl<C: UserClass> TestBackend for SeaormBackend<C> {
    type Store = SuiteStore;
    type Subject = SeaormSubject<C>;
    type Error = Error;

    async fn build() -> Result<Self, Self::Error> {
        let resources = FixtureResources::new();
        let conn = connect().await;
        setup_schema(&conn).await;
        reset_role_rows(&conn).await;
        let config = C::config();
        let store = build_store(&config);
        let counter = store.query_counter_handle();
        let engine = Rolify::new(store, conn, config);
        let subject = SeaormSubject {
            holder: ResourceId::from(1_i64),
            engine,
            _class: PhantomData,
        };
        Ok(Self {
            subject,
            resources,
            counter,
        })
    }

    fn subject(&mut self, login: &str) -> &mut Self::Subject {
        let holder = self
            .holder_id(login)
            .expect("unknown fixture login: expected admin, moderator, god, or zombie");
        self.subject.holder = holder;
        &mut self.subject
    }

    fn holder_id(&self, login: &str) -> Option<ResourceId> {
        fixture_holders()
            .into_iter()
            .find(|(known, _)| *known == login)
            .map(|(_, id)| id)
    }

    fn resource(&self, which: FixtureResource) -> ResourceKey {
        self.resources.key(which)
    }

    async fn reset_roles(&mut self) -> Result<(), Self::Error> {
        reset_role_rows(self.subject.engine.store_with_conn().1).await;
        Ok(())
    }

    async fn create_role_row(&mut self, record: RoleRecord) -> Result<(), Self::Error> {
        let scope = match (record.resource_type.as_deref(), record.resource_id.as_ref()) {
            (None, _) => ResourceRef::Global,
            (Some(type_name), None) => ResourceRef::Class(type_name),
            (Some(type_name), Some(id)) => ResourceRef::Instance(type_name, id),
        };
        let (store, conn) = self.subject.engine.store_with_conn();
        store.find_or_create_by(conn, &record.name, scope).await?;
        Ok(())
    }

    /// `provision_user` primitive: `find_or_create_by` plus `add` for the
    /// holder identity.
    ///
    /// # Panics
    ///
    /// Panics on unknown logins.
    async fn grant_to(
        &mut self,
        login: &str,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> Result<(), Self::Error> {
        let holder = self
            .holder_id(login)
            .expect("unknown fixture login: expected admin, moderator, god, or zombie");
        let (store, conn) = self.subject.engine.store_with_conn();
        let role = store.find_or_create_by(conn, name, scope).await?;
        store.add(conn, &holder, &role).await?;
        Ok(())
    }

    async fn role_row_count(&mut self) -> Result<usize, Self::Error> {
        let conn = self.subject.engine.store_with_conn().1;
        let row = conn
            .query_one_raw(Statement::from_sql_and_values(
                conn.get_database_backend(),
                "SELECT COUNT(*) AS count FROM roles".to_owned(),
                Vec::new(),
            ))
            .await
            .map_err(Error::Db)?
            .expect("COUNT(*) always returns one row");
        let count: i64 = row.try_get("", "count").map_err(Error::Db)?;
        usize::try_from(count).map_err(|_| {
            Error::Core(rolify_core::error::RolifyError::InvalidConfig {
                reason: "role count negative".to_owned(),
            })
        })
    }

    fn reset_query_count(&mut self) {
        self.counter.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    fn query_count(&self) -> Option<usize> {
        Some(self.counter.load(std::sync::atomic::Ordering::Relaxed))
    }

    fn engine(&mut self) -> &mut Rolify<Self::Store> {
        &mut self.subject.engine
    }
}

/// Build the registered store (holder table plus resource registry,
/// mirrors the diesel suite backend wiring).
#[allow(clippy::missing_panics_doc)] // identifiers are validated upstream
fn build_store(config: &RolifyConfig) -> SuiteStore {
    SeaormStore::new(config)
        .for_holder_table("users")
        .register_resource_table("Forum", "forums", "id")
        .register_resource_table("Group", "groups", "id")
        .register_resource_table("Team", "teams", "team_code")
        .register_resource_table("Organization", "organizations", "id")
        .register_resource_table("Company", "organizations", "id")
        .register_resource_table("Right", "rights", "id")
}
