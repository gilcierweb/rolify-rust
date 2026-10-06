//! `MongoDB` parity suite binding - the full Phase 2 portable suite
//! (frozen twelve-module `parity_suite!`) plus the `query_guards`
//! extension, running against `mongo:8.2` containers in BOTH driver modes
//! (async default and `sync` + `rolify-core/is_sync`), per D-13.
//!
//! This is the ADPT-06 gate: `MongoBackend` must produce the identical
//! case outcomes the diesel, sqlx, and sea-orm adapters post on their own
//! engines - null scope is explicit null (D-07), HABTM is two-sided
//! (D-08), and the unique compound index arbitrates (D-14).
//!
//! Runs:
//! - `cargo test -p rolify-mongodb --features suite --test parity_mongo`
//!   (async, driver default)
//! - `cargo test -p rolify-mongodb --features suite,rolify-mongodb/sync,rolify-core/is_sync --test parity_mongo`
//!   (sync, via the D-12 seam)

#![cfg(feature = "suite")]

mod support;

rolify_test::parity_suite!(
    parity_bound,
    crate::support::MongoBackend<rolify_test::fixtures::DefaultUser>
);
