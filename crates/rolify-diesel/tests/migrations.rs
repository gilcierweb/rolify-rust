//! Embedded migrations apply/revert roundtrip test for all engines.
//!
//! This test verifies that the canonical migrations for all three engines
//! embed correctly and apply/revert cleanly on `SQLite` (the hermetic leg),
//! Postgres, and `MySQL` (the parity gate legs per D-14).

#[cfg(feature = "sqlite")]
use diesel::sqlite::SqliteConnection;

#[cfg(feature = "postgres")]
mod pg_migrations {
    use diesel::Connection;
    use diesel::RunQueryDsl;
    use diesel::deserialize::QueryableByName;
    use diesel::pg::PgConnection;
    use diesel::sql_types::Text;
    use diesel_migrations::MigrationHarness;
    use rolify_diesel::MIGRATIONS;
    use testcontainers::ImageExt;
    use testcontainers_modules::{postgres, testcontainers::runners::SyncRunner};

    #[derive(QueryableByName)]
    struct TableNameRow {
        #[diesel(sql_type = Text)]
        table_name: String,
    }

    #[derive(QueryableByName)]
    struct ConstraintNameRow {
        #[diesel(sql_type = Text)]
        constraint_name: String,
    }

    #[derive(QueryableByName)]
    struct IndexNameRow {
        #[diesel(sql_type = Text)]
        indexname: String,
    }

    #[derive(QueryableByName)]
    struct RoleIdRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        id: i64,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[derive(QueryableByName)]
    struct CollationRow {
        #[diesel(sql_type = Text)]
        collation_name: String,
    }

    fn pg_container() -> &'static testcontainers::Container<postgres::Postgres> {
        use std::sync::OnceLock;
        static CONTAINER: OnceLock<testcontainers::Container<postgres::Postgres>> = OnceLock::new();
        CONTAINER.get_or_init(|| {
            let container = postgres::Postgres::default()
                .with_tag("17")
                .start()
                .expect("Docker must be available for Postgres; postgres:17 image will be pulled");
            container
        })
    }

    fn pg_conn() -> PgConnection {
        let container = pg_container();
        let host_port = container
            .get_host_port_ipv4(5432)
            .expect("Postgres port mapping");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
        PgConnection::establish(&url).expect("Postgres connection")
    }

    #[test]
    fn migrations_apply_and_revert_cleanly_on_postgres() {
        let mut conn = pg_conn();

        // Apply migrations
        conn.run_pending_migrations(MIGRATIONS)
            .expect("migrations apply cleanly on Postgres");

        // Verify tables exist by querying information_schema
        let tables: Vec<TableNameRow> = diesel::sql_query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query information_schema");
        assert_eq!(
            tables.len(),
            2,
            "both roles and users_roles tables created on Postgres"
        );

        // Verify UNIQUE constraints by attempting duplicate inserts
        diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&mut conn)
        .expect("first admin role insert on Postgres");
        let dup_result = diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&mut conn);
        assert!(
            dup_result.is_err(),
            "UNIQUE triple constraint rejects duplicate global role on Postgres"
        );

        diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')",
        )
        .execute(&mut conn)
        .expect("user role insert on Postgres");

        let role_ids: Vec<RoleIdRow> =
            diesel::sql_query("SELECT id FROM roles WHERE name IN ('admin', 'user')")
                .load(&mut conn)
                .expect("load role ids on Postgres");
        assert_eq!(role_ids.len(), 2);

        let admin_id = role_ids[0].id;
        let user_id = role_ids[1].id;

        diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', $1)")
            .bind::<diesel::sql_types::BigInt, _>(admin_id)
            .execute(&mut conn)
            .expect("first link insert on Postgres");
        let dup_link =
            diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', $1)")
                .bind::<diesel::sql_types::BigInt, _>(admin_id)
                .execute(&mut conn);
        assert!(
            dup_link.is_err(),
            "UNIQUE pair constraint rejects duplicate link on Postgres"
        );

        diesel::sql_query("DELETE FROM roles WHERE id = $1")
            .bind::<diesel::sql_types::BigInt, _>(user_id)
            .execute(&mut conn)
            .expect("delete role on Postgres");
        let remaining_links: CountRow =
            diesel::sql_query("SELECT COUNT(*) AS count FROM users_roles WHERE role_id = $1")
                .bind::<diesel::sql_types::BigInt, _>(user_id)
                .get_result(&mut conn)
                .expect("count remaining links on Postgres");
        assert_eq!(
            remaining_links.count, 0,
            "FK ON DELETE CASCADE sweeps join rows on Postgres"
        );

        let constraint_name: Vec<ConstraintNameRow> = diesel::sql_query(
            "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'roles' AND constraint_type = 'UNIQUE' AND constraint_name = 'roles_triple_unique'"
        )
        .load(&mut conn)
        .expect("query constraint name");
        assert_eq!(
            constraint_name.len(),
            1,
            "UNIQUE constraint roles_triple_unique exists on Postgres"
        );

        let idx_resource: Vec<IndexNameRow> = diesel::sql_query(
            "SELECT indexname FROM pg_indexes WHERE tablename = 'roles' AND indexname = 'idx_roles_resource'"
        )
        .load(&mut conn)
        .expect("query index");
        assert_eq!(
            idx_resource.len(),
            1,
            "composite index idx_roles_resource exists on Postgres"
        );

        let idx_name: Vec<IndexNameRow> = diesel::sql_query(
            "SELECT indexname FROM pg_indexes WHERE tablename = 'roles' AND indexname = 'idx_roles_name'"
        )
        .load(&mut conn)
        .expect("query index");
        assert_eq!(idx_name.len(), 1, "index idx_roles_name exists on Postgres");

        conn.revert_all_migrations(MIGRATIONS)
            .expect("migrations revert cleanly on Postgres");

        let tables_after: Vec<TableNameRow> = diesel::sql_query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query information_schema after revert");
        assert_eq!(
            tables_after.len(),
            0,
            "tables dropped on revert on Postgres"
        );

        conn.run_pending_migrations(MIGRATIONS)
            .expect("migrations re-apply cleanly on Postgres");
        let tables_reapply: Vec<TableNameRow> = diesel::sql_query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query information_schema after re-apply");
        assert_eq!(
            tables_reapply.len(),
            2,
            "tables recreated on re-apply on Postgres"
        );
    }
}

#[cfg(feature = "mysql")]
mod mysql_migrations {
    use diesel::Connection;
    use diesel::RunQueryDsl;
    use diesel::deserialize::QueryableByName;
    use diesel::mysql::MysqlConnection;
    use diesel::sql_types::Text;
    use diesel_migrations::MigrationHarness;
    use rolify_diesel::MIGRATIONS;
    use testcontainers::ImageExt;
    use testcontainers_modules::{mysql, testcontainers::runners::SyncRunner};

    #[derive(QueryableByName)]
    struct TableNameRow {
        #[diesel(sql_type = Text)]
        table_name: String,
    }

    #[derive(QueryableByName)]
    struct ConstraintNameRow {
        #[diesel(sql_type = Text)]
        constraint_name: String,
    }

    #[derive(QueryableByName)]
    struct IndexNameRow {
        #[diesel(sql_type = Text)]
        index_name: String,
    }

    #[derive(QueryableByName)]
    struct RoleIdRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        id: i64,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[derive(QueryableByName)]
    struct CollationRow {
        #[diesel(sql_type = Text)]
        column_name: String,
        #[diesel(sql_type = Text)]
        collation_name: String,
    }

    fn mysql_container() -> &'static testcontainers::Container<mysql::Mysql> {
        use std::sync::OnceLock;
        static CONTAINER: OnceLock<testcontainers::Container<mysql::Mysql>> = OnceLock::new();
        CONTAINER.get_or_init(|| {
            let container = mysql::Mysql::default()
                .with_tag("8.4")
                .start()
                .expect("Docker must be available for MySQL; mysql:8.4 image will be pulled");
            container
        })
    }

    fn mysql_conn() -> MysqlConnection {
        let container = mysql_container();
        let host_port = container
            .get_host_port_ipv4(3306)
            .expect("MySQL port mapping");
        let url = format!("mysql://root@127.0.0.1:{host_port}/test");
        MysqlConnection::establish(&url).expect("MySQL connection")
    }

    #[test]
    fn migrations_apply_and_revert_cleanly_on_mysql() {
        let mut conn = mysql_conn();

        // Apply migrations
        conn.run_pending_migrations(MIGRATIONS)
            .expect("migrations apply cleanly on MySQL");

        // Verify tables exist by querying information_schema
        let tables: Vec<TableNameRow> = diesel::sql_query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'test' AND table_name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query information_schema");
        assert_eq!(
            tables.len(),
            2,
            "both roles and users_roles tables created on MySQL"
        );

        // Verify UNIQUE constraints by attempting duplicate inserts
        diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&mut conn)
        .expect("first admin role insert on MySQL");
        let dup_result = diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&mut conn);
        assert!(
            dup_result.is_err(),
            "UNIQUE triple constraint rejects duplicate global role on MySQL"
        );

        diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')",
        )
        .execute(&mut conn)
        .expect("user role insert on MySQL");

        let role_ids: Vec<RoleIdRow> =
            diesel::sql_query("SELECT id FROM roles WHERE name IN ('admin', 'user')")
                .load(&mut conn)
                .expect("load role ids on MySQL");
        assert_eq!(role_ids.len(), 2);

        let admin_id = role_ids[0].id;
        let user_id = role_ids[1].id;

        diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
            .bind::<diesel::sql_types::BigInt, _>(admin_id)
            .execute(&mut conn)
            .expect("first link insert on MySQL");
        let dup_link =
            diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
                .bind::<diesel::sql_types::BigInt, _>(admin_id)
                .execute(&mut conn);
        assert!(
            dup_link.is_err(),
            "UNIQUE pair constraint rejects duplicate link on MySQL"
        );

        diesel::sql_query("DELETE FROM roles WHERE id = ?")
            .bind::<diesel::sql_types::BigInt, _>(user_id)
            .execute(&mut conn)
            .expect("delete role on MySQL");
        let remaining_links: CountRow =
            diesel::sql_query("SELECT COUNT(*) AS count FROM users_roles WHERE role_id = ?")
                .bind::<diesel::sql_types::BigInt, _>(user_id)
                .get_result(&mut conn)
                .expect("count remaining links on MySQL");
        assert_eq!(
            remaining_links.count, 0,
            "FK ON DELETE CASCADE sweeps join rows on MySQL"
        );

        let collations: Vec<CollationRow> = diesel::sql_query(
            "SELECT column_name, collation_name FROM information_schema.columns WHERE table_schema = 'test' AND table_name = 'roles' AND column_name IN ('name', 'resource_type', 'resource_id')"
        )
        .load(&mut conn)
        .expect("query collation");
        for row in collations {
            assert!(
                row.collation_name.contains("utf8mb4_bin"),
                "column {} should have utf8mb4_bin collation on MySQL: {}",
                row.column_name,
                row.collation_name
            );
        }

        let constraint_name: Vec<ConstraintNameRow> = diesel::sql_query(
            "SELECT constraint_name FROM information_schema.table_constraints WHERE table_schema = 'test' AND table_name = 'roles' AND constraint_type = 'UNIQUE' AND constraint_name = 'roles_triple_unique'"
        )
        .load(&mut conn)
        .expect("query constraint name");
        assert_eq!(
            constraint_name.len(),
            1,
            "UNIQUE constraint roles_triple_unique exists on MySQL"
        );

        let idx_resource: Vec<IndexNameRow> = diesel::sql_query(
            "SELECT index_name FROM information_schema.statistics WHERE table_schema = 'test' AND table_name = 'roles' AND index_name = 'idx_roles_resource'"
        )
        .load(&mut conn)
        .expect("query index");
        assert_eq!(
            idx_resource.len(),
            1,
            "composite index idx_roles_resource exists on MySQL"
        );

        let idx_name: Vec<IndexNameRow> = diesel::sql_query(
            "SELECT index_name FROM information_schema.statistics WHERE table_schema = 'test' AND table_name = 'roles' AND index_name = 'idx_roles_name'"
        )
        .load(&mut conn)
        .expect("query index");
        assert_eq!(idx_name.len(), 1, "index idx_roles_name exists on MySQL");

        conn.revert_all_migrations(MIGRATIONS)
            .expect("migrations revert cleanly on MySQL");

        let tables_after: Vec<TableNameRow> = diesel::sql_query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'test' AND table_name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query information_schema after revert");
        assert_eq!(tables_after.len(), 0, "tables dropped on revert on MySQL");

        conn.run_pending_migrations(MIGRATIONS)
            .expect("migrations re-apply cleanly on MySQL");
        let tables_reapply: Vec<TableNameRow> = diesel::sql_query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'test' AND table_name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query information_schema after re-apply");
        assert_eq!(
            tables_reapply.len(),
            2,
            "tables recreated on re-apply on MySQL"
        );
    }
}

#[cfg(feature = "sqlite")]
mod sqlite_migrations {
    use diesel::Connection;
    use diesel::RunQueryDsl;
    use diesel::deserialize::QueryableByName;
    use diesel::sqlite::SqliteConnection;
    use diesel_migrations::MigrationHarness;
    use rolify_diesel::MIGRATIONS;

    // Diesel's SQLite path is untyped: bare `String`/`i64` targets do not
    // implement `QueryableByName`, so every load goes through a row struct
    // (the same shape as the Postgres/MySQL modules above).
    #[derive(QueryableByName)]
    struct SqliteNameRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        name: String,
    }

    #[derive(QueryableByName)]
    struct SqliteIdRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        id: i64,
    }

    #[derive(QueryableByName)]
    struct SqliteCountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[test]
    fn migrations_apply_and_revert_cleanly_on_sqlite() {
        let mut conn = SqliteConnection::establish(":memory:").expect("sqlite in-memory");
        // SQLite ships with FKs off; the cascade probe below needs them on.
        diesel::sql_query("PRAGMA foreign_keys = ON")
            .execute(&mut conn)
            .expect("PRAGMA foreign_keys = ON");

        // Apply migrations
        conn.run_pending_migrations(MIGRATIONS)
            .expect("migrations apply cleanly on SQLite");

        // Verify tables exist by querying sqlite_master
        let tables: Vec<SqliteNameRow> = diesel::sql_query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query sqlite_master");
        assert_eq!(
            tables.len(),
            2,
            "both roles and users_roles tables created on SQLite"
        );

        // Verify UNIQUE constraints by attempting duplicate inserts
        diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&mut conn)
        .expect("first admin role insert on SQLite");
        let dup_result = diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&mut conn);
        assert!(
            dup_result.is_err(),
            "UNIQUE triple constraint rejects duplicate global role on SQLite"
        );

        diesel::sql_query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')",
        )
        .execute(&mut conn)
        .expect("user role insert on SQLite");

        let role_ids: Vec<SqliteIdRow> =
            diesel::sql_query("SELECT id FROM roles WHERE name IN ('admin', 'user')")
                .load(&mut conn)
                .expect("load role ids on SQLite");
        assert_eq!(role_ids.len(), 2);

        let admin_id = role_ids[0].id;
        let user_id = role_ids[1].id;

        diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
            .bind::<diesel::sql_types::BigInt, _>(admin_id)
            .execute(&mut conn)
            .expect("first link insert on SQLite");
        let dup_link =
            diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
                .bind::<diesel::sql_types::BigInt, _>(admin_id)
                .execute(&mut conn);
        assert!(
            dup_link.is_err(),
            "UNIQUE pair constraint rejects duplicate link on SQLite"
        );

        // FK cascade: delete role -> join rows swept
        diesel::sql_query("DELETE FROM roles WHERE id = ?")
            .bind::<diesel::sql_types::BigInt, _>(user_id)
            .execute(&mut conn)
            .expect("delete role on SQLite");
        let remaining_links: SqliteCountRow =
            diesel::sql_query("SELECT COUNT(*) AS count FROM users_roles WHERE role_id = ?")
                .bind::<diesel::sql_types::BigInt, _>(user_id)
                .get_result(&mut conn)
                .expect("count remaining links on SQLite");
        assert_eq!(
            remaining_links.count, 0,
            "FK ON DELETE CASCADE sweeps join rows on SQLite"
        );

        // Revert migrations
        conn.revert_all_migrations(MIGRATIONS)
            .expect("migrations revert cleanly on SQLite");

        // Verify tables are gone
        let tables_after: Vec<SqliteNameRow> = diesel::sql_query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query sqlite_master after revert");
        assert_eq!(tables_after.len(), 0, "tables dropped on revert on SQLite");

        // Re-apply migrations (idempotency)
        conn.run_pending_migrations(MIGRATIONS)
            .expect("migrations re-apply cleanly on SQLite");
        let tables_reapply: Vec<SqliteNameRow> = diesel::sql_query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('roles', 'users_roles')"
        )
        .load(&mut conn)
        .expect("query sqlite_master after re-apply");
        assert_eq!(
            tables_reapply.len(),
            2,
            "tables recreated on re-apply on SQLite"
        );
    }
}
