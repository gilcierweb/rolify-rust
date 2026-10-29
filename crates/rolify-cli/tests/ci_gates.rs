//! Pins the phase 07 QUAL-08 CI gates (D-20, D-21, D-23, SC-2, SC-3): a deleted
//! or renamed gate fails this test instead of silently dropping publish hardening.
//!
//! Current CI status:
//! - semver-checks (D-20): REMOVED — crates not yet published to crates.io (baseline missing).
//!   Will be re-added when 0.1.0 publishes.
//! - publish dry-run (D-23): REMOVED — crates not yet published; only `cargo package --list`
//!   runs for file-inclusion proof. Will be re-added when 0.1.0 publishes.

use std::fs;

/// Reads the workspace CI workflow file.
fn read_ci_workflow() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/../../.github/workflows/ci.yml");
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read CI workflow: {path}"))
}

/// Every published rust-version floor has a dedicated msrv leg (D-21).
#[test]
fn ci_covers_every_msrv_floor() {
    let workflow = read_ci_workflow();
    let floors = ["1.85", "1.86", "1.88", "1.94"];
    for floor in floors {
        assert!(
            workflow.contains(floor),
            "ci.yml must carry an msrv leg for the {floor} toolchain floor (D-21)"
        );
    }
}

/// A docs job replicates docs.rs flags so metadata drift fails CI (SC-2).
#[test]
fn ci_replicates_docsrs_build() {
    let workflow = read_ci_workflow();
    assert!(
        workflow.contains("docsrs"),
        "ci.yml must contain the docsrs-replicating doc job (SC-2)"
    );
}

/// The rolify-test feature matrix runs default, suite, faker, all-features legs (SC-3).
#[test]
fn ci_runs_feature_matrix() {
    let workflow = read_ci_workflow();
    assert!(
        workflow.contains("feature-matrix"),
        "ci.yml must contain the rolify-test feature-matrix job (SC-3)"
    );
}

/// File-inclusion proof runs via `cargo package --list` (D-23).
/// Publish dry-run and semver-checks are temporarily removed until
/// crates publish to crates.io (first 0.1.0 release).
#[test]
fn ci_gates_package_file_inclusion() {
    let workflow = read_ci_workflow();
    let crates = [
        "rolify-core",
        "rolify-test",
        "rolify-cli",
        "rolify-diesel",
        "rolify-sqlx",
        "rolify-seaorm",
        "rolify-mongodb",
    ];
    for crate_name in crates {
        let package_leg = format!("cargo package -p {crate_name} --list");
        assert!(
            workflow.contains(&package_leg),
            "ci.yml must contain the file-inclusion leg `{package_leg}` (D-23)"
        );
    }
    // Verify publish dry-run and semver-checks are NOT present (intentionally removed)
    assert!(
        !workflow.contains("cargo publish --dry-run"),
        "ci.yml must NOT contain publish dry-run (crates not yet published)"
    );
    assert!(
        !workflow.contains("semver-checks"),
        "ci.yml must NOT contain semver-checks job (crates not yet published)"
    );
}
