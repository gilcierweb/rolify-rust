use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use walkdir::WalkDir;

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

/// Tests that --dry-run previews an existing tree instead of failing with
/// the collision error (WR-04): a preview is most useful exactly when the
/// target tree already exists, and the documented contract says
/// print-then-exit, not fail.
#[test]
fn dry_run_over_existing_tree_previews() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    // First run: a real write that creates the tree.
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

    // Second run with --dry-run (no --force) over the existing tree:
    // exits 0, prints the plan, and the collision error must not fire.
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
        .stdout(predicate::str::contains("DRY RUN"))
        .stdout(predicate::str::contains("migrations/postgres"))
        .stderr(predicate::str::contains("already exists").not());
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

    #[allow(clippy::items_after_statements)] // local helper co-located for readability
    fn collect_files(dir: &PathBuf) -> Vec<(PathBuf, String)> {
        let mut files = Vec::new();
        for entry in WalkDir::new(dir.join("migrations"))
            .into_iter()
            .filter_map(std::result::Result::ok)
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
        assert_eq!(c1, c2, "content mismatch for {p1:?}");
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
        assert!(dir.join(format!("migrations/{engine}/{stem}")).exists());
        assert!(
            dir.join(format!("migrations/{engine}/{stem}/up.sql"))
                .exists()
        );
        assert!(
            dir.join(format!("migrations/{engine}/{stem}/down.sql"))
                .exists()
        );
    }

    // Verify down.sql order: join first then roles (D-12)
    for engine in ["postgres", "mysql", "sqlite"] {
        let down =
            fs::read_to_string(dir.join(format!("migrations/{engine}/{stem}/down.sql"))).unwrap();
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
            "down.sql should drop join table first for {engine}"
        );
    }
}

/// Tests that scaffolding files are generated for diesel backend.
#[test]
fn diesel_generates_scaffolding_files() {
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

    // Verify scaffolding files for each engine
    for engine in ["postgres", "mysql", "sqlite"] {
        assert!(
            dir.join(format!("{engine}/role_stub.rs")).exists(),
            "role_stub.rs missing for {engine}"
        );
        assert!(
            dir.join(format!("{engine}/holder_stub.rs")).exists(),
            "holder_stub.rs missing for {engine}"
        );
        assert!(
            dir.join(format!("{engine}/config_example.rs")).exists(),
            "config_example.rs missing for {engine}"
        );
        assert!(
            dir.join(format!("{engine}/README.md")).exists(),
            "README.md missing for {engine}"
        );
    }
}

/// Tests that scaffolding files are generated for seaorm backend.
#[test]
fn seaorm_generates_scaffolding_files() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "seaorm",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Verify scaffolding files for seaorm
    assert!(dir.join("seaorm/role_stub.rs").exists());
    assert!(dir.join("seaorm/holder_stub.rs").exists());
    assert!(dir.join("seaorm/config_example.rs").exists());
    assert!(dir.join("seaorm/README.md").exists());
}

/// Tests that scaffolding files are generated for mongodb backend.
#[test]
fn mongodb_generates_scaffolding_files() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "mongodb",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Verify scaffolding files for mongo
    assert!(dir.join("mongo/role_stub.rs").exists());
    assert!(dir.join("mongo/holder_stub.rs").exists());
    assert!(dir.join("mongo/config_example.rs").exists());
    assert!(dir.join("mongo/README.md").exists());
    assert!(dir.join("mongo/INDEX_NOTES.md").exists());
}

/// Tests that custom join table flows into all emitters (D-05 friends).
#[test]
fn custom_join_table_flows_into_all_outputs() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Privilege",
            "Customer",
            "--roles-table",
            "privileges",
            "--join-table",
            "customers_privileges",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // Verify SQL files have custom names
    for engine in ["postgres", "mysql", "sqlite"] {
        let up_sql = fs::read_to_string(dir.join(format!(
            "migrations/{engine}/0000000001_rolify_create_tables/up.sql"
        )))
        .unwrap();

        assert!(
            up_sql.contains("privileges"),
            "roles table name missing in {engine} up.sql"
        );
        assert!(
            up_sql.contains("customers_privileges"),
            "join table name missing in {engine} up.sql"
        );
        assert!(
            !up_sql.contains("users_roles"),
            "default join table should not appear in {engine} up.sql"
        );
    }

    // Verify scaffolding config_example has custom names
    for engine in ["postgres", "mysql", "sqlite"] {
        let config = fs::read_to_string(dir.join(format!("{engine}/config_example.rs"))).unwrap();
        assert!(
            config.contains("privileges"),
            "config missing custom roles table for {engine}"
        );
        assert!(
            config.contains("customers_privileges"),
            "config missing custom join table for {engine}"
        );
    }
}

/// Tests derived join default for Role User equals `users_roles`.
#[test]
fn derived_join_default_for_role_user() {
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

    // Verify default join table is users_roles
    for engine in ["postgres", "mysql", "sqlite"] {
        let up_sql = fs::read_to_string(dir.join(format!(
            "migrations/{engine}/0000000001_rolify_create_tables/up.sql"
        )))
        .unwrap();

        assert!(
            up_sql.contains("users_roles"),
            "default join table should be users_roles for {engine}"
        );
    }
}

/// Tests init alias output diff against generate output exits 0.
#[test]
fn init_alias_output_identity() {
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

    #[allow(clippy::items_after_statements)] // local helper co-located for readability
    fn collect_all_files(dir: &PathBuf) -> Vec<(PathBuf, String)> {
        let mut files = Vec::new();
        for entry in WalkDir::new(dir)
            .into_iter()
            .filter_map(std::result::Result::ok)
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

    let files1 = collect_all_files(&dir1);
    let files2 = collect_all_files(&dir2);

    assert_eq!(files1.len(), files2.len(), "file count mismatch");
    for ((p1, c1), (p2, c2)) in files1.iter().zip(files2.iter()) {
        assert_eq!(p1, p2, "path mismatch: {p1:?} vs {p2:?}");
        assert_eq!(c1, c2, "content mismatch for {p1:?}");
    }
}

/// Tests that dry-run prints file plan to stdout with out-dir absent.
#[test]
fn dry_run_prints_plan_and_no_files() {
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
        .stdout(predicate::str::contains("DRY RUN"))
        .stdout(predicate::str::contains("migrations/postgres"))
        .stdout(predicate::str::contains("migrations/mysql"))
        .stdout(predicate::str::contains("migrations/sqlite"));

    // Verify no files were created
    assert!(
        !dir.join("migrations").exists(),
        "dry-run should not create directories"
    );
    assert!(
        !dir.join("postgres").exists(),
        "dry-run should not create scaffolding dirs"
    );
}

/// Tests that help text stays English ASCII with no em-dash sequence.
#[test]
fn help_text_is_ascii_only() {
    let output = rolify_cli()
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let help_text = String::from_utf8(output).unwrap();

    // Verify no em-dash (U+2014) or en-dash (U+2013)
    assert!(
        !help_text.contains('\u{2014}'),
        "help text contains em-dash"
    );
    assert!(
        !help_text.contains('\u{2013}'),
        "help text contains en-dash"
    );

    // Verify ASCII only
    assert!(
        help_text.is_ascii(),
        "help text contains non-ASCII characters"
    );
}

/// Tests that namespaced input without explicit flags still passes `validate_identifier` or fails with naming error.
#[test]
fn namespaced_input_validation() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    // Namespaced role name should fail validation (contains ::)
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Admin::Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
}

/// Tests that generate --backend sqlx output tree diffs byte-identical
/// against generate --backend diesel output tree, proving D-11 strict identity
/// with no second checked-in tree.
#[test]
fn sqlx_output_matches_diesel_tree() {
    let diesel_dir = test_temp_dir();
    let sqlx_dir = test_temp_dir();
    let diesel_out = diesel_dir.to_str().unwrap();
    let sqlx_out = sqlx_dir.to_str().unwrap();

    // Generate with diesel backend
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--out-dir",
            diesel_out,
        ])
        .assert()
        .success();

    // Generate with sqlx backend
    rolify_cli()
        .args([
            "generate",
            "--backend",
            "sqlx",
            "Role",
            "User",
            "--out-dir",
            sqlx_out,
        ])
        .assert()
        .success();

    // Recursively diff the migration trees: they must be byte-identical (D-11).
    // Scaffolding files (README, stubs, config examples) legitimately carry the
    // backend name and are out of D-11 scope: the identity claim covers the SQL
    // migration bytes only.
    #[allow(clippy::items_after_statements)] // local helper co-located for readability
    fn collect_migration_files(dir: &PathBuf) -> Vec<(PathBuf, String)> {
        let mut files = Vec::new();
        for entry in WalkDir::new(dir.join("migrations"))
            .into_iter()
            .filter_map(std::result::Result::ok)
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

    let diesel_files = collect_migration_files(&diesel_dir);
    let sqlx_files = collect_migration_files(&sqlx_dir);

    assert_eq!(
        diesel_files.len(),
        sqlx_files.len(),
        "diesel and sqlx trees must have the same file count"
    );

    for ((diesel_path, diesel_content), (sqlx_path, sqlx_content)) in
        diesel_files.iter().zip(sqlx_files.iter())
    {
        assert_eq!(
            diesel_path, sqlx_path,
            "path mismatch: {diesel_path:?} vs {sqlx_path:?}"
        );
        assert_eq!(
            diesel_content, sqlx_content,
            "content mismatch for {diesel_path:?}: D-11 strict identity violated"
        );
    }
}

/// Tests that an --backend sqlx run ships the sqlx README into every engine
/// directory: the consumption story keys on the backend the user chose, not
/// on the engine directory the file lands in (CR-02, D-02, D-20).
#[test]
fn sqlx_run_emits_sqlx_readme() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "sqlx",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    for engine in ["postgres", "mysql", "sqlite"] {
        let readme = fs::read_to_string(dir.join(format!("{engine}/README.md"))).unwrap();

        assert!(
            readme.contains("sqlx"),
            "sqlx consumption story missing from {engine}/README.md"
        );
        assert!(
            readme.contains("Migration::new"),
            "programmatic Migrator construction missing from {engine}/README.md"
        );
        assert!(
            !readme.contains("diesel migration run"),
            "diesel runner instructions must not reach sqlx consumers ({engine}/README.md)"
        );
        assert!(
            readme.contains("generate --backend sqlx"),
            "generated-by header must name the sqlx backend in {engine}/README.md"
        );
    }
}

/// Tests that a diesel run keeps the diesel consumption story: the backend
/// re-keying must not displace the diesel document for diesel consumers.
#[test]
fn diesel_run_keeps_diesel_readme() {
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

    for engine in ["postgres", "mysql", "sqlite"] {
        let readme = fs::read_to_string(dir.join(format!("{engine}/README.md"))).unwrap();
        assert!(
            readme.contains("diesel migration run"),
            "diesel runner instructions missing from {engine}/README.md for a diesel run"
        );
        assert!(
            readme.contains("generate --backend diesel"),
            "generated-by header must name the diesel backend in {engine}/README.md"
        );
    }
}

/// Tests that no generated README cites internal planning documents the
/// consumer tree cannot contain (IN-05: every emitted README.md, for all
/// four backends, is free of the planning-file reference).
#[test]
fn generated_readmes_cite_no_planning_docs() {
    for backend in ["diesel", "sqlx", "seaorm", "mongodb"] {
        let dir = test_temp_dir();
        let out_dir = dir.to_str().unwrap();

        rolify_cli()
            .args([
                "generate",
                "--backend",
                backend,
                "Role",
                "User",
                "--out-dir",
                out_dir,
            ])
            .assert()
            .success();

        for entry in WalkDir::new(&dir)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            let is_readme = entry.file_type().is_file()
                && entry.file_name().to_str().is_some_and(|name| name == "README.md");
            if is_readme {
                let readme = fs::read_to_string(entry.path()).unwrap();
                assert!(
                    !readme.contains("06-CONTEXT"),
                    "generated README cites the internal planning doc: {}",
                    entry.path().display()
                );
            }
        }
    }
}

/// Tests that `--roles-table` WITHOUT `--join-table` derives the join table
/// as holder plural + _ + roles-table value (the args.rs help contract), so
/// the migration and the scaffolding config agree everywhere (CR-01, D-05).
#[test]
fn roles_table_without_join_table_agrees_everywhere() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Role",
            "User",
            "--roles-table",
            "privileges",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    // SQL: the derived join table is users_privileges on every engine
    for engine in ["postgres", "mysql", "sqlite"] {
        let up_sql = fs::read_to_string(dir.join(format!(
            "migrations/{engine}/0000000001_rolify_create_tables/up.sql"
        )))
        .unwrap();
        assert!(
            up_sql.contains("CREATE TABLE users_privileges"),
            "derived join table users_privileges missing from {engine} up.sql"
        );
        assert!(
            !up_sql.contains("users_roles"),
            "stale derived join name users_roles in {engine} up.sql"
        );
    }

    // Scaffolding: config_example documents the SAME derived join name
    for engine in ["postgres", "mysql", "sqlite"] {
        let config = fs::read_to_string(dir.join(format!("{engine}/config_example.rs"))).unwrap();
        assert!(
            config.contains("users_privileges"),
            "config_example missing the derived join table for {engine}"
        );
        assert!(
            !config.contains("users_roles"),
            "config_example documents a join table the migration never created ({engine})"
        );
    }
}

/// Tests that an explicit `--join-table` embedding the roles stem survives
/// verbatim in every emitted artifact: the re-scan-free substitution never
/// lets the roles pass touch the inserted join name (WR-01).
#[test]
fn explicit_join_table_with_roles_substring_survives() {
    let dir = test_temp_dir();
    let out_dir = dir.to_str().unwrap();

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "diesel",
            "Privilege",
            "Customer",
            "--roles-table",
            "privileges",
            "--join-table",
            "member_roles_archive",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    for engine in ["postgres", "mysql", "sqlite"] {
        for file_name in ["up.sql", "down.sql"] {
            let sql = fs::read_to_string(dir.join(format!(
                "migrations/{engine}/0000000001_rolify_create_tables/{file_name}"
            )))
            .unwrap();
            assert!(
                sql.contains("member_roles_archive"),
                "explicit join table missing from {engine} {file_name}"
            );
            assert!(
                !sql.contains("member_privileges_archive"),
                "explicit join table mangled by a re-scan in {engine} {file_name}"
            );
        }
    }

    let config = fs::read_to_string(dir.join("postgres/config_example.rs")).unwrap();
    assert!(
        config.contains("member_roles_archive"),
        "config_example must carry the requested join name"
    );
}
