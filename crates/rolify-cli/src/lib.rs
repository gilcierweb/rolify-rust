//! `rolify-cli` - the rolify migration generator.
//!
//! This crate provides the `rolify-cli` binary, which generates rolify migration
//! files for multiple backends (Diesel, Sqlx, `SeaORM`, `MongoDB`) with canonical
//! byte-identity to the reference adapter schemas.
//!
//! ## Architecture
//!
//! - **Compile-time embedding**: SQL templates are embedded via `include_str!`
//!   at build time - no runtime database access (D-13).
//! - **Pure render**: `render.rs` builds the file plan in memory; `writer.rs`
//!   handles all filesystem I/O with dry-run and force semantics (D-04, D-22).
//! - **Single renderer**: Diesel and Sqlx share the same SQL renderer
//!   (emitters/sql.rs) - D-11 identity by construction.
//! - **Longest-first replacement**: join table name replaced before roles table
//!   name to avoid partial replacement (D-14).
//! - **Validation at boundaries**: all interpolated names pass through
//!   `RolifyConfigBuilder::validate_identifier` before substitution (T-06-01).
//!
//! ## Gem generator parity
//!
//! Mirrors `lib/generators/rolify/templates` and `lib/rolify/generators/rolify_generator.rb`.
//! The CLI surface (`generate` with hidden `init` alias, required `--backend`,
//! positional `Role User`, `--out-dir`, `--force`, `--dry-run`, `--roles-table`,
//! `--join-table`) maps 1:1 to the gem's Rails generator options (D-01 through D-05).
//!
//! ## References
//!
//! - D-01: Primary verb is `generate` (gem: `rails g rolify`)
//! - D-02: Required `--backend` with values `diesel|sqlx|seaorm|mongodb`
//! - D-03: Positional `role_name holder_name` (gem: `Role User`)
//! - D-04: `--out-dir`, `--force`, `--dry-run`
//! - D-05: `--roles-table` default `roles`, `--join-table` default derived
//! - D-06: All engines emitted in one run
//! - D-08: `SQLite` emitted
//! - D-11: Single renderer for diesel/sqlx
//! - D-12: Up + down migrations
//! - D-13: Canonical home is rolify-diesel; CLI vendors + drift-guards
//! - D-14: Longest-first identifier replacement
//! - D-15: Drift test extends Phase 4 pattern to CLI
//! - D-18: Fresh-generate only (no regenerate/upgrade)
//! - D-19: Delivered timestamp stem `0000000001_rolify_create_tables`
//! - D-21: Tracer leg - generated Postgres applies + store smoke
//! - D-22: Mirrored trees `migrations/{engine}/{stem}/up.sql|down.sql`

pub mod args;
pub mod emitters;
pub mod error;
pub mod render;
pub mod templates;
pub mod writer;

pub use args::{Backend, Cli, Command, GenerateArgs};
pub use error::CliError;
pub use render::{FileEntry, RenderPlan, render_all};
pub use writer::{WriteOptions, write_plan};
