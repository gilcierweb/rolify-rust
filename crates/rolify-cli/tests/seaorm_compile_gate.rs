//! Compile gate for the checked-in expected `SeaORM` migration (T-06-04, D-16).
//!
//! The expected file is compiled INTO this test binary via a `#[path]` module
//! so `sea-orm-migration` 2.0.4 typechecks it: a template edit that breaks the
//! `MigrationTrait` shape fails HERE, at compile time, instead of in a consumer
//! build. The trait-bound assertion below additionally proves the included
//! `Migration` struct actually implements `MigrationTrait`, and the drift
//! suite (tests/drift.rs) pins the rendered bytes to this exact file.
//!
//! The two roundtrip legs are the CR-03 pin: a `MySQL` 8.4 container leg and
//! a hermetic file-based `SQLite` leg drive the `MigratorTrait` harness over
//! the included expected migration up and down, exercising the non-Postgres
//! dialect match arms of the emitted migration on real engines. The legs run
//! the EXPECTED FILE rather than a runtime-generated one because generator
//! output for default names is byte-equal to it (the drift snapshot gate
//! proves that equality), and custom names are proven by the drift tests;
//! the expected file is a faithful stand-in for the generator output.

// The shared helpers module serves several test binaries; this one only
// needs `test_temp_dir`, so the binary-targeting helper stays unused here.
#[allow(dead_code)]
mod common;

#[path = "expected/seaorm_migration.rs"]
mod expected_migration;

use sea_orm_migration::prelude::*;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::mysql::Mysql;

/// The migrator harness over the included expected migration: the same
/// registration shape a consumer builds from the emitted file.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(expected_migration::Migration)]
    }
}

#[test]
fn expected_migration_implements_migration_trait() {
    fn assert_migration_trait<T: sea_orm_migration::MigrationTrait>() {}
    assert_migration_trait::<expected_migration::Migration>();
}

/// Starts the pinned `MySQL` 8.4 container with the testcontainers module
/// defaults (root user, no password, database "test"), matching the e2e
/// file's bootstrap. Readiness comes from the module, never sleeps.
async fn mysql_container() -> ContainerAsync<Mysql> {
    Mysql::default()
        .with_tag("8.4")
        .start()
        .await
        .expect("failed to start MySQL container")
}

/// Counts the rolify tables that exist on the connected database.
async fn count_rolify_tables_mysql(connection: &sea_orm::DatabaseConnection) -> usize {
    let table_rows = connection
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::MySql,
            "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('roles', 'users_roles')"
                .to_owned(),
            Vec::new(),
        ))
        .await
        .expect("query information_schema on MySQL");
    table_rows.len()
}

/// Runs the emitted migration up and down on a real `MySQL` 8.4 container
/// through the `SeaORM` migrator API (CR-03 pin): the `MySql` match arm of
/// `up()` applies the canonical auto-increment dialect, the unique triple
/// rejects a duplicate insert live, and `down()` reverts both tables.
#[allow(clippy::too_many_lines)] // a single linear up -> probe -> down leg
#[tokio::test]
async fn seaorm_mysql_migration_roundtrip() {
    use sea_orm::ConnectionTrait;

    let container = mysql_container().await;
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(3306).await.unwrap();
    let db_url = format!("mysql://root@{host}:{port}/test");

    let connection = sea_orm::Database::connect(&db_url)
        .await
        .expect("failed to connect to MySQL");

    Migrator::up(&connection, None)
        .await
        .expect("migrator up applies the emitted migration on MySQL");

    assert_eq!(
        count_rolify_tables_mysql(&connection).await,
        2,
        "both roles and users_roles must exist after up on MySQL"
    );

    connection
        .execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .await
        .expect("first admin role insert on MySQL");
    let duplicate_role = connection
        .execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .await;
    assert!(
        duplicate_role.is_err(),
        "roles_triple_unique must reject a duplicate triple on MySQL"
    );

    Migrator::down(&connection, None)
        .await
        .expect("migrator down reverts the emitted migration on MySQL");

    assert_eq!(
        count_rolify_tables_mysql(&connection).await,
        0,
        "both tables must be gone after down on MySQL"
    );
}

/// Reads the create-SQL of a table from `sqlite_master`, or an empty string
/// when the table does not exist.
async fn sqlite_table_sql(connection: &sea_orm::DatabaseConnection, table: &str) -> String {
    let table_rows = connection
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Sqlite,
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?".to_owned(),
            vec![table.into()],
        ))
        .await
        .expect("query sqlite_master");
    match table_rows.first() {
        Some(row) => row.try_get_by("sql").expect("sqlite_master sql column"),
        None => String::new(),
    }
}

/// Runs the emitted migration up and down on a hermetic file-based `SQLite`
/// database under the test workspace (zero Docker; CR-03 pin): the Sqlite
/// match arm of `up()` applies the canonical rowid-alias dialect, the unique
/// triple is present in the schema and rejects a duplicate insert live, and
/// `down()` reverts both tables.
#[allow(clippy::too_many_lines)] // a single linear up -> probe -> down leg
#[tokio::test]
async fn seaorm_sqlite_migration_roundtrip() {
    use sea_orm::ConnectionTrait;

    let workspace_dir = common::test_temp_dir();
    let db_path = workspace_dir.join("seaorm_sqlite_roundtrip.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let connection = sea_orm::Database::connect(&db_url)
        .await
        .expect("failed to connect to SQLite");

    Migrator::up(&connection, None)
        .await
        .expect("migrator up applies the emitted migration on SQLite");

    let roles_sql = sqlite_table_sql(&connection, "roles").await;
    assert!(
        !roles_sql.is_empty(),
        "roles table must exist after up on SQLite"
    );
    assert!(
        roles_sql.contains("roles_triple_unique"),
        "roles table definition must carry the unique triple on SQLite: {roles_sql}"
    );
    let join_sql = sqlite_table_sql(&connection, "users_roles").await;
    assert!(
        !join_sql.is_empty(),
        "users_roles table must exist after up on SQLite"
    );

    connection
        .execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .await
        .expect("first admin role insert on SQLite");
    let duplicate_role = connection
        .execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .await;
    assert!(
        duplicate_role.is_err(),
        "roles_triple_unique must reject a duplicate triple on SQLite"
    );

    Migrator::down(&connection, None)
        .await
        .expect("migrator down reverts the emitted migration on SQLite");

    assert!(
        sqlite_table_sql(&connection, "roles").await.is_empty(),
        "roles table must be gone after down on SQLite"
    );
    assert!(
        sqlite_table_sql(&connection, "users_roles")
            .await
            .is_empty(),
        "users_roles table must be gone after down on SQLite"
    );
}
