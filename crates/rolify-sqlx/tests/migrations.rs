//! Embedded migrations apply/revert roundtrip test for all engines.
//!
//! Ports the acceptance shape from `rolify-diesel/tests/migrations.rs`
//! (apply -> verify structure -> unique-violation probes -> FK-cascade
//! probe -> revert -> re-apply idempotency), exercising the
//! `MIGRATIONS_<ENGINE>` exports end-to-end: the test IS the consumer that
//! invokes `run`/`undo` itself, because the crate never auto-migrates
//! (D-02).
//!
//! SQLite is the hermetic leg (in-memory pool, FKs forced on). Postgres
//! and MySQL run against the pinned testcontainers images (postgres:17 /
//! mysql:8.4, Phase 3 D-14) with the modules' built-in readiness waits:
//! never sleeps.

// The bootstrap ships helpers for the later sqlx test binaries (04-03+);
// this binary consumes the pool builders, so the appliers and reset
// helpers those binaries will use are allowed to sit unused here.
#[allow(dead_code)]
mod support;

/// Assert the error is the engine's unique-violation database error with
/// the expected SQLSTATE-like code, panicking with both values otherwise.
///
/// A1 closure: the SQLite leg asserts the extended result code `2067`
/// (`SQLITE_CONSTRAINT_UNIQUE`) here; if the engine reports a different
/// code, the assertion message surfaces it and the store's catch arm in
/// 04-03 must match the recorded value.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn assert_unique_violation(error: &sqlx::Error, expected_code: &str, constraint: &str) {
    let sqlx::Error::Database(database_error) = error else {
        panic!("expected a database unique violation on {constraint}, got: {error}");
    };
    assert_eq!(
        database_error.code().as_deref(),
        Some(expected_code),
        "unique-violation code for {constraint} differs (A1: record the actual code and adjust)",
    );
}

#[cfg(feature = "postgres")]
mod pg_migrations {
    use sqlx::Row;

    use rolify_sqlx::MIGRATIONS_POSTGRES;

    use crate::support::pg_pool;

    #[tokio::test]
    async fn migrations_apply_and_revert_cleanly_on_postgres() {
        let pool = pg_pool().await;

        // Apply migrations (consumer-invoked, D-02)
        MIGRATIONS_POSTGRES
            .run(&pool)
            .await
            .expect("migrations apply cleanly on Postgres");

        // Structural: both tables exist
        let table_count: i64 = sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query information_schema")
        .get(0);
        assert_eq!(
            table_count, 2,
            "both roles and users_roles tables created on Postgres"
        );

        // Structural: sentinel-typed scope columns are NOT NULL DEFAULT ''
        let sentinel_columns = sqlx::query(
            "SELECT column_name, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = 'roles' \
             AND column_name IN ('resource_type', 'resource_id')",
        )
        .fetch_all(&pool)
        .await
        .expect("query sentinel columns on Postgres");
        assert_eq!(
            sentinel_columns.len(),
            2,
            "roles carries both sentinel-typed scope columns on Postgres"
        );
        for column in &sentinel_columns {
            let column_name: String = column.get("column_name");
            let is_nullable: String = column.get("is_nullable");
            assert_eq!(
                is_nullable, "NO",
                "roles.{column_name} is NOT NULL on Postgres"
            );
            let column_default: Option<String> = column.get("column_default");
            let column_default = column_default.unwrap_or_else(|| {
                panic!("roles.{column_name} carries an explicit DEFAULT on Postgres")
            });
            assert!(
                column_default.contains("''"),
                "roles.{column_name} defaults to the empty-string sentinel, got {column_default}"
            );
        }

        // Structural: both UNIQUE constraints exist by name
        for (table_name, constraint_name) in [
            ("roles", "roles_triple_unique"),
            ("users_roles", "users_roles_pair_unique"),
        ] {
            let constraint_count: i64 = sqlx::query(
                "SELECT COUNT(*) FROM information_schema.table_constraints \
                 WHERE table_schema = 'public' AND table_name = $1 \
                 AND constraint_type = 'UNIQUE' AND constraint_name = $2",
            )
            .bind(table_name)
            .bind(constraint_name)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| {
                panic!("query constraint {constraint_name} on Postgres: {error}")
            })
            .get(0);
            assert_eq!(
                constraint_count, 1,
                "UNIQUE constraint {constraint_name} exists on Postgres"
            );
        }

        // Behavioral: UNIQUE triple rejects a duplicate global role (PG code 23505)
        sqlx::query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&pool)
        .await
        .expect("first admin role insert on Postgres");
        let duplicate_triple_error = sqlx::query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&pool)
        .await
        .expect_err("UNIQUE triple constraint rejects duplicate global role on Postgres");
        crate::assert_unique_violation(&duplicate_triple_error, "23505", "roles_triple_unique");

        let admin_role_id: i64 = sqlx::query("SELECT id FROM roles WHERE name = 'admin'")
            .fetch_one(&pool)
            .await
            .expect("load admin role id on Postgres")
            .get(0);
        sqlx::query("INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')")
            .execute(&pool)
            .await
            .expect("user role insert on Postgres");
        let user_role_id: i64 = sqlx::query("SELECT id FROM roles WHERE name = 'user'")
            .fetch_one(&pool)
            .await
            .expect("load user role id on Postgres")
            .get(0);

        // Behavioral: UNIQUE pair rejects a duplicate join row
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', $1)")
            .bind(admin_role_id)
            .execute(&pool)
            .await
            .expect("first link insert on Postgres");
        let duplicate_link_error =
            sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', $1)")
                .bind(admin_role_id)
                .execute(&pool)
                .await
                .expect_err("UNIQUE pair constraint rejects duplicate link on Postgres");
        crate::assert_unique_violation(&duplicate_link_error, "23505", "users_roles_pair_unique");

        // Give the user role a join row so the cascade probe is real
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u2', $1)")
            .bind(user_role_id)
            .execute(&pool)
            .await
            .expect("user link insert on Postgres");

        // Behavioral: FK ON DELETE CASCADE sweeps the deleted role's join rows only
        sqlx::query("DELETE FROM roles WHERE id = $1")
            .bind(user_role_id)
            .execute(&pool)
            .await
            .expect("delete role on Postgres");
        let swept_links: i64 = sqlx::query("SELECT COUNT(*) FROM users_roles WHERE role_id = $1")
            .bind(user_role_id)
            .fetch_one(&pool)
            .await
            .expect("count swept links on Postgres")
            .get(0);
        assert_eq!(
            swept_links, 0,
            "FK ON DELETE CASCADE sweeps the deleted role's join rows on Postgres"
        );
        let surviving_links: i64 =
            sqlx::query("SELECT COUNT(*) FROM users_roles WHERE role_id = $1")
                .bind(admin_role_id)
                .fetch_one(&pool)
                .await
                .expect("count surviving links on Postgres")
                .get(0);
        assert_eq!(
            surviving_links, 1,
            "FK cascade leaves other roles' join rows untouched on Postgres"
        );

        // Revert via Migrator undo to version 0, then re-apply
        MIGRATIONS_POSTGRES
            .undo(&pool, 0)
            .await
            .expect("migrations revert cleanly on Postgres");
        let tables_after_revert: i64 = sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query information_schema after revert")
        .get(0);
        assert_eq!(
            tables_after_revert, 0,
            "tables dropped on revert on Postgres"
        );

        MIGRATIONS_POSTGRES
            .run(&pool)
            .await
            .expect("migrations re-apply cleanly on Postgres");
        let tables_after_reapply: i64 = sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query information_schema after re-apply")
        .get(0);
        assert_eq!(
            tables_after_reapply, 2,
            "tables recreated on re-apply on Postgres"
        );
    }
}

#[cfg(feature = "mysql")]
mod mysql_migrations {
    use sqlx::Row;

    use rolify_sqlx::MIGRATIONS_MYSQL;

    use crate::support::mysql_pool;

    #[tokio::test]
    async fn migrations_apply_and_revert_cleanly_on_mysql() {
        let pool = mysql_pool().await;

        // Apply migrations (consumer-invoked, D-02)
        MIGRATIONS_MYSQL
            .run(&pool)
            .await
            .expect("migrations apply cleanly on MySQL");

        // Structural: both tables exist
        let table_count: i64 = sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'test' AND table_name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query information_schema")
        .get(0);
        assert_eq!(
            table_count, 2,
            "both roles and users_roles tables created on MySQL"
        );

        // Structural: sentinel-typed scope columns are NOT NULL DEFAULT ''.
        // The lowercase aliases are load-bearing: MySQL's prepared-statement
        // protocol returns information_schema fields under the data
        // dictionary's uppercase names (COLUMN_NAME), and sqlx row access
        // by name is case-sensitive, so the aliases restore the lowercase
        // names the assertions read.
        let sentinel_columns = sqlx::query(
            "SELECT column_name AS column_name, is_nullable AS is_nullable, \
             column_default AS column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'test' AND table_name = 'roles' \
             AND column_name IN ('resource_type', 'resource_id')",
        )
        .fetch_all(&pool)
        .await
        .expect("query sentinel columns on MySQL");
        assert_eq!(
            sentinel_columns.len(),
            2,
            "roles carries both sentinel-typed scope columns on MySQL"
        );
        for column in &sentinel_columns {
            let column_name: String = column.get("column_name");
            let is_nullable: String = column.get("is_nullable");
            assert_eq!(
                is_nullable, "NO",
                "roles.{column_name} is NOT NULL on MySQL"
            );
            let column_default: Option<String> = column.get("column_default");
            let column_default = column_default.unwrap_or_else(|| {
                panic!("roles.{column_name} carries an explicit DEFAULT on MySQL")
            });
            // MySQL's data dictionary renders DEFAULT '' as the empty-string
            // VALUE (not the SQL literal `''` the way Postgres does), so the
            // sentinel proof is: an explicit default exists and it is empty.
            assert!(
                column_default.is_empty(),
                "roles.{column_name} defaults to the empty-string sentinel on MySQL, got {column_default:?}"
            );
        }

        // Structural: both UNIQUE constraints exist by name
        for (table_name, constraint_name) in [
            ("roles", "roles_triple_unique"),
            ("users_roles", "users_roles_pair_unique"),
        ] {
            let constraint_count: i64 = sqlx::query(
                "SELECT COUNT(*) FROM information_schema.table_constraints \
                 WHERE table_schema = 'test' AND table_name = ? \
                 AND constraint_type = 'UNIQUE' AND constraint_name = ?",
            )
            .bind(table_name)
            .bind(constraint_name)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("query constraint {constraint_name} on MySQL: {error}"))
            .get(0);
            assert_eq!(
                constraint_count, 1,
                "UNIQUE constraint {constraint_name} exists on MySQL"
            );
        }

        // Behavioral: UNIQUE triple rejects a duplicate global role.
        // MySQL's unique-violation code: sqlx's DatabaseError::code() maps to
        // the SQLSTATE (23000), NOT the native error number 1062 the Phase 4
        // research assumed; the native number is only on the MySQL-specific
        // MySqlDatabaseError::number(). The 04-03 store catch arm must match
        // THIS recorded value (or use the portable ErrorKind::UniqueViolation).
        sqlx::query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&pool)
        .await
        .expect("first admin role insert on MySQL");
        let duplicate_triple_error = sqlx::query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&pool)
        .await
        .expect_err("UNIQUE triple constraint rejects duplicate global role on MySQL");
        crate::assert_unique_violation(&duplicate_triple_error, "23000", "roles_triple_unique");

        let admin_role_id: i64 = sqlx::query("SELECT id FROM roles WHERE name = 'admin'")
            .fetch_one(&pool)
            .await
            .expect("load admin role id on MySQL")
            .get(0);
        sqlx::query("INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')")
            .execute(&pool)
            .await
            .expect("user role insert on MySQL");
        let user_role_id: i64 = sqlx::query("SELECT id FROM roles WHERE name = 'user'")
            .fetch_one(&pool)
            .await
            .expect("load user role id on MySQL")
            .get(0);

        // Behavioral: UNIQUE pair rejects a duplicate join row
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
            .bind(admin_role_id)
            .execute(&pool)
            .await
            .expect("first link insert on MySQL");
        let duplicate_link_error =
            sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
                .bind(admin_role_id)
                .execute(&pool)
                .await
                .expect_err("UNIQUE pair constraint rejects duplicate link on MySQL");
        crate::assert_unique_violation(&duplicate_link_error, "23000", "users_roles_pair_unique");

        // Give the user role a join row so the cascade probe is real
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u2', ?)")
            .bind(user_role_id)
            .execute(&pool)
            .await
            .expect("user link insert on MySQL");

        // Behavioral: FK ON DELETE CASCADE sweeps the deleted role's join rows only
        sqlx::query("DELETE FROM roles WHERE id = ?")
            .bind(user_role_id)
            .execute(&pool)
            .await
            .expect("delete role on MySQL");
        let swept_links: i64 = sqlx::query("SELECT COUNT(*) FROM users_roles WHERE role_id = ?")
            .bind(user_role_id)
            .fetch_one(&pool)
            .await
            .expect("count swept links on MySQL")
            .get(0);
        assert_eq!(
            swept_links, 0,
            "FK ON DELETE CASCADE sweeps the deleted role's join rows on MySQL"
        );
        let surviving_links: i64 =
            sqlx::query("SELECT COUNT(*) FROM users_roles WHERE role_id = ?")
                .bind(admin_role_id)
                .fetch_one(&pool)
                .await
                .expect("count surviving links on MySQL")
                .get(0);
        assert_eq!(
            surviving_links, 1,
            "FK cascade leaves other roles' join rows untouched on MySQL"
        );

        // Revert via Migrator undo to version 0, then re-apply
        MIGRATIONS_MYSQL
            .undo(&pool, 0)
            .await
            .expect("migrations revert cleanly on MySQL");
        let tables_after_revert: i64 = sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'test' AND table_name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query information_schema after revert")
        .get(0);
        assert_eq!(tables_after_revert, 0, "tables dropped on revert on MySQL");

        MIGRATIONS_MYSQL
            .run(&pool)
            .await
            .expect("migrations re-apply cleanly on MySQL");
        let tables_after_reapply: i64 = sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'test' AND table_name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query information_schema after re-apply")
        .get(0);
        assert_eq!(
            tables_after_reapply, 2,
            "tables recreated on re-apply on MySQL"
        );
    }
}

#[cfg(feature = "sqlite")]
mod sqlite_migrations {
    use sqlx::Row;

    use rolify_sqlx::MIGRATIONS_SQLITE;

    use crate::support::sqlite_memory_pool;

    #[tokio::test]
    async fn migrations_apply_and_revert_cleanly_on_sqlite() {
        let pool = sqlite_memory_pool().await;

        // Apply migrations (consumer-invoked, D-02)
        MIGRATIONS_SQLITE
            .run(&pool)
            .await
            .expect("migrations apply cleanly on SQLite");

        // Structural: both tables exist
        let table_count: i64 = sqlx::query(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query sqlite_master")
        .get(0);
        assert_eq!(
            table_count, 2,
            "both roles and users_roles tables created on SQLite"
        );

        // Structural: sentinel-typed scope columns are NOT NULL DEFAULT ''
        let table_info = sqlx::query("PRAGMA table_info(roles)")
            .fetch_all(&pool)
            .await
            .expect("pragma table_info(roles)");
        let sentinel_columns: Vec<_> = table_info
            .iter()
            .filter(|column| {
                let column_name: String = column.get("name");
                column_name == "resource_type" || column_name == "resource_id"
            })
            .collect();
        assert_eq!(
            sentinel_columns.len(),
            2,
            "roles carries both sentinel-typed scope columns on SQLite"
        );
        for column in &sentinel_columns {
            let column_name: String = column.get("name");
            let not_null: i64 = column.get("notnull");
            assert_eq!(not_null, 1, "roles.{column_name} is NOT NULL on SQLite");
            let default_value: Option<String> = column.get("dflt_value");
            let default_value = default_value.unwrap_or_else(|| {
                panic!("roles.{column_name} carries an explicit DEFAULT on SQLite")
            });
            assert_eq!(
                default_value, "''",
                "roles.{column_name} defaults to the empty-string sentinel on SQLite"
            );
        }

        // Structural: both UNIQUE constraints exist by name (SQLite keeps
        // constraint names only in the stored DDL text)
        for (table_name, constraint_name) in [
            ("roles", "roles_triple_unique"),
            ("users_roles", "users_roles_pair_unique"),
        ] {
            let create_table_sql: String =
                sqlx::query("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?")
                    .bind(table_name)
                    .fetch_one(&pool)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("query sqlite_master for {table_name} on SQLite: {error}")
                    })
                    .get(0);
            assert!(
                create_table_sql.contains(constraint_name),
                "UNIQUE constraint {constraint_name} exists on SQLite: {create_table_sql}"
            );
        }

        // Behavioral: UNIQUE triple rejects a duplicate global role
        // (A1: SQLite's extended result code is 2067, SQLITE_CONSTRAINT_UNIQUE)
        sqlx::query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&pool)
        .await
        .expect("first admin role insert on SQLite");
        let duplicate_triple_error = sqlx::query(
            "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
        )
        .execute(&pool)
        .await
        .expect_err("UNIQUE triple constraint rejects duplicate global role on SQLite");
        crate::assert_unique_violation(&duplicate_triple_error, "2067", "roles_triple_unique");

        let admin_role_id: i64 = sqlx::query("SELECT id FROM roles WHERE name = 'admin'")
            .fetch_one(&pool)
            .await
            .expect("load admin role id on SQLite")
            .get(0);
        sqlx::query("INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')")
            .execute(&pool)
            .await
            .expect("user role insert on SQLite");
        let user_role_id: i64 = sqlx::query("SELECT id FROM roles WHERE name = 'user'")
            .fetch_one(&pool)
            .await
            .expect("load user role id on SQLite")
            .get(0);

        // Behavioral: UNIQUE pair rejects a duplicate join row
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
            .bind(admin_role_id)
            .execute(&pool)
            .await
            .expect("first link insert on SQLite");
        let duplicate_link_error =
            sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
                .bind(admin_role_id)
                .execute(&pool)
                .await
                .expect_err("UNIQUE pair constraint rejects duplicate link on SQLite");
        crate::assert_unique_violation(&duplicate_link_error, "2067", "users_roles_pair_unique");

        // Give the user role a join row so the cascade probe is real
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('u2', ?)")
            .bind(user_role_id)
            .execute(&pool)
            .await
            .expect("user link insert on SQLite");

        // Behavioral: FK ON DELETE CASCADE sweeps the deleted role's join rows only
        // (FKs are enforced because the pool forces the pragma on, Pitfall 14)
        sqlx::query("DELETE FROM roles WHERE id = ?")
            .bind(user_role_id)
            .execute(&pool)
            .await
            .expect("delete role on SQLite");
        let swept_links: i64 = sqlx::query("SELECT COUNT(*) FROM users_roles WHERE role_id = ?")
            .bind(user_role_id)
            .fetch_one(&pool)
            .await
            .expect("count swept links on SQLite")
            .get(0);
        assert_eq!(
            swept_links, 0,
            "FK ON DELETE CASCADE sweeps the deleted role's join rows on SQLite"
        );
        let surviving_links: i64 =
            sqlx::query("SELECT COUNT(*) FROM users_roles WHERE role_id = ?")
                .bind(admin_role_id)
                .fetch_one(&pool)
                .await
                .expect("count surviving links on SQLite")
                .get(0);
        assert_eq!(
            surviving_links, 1,
            "FK cascade leaves other roles' join rows untouched on SQLite"
        );

        // Revert via Migrator undo to version 0, then re-apply
        MIGRATIONS_SQLITE
            .undo(&pool, 0)
            .await
            .expect("migrations revert cleanly on SQLite");
        let tables_after_revert: i64 = sqlx::query(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query sqlite_master after revert")
        .get(0);
        assert_eq!(tables_after_revert, 0, "tables dropped on revert on SQLite");

        MIGRATIONS_SQLITE
            .run(&pool)
            .await
            .expect("migrations re-apply cleanly on SQLite");
        let tables_after_reapply: i64 = sqlx::query(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name IN ('roles', 'users_roles')",
        )
        .fetch_one(&pool)
        .await
        .expect("query sqlite_master after re-apply")
        .get(0);
        assert_eq!(
            tables_after_reapply, 2,
            "tables recreated on re-apply on SQLite"
        );
    }
}
