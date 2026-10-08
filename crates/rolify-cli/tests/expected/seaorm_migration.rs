//! `SeaORM` migration template for rolify schema.
//!
//! This template is hand-maintained (D-16) and mirrors the canonical schema
//! from the vendored SQL templates. The raw-SQL approach (option a per
//! 06-RESEARCH.md Code Examples) reuses the exact canonical strings per engine
//! selected at runtime via the `SchemaManager`'s database backend.
//!
//! CONVERGENCE NOTE (D-16): This template is the second maintained source.
//! The semantic checklist in tests/drift.rs keeps it honest against the
//! canonical SQL. When Phase 5 executes, cross-crate comparison will promote
//! this to a three-way gate.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Execute the canonical up.sql statements ONE AT A TIME: the Postgres
        // extended protocol rejects multiple commands inside a single
        // prepared statement ("cannot insert multiple commands into a
        // prepared statement"), so the canonical script is split into
        // individual statements at render time.
        for statement in [
            r"-- Decision record (D-10): FK cascade from join to roles is deliberate; the
-- gem emits no FKs (divergence documented in the parity matrix). Resource
-- cleanup for deleted consumer resources is app-level: one DELETE on roles by
-- (resource_type, resource_id); cascade sweeps join rows.
--
-- D-01/D-02: sentinel '' strategy: global/class scope rows store empty string
-- in resource_type/resource_id (never SQL NULL) so the UNIQUE triple constraint
-- deduplicates identically on Postgres, MySQL, and SQLite.
--
-- D-05: join table has UNIQUE(user_id, role_id): diverges from the gem's
-- non-unique composite index. This makes add_role race-safe via INSERT with
-- catch-and-ignore of unique violation.
--
-- D-03: VARCHAR sizes: name(255), resource_type(191), resource_id(191).
-- D-12: roles.id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY.
-- D-13: timestamps NOT NULL DEFAULT CURRENT_TIMESTAMP (updated_at static; roles never UPDATE).

CREATE TABLE roles (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    resource_type VARCHAR(191) NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
);",
            r"CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id);",
            r"CREATE INDEX idx_roles_name ON roles (name);",
            r"CREATE TABLE users_roles (
    user_id VARCHAR(191) NOT NULL,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id)
);",
        ] {
            let stmt = sea_orm::Statement::from_string(
                manager.get_database_backend(),
                statement.to_owned(),
            );
            manager.get_connection().execute_raw(stmt).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Down drops join table first (FK order), then roles (D-10, D-12)
        for statement in [
            "DROP TABLE IF EXISTS users_roles;",
            "DROP TABLE IF EXISTS roles;",
        ] {
            let stmt = sea_orm::Statement::from_string(
                manager.get_database_backend(),
                statement.to_owned(),
            );
            manager.get_connection().execute_raw(stmt).await?;
        }
        Ok(())
    }
}
