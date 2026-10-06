//! # rolify-mongodb
//!
//! `MongoDB` adapter for `rolify-core`, a port of the gem Mongoid adapter
//! the official `mongodb` driver (3.9.1, MSRV 1.88). Roles live as
//! documents (`roles` collection by default) with the gem's explicit-null
//! scope and two-sided HABTM links; the adapter only stores them and
//! answers scoped queries - it NEVER decides authorization (no allow/deny
//! enforcement; ADPT-06 boundary).
//!
//! ## Feature matrix (D-11, D-12)
//!
//! | Feature | Default | Effect |
//! |---------|---------|--------|
//! | *(none)* | on | async-first store over `mongodb` (tokio runtime) |
//! | `sync` | off | same `MongoStore` type over `mongodb::sync`; forwards `rolify-core/is_sync` |
//! | `serde` | off | `serde` derives on the document structs (QUAL-03) |
//! | `suite` | off (dev) | forwards `rolify-test/suite` for the shared parity suite |
//!
//! The `sync` mode mirrors the driver's own `mongodb::sync` module: tokio
//! stays linked, work is offloaded onto the driver's internal runtime
//! threads, and library code never calls `block_on` (RESEARCH
//! Anti-Patterns).
//!
//! ## Parity divergences (vs the SQL adapters)
//!
//! - Absent scope persists as **explicit null**, never the `''` sentinel
//!   (D-07; the gem's Mongoid adapter writes `nil`).
//! - Two-sided HABTM in documents: role docs carry `user_ids`, consumer
//!   docs gain `role_ids`, with a fixed write order and emptiness checked
//!   AFTER removal on the authoritative side (D-08).
//! - Store handle is `Database` plus validated collection names (D-16).
//! - Concurrent `find_or_create` is arbitrated by the unique compound
//!   index plus a duplicate-key (Mongo code `11000`) catch-and-re-read
//!   (D-14) - no multi-doc transactions.
//! - Idempotent ensure-index at store setup replaces versioned migrations
//!   (D-10); the CLI (Phase 6) only documents it.
//!
//! ## Architecture
//!
//! - [`document`]: `RoleDoc` / `HolderLinkDoc` BSON shapes; scope fields are
//!   always written (explicit null when absent).
//! - [`error`]: the crate's error currency ([`Error`]).
//! - [`collection`]: the D-12 type-level seam over the driver's async/sync
//!   `Collection`/`Database` trees.
//! - [`index`]: `create_index` ensure for the unique compound.
//! - [`store`]: [`MongoStore`] - the dual-mode store shell carrying the
//!   validated handle; the full SPI lands on it in 05.1-02.
//!
//! ## Sync mode (D-11 shipped now, D-12 same-store shape)
//!
//! Enable with `features = ["sync"]`. The same `MongoStore` type then
//! holds `mongodb::sync` handles and every SPI method runs synchronously:
//! the driver offloads onto its own internal runtime, tokio stays linked,
//! and this crate never calls `block_on` nor spawns a runtime.
//!
//! ```text
//! let client = mongodb::sync::Client::with_uri_str("mongodb://...")?;
//! let database = client.database("app");
//! let store = MongoStore::new(&database, &RolifyConfig::default())
//!     .for_holder_collection("users");
//! store.ensure_indexes()?;            // sync in sync builds
//! ```

pub mod collection;
pub mod document;
mod error;
pub mod index;
mod store;

pub use document::{HolderLinkDoc, ObjectId, RoleDoc};
pub use error::{Error, is_duplicate_key};
pub use store::MongoStore;
