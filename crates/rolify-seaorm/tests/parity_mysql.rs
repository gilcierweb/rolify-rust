//! `MySQL` parity suite binding - the full Phase 2 portable suite
//! (frozen twelve-module `parity_suite!`) plus the `query_guards`
//! extension, running against `mysql:8.4` (`utf8mb4_bin` collation) with
//! readiness waits via testcontainers.
//!
//! Run: `cargo test -p rolify-seaorm --features mysql,suite --test parity_mysql`

#![cfg(all(feature = "mysql", feature = "suite", not(feature = "postgres")))]

mod support;

use rolify_test::fixtures::DefaultUser;

rolify_test::parity_suite!(
    parity_mysql,
    crate::support::SeaormBackend<rolify_test::fixtures::DefaultUser>
);
