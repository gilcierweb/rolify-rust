use rolify_cli::render::{render_all, RenderPlan};
use rolify_cli::args::Backend;
use std::fs;

/// Reads the canonical diesel migration file for the given engine and file.
fn read_canonical(engine: &str, file: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!(
        "{}/../rolify-diesel/migrations/{}/0000000001_rolify_create_tables/{}",
        manifest_dir, engine, file
    );
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read canonical file: {}", path))
}

/// Tests that the renderer output matches the canonical diesel files byte-for-byte.
#[test]
fn renderer_matches_canonical_default_names() {
    let engines = ["postgres", "mysql", "sqlite"];
    for engine in engines {
        let backend = match engine {
            "postgres" | "mysql" | "sqlite" => Backend::Diesel,
            _ => panic!("unknown engine: {}", engine),
        };

        let plan = RenderPlan {
            backend,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: "roles".to_string(),
            join_table: "users_roles".to_string(),
        };

        let rendered = render_all(&plan).unwrap_or_else(|e| panic!("render failed for {}: {:?}", engine, e));
        let files = rendered.get(engine).unwrap_or_else(|| panic!("no files for engine: {}", engine));

        // Find up.sql and down.sql
        let up_file = files.iter().find(|f| f.path.ends_with("up.sql")).unwrap();
        let down_file = files.iter().find(|f| f.path.ends_with("down.sql")).unwrap();

        let expected_up = read_canonical(engine, "up.sql");
        let expected_down = read_canonical(engine, "down.sql");

        assert_eq!(
            up_file.content, expected_up,
            "up.sql mismatch for engine: {}",
            engine
        );
        assert_eq!(
            down_file.content, expected_down,
            "down.sql mismatch for engine: {}",
            engine
        );
    }
}

/// Tests custom names replacement (longest-first rule).
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
    assert!(content.contains("customers_privileges"), "join table name not found");
    assert!(content.contains("privileges"), "roles table name not found");
    assert!(content.contains("privileges_triple_unique"), "roles unique constraint not renamed");
    assert!(content.contains("idx_privileges_resource"), "roles resource index not renamed");
    assert!(content.contains("customers_privileges_pair_unique"), "join unique constraint not renamed");
    // Ensure no partial replacement (e.g., "users_privileges" would be wrong)
    assert!(!content.contains("users_privileges"), "partial replacement detected");
    assert!(!content.contains("customers_roles"), "partial replacement detected");
}

/// Tests that malicious identifiers are rejected by validate_identifier.
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