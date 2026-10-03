//! The ported parity suite: generic case functions plus the frozen
//! [`parity_suite!`] binding macro (D-12, D-14).
//!
//! ## Binding contract
//!
//! One `parity_suite!` line per backend - the port of the gem's
//! `it_behaves_like`. The case list lives inside `rolify-test`, so consumer
//! code cannot drift: adding cases happens in the owning module file, never
//! at the binding site. The macro body is FROZEN in 02-01: later plans add
//! cases only inside their own module file's macro; the 12-binding list
//! never changes.
//!
//! ## Phase-3 consumer note
//!
//! The generated wrappers use the dual-mode `maybe_async::test` +
//! `tokio::test` attribute, so adapter test targets must have `maybe-async`
//! and `tokio` available as dev-dependencies, plus an `is_sync` feature
//! with the workspace's forwarding semantics. The binding file to copy lives
//! at `rolify-test/tests/parity_suite.rs`.
//!
//! ## Closure map (02-09)
//!
//! All twelve modules below are filled by plans 02-01..02-08 - no stub
//! expansion point remains, and the phase gate proves the full suite
//! green against `InMemoryStore` in both modes:
//!
//! | Module | Filled by |
//! |---|---|
//! | `add_role` | 02-01 |
//! | `callbacks` | 02-03 |
//! | `finders` | 02-07 |
//! | `has_all_roles` | 02-06 |
//! | `has_any_role` | 02-06 |
//! | `has_role` | 02-02 |
//! | `only_has_role` | 02-06 |
//! | `query_guards` | 03-04 |
//! | `remove_role` | 02-03 |
//! | `resource_queries` | 02-08 |
//! | `resource_reads` | 02-05 |
//! | `roles` | 02-04 |
//! | `scopes` | 02-04 |
//!
//! ## Dynamic exclusion
//!
//! `shared_examples_for_dynamic` (147 lines, the `method_missing`
//! dynamic-shortcut anti-feature) intentionally has NO module here:
//! dynamic shortcuts are REQUIREMENTS out-of-scope (Rust has no
//! `method_missing`; `has_role` is the documented idiom). The
//! twelve-module list is closed, and `suite_is_fully_expanded` in
//! `tests/parity_suite.rs` re-asserts the exclusion at test time.

pub mod add_role;
pub mod callbacks;
pub mod finders;
pub mod has_all_roles;
pub mod has_any_role;
pub mod has_role;
pub mod only_has_role;
pub mod query_guards;
pub mod remove_role;
pub mod resource_queries;
pub mod resource_reads;
pub mod roles;
pub mod scopes;

/// Expand the full 12-module case suite for one backend (D-12).
///
/// `parity_suite!(BackendType)` binds under the default module name
/// `parity_bound`; `parity_suite!(binding_name, BackendType)` binds under
/// the given module name. The 12 bindings follow the outline order: the 10
/// portable `shared_examples` files, then `resource_reads` and
/// `resource_queries` (the Suite-Extension note).
#[macro_export]
macro_rules! parity_suite {
    ($backend:ty) => {
        $crate::parity_suite!(parity_bound, $backend);
    };
    ($binding:ident, $backend:ty) => {
        mod $binding {
            $crate::parity_add_role_cases!($backend);
            $crate::parity_callbacks_cases!($backend);
            $crate::parity_finders_cases!($backend);
            $crate::parity_has_all_roles_cases!($backend);
            $crate::parity_has_any_role_cases!($backend);
            $crate::parity_has_role_cases!($backend);
            $crate::parity_only_has_role_cases!($backend);
            $crate::parity_query_guards_cases!($backend);
            $crate::parity_remove_role_cases!($backend);
            $crate::parity_roles_cases!($backend);
            $crate::parity_scopes_cases!($backend);
            $crate::parity_resource_reads_cases!($backend);
            $crate::parity_resource_queries_cases!($backend);
        }
    };
}
