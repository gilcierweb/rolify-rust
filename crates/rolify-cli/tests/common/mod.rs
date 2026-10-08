//! Shared helpers for the rolify-cli integration test binaries (IN-04).
//!
//! Each file under `tests/` compiles as its own crate, so helpers used by
//! more than one integration test live here exactly once and are pulled in
//! with `mod common;`.

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;

/// Path to the rolify-cli binary.
pub fn rolify_cli() -> Command {
    Command::cargo_bin("rolify-cli").unwrap()
}

/// Creates a temporary directory in a safe location to avoid /tmp quota issues.
pub fn test_temp_dir() -> PathBuf {
    // Use a subdirectory of the project's target dir to avoid /tmp quota issues
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-workspace");
    fs::create_dir_all(&base).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("rolify-cli-test-")
        .tempdir_in(&base)
        .unwrap();
    dir.keep()
}
