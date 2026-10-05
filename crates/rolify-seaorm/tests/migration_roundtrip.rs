//! Migration roundtrip proof for the native `sea-orm-migration` Migrator
//! (D-04): apply, constraint and index assertions, duplicate probes,
//! cascade sweep, revert, re-apply - on containerized Postgres 17 and
//! `MySQL` 8.4 (testcontainers 0.27.3 + modules 0.15.0, readiness waits,
//! never sleeps).
//!
//! Run:
//! - `cargo test -p rolify-seaorm --features postgres --test migration_roundtrip`
//! - `cargo test -p rolify-seaorm --features mysql --test migration_roundtrip`

#[cfg(feature = "postgres")]
mod postgres_roundtrip {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;
    use std::sync::OnceLock;
    use testcontainers_modules::postgres;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;
    use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

    static PG_CONTAINER: OnceLock<ContainerAsync<postgres::Postgres>> = OnceLock::new();

    async fn postgres_container() -> &'static ContainerAsync<postgres::Postgres> {
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

    async fn connect() -> sea_orm::DatabaseConnection {
        let container = postgres_container().await;
        let host_port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("Postgres port mapping");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
        Database::connect(&url).await.expect("connect to Postgres")
    }

    fn statement(sql: &str) -> Statement {
        Statement::from_sql_and_values(DbBackend::Postgres, sql.to_owned(), Vec::new())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // a single end-to-end leg mirrors the diesel migrations.rs roundtrip shape (04-07 posture)
    async fn migration_roundtrip_on_postgres() {
        let conn = connect().await;

        rolify_seaorm::Migrator::up(&conn, None)
            .await
            .expect("Migrator up applies cleanly on Postgres");

        let tables = conn
            .query_all_raw(statement(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')",
            ))
            .await
            .expect("query information_schema");
        assert_eq!(tables.len(), 2, "both roles and users_roles created");

        conn.execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .await
        .expect("first admin role insert");
        let dup_role = conn
            .execute_unprepared(
                "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
            )
            .await;
        assert!(
            dup_role.is_err(),
            "roles_triple_unique rejects duplicate global role"
        );

        conn.execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')",
        )
        .await
        .expect("user role insert");

        let role_ids = conn
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM roles WHERE name IN ('admin', 'user') ORDER BY id".to_owned(),
                Vec::new(),
            ))
            .await
            .expect("load role ids");
        assert_eq!(role_ids.len(), 2);
        let admin_id: i64 = role_ids[0].try_get_by("id").expect("id column");
        let user_role_id: i64 = role_ids[1].try_get_by("id").expect("id column");

        conn.execute_unprepared(&format!(
            "INSERT INTO users_roles (user_id, role_id) VALUES ('u1', {admin_id})"
        ))
        .await
        .expect("first link insert");
        let dup_link = conn
            .execute_unprepared(&format!(
                "INSERT INTO users_roles (user_id, role_id) VALUES ('u1', {admin_id})"
            ))
            .await;
        assert!(
            dup_link.is_err(),
            "users_roles_pair_unique rejects duplicate link"
        );

        conn.execute_unprepared(&format!("DELETE FROM roles WHERE id = {user_role_id}"))
            .await
            .expect("delete role row");
        let cascade_probe = conn
            .execute_unprepared(&format!("DELETE FROM roles WHERE id = {admin_id}"))
            .await;
        assert!(cascade_probe.is_ok());
        let remaining = conn
            .query_all_raw(statement("SELECT COUNT(*) AS count FROM users_roles"))
            .await
            .expect("count remaining links");
        let count: i64 = remaining[0].try_get_by("count").expect("count column");
        assert_eq!(count, 0, "FK ON DELETE CASCADE sweeps join rows");

        let constraints = conn
            .query_all_raw(statement(
                "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'roles' AND constraint_type = 'UNIQUE' AND constraint_name = 'roles_triple_unique'",
            ))
            .await
            .expect("query constraint name");
        assert_eq!(constraints.len(), 1, "roles_triple_unique exists");

        let indexes = conn
            .query_all_raw(statement(
                "SELECT indexname FROM pg_indexes WHERE tablename = 'roles' AND indexname IN ('idx_roles_resource', 'idx_roles_name')",
            ))
            .await
            .expect("query index names");
        assert_eq!(indexes.len(), 2, "both roles indexes exist");

        rolify_seaorm::Migrator::down(&conn, None)
            .await
            .expect("Migrator down reverts cleanly");
        let tables_after = conn
            .query_all_raw(statement(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')",
            ))
            .await
            .expect("query information_schema after revert");
        assert_eq!(tables_after.len(), 0, "revert drops both tables");

        rolify_seaorm::Migrator::up(&conn, None)
            .await
            .expect("re-apply after revert succeeds");
        let tables_final = conn
            .query_all_raw(statement(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')",
            ))
            .await
            .expect("query information_schema after re-apply");
        assert_eq!(tables_final.len(), 2, "re-apply recreates both tables");
    }
}

#[cfg(feature = "mysql")]
mod mysql_roundtrip {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;
    use std::sync::OnceLock;
    use testcontainers_modules::mysql;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;
    use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

    static MYSQL_CONTAINER: OnceLock<ContainerAsync<mysql::Mysql>> = OnceLock::new();

    async fn mysql_container() -> &'static ContainerAsync<mysql::Mysql> {
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

    async fn connect() -> sea_orm::DatabaseConnection {
        let container = mysql_container().await;
        let host_port = container
            .get_host_port_ipv4(3306)
            .await
            .expect("MySQL port mapping");
        let url = format!("mysql://root@127.0.0.1:{host_port}/test");
        Database::connect(&url).await.expect("connect to MySQL")
    }

    fn statement(sql: &str) -> Statement {
        Statement::from_sql_and_values(DbBackend::MySql, sql.to_owned(), Vec::new())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // a single end-to-end leg mirrors the diesel migrations.rs roundtrip shape (04-07 posture)
    async fn migration_roundtrip_on_mysql() {
        let conn = connect().await;

        rolify_seaorm::Migrator::up(&conn, None)
            .await
            .expect("Migrator up applies cleanly on MySQL");

        let tables = conn
            .query_all_raw(statement(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('roles', 'users_roles')",
            ))
            .await
            .expect("query information_schema");
        assert_eq!(tables.len(), 2, "both roles and users_roles created");

        conn.execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .await
        .expect("first admin role insert");
        let dup_role = conn
            .execute_unprepared(
                "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
            )
            .await;
        assert!(
            dup_role.is_err(),
            "roles_triple_unique rejects duplicate global role"
        );

        conn.execute_unprepared(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')",
        )
        .await
        .expect("user role insert");

        let role_ids = conn
            .query_all_raw(statement(
                "SELECT id FROM roles WHERE name IN ('admin', 'user') ORDER BY id",
            ))
            .await
            .expect("load role ids");
        assert_eq!(role_ids.len(), 2);
        let admin_id: i64 = role_ids[0].try_get_by("id").expect("id column");
        let _user_role_id: i64 = role_ids[1].try_get_by("id").expect("id column");

        conn.execute_unprepared(&format!(
            "INSERT INTO users_roles (user_id, role_id) VALUES ('u1', {admin_id})"
        ))
        .await
        .expect("first link insert");
        let dup_link = conn
            .execute_unprepared(&format!(
                "INSERT INTO users_roles (user_id, role_id) VALUES ('u1', {admin_id})"
            ))
            .await;
        assert!(
            dup_link.is_err(),
            "users_roles_pair_unique rejects duplicate link"
        );

        conn.execute_unprepared(&format!("DELETE FROM roles WHERE id = {admin_id}"))
            .await
            .expect("delete role row sweeps links");
        let remaining = conn
            .query_all_raw(statement("SELECT COUNT(*) AS count FROM users_roles"))
            .await
            .expect("count remaining links");
        // MySQL reports COUNT(*) via integer types compatible with i64.
        let count: i64 = remaining[0].try_get_by("count").expect("count column");
        assert_eq!(count, 0, "FK ON DELETE CASCADE sweeps join rows");

        let constraints = conn
            .query_all_raw(statement(
                "SELECT constraint_name FROM information_schema.table_constraints WHERE table_schema = DATABASE() AND table_name = 'roles' AND constraint_type = 'UNIQUE' AND constraint_name = 'roles_triple_unique'",
            ))
            .await
            .expect("query constraint name");
        assert_eq!(constraints.len(), 1, "roles_triple_unique exists");

        let indexes = conn
            .query_all_raw(statement(
                "SELECT DISTINCT index_name FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = 'roles' AND index_name IN ('idx_roles_resource', 'idx_roles_name')",
            ))
            .await
            .expect("query index names");
        assert_eq!(indexes.len(), 2, "both roles indexes exist");

        rolify_seaorm::Migrator::down(&conn, None)
            .await
            .expect("Migrator down reverts cleanly");
        let tables_after = conn
            .query_all_raw(statement(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('roles', 'users_roles')",
            ))
            .await
            .expect("query information_schema after revert");
        assert_eq!(tables_after.len(), 0, "revert drops both tables");

        rolify_seaorm::Migrator::up(&conn, None)
            .await
            .expect("re-apply after revert succeeds");
        let tables_final = conn
            .query_all_raw(statement(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('roles', 'users_roles')",
            ))
            .await
            .expect("query information_schema after re-apply");
        assert_eq!(tables_final.len(), 2, "re-apply recreates both tables");
    }
}
