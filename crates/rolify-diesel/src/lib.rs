//! # rolify-diesel
//!
//! Diesel sync reference adapter for the rolify workspace.
//!
//! ## Feature matrix
//!
//! | Mode | Cargo features | Backend | Notes |
//! |---|---|---|---|
//! | sync (default) | (none, or `sync`) | engine feature required | On by default (just-works posture, D-08); forwards `rolify-core/is_sync` via `is_sync` |
//! | sync + Postgres | `sync,postgres` | `diesel::pg::PgConnection` | Pulls `diesel_migrations` |
//! | sync + `MySQL` | `sync,mysql` | `diesel::mysql::MysqlConnection` | Pulls `diesel_migrations` |
//! | sync + `SQLite` | `sync,sqlite` | `diesel::sqlite::SqliteConnection` | Pulls `diesel_migrations` + bundled `libsqlite3-sys` |
//! | async | `async` | engine feature required | Opt-in: `default-features = false` (D-08); pulls `diesel-async` 0.9 |
//! | async + Postgres | `async,postgres` | `diesel_async::AsyncPgConnection` | Same `DieselStore` type, same SQL templates, `diesel_async::RunQueryDsl` execution |
//! | async + bb8 pool | `async,bb8` | - | Mirrors diesel-async's own `bb8` feature (D-09); the canonical gate pool |
//! | async + deadpool pool | `async,deadpool` | - | Mirrors diesel-async's own `deadpool` feature (D-09); compile-check + smoke only (D-09) |
//! | async + Postgres + bb8 | `async,bb8,postgres` | `bb8::Pool<AsyncPgConnection>` | Tracer path (this phase): full grant/check/revoke on real Postgres |
//!
//! **Mutual exclusion:** `sync` and `async` are mutually exclusive (feature
//! unification would put the core sync flag and diesel-async in one graph;
//! Phase 1 D-05, Phase 4 D-08). Exactly one of `postgres`, `mysql`,
//! `sqlite` may be enabled per build. Engine features are mode-agnostic
//! (D-08): an engine feature without either mode fails to compile with a
//! guarded message, so `default-features = false, features = ["async",
//! "postgres"]` is expressible. The `compile_error!` guards below enforce
//! all of this.
//!
//! **MSRV floor (D-10):** `rust-version = "1.86"` in BOTH modes (diesel-async
//! 0.9 declares 1.84, effectively 1.86 via diesel ~2.3.9); CI checks the
//! async leg with `cargo +1.86 check -p rolify-diesel --no-default-features
//! --features async,bb8,postgres`.
//!
//! **Workspace gate note (Pitfall 11):** with `default = ["sync"]`, every
//! bare workspace build unifies `rolify-core/is_sync` ON for the whole
//! graph, so the bare `cargo test --workspace` exercises the sync mode
//! only (async-only members do not compile under a unified sync graph;
//! see `rolify-core`'s mode-matrix warning). The dual-mode convention
//! therefore splits: the sync truth comes from the per-package legs
//! (`cargo test -p rolify-diesel --no-default-features --features
//! sync,<engine>[,suite]`), and the async-mode workspace gate is
//! `cargo test --workspace --exclude rolify-diesel` plus per-package
//! async feature legs (04-09 bakes this into CI).
//!
//! **Default features = `["sync"]`:** `cargo check -p rolify-diesel` (no
//! features) compiles the sync stub (inert without an engine) and never
//! pulls diesel-async into a consumer graph by default.
//!
//! ## Parity
//!
//! Behavioral parity with the gem's `ActiveRecord` adapter is tracked in the
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
//! use diesel_migrations::MigrationHarness;
//! use rolify_diesel::{DieselStore, MIGRATIONS};
//! use rolify_core::config::RolifyConfig;
//!
//! let config = RolifyConfig::builder().build().unwrap();
//! let store = DieselStore::new(&config);
//! let mut conn = PgConnection::establish(&database_url)?;
//! // Run embedded migrations explicitly — the crate NEVER migrates automatically (D-11).
//! conn.run_pending_migrations(MIGRATIONS)?;
//! // ... use store with SPI methods
//! ```
//!
//! **Local prerequisites:** Building with the `postgres` or `mysql` features
//! requires the respective client development headers (`libpq-dev` and
//! `libmysqlclient-dev` on Debian/Ubuntu). The `sqlite` feature uses the
//! bundled `libsqlite3-sys` and has no external dependencies.
//!
//! The store is generic over the connection type, so pool checkouts,
//! raw connections, and caller-owned transactions all work uniformly
//! (SC-5).

// Dual-mode note (D-08): the `sync` feature (default) forwards
// `rolify-core/is_sync` through `is_sync`; the `async` feature (opt-in,
// `default-features = false`) rides diesel-async over the SAME DieselStore
// type and the SAME SQL templates (Phase 3 D-06 mirrored execution).

// Mode mutual exclusion (Phase 1 D-05, Phase 4 D-08): feature unification
// must never put the core sync flag and diesel-async in one graph.
#[cfg(all(feature = "sync", feature = "async"))]
compile_error!(
    "features `sync` and `async` are mutually exclusive: feature unification would put `rolify-core/is_sync` and `diesel-async` in one build graph (Phase 1 D-05, Phase 4 D-08)"
);

// Engine mutual exclusion: exactly one backend must be enabled.
#[cfg(all(feature = "postgres", feature = "mysql"))]
compile_error!("features `postgres` and `mysql` are mutually exclusive");
#[cfg(all(feature = "postgres", feature = "sqlite"))]
compile_error!("features `postgres` and `sqlite` are mutually exclusive");
#[cfg(all(feature = "mysql", feature = "sqlite"))]
compile_error!("features `mysql` and `sqlite` are mutually exclusive");

// Mode-agnostic engine guards (D-08): an engine feature requires exactly
// one of the two mode features (`sync` or `async`).
#[cfg(all(feature = "postgres", not(any(feature = "sync", feature = "async"))))]
compile_error!("feature `postgres` requires one of the mode features `sync` or `async`");
#[cfg(all(feature = "mysql", not(any(feature = "sync", feature = "async"))))]
compile_error!("feature `mysql` requires one of the mode features `sync` or `async`");
#[cfg(all(feature = "sqlite", not(any(feature = "sync", feature = "async"))))]
compile_error!("feature `sqlite` requires one of the mode features `sync` or `async`");

// Re-export the migrations constant for the enabled backend.
// Path is relative to the crate root (where Cargo.toml lives).
// The integer kind tree is the default embedded migration (D-08-02);
// the other kind trees exist for the CLI drift guard (Plan 05).
#[cfg(feature = "postgres")]
pub const MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("migrations/postgres/integer");
#[cfg(feature = "mysql")]
pub const MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("migrations/mysql/integer");
#[cfg(feature = "sqlite")]
pub const MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("migrations/sqlite/integer");

pub mod dialect;
pub mod error;
pub mod holder;
pub mod rows;
pub mod sentinel;
pub mod sql;
pub mod store;

pub use error::Error;
pub use store::DieselStore;
