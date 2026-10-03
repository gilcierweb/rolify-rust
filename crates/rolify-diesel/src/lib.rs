//! # rolify-diesel
//!
//! Diesel sync reference adapter for the rolify workspace.
//!
//! ## Feature matrix
//!
//! | Mode | Cargo feature | Backend | Notes |
//! |---|---|---|---|
//! | sync | `sync` | *(required)* | Forwards `rolify-core/is_sync` |
//! | sync + Postgres | `postgres` | `diesel::pg::Pg` | Implies `sync`; pulls `diesel_migrations` |
//! | sync + MySQL | `mysql` | `diesel::mysql::Mysql` | Implies `sync`; pulls `diesel_migrations` |
//! | sync + SQLite | `sqlite` | `diesel::sqlite::Sqlite` | Implies `sync`; pulls `diesel_migrations` + bundled `libsqlite3-sys` |
//! | async | *(Phase 4)* | `diesel-async` | Not in this crate — separate `async` feature lands in Phase 4 |
//!
//! **Mutual exclusion:** exactly one of `postgres`, `mysql`, `sqlite` may be
//! enabled per build. The `compile_error!` guards below enforce this.
//!
//! **Default features = empty:** `cargo check -p rolify-diesel` (no features)
//! compiles an inert stub that does not disturb the workspace dual-mode gates
//! (both `cargo test --workspace` and `cargo test --workspace --features
//! rolify-core/is_sync` must stay green per conventions item 4).
//!
//! ## Parity
//!
//! Behavioral parity with the gem's ActiveRecord adapter is tracked in the
//! project parity matrix (Phase 7). Deliberate divergences locked in this
//! phase's CONTEXT.md:
//!
//! - D-01/D-02: sentinel `''` instead of `NULL` for global/class scope
//!   (portable uniqueness across PG/MySQL/SQLite).
//! - D-05: `UNIQUE(user_id, role_id)` on the join table (gem uses non-unique
//!   index).
//! - D-10: `FK role_id REFERENCES roles(id) ON DELETE CASCADE` (gem emits zero
//!   FKs). Resource cleanup is app-level: `DELETE FROM roles WHERE
//!   resource_type = ? AND resource_id = ?`; cascade sweeps join rows.
//! - D-09: zero DSL extensions — only the sealed SPI + constructors.
//!
//! The Phase 2 portable suite (bound via `rolify_test::parity_suite!`) runs
//! against this adapter on all three engines.
//!
//! ## Architecture
//!
//! - [`error`]: `Error` enum wrapping `diesel::result::Error` + `RolifyError`.
//! - [`store`]: `DieselStore` + `RoleStore`/`ResourceStore` impls (generic
//!   over the sync connection type; SC-5 executor pattern).
//! - [`sql`]: SQL template builders mirroring `role_adapter.rb:106-121`
//!   1:1 (D-06). One function per SPI member.
//! - [`dialect`]: per-backend `placeholder()` + `quote_identifier()` helpers.
//! - [`rows`]: `QueryableByName` row structs for by-name deserialization.
//! - [`sentinel`]: `None` ↔ `''` translation at the adapter boundary only.
//! - Public `MIGRATIONS: EmbeddedMigrations` per backend (D-11).
//!
//! ## Usage
//!
//! ```ignore
//! use diesel::prelude::*;
//! use rolify_diesel::{DieselStore, MIGRATIONS};
//! use rolify_core::config::RolifyConfig;
//!
//! let config = RolifyConfig::builder().build().unwrap();
//! let store = DieselStore::new(&config);
//! let mut conn = PgConnection::establish(&database_url)?;
//! MIGRATIONS.run_pending_migrations(&mut conn)?;
//! // ... use store with SPI methods
//! ```
//!
//! The store is generic over the connection type, so pool checkouts,
//! raw connections, and caller-owned transactions all work uniformly
//! (SC-5).

// Dual-mode note: this crate is sync-only in Phase 3.
// The `sync` feature forwards `rolify-core/is_sync`.
// Async support via `diesel-async` arrives in Phase 4 as the `async` feature.

// Mutual exclusion guards: exactly one backend must be enabled.
#[cfg(all(feature = "postgres", feature = "mysql"))]
compile_error!("features `postgres` and `mysql` are mutually exclusive");
#[cfg(all(feature = "postgres", feature = "sqlite"))]
compile_error!("features `postgres` and `sqlite` are mutually exclusive");
#[cfg(all(feature = "mysql", feature = "sqlite"))]
compile_error!("features `mysql` and `sqlite` are mutually exclusive");

// Backend features require the mode feature `sync` (which forwards rolify-core/is_sync).
#[cfg(all(feature = "postgres", not(feature = "sync")))]
compile_error!("feature `postgres` requires feature `sync`");
#[cfg(all(feature = "mysql", not(feature = "sync")))]
compile_error!("feature `mysql` requires feature `sync`");
#[cfg(all(feature = "sqlite", not(feature = "sync")))]
compile_error!("feature `sqlite` requires feature `sync`");

// Re-export the migrations constant for the enabled backend.
// Path is relative to the crate root (where Cargo.toml lives).
#[cfg(feature = "postgres")]
pub const MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("migrations/postgres");
#[cfg(feature = "mysql")]
pub const MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("migrations/mysql");
#[cfg(feature = "sqlite")]
pub const MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("migrations/sqlite");

pub mod error;
pub mod sentinel;

pub use error::Error;