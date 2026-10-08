//! `SeaORM` migration template for rolify schema.
//!
//! This template is hand-maintained (D-16) and carries the canonical schema
//! from the vendored per-engine SQL templates: every statement is
//! byte-derived at render time from the canonical `up.sql` and `down.sql`
//! for its engine, and `up()`/`down()` select the right dialect array at
//! runtime via a match on the `SchemaManager`'s database backend (D-10,
//! D-16). Statements execute ONE AT A TIME because the Postgres extended
//! protocol rejects multiple commands inside a single prepared statement
//! ("cannot insert multiple commands into a prepared statement").
//!
//! CONVERGENCE NOTE (D-16): This template is the second maintained source.
//! The semantic checklist and the wiring drift test in tests/drift.rs keep
//! it honest against the canonical per-engine SQL trees.

use sea_orm::{DbBackend, DbErr};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Postgres DDL, one statement per entry (canonical `up.sql`).
const POSTGRES_UP_STATEMENTS: &[&str] = &[
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
];

/// `MySQL` DDL, one statement per entry (canonical `up.sql`).
const MYSQL_UP_STATEMENTS: &[&str] = &[
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
-- RESEARCH Pitfall 8: MySQL string columns declare CHARACTER SET utf8mb4
-- COLLATE utf8mb4_bin for byte-exact role-name and id comparison. The
-- charset/collate clauses follow the data type (MySQL grammar position);
-- the server rejects them after NOT NULL/DEFAULT (error 1064, verified
-- against mysql:8.4).
-- D-12: roles.id BIGINT AUTO_INCREMENT PRIMARY KEY.
-- D-13: timestamps NOT NULL DEFAULT CURRENT_TIMESTAMP (updated_at static; roles never UPDATE).

CREATE TABLE roles (
    id            BIGINT AUTO_INCREMENT PRIMARY KEY,
    name          VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    resource_type VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
    r"CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id);",
    r"CREATE INDEX idx_roles_name ON roles (name);",
    r"CREATE TABLE users_roles (
    user_id VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    role_id BIGINT NOT NULL,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id),
    CONSTRAINT users_roles_role_id_fk FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
];

/// `SQLite` DDL, one statement per entry (canonical `up.sql`).
const SQLITE_UP_STATEMENTS: &[&str] = &[
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
-- D-12: roles.id INTEGER PRIMARY KEY (rowid alias).
-- D-13: timestamps NOT NULL DEFAULT CURRENT_TIMESTAMP (updated_at static; roles never UPDATE).

CREATE TABLE roles (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    resource_type TEXT NOT NULL DEFAULT '',
    resource_id   TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
);",
    r"CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id);",
    r"CREATE INDEX idx_roles_name ON roles (name);",
    r"CREATE TABLE users_roles (
    user_id TEXT NOT NULL,
    role_id INTEGER NOT NULL,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id),
    CONSTRAINT users_roles_role_id_fk FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
);",
];

/// Postgres down: join table first (FK child), then roles (D-10, D-12).
const POSTGRES_DOWN_STATEMENTS: &[&str] = &[
    r"-- Down migration: drop join table first (FK order), then roles.

DROP TABLE IF EXISTS users_roles;",
    r"DROP TABLE IF EXISTS roles;",
];

/// `MySQL` down: join table first (FK child), then roles (D-10, D-12).
const MYSQL_DOWN_STATEMENTS: &[&str] = &[
    r"-- Down migration: drop join table first (FK order), then roles.

DROP TABLE IF EXISTS users_roles;",
    r"DROP TABLE IF EXISTS roles;",
];

/// `SQLite` down: join table first (FK child), then roles (D-10, D-12).
const SQLITE_DOWN_STATEMENTS: &[&str] = &[
    r"-- Down migration: drop join table first (FK order), then roles.

DROP TABLE IF EXISTS users_roles;",
    r"DROP TABLE IF EXISTS roles;",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Select the canonical statement set for the CONNECTED dialect at
        // runtime (CR-03): each array carries the canonical bytes for its
        // own engine, executed one statement at a time.
        let (backend, statements) = match manager.get_database_backend() {
            backend @ DbBackend::Postgres => (backend, POSTGRES_UP_STATEMENTS),
            backend @ DbBackend::MySql => (backend, MYSQL_UP_STATEMENTS),
            backend @ DbBackend::Sqlite => (backend, SQLITE_UP_STATEMENTS),
            other => {
                return Err(DbErr::Custom(format!(
                    "rolify migrations support Postgres, MySQL, and SQLite; got {other:?}"
                )));
            }
        };
        for statement in statements {
            let stmt = sea_orm::Statement::from_string(backend, (*statement).to_owned());
            manager.get_connection().execute_raw(stmt).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Down drops join table first (FK order), then roles (D-10, D-12),
        // on every dialect.
        let (backend, statements) = match manager.get_database_backend() {
            backend @ DbBackend::Postgres => (backend, POSTGRES_DOWN_STATEMENTS),
            backend @ DbBackend::MySql => (backend, MYSQL_DOWN_STATEMENTS),
            backend @ DbBackend::Sqlite => (backend, SQLITE_DOWN_STATEMENTS),
            other => {
                return Err(DbErr::Custom(format!(
                    "rolify migrations support Postgres, MySQL, and SQLite; got {other:?}"
                )));
            }
        };
        for statement in statements {
            let stmt = sea_orm::Statement::from_string(backend, (*statement).to_owned());
            manager.get_connection().execute_raw(stmt).await?;
        }
        Ok(())
    }
}
