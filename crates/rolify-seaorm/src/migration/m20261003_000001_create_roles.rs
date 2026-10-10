//! Initial migration: canonical `roles` + `users_roles` tables.
//!
//! Physical schema is identical to the canonical Phase 3 `up.sql` trees
//! (D-04: native `sea-orm-migration` format, same physical schema).
//! The decision comments from the diesel `up.sql` files are carried here
//! as document comments:
//!
//! - D-10: FK cascade from join to roles is deliberate; the gem emits
//!   no FKs (parity-matrix divergence). Resource cleanup for deleted
//!   consumer resources is app-level (`DELETE FROM roles WHERE
//!   resource_type = ... AND resource_id = ...`); cascade sweeps links.
//! - D-01/D-02: sentinel `''` strategy - global/class scope rows store
//!   empty string in `resource_type`/`resource_id` (never SQL NULL) so
//!   the `UNIQUE` triple deduplicates identically on Postgres and `MySQL`.
//! - D-05: join table has `UNIQUE(user_id, role_id)` - diverges from the
//!   gem's non-unique composite index; makes `add_role` race-safe via
//!   insert + catch-unique-violation.
//! - D-03: `VARCHAR` sizes - `name(255)`, `resource_type(191)`,
//!   `resource_id(191)`.
//! - RESEARCH Pitfall 8 (`MySQL`): string columns declare
//!   `CHARACTER SET utf8mb4 COLLATE utf8mb4_bin` for byte-exact role-name
//!   and id comparison; charset/collate clauses follow the data type.
//! - D-12: `roles.id` is `BIGINT GENERATED ALWAYS AS IDENTITY` on
//!   Postgres, `BIGINT AUTO_INCREMENT` on `MySQL`.
//! - D-13: timestamps `NOT NULL DEFAULT CURRENT_TIMESTAMP`.
//! - D-08-04: holder id kind adds per-kind column types for `user_id`:
//!   integer = `BIGINT`, uuid = `UUID` (PG) / `BINARY(16)` (`MySQL`) / `TEXT` (`SQLite`),
//!   string = `VARCHAR(191)`.

use rolify_core::config::HolderIdKind;
use sea_orm::{DbBackend, DbErr};
use sea_orm_migration::prelude::*;

/// The initial canonical-schema migration.
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &'static str {
        "m20261003_000001_create_roles"
    }
}

/// Migration direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationDirection {
    Up,
    Down,
}

/// Get the migration statements for the given backend, holder id kind, and direction.
/// This aligns with the CLI seaorm emitter which bakes kind into per-dialect arrays.
///
/// # Errors
///
/// Returns [`DbErr::Custom`] if the backend is not Postgres or `MySQL` (e.g., `SQLite`).
pub fn get_statements(
    backend: DbBackend,
    kind: HolderIdKind,
    direction: MigrationDirection,
) -> Result<&'static [&'static str], DbErr> {
    match direction {
        MigrationDirection::Up => match backend {
            DbBackend::Postgres => match kind {
                HolderIdKind::Integer => Ok(POSTGRES_UP_INTEGER),
                HolderIdKind::Uuid => Ok(POSTGRES_UP_UUID),
                HolderIdKind::String => Ok(POSTGRES_UP_STRING),
            },
            DbBackend::MySql => match kind {
                HolderIdKind::Integer => Ok(MYSQL_UP_INTEGER),
                HolderIdKind::Uuid => Ok(MYSQL_UP_UUID),
                HolderIdKind::String => Ok(MYSQL_UP_STRING),
            },
            other => Err(DbErr::Custom(format!(
                "rolify-seaorm migrations support Postgres and MySQL only; got {other:?}"
            ))),
        },
        MigrationDirection::Down => Ok(DOWN_STATEMENTS),
    }
}

/// Postgres DDL for integer holder id kind (default, D-08-02).
const POSTGRES_UP_INTEGER: &[&str] = &[
    "CREATE TABLE roles (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    resource_type VARCHAR(191) NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
)",
    "CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id)",
    "CREATE INDEX idx_roles_name ON roles (name)",
    "CREATE TABLE users_roles (
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id)
)",
];

/// Postgres DDL for uuid holder id kind (D-08-04: native UUID).
const POSTGRES_UP_UUID: &[&str] = &[
    "CREATE TABLE roles (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    resource_type VARCHAR(191) NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
)",
    "CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id)",
    "CREATE INDEX idx_roles_name ON roles (name)",
    "CREATE TABLE users_roles (
    user_id UUID NOT NULL,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id)
)",
];

/// Postgres DDL for string holder id kind (D-08-04: VARCHAR(191)).
const POSTGRES_UP_STRING: &[&str] = &[
    "CREATE TABLE roles (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    resource_type VARCHAR(191) NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
)",
    "CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id)",
    "CREATE INDEX idx_roles_name ON roles (name)",
    "CREATE TABLE users_roles (
    user_id VARCHAR(191) NOT NULL,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id)
)",
];

/// `MySQL` DDL for integer holder id kind (default, D-08-02).
const MYSQL_UP_INTEGER: &[&str] = &[
    "CREATE TABLE roles (
    id            BIGINT AUTO_INCREMENT PRIMARY KEY,
    name          VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    resource_type VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
    "CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id)",
    "CREATE INDEX idx_roles_name ON roles (name)",
    "CREATE TABLE users_roles (
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id),
    CONSTRAINT users_roles_role_id_fk FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
];

/// `MySQL` DDL for uuid holder id kind (D-08-04: `BINARY(16)`).
const MYSQL_UP_UUID: &[&str] = &[
    "CREATE TABLE roles (
    id            BIGINT AUTO_INCREMENT PRIMARY KEY,
    name          VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    resource_type VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
    "CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id)",
    "CREATE INDEX idx_roles_name ON roles (name)",
    "CREATE TABLE users_roles (
    user_id BINARY(16) NOT NULL,
    role_id BIGINT NOT NULL,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id),
    CONSTRAINT users_roles_role_id_fk FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
];

/// `MySQL` DDL for string holder id kind (D-08-04: `VARCHAR(191)` with charset).
const MYSQL_UP_STRING: &[&str] = &[
    "CREATE TABLE roles (
    id            BIGINT AUTO_INCREMENT PRIMARY KEY,
    name          VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    resource_type VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
    "CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id)",
    "CREATE INDEX idx_roles_name ON roles (name)",
    "CREATE TABLE users_roles (
    user_id VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    role_id BIGINT NOT NULL,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id),
    CONSTRAINT users_roles_role_id_fk FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
];

/// Down migration: join table first (FK child), then roles.
const DOWN_STATEMENTS: &[&str] = &["DROP TABLE users_roles", "DROP TABLE roles"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Default to integer kind (D-08-02) for the embedded migration.
        // Other kinds are tested via CLI-generated migrations.
        let statements = get_statements(manager.get_database_backend(), HolderIdKind::Integer, MigrationDirection::Up)?;
        for statement in statements {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let statements = get_statements(manager.get_database_backend(), HolderIdKind::Integer, MigrationDirection::Down)?;
        for statement in statements {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_statements_postgres_integer_up() {
        let stmts = get_statements(DbBackend::Postgres, HolderIdKind::Integer, MigrationDirection::Up).unwrap();
        assert!(!stmts.is_empty());
        // Verify user_id is BIGINT
        let create_join = stmts.iter().find(|s| s.contains("CREATE TABLE users_roles")).unwrap();
        assert!(create_join.contains("user_id BIGINT NOT NULL"));
    }

    #[test]
    fn get_statements_postgres_uuid_up() {
        let stmts = get_statements(DbBackend::Postgres, HolderIdKind::Uuid, MigrationDirection::Up).unwrap();
        assert!(!stmts.is_empty());
        let create_join = stmts.iter().find(|s| s.contains("CREATE TABLE users_roles")).unwrap();
        assert!(create_join.contains("user_id UUID NOT NULL"));
    }

    #[test]
    fn get_statements_postgres_string_up() {
        let stmts = get_statements(DbBackend::Postgres, HolderIdKind::String, MigrationDirection::Up).unwrap();
        assert!(!stmts.is_empty());
        let create_join = stmts.iter().find(|s| s.contains("CREATE TABLE users_roles")).unwrap();
        assert!(create_join.contains("user_id VARCHAR(191) NOT NULL"));
    }

    #[test]
    fn get_statements_mysql_integer_up() {
        let stmts = get_statements(DbBackend::MySql, HolderIdKind::Integer, MigrationDirection::Up).unwrap();
        assert!(!stmts.is_empty());
        let create_join = stmts.iter().find(|s| s.contains("CREATE TABLE users_roles")).unwrap();
        assert!(create_join.contains("user_id BIGINT NOT NULL"));
    }

    #[test]
    fn get_statements_mysql_uuid_up() {
        let stmts = get_statements(DbBackend::MySql, HolderIdKind::Uuid, MigrationDirection::Up).unwrap();
        assert!(!stmts.is_empty());
        let create_join = stmts.iter().find(|s| s.contains("CREATE TABLE users_roles")).unwrap();
        assert!(create_join.contains("user_id BINARY(16) NOT NULL"));
        // BINARY(16) should not have charset clause
        assert!(!create_join.contains("CHARACTER SET"));
    }

    #[test]
    fn get_statements_mysql_string_up() {
        let stmts = get_statements(DbBackend::MySql, HolderIdKind::String, MigrationDirection::Up).unwrap();
        assert!(!stmts.is_empty());
        let create_join = stmts.iter().find(|s| s.contains("CREATE TABLE users_roles")).unwrap();
        assert!(create_join.contains("user_id VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL"));
    }

    #[test]
    fn get_statements_down() {
        let stmts = get_statements(DbBackend::Postgres, HolderIdKind::Integer, MigrationDirection::Down).unwrap();
        assert_eq!(stmts, DOWN_STATEMENTS);
    }

    #[test]
    fn get_statements_unsupported_backend() {
        // SQLite is not supported by this migration module
        let result = get_statements(DbBackend::Sqlite, HolderIdKind::Integer, MigrationDirection::Up);
        assert!(result.is_err());
    }
}