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

/// Returns the up migration for the given backend.
#[must_use]
pub fn up(backend: &str) -> &str {
    match backend {
        "diesel" | "sqlx" => POSTGRES_UP,
        "seaorm" => POSTGRES_UP, // SeaORM uses Postgres dialect
        "mongodb" => "",         // MongoDB uses document templates, not SQL
        _ => panic!("unknown backend: {backend}"),
    }
}

/// Returns the down migration for the given backend.
#[must_use]
pub fn down(backend: &str) -> &str {
    match backend {
        "diesel" | "sqlx" => POSTGRES_DOWN,
        "seaorm" => POSTGRES_DOWN, // SeaORM uses Postgres dialect
        "mongodb" => "",           // MongoDB uses document templates, not SQL
        _ => panic!("unknown backend: {backend}"),
    }
}