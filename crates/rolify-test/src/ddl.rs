//! Per-engine fixture table DDL (Phase 4 decision D-06): the single
//! source of the fixture tables every backend binding seeds before
//! running the ported parity suite.
//!
//! The statements mirror `rolify/spec/support/schema.rb` (the gem's spec
//! schema) and were moved verbatim from the diesel reference adapter's
//! inlined `setup_fixtures` batches
//! (`rolify-diesel/tests/support/mod.rs`, Phase 3). D-06 relocates them
//! into the shared suite so that every backend (diesel in 04-10, sqlx in
//! 04-03/04-06, the diesel-async rider in 04-05/04-07, SeaORM and
//! MongoDB in Phase 5) consumes provably identical fixtures by
//! construction.
//!
//! ## Consumption contract
//!
//! Each adapter iterates the array for its engine and executes every
//! statement on its own connection. `rolify-test` deliberately carries
//! no backend dependency and no executor: the suite publishes statement
//! strings and the adapter is the execution authority, preserving the
//! adapter-to-suite dependency direction (the Phase 1 D-06 seven-crate
//! isolation).
//!
//! The tables cover the suite's fixture matrix: `users`, `customers`,
//! `forums`, `groups`, `teams` (string primary key `team_code`, the
//! gem's non-integer PK case), `organizations` (the STI `type` column),
//! `rights`, `moderators_rights`, and `admin_rights` (the
//! `Admin::Moderator` custom pair).

/// Fixture DDL for `PostgreSQL`: `BIGSERIAL` primary keys, one statement
/// per table in the gem fixture order.
pub const POSTGRES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS users (
                id BIGSERIAL PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'User',
                name VARCHAR(255) NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS customers (
                id BIGSERIAL PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'Customer',
                name VARCHAR(255) NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS forums (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS groups (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS teams (
                team_code VARCHAR(191) PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS organizations (
                id BIGSERIAL PRIMARY KEY,
                type VARCHAR(191) NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS rights (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            );",
    "CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            );",
];

/// Fixture DDL for `MySQL`: `AUTO_INCREMENT` primary keys with `InnoDB`
/// table options, one statement per table in the gem fixture order.
pub const MYSQL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS users (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'User',
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS customers (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'Customer',
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS forums (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS `groups` (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS teams (
                team_code VARCHAR(191) PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS organizations (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                type VARCHAR(191) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS rights (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    "CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
];

/// Fixture DDL for `SQLite`: `INTEGER PRIMARY KEY AUTOINCREMENT`, one
/// statement per table in the gem fixture order.
pub const SQLITE: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rolify_type TEXT NOT NULL DEFAULT 'User',
                name TEXT NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS customers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rolify_type TEXT NOT NULL DEFAULT 'Customer',
                name TEXT NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS forums (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS groups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS teams (
                team_code TEXT PRIMARY KEY,
                name TEXT NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS organizations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                type TEXT NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS rights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );",
    "CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id TEXT NOT NULL,
                right_id INTEGER NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            );",
    "CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id TEXT NOT NULL,
                right_id INTEGER NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            );",
];

#[cfg(test)]
mod tests {
    use super::{MYSQL, POSTGRES, SQLITE};

    const FIXTURE_TABLES: [&str; 9] = [
        "users",
        "customers",
        "forums",
        "groups",
        "teams",
        "organizations",
        "rights",
        "moderators_rights",
        "admin_rights",
    ];

    #[test]
    fn every_engine_creates_the_nine_fixture_tables_in_order() {
        for statements in [POSTGRES, MYSQL, SQLITE] {
            assert_eq!(
                statements.len(),
                FIXTURE_TABLES.len(),
                "one statement per fixture table"
            );
            for (statement, table) in statements.iter().zip(FIXTURE_TABLES) {
                assert!(
                    statement.starts_with(&format!("CREATE TABLE IF NOT EXISTS {table} (")),
                    "expected a create for `{table}`, got: {statement}"
                );
            }
        }
    }

    #[test]
    fn every_statement_is_terminated_for_single_statement_execution() {
        for statements in [POSTGRES, MYSQL, SQLITE] {
            for statement in statements {
                assert!(
                    statement.starts_with("CREATE TABLE IF NOT EXISTS "),
                    "unexpected statement shape: {statement}"
                );
                assert!(
                    statement.ends_with(';'),
                    "unterminated statement: {statement}"
                );
            }
        }
    }
}
