//! MySQL parity suite binding — the full Phase 2 portable suite
//! running against real MySQL via testcontainers.
//!
//! This is the SC-1 parity gate: the complete ported suite (12 modules,
//! 123 wrappers) plus the 03-04 query_guards extension (2 wrappers),
//! all executing on mysql:8.4 with readiness waits.
//!
//! Runs: `cargo test -p rolify-sqlx --features mysql,suite`

#![cfg(all(feature = "mysql", feature = "suite"))]

mod support;

rolify_test::parity_suite!(parity_mysql, crate::support::sqlx_backend::SqlxBackend<sqlx::MySql>);