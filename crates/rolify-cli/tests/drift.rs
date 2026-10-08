use rolify_cli::args::Backend;
use rolify_cli::emitters::{mongo::render_mongo, seaorm::render_seaorm};
use rolify_cli::render::{RenderPlan, render_all};
use std::fs;

/// Reads the canonical diesel migration file for the given engine and file.
fn read_canonical(engine: &str, file: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!(
        "{manifest_dir}/../rolify-diesel/migrations/{engine}/0000000001_rolify_create_tables/{file}"
    );
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read canonical file: {path}"))
}

/// Reads the expected snapshot file for `SeaORM` or Mongo.
fn read_expected_snapshot(name: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/tests/expected/{name}");
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read expected snapshot: {path}"))
}

/// Tests that the renderer output matches the canonical diesel files byte-for-byte.
#[test]
fn renderer_matches_canonical_default_names() {
    let engines = ["postgres", "mysql", "sqlite"];
    for engine in engines {
        let backend = match engine {
            "postgres" | "mysql" | "sqlite" => Backend::Diesel,
            _ => panic!("unknown engine: {engine}"),
        };

        let plan = RenderPlan {
            backend,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: "roles".to_string(),
            join_table: "users_roles".to_string(),
        };

        let rendered =
            render_all(&plan).unwrap_or_else(|e| panic!("render failed for {engine}: {e:?}"));
        let files = rendered
            .get(engine)
            .unwrap_or_else(|| panic!("no files for engine: {engine}"));

        // Find up.sql and down.sql
        let up_file = files.iter().find(|f| f.path.ends_with("up.sql")).unwrap();
        let down_file = files.iter().find(|f| f.path.ends_with("down.sql")).unwrap();

        let expected_up = read_canonical(engine, "up.sql");
        let expected_down = read_canonical(engine, "down.sql");

        assert_eq!(
            up_file.content, expected_up,
            "up.sql mismatch for engine: {engine}"
        );
        assert_eq!(
            down_file.content, expected_down,
            "down.sql mismatch for engine: {engine}"
        );
    }
}

/// Tests custom names replacement (longest-first rule) for SQL backends.
#[test]
fn renderer_custom_names_longest_first() {
    let plan = RenderPlan {
        backend: Backend::Diesel,
        role_name: "Privilege".to_string(),
        holder_name: "Customer".to_string(),
        roles_table: "privileges".to_string(),
        join_table: "customers_privileges".to_string(),
    };

    let rendered = render_all(&plan).unwrap();
    let files = rendered.get("postgres").unwrap();

    let up_file = files.iter().find(|f| f.path.ends_with("up.sql")).unwrap();
    let content = &up_file.content;

    // Verify longest-first replacement: join table replaced before roles table
    assert!(
        content.contains("customers_privileges"),
        "join table name not found"
    );
    assert!(content.contains("privileges"), "roles table name not found");
    assert!(
        content.contains("privileges_triple_unique"),
        "roles unique constraint not renamed"
    );
    assert!(
        content.contains("idx_privileges_resource"),
        "roles resource index not renamed"
    );
    assert!(
        content.contains("customers_privileges_pair_unique"),
        "join unique constraint not renamed"
    );
    // Ensure no partial replacement (e.g., "users_privileges" would be wrong)
    assert!(
        !content.contains("users_privileges"),
        "partial replacement detected"
    );
    assert!(
        !content.contains("customers_roles"),
        "partial replacement detected"
    );
}

/// Tests `SeaORM` renderer matches the expected snapshot (byte-for-byte).
#[test]
fn seaorm_renderer_matches_snapshot() {
    let plan = RenderPlan {
        backend: Backend::Seaorm,
        role_name: "Role".to_string(),
        holder_name: "User".to_string(),
        roles_table: "roles".to_string(),
        join_table: "users_roles".to_string(),
    };

    let rendered = render_seaorm(&plan).unwrap();
    let expected = read_expected_snapshot("seaorm_migration.rs");

    assert_eq!(rendered, expected, "SeaORM migration snapshot mismatch");
}

/// Tests `SeaORM` custom names substitution (longest-first rule).
#[test]
fn seaorm_renderer_custom_names_longest_first() {
    let plan = RenderPlan {
        backend: Backend::Seaorm,
        role_name: "Privilege".to_string(),
        holder_name: "Customer".to_string(),
        roles_table: "privileges".to_string(),
        join_table: "customers_privileges".to_string(),
    };

    let rendered = render_seaorm(&plan).unwrap();

    // Verify longest-first replacement: join table replaced before roles table
    assert!(
        rendered.contains("customers_privileges"),
        "join table name not found"
    );
    assert!(
        rendered.contains("privileges"),
        "roles table name not found"
    );
    assert!(
        !rendered.contains("users_roles"),
        "default join table should not appear"
    );
    assert!(
        !rendered.contains("users_privileges"),
        "partial replacement detected"
    );
    assert!(
        !rendered.contains("customers_roles"),
        "partial replacement detected"
    );
}

/// Tests `SeaORM` semantic checklist (Pitfall 7).
#[test]
fn seaorm_renderer_semantic_checklist() {
    let plan = RenderPlan {
        backend: Backend::Seaorm,
        role_name: "Role".to_string(),
        holder_name: "User".to_string(),
        roles_table: "roles".to_string(),
        join_table: "users_roles".to_string(),
    };

    let rendered = render_seaorm(&plan).unwrap();

    // Semantic checklist (Pitfall 7)
    assert!(
        rendered.contains("roles_triple_unique"),
        "unique triple missing"
    );
    assert!(
        rendered.contains("idx_roles_resource"),
        "resource index missing"
    );
    assert!(rendered.contains("idx_roles_name"), "name index missing");
    assert!(rendered.contains("DEFAULT ''"), "sentinel default missing");
    assert!(rendered.contains("ON DELETE CASCADE"), "FK cascade missing");

    // Raw-string integrity (G1 guard): the SQL is embedded inside r#"..."#,
    // which never interprets backslash escapes, so a literal backslash-n
    // sequence would collapse the script into one comment-only line and make
    // up() a silent no-op at runtime.
    assert!(
        !rendered.contains("\\n"),
        "literal \\n sequences must not appear inside the raw string (raw strings preserve real newlines)"
    );

    // Down drops join first
    let join_pos = rendered.find("users_roles").unwrap();
    let roles_pos = rendered.rfind("roles").unwrap();
    assert!(join_pos < roles_pos, "down should drop join table first");
}

/// Tests Mongo renderer matches the expected snapshot (byte-for-byte).
#[test]
fn mongo_renderer_matches_snapshot() {
    let plan = RenderPlan {
        backend: Backend::Mongodb,
        role_name: "Role".to_string(),
        holder_name: "User".to_string(),
        roles_table: "roles".to_string(),
        join_table: "users_roles".to_string(),
    };

    let (role_doc, index_notes) = render_mongo(&plan).unwrap();
    let expected = read_expected_snapshot("mongo_docs.rs");

    // Reconstruct the full template from rendered parts
    let reconstructed = format!("{role_doc}{index_notes}");

    assert_eq!(reconstructed, expected, "Mongo template snapshot mismatch");
}

/// Tests the emitted Mongo halves are structurally well-formed: role.rs is
/// complete Rust closing on the consumer role-ids struct with one trailing
/// newline, `INDEX_NOTES.md` is pure Markdown, and neither half carries the
/// conditional-skip serde attribute that would contradict the explicit-null
/// contract (WR-02, WR-03).
#[test]
fn mongo_emitted_files_are_well_formed() {
    let plan = RenderPlan {
        backend: Backend::Mongodb,
        role_name: "Role".to_string(),
        holder_name: "User".to_string(),
        roles_table: "roles".to_string(),
        join_table: "users_roles".to_string(),
    };

    let (role_doc, index_notes) = render_mongo(&plan).unwrap();

    // role.rs ends with exactly one trailing newline, and its final
    // non-empty line is not a doc-comment marker: the consumer struct, not
    // an orphaned doc block, closes the file (WR-02).
    assert!(role_doc.ends_with('\n'), "role_doc must end with a newline");
    assert!(
        !role_doc.ends_with("\n\n"),
        "role_doc must end with a single trailing newline, not a blank line"
    );
    let final_non_empty = role_doc
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("role_doc must have a non-empty final line");
    assert!(
        !final_non_empty.starts_with("///"),
        "orphaned doc comment at role_doc tail: {final_non_empty}"
    );
    assert_eq!(
        final_non_empty, "}",
        "role_doc must close on the consumer role-ids struct"
    );
    assert!(
        role_doc.contains("pub struct ConsumerRoleIds"),
        "consumer role-ids struct missing from role_doc"
    );

    // The notes half is pure Markdown: opens on a level-one heading and
    // carries no Rust item declaration (WR-02).
    assert!(
        index_notes.starts_with("# "),
        "index notes must open with a Markdown level-one heading"
    );
    assert!(
        !index_notes.contains("const"),
        "index notes must not carry a Rust const declaration"
    );

    // The derive agrees with the documented explicit-null contract: absent
    // scope persists as BSON null, never as an omitted field (WR-03, D-07).
    assert!(
        !role_doc.contains("skip_serializing_if"),
        "conditional-skip serde attribute must not appear on the scope fields"
    );
    assert!(
        !index_notes.contains("skip_serializing_if"),
        "conditional-skip serde attribute must not appear in the notes"
    );
}

/// Tests Mongo custom names substitution.
#[test]
fn mongo_renderer_custom_names_substitution() {
    let plan = RenderPlan {
        backend: Backend::Mongodb,
        role_name: "Privilege".to_string(),
        holder_name: "Customer".to_string(),
        roles_table: "privileges".to_string(),
        join_table: "customers_privileges".to_string(),
    };

    let (role_doc, _) = render_mongo(&plan).unwrap();

    // Table name substitution in comments/documentation
    assert!(
        role_doc.contains("privileges"),
        "roles table name not substituted"
    );
    assert!(
        role_doc.contains("customers_privileges"),
        "join table name not substituted"
    );
    assert!(
        !role_doc.contains("users_roles"),
        "default join table should not appear"
    );
}

/// Tests that malicious identifiers are rejected by `validate_identifier`.
#[test]
fn validate_identifier_rejects_malicious() {
    use rolify_core::config::RolifyConfigBuilder;
    use rolify_core::error::RolifyError;

    // SQL injection attempt
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("roles; DROP TABLE users"),
        Err(RolifyError::InvalidConfig { .. })
    ));

    // Path traversal
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("../etc/passwd"),
        Err(RolifyError::InvalidConfig { .. })
    ));

    // Empty string
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier(""),
        Err(RolifyError::InvalidConfig { .. })
    ));

    // Invalid start character
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("123invalid"),
        Err(RolifyError::InvalidConfig { .. })
    ));

    // Invalid characters
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("invalid-name"),
        Err(RolifyError::InvalidConfig { .. })
    ));
}

/// Tests that no rendered SQL migration carries an em-dash (U+2014): the
/// project forbids the character in code and comments, so the shipped-SQL
/// convention must not silently regress even if a canonical tree drifts
/// (WR-06).
#[test]
fn rendered_sql_carries_no_em_dash() {
    let plan = RenderPlan {
        backend: Backend::Diesel,
        role_name: "Role".to_string(),
        holder_name: "User".to_string(),
        roles_table: "roles".to_string(),
        join_table: "users_roles".to_string(),
    };

    let rendered = render_all(&plan).unwrap();

    for engine in ["postgres", "mysql", "sqlite"] {
        let files = rendered
            .get(engine)
            .unwrap_or_else(|| panic!("no files for engine: {engine}"));

        for file in files {
            if file.path.ends_with("up.sql") || file.path.ends_with("down.sql") {
                assert!(
                    !file.content.contains('\u{2014}'),
                    "rendered {} for engine {engine} carries an em-dash (U+2014)",
                    file.path
                );
            }
        }
    }
}

/// Tests custom-names rendering matches the checked-in snapshots byte-for-byte,
/// proving the longest-first rule with renamed identifiers (T-06-06).
#[test]
fn custom_names_postgres_matches_snapshots() {
    let plan = RenderPlan {
        backend: Backend::Diesel,
        role_name: "Privilege".to_string(),
        holder_name: "Customer".to_string(),
        roles_table: "privileges".to_string(),
        join_table: "customers_privileges".to_string(),
    };

    let rendered = render_all(&plan).unwrap();
    let files = rendered.get("postgres").unwrap();

    let up_file = files.iter().find(|f| f.path.ends_with("up.sql")).unwrap();
    let down_file = files.iter().find(|f| f.path.ends_with("down.sql")).unwrap();

    let expected_up = read_expected_snapshot("custom_privileges_postgres_up.sql");
    let expected_down = read_expected_snapshot("custom_privileges_postgres_down.sql");

    // Full-file equality, not just contains (Pitfall 2)
    assert_eq!(
        up_file.content, expected_up,
        "custom up.sql snapshot mismatch (longest-first rule violated)"
    );
    assert_eq!(
        down_file.content, expected_down,
        "custom down.sql snapshot mismatch"
    );

    // Explicit renamed-identifier assertions (T-06-06)
    let content = &up_file.content;
    assert!(
        content.contains("privileges_triple_unique"),
        "renamed triple unique missing"
    );
    assert!(
        content.contains("idx_privileges_resource"),
        "renamed resource index missing"
    );
    assert!(
        content.contains("idx_privileges_name"),
        "renamed name index missing"
    );
    assert!(
        content.contains("customers_privileges_pair_unique"),
        "renamed join pair unique missing"
    );
    assert!(
        !content.contains("users_privileges"),
        "partial replacement detected"
    );
    assert!(
        !content.contains("customers_roles"),
        "partial replacement detected"
    );
}
