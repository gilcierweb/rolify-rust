//! `MySQL` async parity suite binding: the full Phase 2 portable suite
//! running against real `MySQL` via testcontainers.
//!
//! This is the SC-1 async parity gate: the complete ported suite (12 modules,
//! 123 wrappers) plus the 03-04 `query_guards` extension (2 wrappers),
//! all executing on mysql:8.4 with readiness waits over bb8 pool.
//!
//! Runs: `cargo test -p rolify-diesel --no-default-features --features async,bb8,mysql,suite`

#![cfg(all(feature = "async", feature = "mysql", feature = "suite"))]

mod support;

rolify_test::parity_suite!(
    parity_mysql_async,
    crate::support::diesel_async_mysql_backend::DieselAsyncMysqlBackend
);
