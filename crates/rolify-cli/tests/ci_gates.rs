//! Pins the phase 07 QUAL-08 CI gates (D-20, D-21, D-23, SC-2, SC-3): a deleted
//! or renamed gate fails this test instead of silently dropping publish hardening.

use std::fs;

/// Reads the workspace CI workflow file.
fn read_ci_workflow() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/../../.github/workflows/ci.yml");
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read CI workflow: {path}"))
}

/// cargo-semver-checks runs check-release against every publishable crate (D-20 deny).
#[test]
fn ci_contains_semver_checks_deny_gate() {
    let workflow = read_ci_workflow();
    assert!(
        workflow.contains("semver-checks"),
        "ci.yml must contain the cargo-semver-checks deny job (D-20)"
    );
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

/// Every publishable crate carries a cargo package leg plus a publish dry-run leg (D-23).
#[test]
fn ci_gates_publish_with_dry_run() {
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
        let publish_leg = format!("cargo publish -p {crate_name} --dry-run");
        let package_leg = format!("cargo package -p {crate_name} --list");
        assert!(
            workflow.contains(&package_leg),
            "ci.yml must contain the file-inclusion leg `{package_leg}` (D-23)"
        );
        assert!(
            workflow.contains(&publish_leg),
            "ci.yml must contain the publish dry-run leg `{publish_leg}` (D-23)"
        );
    }
}
