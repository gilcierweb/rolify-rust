//! Embedded canonical SQL templates.
//!
//! These are byte-identical copies of the canonical rolify-diesel migrations
//! (the canonical home per D-13). Any drift from the canonical source is
//! caught by the drift test (tests/drift.rs).
//!
//! Paths are source-file-relative inside the rolify-cli crate, never reaching
//! across the crate boundary (publish-safe per D-13).

pub const POSTGRES_UP: &str = include_str!("../../templates/postgres/up.sql");
pub const POSTGRES_DOWN: &str = include_str!("../../templates/postgres/down.sql");
pub const MYSQL_UP: &str = include_str!("../../templates/mysql/up.sql");
pub const MYSQL_DOWN: &str = include_str!("../../templates/mysql/down.sql");
pub const SQLITE_UP: &str = include_str!("../../templates/sqlite/up.sql");
pub const SQLITE_DOWN: &str = include_str!("../../templates/sqlite/down.sql");

// Hand-maintained templates for SeaORM and Mongo (D-16, D-17)
pub const SEAORM_MIGRATION: &str = include_str!("../../templates/seaorm_migration.rs.txt");
pub const MONGO_DOCS: &str = include_str!("../../templates/mongo_docs.rs.txt");

// Scaffolding templates (D-20)
pub mod scaffolding {
    pub const ROLE_STUB: &str = include_str!("../../templates/scaffolding/role_stub.rs.txt");
    pub const HOLDER_STUB: &str = include_str!("../../templates/scaffolding/holder_stub.rs.txt");
    pub const CONFIG_EXAMPLE: &str = include_str!("../../templates/scaffolding/config_example.rs.txt");
    pub const README_DIESEL: &str = include_str!("../../templates/scaffolding/README_diesel.md.txt");
    pub const README_SQLX: &str = include_str!("../../templates/scaffolding/README_sqlx.md.txt");
    pub const README_SEAORM: &str = include_str!("../../templates/scaffolding/README_seaorm.md.txt");
    pub const README_MONGODB: &str = include_str!("../../templates/scaffolding/README_mongodb.md.txt");

    /// Renders the role stub with substitutions.
    #[must_use] 
    pub fn role_stub(role_name: &str, backend: &str, holder_name: &str) -> String {
        ROLE_STUB
            .replace("{role_name}", role_name)
            .replace("{backend}", backend)
            .replace("{holder_name}", holder_name)
    }

    /// Renders the holder stub with substitutions.
    #[must_use] 
    pub fn holder_stub(holder_name: &str, backend: &str, role_name: &str) -> String {
        HOLDER_STUB
            .replace("{holder_name}", holder_name)
            .replace("{backend}", backend)
            .replace("{role_name}", role_name)
    }

    /// Renders the config example with substitutions.
    #[must_use] 
    pub fn config_example(
        backend: &str,
        role_name: &str,
        holder_name: &str,
        roles_table: &str,
        join_table: &str,
    ) -> String {
        CONFIG_EXAMPLE
            .replace("{backend}", backend)
            .replace("{role_name}", role_name)
            .replace("{holder_name}", holder_name)
            .replace("{roles_table}", roles_table)
            .replace("{join_table}", join_table)
    }

    /// Returns the README template for the given engine.
    ///
    /// # Errors
    ///
    /// Returns an error string when the README name is unknown.
    pub fn readme(name: &str) -> Result<&'static str, &'static str> {
        match name {
            "README_diesel" => Ok(README_DIESEL),
            "README_sqlx" => Ok(README_SQLX),
            "README_seaorm" => Ok(README_SEAORM),
            "README_mongodb" => Ok(README_MONGODB),
            _ => Err("unknown README template"),
        }
    }
}

/// Returns the up migration for the given engine.
///
/// # Panics
///
/// Panics when the engine is not one of postgres, mysql, sqlite, seaorm, mongodb.
#[must_use]
pub fn up(engine: &str) -> &str {
    match engine {
        "postgres" | "seaorm" => POSTGRES_UP, // SeaORM uses the Postgres dialect
        "mysql" => MYSQL_UP,
        "sqlite" => SQLITE_UP,
        "mongodb" => "", // MongoDB uses document templates, not SQL
        _ => panic!("unknown engine: {engine}"),
    }
}

/// Returns the down migration for the given engine.
///
/// # Panics
///
/// Panics when the engine is not one of postgres, mysql, sqlite, seaorm, mongodb.
#[must_use]
pub fn down(engine: &str) -> &str {
    match engine {
        "postgres" | "seaorm" => POSTGRES_DOWN, // SeaORM uses the Postgres dialect
        "mysql" => MYSQL_DOWN,
        "sqlite" => SQLITE_DOWN,
        "mongodb" => "", // MongoDB uses document templates, not SQL
        _ => panic!("unknown engine: {engine}"),
    }
}

/// Returns the `SeaORM` migration template.
#[must_use]
pub fn seaorm_migration() -> &'static str {
    SEAORM_MIGRATION
}

/// Returns the Mongo docs template.
#[must_use]
pub fn mongo_docs() -> &'static str {
    MONGO_DOCS
}