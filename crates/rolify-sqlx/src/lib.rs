//! # rolify-sqlx
//!
//! Async `SQLx` adapter for the rolify workspace: hand-written SQL with
//! runtime binds (`sqlx::query` + `.bind`), executed over the sealed
//! `rolify` SPI in its default (async) mode.
//!
//! ## Feature matrix
//!
//! | Mode | Cargo feature | Backend | Notes |
//! |---|---|---|---|
//! | async + Postgres | `postgres` | `sqlx::Postgres` | Enables `sqlx/postgres` |
//! | async + `MySQL` | `mysql` | `sqlx::MySql` | Enables `sqlx/mysql` |
//! | async + `SQLite` | `sqlite` | `sqlx::Sqlite` | Enables `sqlx/sqlite`; local convenience, non-gate |
//!
//! Engine features are **additive**, mirroring sqlx itself (D-11/D-13): a
//! single build may enable any combination, including all three. There are no
//! mutual-exclusion guards because there is nothing to exclude.
//!
//! **Async-only by construction (Phase 1 D-05):** this crate declares no
//! sync-mode feature of any name and never forwards `rolify-core/is_sync`.
//! As `rolify-core`'s own mode-matrix docs warn, features unify across one
//! cargo graph: if any crate in the graph enables `rolify-core/is_sync`, the
//! whole graph is sync, and code written against the async SPI (this crate)
//! fails to compile against it by construction. A consumer cannot express a
//! synchronous `rolify-sqlx`.
//!
//! **MSRV 1.94 (D-10):** the floor is ratified by sqlx 0.9. The workspace
//! pins `sqlx = "=0.9.0"` with the split `runtime-tokio` +
//! `tls-rustls-ring` features (the 0.8-style combined runtime+TLS feature
//! names were deleted in 0.9).
//!
//! ## Architecture
//!
//! - [`error`]: `Error` enum wrapping `sqlx::Error` + `RolifyError`.
//! - [`sentinel`]: `None` ↔ `''` translation at the adapter boundary only.
//!
//! The store, SQL templates, dialect, and row types land in later plans of
//! this phase; the `MIGRATIONS_<ENGINE>` exports ship with the vendored
//! migration trees (D-01/D-02).

pub mod error;
pub mod sentinel;

pub use error::Error;
pub use sentinel::{from_storage, resource_id_from_storage, resource_id_to_storage, to_storage};
