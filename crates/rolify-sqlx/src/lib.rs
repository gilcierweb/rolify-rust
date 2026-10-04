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
//! - [`sentinel`]: `None` <-> `''` translation at the adapter boundary only.
//! - `MIGRATIONS_POSTGRES` / `MIGRATIONS_MYSQL` / `MIGRATIONS_SQLITE`:
//!   per-engine `sqlx::migrate::Migrator` statics built from the vendored
//!   migration trees (D-01/D-02). Consumers invoke `.run(&pool)` themselves;
//!   the crate never migrates automatically.
//! - [`dialect`]: runtime `DB::NAME` switches (placeholder syntax,
//!   identifier quoting) shared by every engine.
//! - [`SqlxStore`]: the generic async `RoleStore` (D-13) over
//!   hand-written SQL with runtime binds (D-12), its statement templates
//!   in the private `sql` module and its row decoders in the private
//!   `rows` module. `ResourceStore` lands with the resource-side
//!   expansion in the next plan of this phase.

// Vendored migration exports (D-01/D-04/D-05): flat sqlx layout, byte-identical
// to the canonical `rolify-diesel` trees; `tests/drift_guard.rs` enforces the
// identity on every `cargo test` run (D-03).

/// Postgres migrations vendored byte-identically from the canonical
/// `rolify-diesel` tree.
///
/// Consumers invoke `MIGRATIONS_POSTGRES.run(&pool).await` themselves; this
/// crate never migrates automatically (D-02, mirroring rolify-diesel's
/// Phase 3 D-11).
#[cfg(feature = "postgres")]
pub static MIGRATIONS_POSTGRES: sqlx::migrate::Migrator = sqlx::migrate!("migrations/postgres");

/// `MySQL` migrations vendored byte-identically from the canonical
/// `rolify-diesel` tree.
///
/// Consumers invoke `MIGRATIONS_MYSQL.run(&pool).await` themselves; this
/// crate never migrates automatically (D-02, mirroring rolify-diesel's
/// Phase 3 D-11).
#[cfg(feature = "mysql")]
pub static MIGRATIONS_MYSQL: sqlx::migrate::Migrator = sqlx::migrate!("migrations/mysql");

/// `SQLite` migrations vendored byte-identically from the canonical
/// `rolify-diesel` tree.
///
/// Consumers invoke `MIGRATIONS_SQLITE.run(&pool).await` themselves; this
/// crate never migrates automatically (D-02, mirroring rolify-diesel's
/// Phase 3 D-11).
#[cfg(feature = "sqlite")]
pub static MIGRATIONS_SQLITE: sqlx::migrate::Migrator = sqlx::migrate!("migrations/sqlite");

pub mod dialect;
pub mod error;
pub mod sentinel;

mod rows;
mod sql;
mod store;

pub use dialect::{placeholder, quote_identifier};
pub use error::Error;
pub use sentinel::{from_storage, resource_id_from_storage, resource_id_to_storage, to_storage};
pub use store::SqlxStore;
