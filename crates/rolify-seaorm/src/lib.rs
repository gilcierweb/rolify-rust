//! # rolify-seaorm
//!
//! `SeaORM` 2.0 adapter for the rolify workspace: stores role rows and
//! answers scoped queries with the gem's precedence semantics. It stores
//! and queries role assignments; it never decides authorization
//! (enforcement stays in the consumer application, same boundary the gem
//! keeps).
//!
//! ## Feature matrix
//!
//! | Cargo features | Backend | Notes |
//! |---|---|---|
//! | (none) | none | Inert compile: the library always links both sqlx engines; engine features only gate containerized integration tests |
//! | `postgres` | `sqlx` Postgres via `SeaORM` | Container parity leg (`postgres:17`) |
//! | `mysql` | `sqlx` `MySQL` via `SeaORM` | Container parity leg (`mysql:8.4`) |
//! | `suite` | - | Developer-only: forwards `rolify-test/suite` for the ported `shared_examples` gate |
//!
//! **Async-only posture (D-05):** this crate declares no sync feature of
//! any name and never forwards `rolify-core/is_sync`; there is no sync
//! mode by construction (Phase 1 D-05 precedent).
//!
//! **Additive engines (unlike `rolify-diesel`):** `SeaORM` backends combine
//! in one build (`sqlx` features are additive), so no mutual-exclusion
//! `compile_error!` guards exist here; `postgres` and `mysql` may both be
//! enabled.
//!
//! **MSRV floor (D-10 parity):** `rust-version = "1.94"`; CI checks
//! `cargo +1.94 check -p rolify-seaorm --features postgres,mysql`.
//!
//! ## Parity divergences (locked in `05-CONTEXT.md`)
//!
//! - D-01: hybrid strategy - role-row CRUD and link writes use the static
//!   entities (`Entity` find/insert/delete); ONLY the three gem ladder
//!   branches live in raw SQL (statement builders), keeping executors
//!   `ConnectionTrait`-generic while the ladder stays auditable.
//! - D-06: entities expose `String` scope columns with the `''` sentinel;
//!   the `None` <-> `''` translation lives entirely at the adapter
//!   boundary and never leaks into the SPI or the suite.
//! - D-03: ladder-only raw cut - only the three gem ladder branches
//!   (`role_adapter.rb:106-121`) live in raw SQL; everything else uses
//!   SeaORM `Entity`/`QueryFilter`. Registry-driven finder reads
//!   (`holders_where`, `roles_matching`, `resources_find`) join consumer
//!   tables whose names arrive at runtime, so they stay raw statements by
//!   construction (same reading the diesel/sqlx adapters take).
//! - D-04: migrations use the native `sea-orm-migration` format (never
//!   vendored `.sql` copies), reproducing the canonical physical schema
//!   byte-faithfully (sentinel `''`, unique composite, composite indexes,
//!   FK `role_id` with cascade). Consequence: the Phase 6 CLI needs a
//!   dedicated SeaORM emitter (the canonical `.sql` files are not reused
//!   verbatim).
//! - D-05: the store is generic over `ConnectionTrait`, so pool,
//!   loose connection, and caller-owned transaction work uniformly
//!   (SC-5). The trait is not dyn-compatible; never `Box<dyn ConnectionTrait>`.
//!
//! **Complementary to the native `rbac` module:** `SeaORM` 2.0 ships an
//! opt-in `rbac` feature that is enforcement-oriented (one role per
//! holder, table-scoped CRUD permissions). rolify is multi-role,
//! instance-scoped, and enforcement-free, so this adapter is built on
//! plain entities and **never enables the `rbac` cargo feature**. A
//! bridge between the two models is post-v1 roadmap work (V2-02).
//!
//! ## Usage
//!
//! ```ignore
//! use sea_orm_migration::MigratorTrait;
//! use rolify_seaorm::Migrator;
//!
//! let conn = sea_orm::Database::connect(&database_url).await?;
//! // Run migrations explicitly - the crate NEVER migrates automatically (D-04/D-11 precedent).
//! Migrator::up(&conn, None).await?;
//! // ... SeaormStore lands in plan 05-03 and takes any `&impl ConnectionTrait`
//! ```

pub mod entity;
pub mod error;
pub mod ladder;
pub mod migration;
pub mod store;

pub use error::Error;
pub use migration::Migrator;
pub use store::SeaormStore;
