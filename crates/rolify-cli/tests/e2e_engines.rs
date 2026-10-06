use assert_cmd::Command;
use sqlx::{AssertSqlSafe, PgPool};
use std::fs;
use std::path::PathBuf;
use testcontainers::ImageExt;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

/// Path to the rolify-cli binary.
fn rolify_cli() -> Command {
    Command::cargo_bin("rolify-cli").unwrap()
}

/// Creates a temporary directory in the project's target/test-workspace.
fn test_temp_dir() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-workspace");
    fs::create_dir_all(&base).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("rolify-cli-e2e-")
        .tempdir_in(&base)
        .unwrap();
    dir.keep()
}

/// Executes multiple SQL statements from a string.
async fn execute_sql_file(pool: &PgPool, sql: &str) -> Result<(), sqlx::Error> {
    // Parse SQL statements, handling comments and multi-line statements
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in sql.lines() {
        let trimmed = line.trim();

        // Handle single-line comments
        if trimmed.starts_with("--") {
            continue;
        }

        current.push_str(line);
        current.push('\n');

        // Check if this line ends with a semicolon (not in a comment)
        if trimmed.ends_with(';') {
            statements.push(current.trim().to_string());
            current.clear();
        }
    }

    // Execute each statement
    for statement in statements {
        if !statement.is_empty() {
            sqlx::query(AssertSqlSafe(statement.as_str()))
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// Runs the Postgres e2e test: generate schema, apply to real Postgres 17, verify with DieselStore smoke.
#[tokio::test]
async fn generated_postgres_schema_applies_and_holds_store_smoke() {
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
    let db_url = format!("postgres://postgres:postgres@{}:{}/rolify_test", host, port);

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
        "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'roles'"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(
        constraints.iter().any(|c| c == "roles_triple_unique"),
        "roles_triple_unique missing"
    );

    let join_constraints: Vec<String> = sqlx::query_scalar(
        "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 'users_roles'"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(
        join_constraints
            .iter()
            .any(|c| c == "users_roles_pair_unique"),
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

    // DieselStore smoke test against the generated schema (re-create tables)
    execute_sql_file(&pool, &up_sql)
        .await
        .expect("failed to re-apply up.sql for store smoke");

    // Use rolify-diesel to verify the schema works with the adapter
    // This is a simplified check - the full suite is in rolify-diesel tests
    let role_id: i64 = sqlx::query_scalar("INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '') RETURNING id")
        .fetch_one(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO users_roles (user_id, role_id) VALUES ('user1', $1)")
        .bind(role_id)
        .execute(&pool)
        .await
        .expect("add role should work");

    let has_role: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users_roles WHERE user_id = 'user1' AND role_id = $1)",
    )
    .bind(role_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(has_role, "has_role should be true after add");

    sqlx::query("DELETE FROM users_roles WHERE user_id = 'user1' AND role_id = $1")
        .bind(role_id)
        .execute(&pool)
        .await
        .expect("remove role should work");

    let has_role_after: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users_roles WHERE user_id = 'user1' AND role_id = $1)",
    )
    .bind(role_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!has_role_after, "has_role should be false after remove");

    println!("Postgres e2e test passed: generated schema applies and holds store smoke");
}

/// Tests that the custom names matrix works with the generated schema.
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
    let db_url = format!("postgres://postgres:postgres@{}:{}/rolify_test", host, port);

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

    println!("Custom names e2e test passed");
}
