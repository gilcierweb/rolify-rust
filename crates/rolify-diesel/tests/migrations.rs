//! Embedded migrations apply/revert roundtrip test (SQLite leg).
//!
//! This test verifies that the canonical migrations for all three engines
//! embed correctly and apply/revert cleanly on SQLite (the hermetic leg).
//! The Postgres/MySQL legs run in the containerized parity suite
//! (D-14: SQLite = local convenience, not the parity gate).

#![cfg(all(feature = "sync", feature = "sqlite"))]

use diesel::Connection;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::MigrationHarness;
use rolify_diesel::MIGRATIONS;

#[test]
fn migrations_apply_and_revert_cleanly_on_sqlite() {
    let mut conn = SqliteConnection::establish(":memory:").expect("sqlite in-memory");

    // Apply migrations
    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations apply cleanly");

    // Verify tables exist by querying sqlite_master
    let tables: Vec<String> = diesel::sql_query(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('roles', 'users_roles')"
    )
    .load(&mut conn)
    .expect("query sqlite_master");
    assert_eq!(tables.len(), 2, "both roles and users_roles tables created");

    // Verify UNIQUE constraints by attempting duplicate inserts
    // 1. roles triple unique (name, resource_type, resource_id)
    diesel::sql_query(
        "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')"
    )
    .execute(&mut conn)
    .expect("first admin role insert");
    let dup_result = diesel::sql_query(
        "INSERT INTO roles (name, resource_type, resource_id) VALUES ('admin', '', '')"
    )
    .execute(&mut conn);
    assert!(dup_result.is_err(), "UNIQUE triple constraint rejects duplicate global role");

    // 2. users_roles pair unique (user_id, role_id)
    // First create a second role for the test
    diesel::sql_query(
        "INSERT INTO roles (name, resource_type, resource_id) VALUES ('user', '', '')"
    )
    .execute(&mut conn)
    .expect("user role insert");

    // Get role_ids
    let role_ids: Vec<i64> = diesel::sql_query("SELECT id FROM roles WHERE name IN ('admin', 'user')")
        .load(&mut conn)
        .expect("load role ids");
    assert_eq!(role_ids.len(), 2);

    let admin_id = role_ids[0];
    let user_id = role_ids[1];

    diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
        .bind::<diesel::sql_types::BigInt, _>(admin_id)
        .execute(&mut conn)
        .expect("first link insert");
    let dup_link = diesel::sql_query("INSERT INTO users_roles (user_id, role_id) VALUES ('u1', ?)")
        .bind::<diesel::sql_types::BigInt, _>(admin_id)
        .execute(&mut conn);
    assert!(dup_link.is_err(), "UNIQUE pair constraint rejects duplicate link");

    // 3. FK cascade: delete role -> join rows swept
    diesel::sql_query("DELETE FROM roles WHERE id = ?")
        .bind::<diesel::sql_types::BigInt, _>(user_id)
        .execute(&mut conn)
        .expect("delete role");
    let remaining_links: i64 = diesel::sql_query("SELECT COUNT(*) FROM users_roles WHERE role_id = ?")
        .bind::<diesel::sql_types::BigInt, _>(user_id)
        .get_result(&mut conn)
        .expect("count remaining links");
    assert_eq!(remaining_links, 0, "FK ON DELETE CASCADE sweeps join rows");

    // Revert migrations
    conn.revert_all_migrations(MIGRATIONS)
        .expect("migrations revert cleanly");

    // Verify tables are gone
    let tables_after: Vec<String> = diesel::sql_query(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('roles', 'users_roles')"
    )
    .load(&mut conn)
    .expect("query sqlite_master after revert");
    assert_eq!(tables_after.len(), 0, "tables dropped on revert");

    // Re-apply migrations (idempotency)
    conn.run_pending_migrations(MIGRATIONS)
        .expect("migrations re-apply cleanly");
    let tables_reapply: Vec<String> = diesel::sql_query(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('roles', 'users_roles')"
    )
    .load(&mut conn)
    .expect("query sqlite_master after re-apply");
    assert_eq!(tables_reapply.len(), 2, "tables recreated on re-apply");
}