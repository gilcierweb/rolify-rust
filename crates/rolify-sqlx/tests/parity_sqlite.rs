//! SQLite parity suite binding — the full Phase 2 portable suite
//! running against hermetic in-memory SQLite.
//!
//! This is the NON-GATE convenience leg (Phase 4 D-11): local feedback
//! without Docker, excluding locking-sensitive cases by the suite's own
//! dynamic exclusion (single-writer SQLite makes concurrency tests
//! meaningless). This leg never blocks the phase gate; the gate engines
//! are containerized Postgres and MySQL.
//!
//! Runs: `cargo test -p rolify-sqlx --features sqlite,suite`

#![cfg(all(feature = "sqlite", feature = "suite"))]

mod support;

rolify_test::parity_suite!(parity_sqlite, crate::support::sqlx_backend::SqlxBackend<sqlx::Sqlite>);