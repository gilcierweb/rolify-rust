use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;

/// Path to the rolify-cli binary.
fn rolify_cli() -> Command {
    Command::cargo_bin("rolify-cli").unwrap()
}

/// Creates a temporary directory in a safe location to avoid /tmp quota issues.
fn test_temp_dir() -> PathBuf {
    // Use a subdirectory of the project's target dir to avoid /tmp quota issues
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-workspace");
    fs::create_dir_all(&base).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("rolify-cli-test-")
        .tempdir_in(&base)
        .unwrap();
    dir.keep()
}

#[test]
fn help_lists_generate() {
    rolify_cli()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("generate"));
}

#[test]
fn version_prints() {
    rolify_cli()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("0.1.0"));
}

#[test]
fn missing_backend_fails() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args(["generate", "Role", "User", "--out-dir", out_dir])
        .assert()
        .failure()
        .stderr(predicate::str::contains("required"));
}

#[test]
fn generate_accepts_positional_role_user() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Verify the mirrored tree structure
    assert!(
        dir.join("migrations/postgres/0000000001_rolify_create_tables/up.sql")
            .exists()
    );
    assert!(
        dir.join("migrations/postgres/0000000001_rolify_create_tables/down.sql")
            .exists()
    );
    assert!(
        dir.join("migrations/mysql/0000000001_rolify_create_tables/up.sql")
            .exists()
    );
    assert!(
        dir.join("migrations/mysql/0000000001_rolify_create_tables/down.sql")
            .exists()
    );
    assert!(
        dir.join("migrations/sqlite/0000000001_rolify_create_tables/up.sql")
            .exists()
    );
    assert!(
        dir.join("migrations/sqlite/0000000001_rolify_create_tables/down.sql")
            .exists()
    );
}

#[test]
fn dry_run_creates_no_files() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("DRY RUN"));

    // Verify no files were created
    assert!(
        !dir.join("migrations").exists(),
        "dry-run should not create directories"
    );
}

#[test]
fn second_run_without_force_fails_naming_collision() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    // First run
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Second run without --force should fail
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"))
        .stderr(predicate::str::contains("up.sql"));
}

#[test]
fn force_overwrites_existing() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    // First run
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Second run with --force should succeed
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
            "--force",
        ])
        .assert()
        .success();
}

#[test]
fn init_alias_produces_identical_tree() {
    let dir1 = test_temp_dir();
    let dir2 = test_temp_dir();
    let out_dir1 = dir1.to_str().unwrap();
    let out_dir2 = dir2.to_str().unwrap();

    // Generate with 'generate'
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir1,
        ])
        .assert()
        .success();

    // Generate with 'init' alias
    rolify_cli()
        .args([
            "init",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir2,
        ])
        .assert()
        .success();

    // Compare trees recursively - they should be identical
    fn collect_files(dir: &PathBuf) -> Vec<(PathBuf, String)> {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(dir.join("migrations"))
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let path = entry.path().to_path_buf();
                let content = fs::read_to_string(&path).unwrap();
                files.push((path.strip_prefix(dir).unwrap().to_path_buf(), content));
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    let files1 = collect_files(&dir1);
    let files2 = collect_files(&dir2);

    assert_eq!(files1.len(), files2.len(), "file count mismatch");
    for ((p1, c1), (p2, c2)) in files1.iter().zip(files2.iter()) {
        assert_eq!(p1, p2, "path mismatch");
        assert_eq!(c1, c2, "content mismatch for {:?}", p1);
    }
}

#[test]
fn diesel_tree_contains_all_engines() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Verify all three engines are emitted in one run (D-06)
    assert!(dir.join("migrations/postgres").exists());
    assert!(dir.join("migrations/mysql").exists());
    assert!(dir.join("migrations/sqlite").exists());

    // Verify stem and files (D-19, D-22)
    let stem = "0000000001_rolify_create_tables";
    for engine in ["postgres", "mysql", "sqlite"] {
        assert!(dir.join(format!("migrations/{}/{}", engine, stem)).exists());
        assert!(
            dir.join(format!("migrations/{}/{}/up.sql", engine, stem))
                .exists()
        );
        assert!(
            dir.join(format!("migrations/{}/{}/down.sql", engine, stem))
                .exists()
        );
    }

    // Verify down.sql order: join first then roles (D-12)
    for engine in ["postgres", "mysql", "sqlite"] {
        let down = fs::read_to_string(dir.join(format!("migrations/{}/{}/down.sql", engine, stem)))
            .unwrap();
        // Find the DROP TABLE statements specifically
        let join_drop = down
            .find("DROP TABLE IF EXISTS users_roles")
            .unwrap_or(usize::MAX);
        let roles_drop = down
            .find("DROP TABLE IF EXISTS roles")
            .unwrap_or(usize::MAX);
        // join table should be dropped before roles table
        assert!(
            join_drop < roles_drop,
            "down.sql should drop join table first for {}",
            engine
        );
    }
}
