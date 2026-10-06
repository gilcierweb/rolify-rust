use rolify_cli::args::Backend;
use rolify_cli::render::{RenderPlan, render_all};
use rolify_cli::emitters::{seaorm::render_seaorm, mongo::render_mongo};
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

    assert_eq!(
        rendered, expected,
        "SeaORM migration snapshot mismatch"
    );
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
    assert!(rendered.contains("customers_privileges"), "join table name not found");
    assert!(rendered.contains("privileges"), "roles table name not found");
    assert!(!rendered.contains("users_roles"), "default join table should not appear");
    assert!(!rendered.contains("users_privileges"), "partial replacement detected");
    assert!(!rendered.contains("customers_roles"), "partial replacement detected");
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
    assert!(rendered.contains("roles_triple_unique"), "unique triple missing");
    assert!(rendered.contains("idx_roles_resource"), "resource index missing");
    assert!(rendered.contains("idx_roles_name"), "name index missing");
    assert!(rendered.contains("DEFAULT ''"), "sentinel default missing");
    assert!(rendered.contains("ON DELETE CASCADE"), "FK cascade missing");
    
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
    
    assert_eq!(
        reconstructed, expected,
        "Mongo template snapshot mismatch"
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
    assert!(role_doc.contains("privileges"), "roles table name not substituted");
    assert!(role_doc.contains("customers_privileges"), "join table name not substituted");
    assert!(!role_doc.contains("users_roles"), "default join table should not appear");
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
    assert!(content.contains("privileges_triple_unique"), "renamed triple unique missing");
    assert!(content.contains("idx_privileges_resource"), "renamed resource index missing");
    assert!(content.contains("idx_privileges_name"), "renamed name index missing");
    assert!(
        content.contains("customers_privileges_pair_unique"),
        "renamed join pair unique missing"
    );
    assert!(!content.contains("users_privileges"), "partial replacement detected");
    assert!(!content.contains("customers_roles"), "partial replacement detected");
}