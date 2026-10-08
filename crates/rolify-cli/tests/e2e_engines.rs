mod common;

use common::{rolify_cli, test_temp_dir};
use sqlx::{AssertSqlSafe, MySqlPool, PgPool, SqlitePool};
use std::fs;
use testcontainers::ImageExt;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mysql::Mysql;
use testcontainers_modules::postgres::Postgres;

/// Splits a SQL file into individual statements, skipping comment lines.
fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in sql.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("--") {
            continue;
        }

        current.push_str(line);
        current.push('\n');

        if trimmed.ends_with(';') {
            statements.push(current.trim().to_string());
            current.clear();
        }
    }

    statements
}

/// Executes multiple SQL statements on a Postgres pool.
async fn execute_sql_file(pool: &PgPool, sql: &str) -> Result<(), sqlx::Error> {
    for statement in split_sql_statements(sql) {
        sqlx::query(AssertSqlSafe(statement.as_str()))
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Executes multiple SQL statements on a `MySQL` pool.
async fn execute_mysql_sql_file(pool: &MySqlPool, sql: &str) -> Result<(), sqlx::Error> {
    for statement in split_sql_statements(sql) {
        sqlx::query(AssertSqlSafe(statement.as_str()))
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Executes multiple SQL statements on a `SQLite` pool.
async fn execute_sqlite_sql_file(pool: &SqlitePool, sql: &str) -> Result<(), sqlx::Error> {
    for statement in split_sql_statements(sql) {
        sqlx::query(AssertSqlSafe(statement.as_str()))
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Runs the Postgres e2e test: generate schema, apply to real Postgres 17, verify with `DieselStore` smoke.
#[allow(clippy::too_many_lines)] // generate → apply → seed → smoke is a single linear leg
#[tokio::test]
async fn generated_postgres_schema_applies_and_holds_store_smoke() {
    use diesel::Connection;
    use rolify_core::config::RolifyConfig;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName};
    use rolify_core::store::RoleStore;
    use rolify_diesel::DieselStore;

    // Start Postgres 17 container
    let postgres = Postgres::default()
        .with_db_name("rolify_test")
        .with_user("postgres")
        .with_password("postgres")
        .with_tag("17")
        .start()
        .await
        .expect("failed to start Postgres container");

    let host = postgres.get_host().await.unwrap();
    let port = postgres.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@{host}:{port}/rolify_test");

    // Generate postgres migrations
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Read the generated up.sql
    let up_sql =
        fs::read_to_string(dir.join("migrations/postgres/0000000001_rolify_create_tables/up.sql"))
            .unwrap();

    // Apply the schema to the real Postgres container
    let pool = PgPool::connect(&db_url)
        .await
        .expect("failed to connect to Postgres");

    // Execute the up.sql (multiple statements)
    execute_sql_file(&pool, &up_sql)
        .await
        .expect("failed to apply generated up.sql");

    // Verify tables exist with correct structure using information_schema
    let roles_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let users_roles_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'users_roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(roles_count, 1, "roles table should exist");
    assert_eq!(users_roles_count, 1, "users_roles table should exist");

    // Verify constraints
    let constraints: Vec<String> = sqlx::query_scalar(
        "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'roles'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(
        constraints
            .iter()
            .any(|constraint| constraint == "roles_triple_unique"),
        "roles_triple_unique missing"
    );

    let join_constraints: Vec<String> = sqlx::query_scalar(
        "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'users_roles'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(
        join_constraints
            .iter()
            .any(|constraint| constraint == "users_roles_pair_unique"),
        "users_roles_pair_unique missing"
    );

    // Test duplicate admin insert errors on roles_triple_unique
    sqlx::query("INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')")
        .execute(&pool)
        .await
        .expect("first admin insert should succeed");

    let duplicate_result = sqlx::query(
        "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
    )
    .execute(&pool)
    .await;
    assert!(
        duplicate_result.is_err(),
        "duplicate admin should fail on roles_triple_unique"
    );

    // Test join duplicate errors on users_roles_pair_unique
    let role_id: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE name = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('user1', $1)")
        .bind(role_id)
        .execute(&pool)
        .await
        .expect("first join insert should succeed");

    let duplicate_join =
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('user1', $1)")
            .bind(role_id)
            .execute(&pool)
            .await;
    assert!(
        duplicate_join.is_err(),
        "duplicate join should fail on users_roles_pair_unique"
    );

    // Test cascade delete: deleting a role row cascades join rows to zero
    sqlx::query("DELETE FROM roles WHERE name = 'admin'")
        .execute(&pool)
        .await
        .expect("delete role should succeed");

    let join_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users_roles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(join_count, 0, "join rows should cascade to zero");

    // Verify down.sql reverts both tables
    let down_sql = fs::read_to_string(
        dir.join("migrations/postgres/0000000001_rolify_create_tables/down.sql"),
    )
    .unwrap();

    execute_sql_file(&pool, &down_sql)
        .await
        .expect("failed to apply down.sql");

    let roles_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let users_roles_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'users_roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(roles_after_down, 0, "roles table should be dropped");
    assert_eq!(
        users_roles_after_down, 0,
        "users_roles table should be dropped"
    );

    // DieselStore reference-adapter leg (D-21): the rolify-diesel adapter
    // drives add/has/remove through its real SPI against the schema this
    // generator produced, on the same container. This proves the generated
    // schema is interchangeable with the canonical one for the reference
    // adapter, not just for raw SQL.
    execute_sql_file(&pool, &up_sql)
        .await
        .expect("failed to re-apply up.sql for store smoke");

    let mut diesel_conn = diesel::pg::PgConnection::establish(&db_url)
        .expect("diesel connection to the CLI-generated schema");

    let config = RolifyConfig::builder()
        .build()
        .expect("default config builds");
    let mut store = DieselStore::new(&config);
    let holder_id = ResourceId::from("user1");

    // add_role :admin (gem parity: user.add_role "admin", global scope)
    let admin_role = store
        .find_or_create_by(
            &mut diesel_conn,
            &RoleName::from("admin"),
            ResourceRef::Global,
        )
        .expect("find_or_create_by admin global on generated schema");
    let added = store
        .add(&mut diesel_conn, &holder_id, &admin_role)
        .expect("add admin to user1 on generated schema");
    assert!(added, "first add creates the link");

    // has_role? :admin => true (gem parity: where_ ladder, Global filter)
    let roles = store
        .where_(
            &mut diesel_conn,
            &holder_id,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            },
        )
        .expect("where_ admin global on generated schema");
    assert_eq!(
        roles.len(),
        1,
        "user1 should hold exactly the admin role on the generated schema"
    );

    // remove_role :admin (gem parity: NameOnly sweep, remove_role_if_empty true)
    store
        .remove(
            &mut diesel_conn,
            &holder_id,
            &RoleName::from("admin"),
            RemovalTarget::NameOnly,
            true,
        )
        .expect("remove admin from user1 on generated schema");

    // has_role? :admin => false after removal
    let roles_after = store
        .where_(
            &mut diesel_conn,
            &holder_id,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            },
        )
        .expect("where_ after removal on generated schema");
    assert!(
        roles_after.is_empty(),
        "user1 should hold no roles after removal on the generated schema"
    );

    println!("Postgres e2e test passed: generated schema applies and holds store smoke");
}

/// Runs the `MySQL` e2e test: generate schema, apply to real `MySQL` 8.4, verify roundtrip.
#[allow(clippy::too_many_lines)] // linear generate → apply → smoke e2e leg
#[tokio::test]
async fn generated_mysql_schema_roundtrip() {
    // Start MySQL 8.4 container (testcontainers-modules defaults: root user,
    // no password, database "test", matching the documented example URL)
    let mysql = Mysql::default()
        .with_tag("8.4")
        .start()
        .await
        .expect("failed to start MySQL container");

    let host = mysql.get_host().await.unwrap();
    let port = mysql.get_host_port_ipv4(3306).await.unwrap();
    let db_url = format!("mysql://root@{host}:{port}/test");

    // Generate mysql migrations
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Read the generated up.sql
    let up_sql =
        fs::read_to_string(dir.join("migrations/mysql/0000000001_rolify_create_tables/up.sql"))
            .unwrap();

    // Apply the schema to the real MySQL container
    let pool = MySqlPool::connect(&db_url)
        .await
        .expect("failed to connect to MySQL");

    // Execute the up.sql (multiple statements)
    execute_mysql_sql_file(&pool, &up_sql)
        .await
        .expect("failed to apply generated up.sql");

    // Verify tables exist with correct structure using information_schema
    let roles_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'roles' AND table_schema = 'test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let users_roles_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'users_roles' AND table_schema = 'test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(roles_count, 1, "roles table should exist");
    assert_eq!(users_roles_count, 1, "users_roles table should exist");

    // Verify constraints
    let constraints: Vec<String> = sqlx::query_scalar(
        "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'roles' AND table_schema = 'test'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(
        constraints
            .iter()
            .any(|constraint| constraint == "roles_triple_unique"),
        "roles_triple_unique missing"
    );

    let join_constraints: Vec<String> = sqlx::query_scalar(
        "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'users_roles' AND table_schema = 'test'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(
        join_constraints
            .iter()
            .any(|constraint| constraint == "users_roles_pair_unique"),
        "users_roles_pair_unique missing"
    );

    // Verify utf8mb4 collation (engine-specific DDL from canonical mysql)
    let collation: String = sqlx::query_scalar(
        "SELECT table_collation FROM information_schema.tables WHERE table_name = 'roles' AND table_schema = 'test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        collation.contains("utf8mb4"),
        "roles table should have utf8mb4 collation, got: {collation}"
    );

    // Test duplicate admin insert errors on roles_triple_unique
    sqlx::query("INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')")
        .execute(&pool)
        .await
        .expect("first admin insert should succeed");

    let duplicate_result = sqlx::query(
        "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
    )
    .execute(&pool)
    .await;
    assert!(
        duplicate_result.is_err(),
        "duplicate admin should fail on roles_triple_unique"
    );

    // Test join duplicate errors on users_roles_pair_unique
    let role_id: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE name = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('user1', ?)")
        .bind(role_id)
        .execute(&pool)
        .await
        .expect("first join insert should succeed");

    let duplicate_join =
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('user1', ?)")
            .bind(role_id)
            .execute(&pool)
            .await;
    assert!(
        duplicate_join.is_err(),
        "duplicate join should fail on users_roles_pair_unique"
    );

    // Test cascade delete: deleting a role row cascades join rows to zero
    sqlx::query("DELETE FROM roles WHERE name = 'admin'")
        .execute(&pool)
        .await
        .expect("delete role should succeed");

    let join_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users_roles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(join_count, 0, "join rows should cascade to zero");

    // Verify down.sql reverts both tables
    let down_sql =
        fs::read_to_string(dir.join("migrations/mysql/0000000001_rolify_create_tables/down.sql"))
            .unwrap();

    execute_mysql_sql_file(&pool, &down_sql)
        .await
        .expect("failed to apply down.sql");

    let roles_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'roles' AND table_schema = 'test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let users_roles_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'users_roles' AND table_schema = 'test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(roles_after_down, 0, "roles table should be dropped");
    assert_eq!(
        users_roles_after_down, 0,
        "users_roles table should be dropped"
    );

    // Re-apply up.sql and verify it works again
    execute_mysql_sql_file(&pool, &up_sql)
        .await
        .expect("failed to re-apply up.sql");

    let roles_after_reapply: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'roles' AND table_schema = 'test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        roles_after_reapply, 1,
        "roles table should exist after re-apply"
    );

    println!("MySQL e2e test passed: generated schema roundtrip verified");
}

/// Runs the `SQLite` e2e test: generate schema, apply to hermetic `SQLite`, verify roundtrip.
/// `SQLite` is file-based (no container needed, zero Docker required).
#[allow(clippy::too_many_lines)] // linear generate → apply → smoke e2e leg
#[tokio::test]
async fn generated_sqlite_schema_roundtrip() {
    // Generate sqlite migrations
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Read the generated up.sql
    let up_sql =
        fs::read_to_string(dir.join("migrations/sqlite/0000000001_rolify_create_tables/up.sql"))
            .unwrap();

    // Use a file-based SQLite database (hermetic, no Docker)
    let db_path = dir.join("test.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let pool = SqlitePool::connect(&db_url)
        .await
        .expect("failed to connect to SQLite");

    // Enable foreign keys for SQLite (PRAGMA foreign_keys = ON per engine contract)
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("failed to enable foreign keys");

    // Execute the up.sql (multiple statements)
    execute_sqlite_sql_file(&pool, &up_sql)
        .await
        .expect("failed to apply generated up.sql");

    // Verify tables exist with correct structure using sqlite_master
    let roles_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let users_roles_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'users_roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(roles_count, 1, "roles table should exist");
    assert_eq!(users_roles_count, 1, "users_roles table should exist");

    // Verify unique triple constraint via sqlite_master (table definition carries it)
    let roles_table_sql: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'roles'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        roles_table_sql.contains("roles_triple_unique"),
        "roles_triple_unique missing in table definition: {roles_table_sql}"
    );

    // Verify indexes
    let roles_indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'roles'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let indexes_str = roles_indexes.join(", ");
    assert!(
        indexes_str.contains("idx_roles_resource"),
        "idx_roles_resource missing: {indexes_str}"
    );
    assert!(
        indexes_str.contains("idx_roles_name"),
        "idx_roles_name missing: {indexes_str}"
    );

    // Verify join table pair unique
    let join_table_sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'users_roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        join_table_sql.contains("users_roles_pair_unique"),
        "users_roles_pair_unique missing in join table definition: {join_table_sql}"
    );

    // Verify INTEGER PRIMARY KEY rowid shape: insert without explicit id, read back the generated key
    let inserted_id: i64 = sqlx::query_scalar(
        "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        inserted_id > 0,
        "id should be auto-assigned for INTEGER PRIMARY KEY"
    );

    // Test duplicate admin insert errors on roles_triple_unique
    let duplicate_result = sqlx::query(
        "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')",
    )
    .execute(&pool)
    .await;
    assert!(
        duplicate_result.is_err(),
        "duplicate admin should fail on roles_triple_unique"
    );

    // Test join duplicate errors on users_roles_pair_unique
    let role_id: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE name = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('user1', ?)")
        .bind(role_id)
        .execute(&pool)
        .await
        .expect("first join insert should succeed");

    let duplicate_join =
        sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('user1', ?)")
            .bind(role_id)
            .execute(&pool)
            .await;
    assert!(
        duplicate_join.is_err(),
        "duplicate join should fail on users_roles_pair_unique"
    );

    // Test cascade delete: deleting a role row cascades join rows to zero
    sqlx::query("DELETE FROM roles WHERE name = 'admin'")
        .execute(&pool)
        .await
        .expect("delete role should succeed");

    let join_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users_roles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(join_count, 0, "join rows should cascade to zero");

    // Verify down.sql reverts both tables
    let down_sql =
        fs::read_to_string(dir.join("migrations/sqlite/0000000001_rolify_create_tables/down.sql"))
            .unwrap();

    execute_sqlite_sql_file(&pool, &down_sql)
        .await
        .expect("failed to apply down.sql");

    let roles_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let users_roles_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'users_roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(roles_after_down, 0, "roles table should be dropped");
    assert_eq!(
        users_roles_after_down, 0,
        "users_roles table should be dropped"
    );

    // Re-apply up.sql and verify it works again
    execute_sqlite_sql_file(&pool, &up_sql)
        .await
        .expect("failed to re-apply up.sql");

    let roles_after_reapply: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'roles'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        roles_after_reapply, 1,
        "roles table should exist after re-apply"
    );

    println!("SQLite e2e test passed: generated schema roundtrip verified");
}

/// Tests that the custom names matrix works with the generated schema on Postgres.
#[allow(clippy::too_many_lines)] // linear generate → apply → smoke e2e leg
#[tokio::test]
async fn generated_custom_names_schema_applies() {
    // Start Postgres 17 container
    let postgres = Postgres::default()
        .with_db_name("rolify_test")
        .with_user("postgres")
        .with_password("postgres")
        .with_tag("17")
        .start()
        .await
        .expect("failed to start Postgres container");

    let host = postgres.get_host().await.unwrap();
    let port = postgres.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@{host}:{port}/rolify_test");

    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    // Generate with custom names
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Privilege",
            "Customer",
            "--roles-table",
            "privileges",
            "--join-table",
            "customers_privileges",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Verify the generated postgres up.sql has the custom names
    let up_sql =
        fs::read_to_string(dir.join("migrations/postgres/0000000001_rolify_create_tables/up.sql"))
            .unwrap();

    assert!(
        up_sql.contains("privileges_triple_unique"),
        "missing renamed unique constraint"
    );
    assert!(
        up_sql.contains("idx_privileges_resource"),
        "missing renamed resource index"
    );
    assert!(
        up_sql.contains("customers_privileges_pair_unique"),
        "missing renamed join constraint"
    );
    assert!(
        !up_sql.contains("users_roles"),
        "should not contain default join table name"
    );

    // Apply to Postgres and verify
    let pool = PgPool::connect(&db_url)
        .await
        .expect("failed to connect to Postgres");

    execute_sql_file(&pool, &up_sql)
        .await
        .expect("failed to apply custom names up.sql");

    // Verify tables exist with custom names
    let privileges_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'privileges'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let customers_privileges_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'customers_privileges'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(privileges_count, 1, "privileges table should exist");
    assert_eq!(
        customers_privileges_count, 1,
        "customers_privileges table should exist"
    );

    // Verify renamed constraint live: duplicate privilege insert errors on privileges_triple_unique
    sqlx::query(
        "INSERT INTO privileges (name, resource_type, resource_id) VALUES ('admin', '', '')",
    )
    .execute(&pool)
    .await
    .expect("first privilege insert should succeed");

    let duplicate_privilege = sqlx::query(
        "INSERT INTO privileges (name, resource_type, resource_id) VALUES ('admin', '', '')",
    )
    .execute(&pool)
    .await;
    assert!(
        duplicate_privilege.is_err(),
        "duplicate privilege should fail on privileges_triple_unique"
    );

    // Verify down drops customers_privileges first (renamed join first)
    let down_sql = fs::read_to_string(
        dir.join("migrations/postgres/0000000001_rolify_create_tables/down.sql"),
    )
    .unwrap();

    assert!(
        down_sql
            .find("DROP TABLE IF EXISTS customers_privileges")
            .unwrap_or(usize::MAX)
            < down_sql
                .find("DROP TABLE IF EXISTS privileges")
                .unwrap_or(usize::MAX),
        "down.sql should drop customers_privileges before privileges"
    );

    execute_sql_file(&pool, &down_sql)
        .await
        .expect("failed to apply custom names down.sql");

    let privileges_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'privileges'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        privileges_after_down, 0,
        "privileges table should be dropped"
    );

    println!("Custom names Postgres e2e test passed");
}

/// Tests custom names live on `SQLite` too (hermetic, no Docker).
#[allow(clippy::too_many_lines)] // linear generate → apply → smoke e2e leg
#[tokio::test]
async fn generated_custom_names_sqlite_applies() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    // Generate with custom names
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Privilege",
            "Customer",
            "--roles-table",
            "privileges",
            "--join-table",
            "customers_privileges",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Read the generated sqlite up.sql
    let up_sql =
        fs::read_to_string(dir.join("migrations/sqlite/0000000001_rolify_create_tables/up.sql"))
            .unwrap();

    // Verify custom names in sqlite up.sql
    assert!(
        up_sql.contains("privileges_triple_unique"),
        "missing renamed unique constraint in sqlite"
    );
    assert!(
        up_sql.contains("idx_privileges_resource"),
        "missing renamed resource index in sqlite"
    );
    assert!(
        up_sql.contains("customers_privileges_pair_unique"),
        "missing renamed join constraint in sqlite"
    );
    assert!(
        !up_sql.contains("users_roles"),
        "should not contain default join table name in sqlite"
    );

    // Use a file-based SQLite database (hermetic, no Docker)
    let db_path = dir.join("custom_test.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let pool = SqlitePool::connect(&db_url)
        .await
        .expect("failed to connect to SQLite");

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("failed to enable foreign keys");

    execute_sqlite_sql_file(&pool, &up_sql)
        .await
        .expect("failed to apply custom names up.sql");

    // Verify tables exist with custom names
    let privileges_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'privileges'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let customers_privileges_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'customers_privileges'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(privileges_count, 1, "privileges table should exist");
    assert_eq!(
        customers_privileges_count, 1,
        "customers_privileges table should exist"
    );

    // Verify renamed constraint live: duplicate privilege insert errors
    sqlx::query(
        "INSERT INTO privileges (name, resource_type, resource_id) VALUES ('admin', '', '')",
    )
    .execute(&pool)
    .await
    .expect("first privilege insert should succeed");

    let duplicate_privilege = sqlx::query(
        "INSERT INTO privileges (name, resource_type, resource_id) VALUES ('admin', '', '')",
    )
    .execute(&pool)
    .await;
    assert!(
        duplicate_privilege.is_err(),
        "duplicate privilege should fail on privileges_triple_unique"
    );

    // Verify down drops customers_privileges first
    let down_sql =
        fs::read_to_string(dir.join("migrations/sqlite/0000000001_rolify_create_tables/down.sql"))
            .unwrap();

    assert!(
        down_sql
            .find("DROP TABLE IF EXISTS customers_privileges")
            .unwrap_or(usize::MAX)
            < down_sql
                .find("DROP TABLE IF EXISTS privileges")
                .unwrap_or(usize::MAX),
        "down.sql should drop customers_privileges before privileges"
    );

    execute_sqlite_sql_file(&pool, &down_sql)
        .await
        .expect("failed to apply down.sql");

    let privileges_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'privileges'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let customers_privileges_after_down: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'customers_privileges'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        privileges_after_down, 0,
        "privileges table should be dropped"
    );
    assert_eq!(
        customers_privileges_after_down, 0,
        "customers_privileges table should be dropped"
    );

    println!("Custom names SQLite e2e test passed");
}
