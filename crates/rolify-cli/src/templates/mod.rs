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

/// Returns the up migration for the given engine.
#[must_use]
pub fn up(engine: &str) -> &str {
    match engine {
        "postgres" => POSTGRES_UP,
        "mysql" => MYSQL_UP,
        "sqlite" => SQLITE_UP,
        "seaorm" => POSTGRES_UP, // SeaORM uses Postgres dialect
        "mongodb" => "",         // MongoDB uses document templates, not SQL
        _ => panic!("unknown engine: {engine}"),
    }
}

/// Returns the down migration for the given engine.
#[must_use]
pub fn down(engine: &str) -> &str {
    match engine {
        "postgres" => POSTGRES_DOWN,
        "mysql" => MYSQL_DOWN,
        "sqlite" => SQLITE_DOWN,
        "seaorm" => POSTGRES_DOWN, // SeaORM uses Postgres dialect
        "mongodb" => "",           // MongoDB uses document templates, not SQL
        _ => panic!("unknown engine: {engine}"),
    }
}

/// Returns the SeaORM migration template.
#[must_use]
pub fn seaorm_migration() -> &'static str {
    SEAORM_MIGRATION
}

/// Returns the Mongo docs template.
#[must_use]
pub fn mongo_docs() -> &'static str {
    MONGO_DOCS
}
