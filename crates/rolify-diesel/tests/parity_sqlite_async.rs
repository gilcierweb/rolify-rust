//! `SQLite` async parity suite binding: the full Phase 2 portable suite
//! running against hermetic `SQLite` in-memory.
//!
//! This is the NON-GATE convenience leg (D-11 applied to the rider):
//! the complete ported suite executes on diesel-async's `SyncConnectionWrapper`
//! `SQLite` support without containers. Locking-sensitive cases (concurrency)
//! are not meaningful on single-writer `SQLite` and are excluded from the gate.
//!
//! Runs: `cargo test -p rolify-diesel --no-default-features --features async,sqlite,suite`

#![cfg(all(feature = "async", feature = "sqlite", feature = "suite"))]

mod support;

rolify_test::parity_suite!(
    parity_sqlite_async,
    crate::support::diesel_async_sqlite_backend::DieselAsyncSqliteBackend
);
