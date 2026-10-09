//! Pins the phase 07 QUAL-07 published docs: the PARITY.md ledger and the
//! coming-from-Ruby guide, so content regressions fail CI (D-16, D-17, D-19).

use std::fs;

/// Reads a workspace-root file relative to this crate's manifest dir.
fn read_workspace_file(name: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/../../{name}");
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read workspace file: {path}"))
}

/// The ledger cites the gem source files its divergences reference (D-19 completeness).
#[test]
fn parity_ledger_contains_gem_file_cites() {
    let ledger = read_workspace_file("PARITY.md");
    let gem_files = [
        "role_adapter.rb",
        "role.rb",
        "finders.rb",
        "resource.rb",
        "migration.rb",
    ];
    for gem_file in gem_files {
        assert!(
            ledger.contains(gem_file),
            "PARITY.md must cite {gem_file}: every deliberate divergence carries a gem file:line reference"
        );
    }
}

/// Every ledger entry names its pinning test or doc source (D-19 entry format).
#[test]
fn parity_ledger_entries_carry_pinning_tests() {
    let ledger = read_workspace_file("PARITY.md");
    let pin_count = ledger.matches("Pinned by").count();
    assert!(
        pin_count >= 15,
        "PARITY.md must carry at least 15 'Pinned by' pins, found {pin_count}"
    );
}

/// The review-flag convention stays declared: the legend line is greppable by
/// design, so this assertion survives the publish sweep clearing open flags (T-7-10).
#[test]
fn parity_ledger_declares_review_flag_convention() {
    let ledger = read_workspace_file("PARITY.md");
    let flag_count = ledger.matches("review flag").count();
    assert!(
        flag_count >= 1,
        "PARITY.md must declare the review-flag convention (legend plus open flags)"
    );
}

/// Published docs carry no em-dash characters (project ASCII convention).
#[test]
fn parity_ledger_is_ascii_only_no_em_dash() {
    let ledger = read_workspace_file("PARITY.md");
    assert!(
        !ledger.contains('\u{2014}'),
        "PARITY.md must not contain em-dash (U+2014)"
    );
}

/// The migration table maps the `RSpec` `have_role` matcher to `assert_has_role` (D-03, D-17).
#[test]
fn coming_from_ruby_maps_have_role_to_assert_has_role() {
    let guide = read_workspace_file("COMING-FROM-RUBY.md");
    assert!(
        guide.contains("assert_has_role"),
        "COMING-FROM-RUBY.md must map have_role to assert_has_role in the alias migration table"
    );
}

/// The workspace README points to the ledger and the guide (D-16).
#[test]
fn readme_points_to_parity_ledger_and_guide() {
    let readme = read_workspace_file("README.md");
    assert!(
        readme.contains("PARITY.md"),
        "README.md must point to PARITY.md"
    );
    assert!(
        readme.contains("COMING-FROM-RUBY.md"),
        "README.md must point to COMING-FROM-RUBY.md"
    );
}
