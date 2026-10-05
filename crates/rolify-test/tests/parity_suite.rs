//! The D-12 `InMemoryStore` binding: the template Phases 3-5 copy per
//! adapter (one binding line per backend).
//!
//! Bindings:
//!
//! | Binding | Backend | Added by |
//! |---|---|---|
//! | `in_memory_default_user` | `InMemoryBackend<DefaultUser>` (default config) | 02-01 |
//! | `in_memory_strict_user` | `InMemoryBackend<StrictUserClass>` (`rolify strict: true`, active_record.rb:24-26) | 02-02; later strict-dependent modules (02-07 finders strict context) inherit this binding unchanged |
//!
//! ## Closure table (02-09)
//!
//! Every module below is a real expansion - no stub arm remains - and
//! the frozen macro binds all twelve for BOTH bindings above, so the
//! full suite is 121 wrappers per binding. Self-gated cases (the gem
//! runs some files only for `User`, others only for `StrictUser`) still
//! EXECUTE and PASS under both bindings: they early-return `Ok(())` in
//! the binding their rows do not own. Config-driven cases (the
//! callbacks rows and the `remove_role_if_empty(false)` keep-rows case)
//! run against their own fixed `InMemoryBackend` user classes in every
//! binding.
//!
//! | Module | Filled by | Wrappers | Strict-binding behavior |
//! |---|---|---|---|
//! | `add_role` | 02-01 | 15 | runs in full (writes are config-agnostic) |
//! | `has_role` | 02-02 | 28 | self-gated split (matrix = default binding, strict family = strict binding) |
//! | `remove_role` | 02-03 | 15 | runs in full (keep-rows case binds a fixed config) |
//! | `callbacks` | 02-03 | 7 | runs in full (fixed hook-config classes) |
//! | `scopes` | 02-04 | 4 | runs in full (zero-I/O snapshot narrows) |
//! | `roles` | 02-04 | 3 | runs in full (strict-safe rows only) |
//! | `resource_reads` | 02-05 | 9 | runs in full |
//! | `has_all_roles` | 02-06 | 7 | self-gated split |
//! | `has_any_role` | 02-06 | 4 | runs in full (no strict redirect, role.rb:69-75) |
//! | `only_has_role` | 02-06 | 10 | self-gated split |
//! | `finders` | 02-07 | 9 | self-gated split (strict-awareness rows = strict binding) |
//! | `resource_queries` | 02-08 | 10 | runs in full |
//!
//! Total: 121 wrappers per binding (15 + 28 + 15 + 7 + 4 + 3 + 9 + 7 +
//! 4 + 10 + 9 + 10). The `suite_is_fully_expanded` test proves the
//! structure (no stub arm, per-plan case-fn minimums met) by parsing
//! the module files directly; the phase-gate cargo runs prove the
//! behavior.
//!
//! ## Dynamic exclusion
//!
//! `shared_examples_for_dynamic` (147 lines, the `method_missing`
//! dynamic-shortcut anti-feature) intentionally has NO module: dynamic
//! shortcuts are REQUIREMENTS out-of-scope, and
//! `suite_is_fully_expanded` re-asserts the exclusion at test time
//! (no module file, no binding).
#![cfg(feature = "suite")]

use std::fs;
use std::path::Path;

rolify_test::parity_suite!(
    in_memory_default_user,
    rolify_test::backend::InMemoryBackend<rolify_test::fixtures::DefaultUser>
);
rolify_test::parity_suite!(
    in_memory_strict_user,
    rolify_test::backend::InMemoryBackend<rolify_test::fixtures::StrictUserClass>
);

/// The stub expansion arm from the 02-01 scaffolding: no filled module
/// may still contain it.
const STUB_ARM: &str = "=> {};";

/// The case-fn marker every module's generic cases carry
/// (`#[maybe_async]` keeps the `pub async fn` source shape in both
/// modes; only the compiled signature changes).
const CASE_FN_MARKER: &str = "pub async fn";

/// Per-module case-fn minimums, the plan-level pin behind the 121
/// wrapper total; helper fns only add to the counts, never subtract.
const MODULE_CASE_MINIMUMS: &[(&str, usize)] = &[
    ("add_role", 15),
    ("callbacks", 7),
    ("finders", 9),
    ("has_all_roles", 7),
    ("has_any_role", 4),
    ("has_role", 28),
    ("only_has_role", 10),
    ("query_guards", 2),
    ("remove_role", 15),
    ("resource_queries", 10),
    ("resource_reads", 9),
    ("roles", 3),
    ("scopes", 4),
];

/// The wrapper total the thirteen minimums sum to (15 + 28 + 15 + 7 + 4 +
/// 3 + 9 + 7 + 4 + 10 + 9 + 10 + 2): the structural half of the 02-09 gate
/// plus the 03-04 `query_guards` extension.
const WRAPPER_TOTAL_MINIMUM: usize = 123;

/// Structural closure proof for the 02-09/03-04 phase gate: every suite
/// module is a real expansion.
///
/// Parses the thirteen module FILES directly - a nested `cargo test`
/// from inside a cargo test binary would deadlock on the
/// build-directory lock, so this test never spawns cargo; the
/// behavioral green proof comes from the phase-gate cargo runs. Per
/// module it asserts: no stub expansion arm remains, and the file
/// declares at least its per-plan case-fn minimum. It also re-asserts
/// the dynamic exclusion: no `dynamic` module file, no
/// `parity_dynamic_cases!` binding. The per-module case-fn counts and
/// the 121-wrapper total are the structural half of the gate; the
/// per-binding wrapper macros expand exactly these case fns.
///
/// Plain sync by design (no store, no async): it reads source text
/// only, so it runs identically in both modes under the crate-level
/// `suite` feature gate.
#[test]
fn suite_is_fully_expanded() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let suite_dir = manifest_dir.join("src").join("suite");

    let mut total_found = 0;
    for (module, minimum) in MODULE_CASE_MINIMUMS {
        let module_path = suite_dir.join(format!("{module}.rs"));
        let source = fs::read_to_string(&module_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", module_path.display()));
        assert!(
            !source.contains(STUB_ARM),
            "{module} still carries a stub expansion arm ({STUB_ARM})"
        );
        let found = source.matches(CASE_FN_MARKER).count();
        assert!(
            found >= *minimum,
            "{module} declares {found} case fns, expected at least {minimum}"
        );
        total_found += found;
    }
    assert!(
        total_found >= WRAPPER_TOTAL_MINIMUM,
        "the thirteen modules declare {total_found} case fns in total, expected at least {WRAPPER_TOTAL_MINIMUM}"
    );

    let suite_rs_path = manifest_dir.join("src").join("suite.rs");
    let suite_source = fs::read_to_string(&suite_rs_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", suite_rs_path.display()));
    for (module, _) in MODULE_CASE_MINIMUMS {
        let binding = format!("parity_{module}_cases!");
        assert!(
            suite_source.contains(&binding),
            "the frozen macro is missing the {binding} binding"
        );
    }
    assert!(
        !suite_source.contains("parity_dynamic_cases"),
        "the dynamic anti-feature must stay excluded: no parity_dynamic_cases binding"
    );
    assert!(
        !suite_source.contains("pub mod dynamic;"),
        "the dynamic anti-feature must stay excluded: no dynamic module declaration"
    );
    assert!(
        !suite_dir.join("dynamic.rs").exists(),
        "src/suite/dynamic.rs must not exist: dynamic shortcuts are out of scope"
    );
}
