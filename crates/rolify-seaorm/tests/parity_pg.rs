//! Postgres parity suite binding - the full Phase 2 portable suite
//! (frozen twelve-module `parity_suite!`) plus the query_guards
//! extension, running against `postgres:17` with readiness waits via
//! testcontainers.
//!
//! Gate for ADPT-05 (D-14/D-15 analogs): `SeaormBackend` must produce
//! the identical case outcomes the diesel reference and sqlx adapters
//! post on their own engines.
//!
//! Run: `cargo test -p rolify-seaorm --features postgres,suite --test parity_pg`

#![cfg(all(feature = "postgres", feature = "suite"))]

mod support;

rolify_test::parity_suite!(
    parity_pg,
    crate::support::SeaormBackend<rolify_test::fixtures::DefaultUser>
);
