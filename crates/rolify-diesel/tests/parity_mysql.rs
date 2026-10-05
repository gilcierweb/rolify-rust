//! `MySQL` parity suite binding — the full Phase 2 portable suite
//! running against real `MySQL` via testcontainers.
//!
//! This is the SC-1 parity gate: the complete ported suite (12 modules,
//! 123 wrappers) plus the 03-04 `query_guards` extension (2 wrappers),
//! all executing on mysql:8.4 with readiness waits.
//!
//! Runs: `cargo test -p rolify-diesel --no-default-features --features sync,mysql,suite`
//!
//! Note: `MySQL` leg requires libmysqlclient at compile time. CI installs it;
//! local builds may need `sudo apt install libmysqlclient-dev`.

#![cfg(all(feature = "sync", feature = "mysql", feature = "suite"))]

mod support;

rolify_test::parity_suite!(parity_mysql, crate::support::diesel_backend::DieselBackend);
