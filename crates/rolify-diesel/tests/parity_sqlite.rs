//! SQLite parity suite binding — the full Phase 2 portable suite
//! running against local SQLite (convenience, non-gate).
//!
//! This is a local-only convenience leg (D-14): runs the identical suite
//! on SQLite in-memory for fast feedback without Docker. The concurrency
//! test case is excluded (SQLite single-writer SQLITE_BUSY makes it
//! non-deterministic and not representative of PG/MySQL).
//!
//! Runs: `cargo test -p rolify-diesel --no-default-features --features sync,sqlite,suite`

#![cfg(all(feature = "sync", feature = "sqlite", feature = "suite"))]

mod support;

rolify_test::parity_suite!(parity_sqlite, crate::support::diesel_backend::DieselBackend);
