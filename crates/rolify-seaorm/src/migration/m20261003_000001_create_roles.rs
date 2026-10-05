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

use sea_orm::{DbBackend, DbErr};
use sea_orm_migration::prelude::*;

/// The initial canonical-schema migration.
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &'static str {
        "m20261003_000001_create_roles"
    }
}

/// Postgres DDL, one statement per entry (canonical Phase 3 schema).
const POSTGRES_STATEMENTS: &[&str] = &[
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

/// `MySQL` DDL (`InnoDB` + `utf8mb4`/`utf8mb4_bin` collation branch).
const MYSQL_STATEMENTS: &[&str] = &[
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
        let statements = match manager.get_database_backend() {
            DbBackend::Postgres => POSTGRES_STATEMENTS,
            DbBackend::MySql => MYSQL_STATEMENTS,
            other => {
                return Err(DbErr::Custom(format!(
                    "rolify-seaorm migrations support Postgres and MySQL only; got {other:?}"
                )));
            }
        };
        for statement in statements {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for statement in DOWN_STATEMENTS {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }
}
