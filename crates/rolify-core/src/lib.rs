//! # rolify-core
//!
//! Core types, pure semantics kernel, and sealed adapter SPI for the Rust port
//! of the [rolify](https://github.com/RolifyCommunity/rolify) gem: scoped role
//! management (RBAC) **without** authorization enforcement. Roles attach to a
//! resource in three scopes (global, class/resource type, or instance), and
//! the gem's match ladder decides what satisfies a query.
//!
//! ## Mode matrix (sync/async duality)
//!
//! One source of truth for every trait signature, flipped by a single feature:
//!
//! | Mode | Cargo feature | SPI/consumer trait shape | Runtime |
//! |---|---|---|---|
//! | async (**default**) | *(none)* | `fn f(..) -> impl Future<Output = T> + Send` | tokio & friends |
//! | sync | `is_sync` | `fn f(..) -> T` | plain blocking calls |
//!
//! **One mode per build graph.** Features are additive and unify across a
//! single `cargo` invocation: if any crate in the graph enables
//! `rolify-core/is_sync`, the whole graph is sync. Do not mix a sync consumer
//! with an async adapter (or vice versa) in one binary: pick one mode per
//! application.
//!
//! The duality is implemented with [`maybe-async`](https://docs.rs/maybe-async)
//! `AFIT`: public traits are written in the desugared
//! `-> impl Future<Output = T> + Send` form (never bare `async fn`), and the
//! macro mechanically strips futures/awaits under `is_sync`. The pure kernel
//! ([`kernel`]) and all value types are mode-agnostic.
//!
//! ## Architecture
//!
//! - [`role`], [`query`], [`resource`]: value types (`RoleName`, `RoleRecord`,
//!   `RoleQuery`, `ResourceFilter`, `ResourceRef`, the `Resource` consumer trait).
//! - [`kernel`]: pure predicates implementing the gem's match ladder, a verbatim
//!   port of `role_adapter.rb` `build_query`/`find_cached`. No I/O anywhere.
//! - [`store`]: the soft-sealed `RoleStore` SPI that backend adapters implement
//!   (`rolify-diesel`, `rolify-sqlx`, `rolify-seaorm`, `rolify-mongodb`).
//! - [`user`]: the `RolifyUser` consumer trait with the gem's concern logic as
//!   provided methods.
//! - [`config`]: `RolifyConfig` builder (strict mode, `remove_role_if_empty`,
//!   callbacks), replacing the gem's global class variables.
//! - [`error`]: [`RolifyError`], the shared error enum.

pub mod config;
pub mod error;
pub mod kernel;
pub mod query;
pub mod resource;
pub mod role;
pub mod store;
pub mod user;

pub use error::RolifyError;
