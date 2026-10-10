use rolify_cli::args::Backend;
use rolify_cli::emitters::{mongo::render_mongo, seaorm::render_seaorm};
use rolify_cli::render::{RenderPlan, render_all};
use rolify_core::config::HolderIdKind;
use std::fs;

/// The three holder id kinds and their canonical subtree names, ordered as the
/// kind-decided trees land on disk (`integer`, `uuid`, `string`).
const KINDS: [(HolderIdKind, &str); 3] = [
    (HolderIdKind::Integer, "integer"),
    (HolderIdKind::Uuid, "uuid"),
    (HolderIdKind::String, "string"),
];

/// Reads the canonical diesel migration file for the given engine, kind, and
/// file. The kind-decided trees live one level deeper than the legacy
/// kind-less trees (`{engine}/{kind}/0000000001_rolify_create_tables/{file}`).
fn read_canonical(engine: &str, kind: &str, file: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!(
        "{manifest_dir}/../rolify-diesel/migrations/{engine}/{kind}/0000000001_rolify_create_tables/{file}"
    );
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read canonical file: {path}"))
}

/// Reads the canonical sqlx migration file for the given engine, kind, and
/// file. The sqlx tree is flat: the up script is
/// `{engine}/{kind}/0000000001_rolify_create_tables.sql` and the down script
/// carries a `.down.sql` suffix.
fn read_sqlx_canonical(engine: &str, kind: &str, file: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let suffix = if file == "up.sql" {
        "0000000001_rolify_create_tables.sql"
    } else {
        "0000000001_rolify_create_tables.down.sql"
    };
    let path = format!("{manifest_dir}/../rolify-sqlx/migrations/{engine}/{kind}/{suffix}");
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read sqlx canonical file: {path}"))
}

/// Reads the expected snapshot file for `SeaORM` or Mongo.
fn read_expected_snapshot(name: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/tests/expected/{name}");
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("failed to read expected snapshot: {path}"))
}

/// Asserts `actual` equals the checked-in snapshot `name`, OR rewrites the
/// snapshot when `ROLIFY_UPDATE_SNAPSHOTS=1` is set (T-08-17). Regeneration
/// therefore always flows through the library render that produced `actual`;
/// the snapshot is never hand-edited.
fn assert_snapshot(name: &str, actual: &str) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/tests/expected/{name}");
    if std::env::var("ROLIFY_UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        fs::write(&path, actual)
            .unwrap_or_else(|_| panic!("failed to rewrite snapshot via library render: {path}"));
        return;
    }
    let expected = read_expected_snapshot(name);
    assert_eq!(actual, expected, "snapshot mismatch: {name}");
}

/// Tests that the default renderer output (no flags -> integer kind) matches
/// the canonical diesel integer tree byte-for-byte.
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
            holder_id_kind: HolderIdKind::Integer,
            with_holder_fk: false,
        };

        let rendered = render_all(&plan)
            .unwrap_or_else(|error| panic!("render failed for {engine}: {error:?}"));
        let files = rendered
            .get(engine)
            .unwrap_or_else(|| panic!("no files for engine: {engine}"));

        // Find up.sql and down.sql
        let up_file = files
            .iter()
            .find(|file| file.path.ends_with("up.sql"))
            .unwrap();
        let down_file = files
            .iter()
            .find(|file| file.path.ends_with("down.sql"))
            .unwrap();

        let expected_up = read_canonical(engine, "integer", "up.sql");
        let expected_down = read_canonical(engine, "integer", "down.sql");

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

/// Tests that the renderer output matches the canonical per-kind diesel tree
/// byte-for-byte for all three holder id kinds across all three engines, and
/// that the sqlx parallel trees carry the identical bytes (D-11 identity,
/// held in the drift guard as well as by construction).
#[test]
fn renderer_matches_canonical_all_kinds() {
    for engine in ["postgres", "mysql", "sqlite"] {
        for (kind, kind_name) in KINDS {
            let plan = RenderPlan {
                backend: Backend::Diesel,
                role_name: "Role".to_string(),
                holder_name: "User".to_string(),
                roles_table: "roles".to_string(),
                join_table: "users_roles".to_string(),
                holder_id_kind: kind,
                with_holder_fk: false,
            };

            let rendered = render_all(&plan)
                .unwrap_or_else(|error| panic!("render failed for {engine}/{kind_name}: {error:?}"));
            let files = rendered
                .get(engine)
                .unwrap_or_else(|| panic!("no files for engine: {engine}"));

            let up_file = files
                .iter()
                .find(|file| file.path.ends_with("up.sql"))
                .unwrap();
            let down_file = files
                .iter()
                .find(|file| file.path.ends_with("down.sql"))
                .unwrap();

            let expected_up = read_canonical(engine, kind_name, "up.sql");
            let expected_down = read_canonical(engine, kind_name, "down.sql");

            assert_eq!(
                up_file.content, expected_up,
                "up.sql mismatch for engine {engine}, kind {kind_name}"
            );
            assert_eq!(
                down_file.content, expected_down,
                "down.sql mismatch for engine {engine}, kind {kind_name}"
            );

            // D-11 identity is re-asserted in the drift guard: the sqlx
            // parallel tree must carry the same bytes for this kind.
            assert_eq!(
                read_sqlx_canonical(engine, kind_name, "up.sql"),
                expected_up,
                "sqlx up.sql diverged from the diesel canonical tree for engine {engine}, kind {kind_name}"
            );
            assert_eq!(
                read_sqlx_canonical(engine, kind_name, "down.sql"),
                expected_down,
                "sqlx down.sql diverged from the diesel canonical tree for engine {engine}, kind {kind_name}"
            );
        }
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
    };

    let rendered = render_all(&plan).unwrap();
    let files = rendered.get("postgres").unwrap();

    let up_file = files
        .iter()
        .find(|file| file.path.ends_with("up.sql"))
        .unwrap();
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
    };

    let rendered = render_seaorm(&plan).unwrap();
    assert_snapshot("seaorm_migration.rs", &rendered);
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
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

/// Tests that an explicit join table embedding the roles stem survives the
/// `SeaORM` substitution verbatim: the rendered migration carries the
/// requested name, never the re-scanned mangled variant (WR-01).
#[test]
fn seaorm_explicit_join_name_survives_substitution() {
    let plan = RenderPlan {
        backend: Backend::Seaorm,
        role_name: "Privilege".to_string(),
        holder_name: "Customer".to_string(),
        roles_table: "privileges".to_string(),
        join_table: "member_roles_archive".to_string(),
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
    };

    let rendered = render_seaorm(&plan).unwrap();

    assert!(
        rendered.contains("member_roles_archive"),
        "explicit join name missing from the seaorm migration"
    );
    assert!(
        !rendered.contains("member_privileges_archive"),
        "explicit join name mangled by a re-scan in the seaorm migration"
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
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

/// Splits a SQL script into whole statements with test-local logic,
/// mirroring the emitter's documented contract (comments ride along; only a
/// non-comment line ending in a semicolon terminates). The wiring proof
/// must NOT import the emitter's own splitting logic (T-06-13).
fn split_statements_test_local(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in sql.lines() {
        current.push_str(line);
        current.push('\n');

        let trimmed = line.trim();
        if !trimmed.starts_with("--") && trimmed.ends_with(';') {
            statements.push(current.trim().to_owned());
            current.clear();
        }
    }
    assert!(
        current.trim().is_empty(),
        "test derivation hit an unterminated statement tail"
    );
    statements
}

/// Mirrors the emitters' substitution contract (D-14): split on the
/// canonical join sentinel first, replace the roles stem per ORIGINAL
/// segment, join the requested name last. Test-local by design: the wiring
/// proof must not import the emitter's own logic (T-06-13).
fn substitute_names_test_local(sql: &str, roles_table: &str, join_table: &str) -> String {
    sql.split("users_roles")
        .map(|segment| segment.replace("roles", roles_table))
        .collect::<Vec<_>>()
        .join(join_table)
}

/// Derives the expected down statements (join drop first, roles drop
/// second) from the canonical down.sql with test-local logic mirroring the
/// documented extraction contract: whole statements, comment lines ignored
/// for classification, substitution applied per extracted statement.
fn derive_down_statements_test_local(
    down_sql: &str,
    roles_table: &str,
    join_table: &str,
) -> Vec<String> {
    let mut join_drop = String::new();
    let mut roles_drop = String::new();

    for statement in split_statements_test_local(down_sql) {
        let executable: String = statement
            .lines()
            .filter(|line| !line.trim().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");

        if !executable.contains("DROP TABLE") {
            continue;
        }
        let claims_join = executable.contains("users_roles");
        let claims_roles = executable.replace("users_roles", "").contains("roles");
        if claims_join {
            join_drop = substitute_names_test_local(&statement, roles_table, join_table);
        } else if claims_roles {
            roles_drop = substitute_names_test_local(&statement, roles_table, join_table);
        }
    }

    assert!(
        !join_drop.is_empty() && !roles_drop.is_empty(),
        "canonical down must carry both drop classes"
    );
    vec![join_drop, roles_drop]
}

/// Parses the raw-string elements of a const statement array out of the
/// rendered migration. Elements are `r"..."` spans; the canonical SQL
/// carries no double quotes, so the closing `",` is unambiguous.
fn parse_statement_array(rendered: &str, array_name: &str) -> Vec<String> {
    let header = format!("const {array_name}: &[&str] = &[");
    let header_position = rendered
        .find(&header)
        .unwrap_or_else(|| panic!("array {array_name} missing from the rendered migration"));
    let body_start = header_position + header.len();
    let body_length = rendered[body_start..]
        .find("\n];")
        .unwrap_or_else(|| panic!("array {array_name} has no closing bracket"));
    let body = &rendered[body_start..body_start + body_length];

    let mut elements = Vec::new();
    let mut current: Option<String> = None;
    for line in body.lines() {
        if let Some(accumulated) = current.take() {
            if let Some(final_piece) = line.strip_suffix("\",") {
                elements.push(format!("{accumulated}\n{final_piece}"));
            } else {
                current = Some(format!("{accumulated}\n{line}"));
            }
        } else if let Some(rest) = line.trim_start().strip_prefix("r\"") {
            if let Some(single_line) = rest.strip_suffix("\",") {
                elements.push(single_line.to_owned());
            } else {
                current = Some(rest.to_owned());
            }
        }
    }
    assert!(
        current.is_none(),
        "array {array_name} carries an unterminated element"
    );
    elements
}

/// Tests that every emitted per-dialect statement array equals the
/// statements independently derived from the canonical on-disk trees for
/// that engine, element for element (T-06-13, CR-03): a postgres-into-mysql
/// mis-wiring cannot pass this gate, and the substitution bytes flow into
/// every array. The derivation is test-local end to end; it never imports
/// the emitter's own splitting or extraction logic.
#[test]
fn seaorm_dialect_arrays_match_canonical_per_engine() {
    let roles_table = "privileges";
    let join_table = "customers_privileges";
    let plan = RenderPlan {
        backend: Backend::Seaorm,
        role_name: "Privilege".to_string(),
        holder_name: "Customer".to_string(),
        roles_table: roles_table.to_string(),
        join_table: join_table.to_string(),
        holder_id_kind: rolify_core::config::HolderIdKind::Integer,
        with_holder_fk: false,
    };

    let rendered = render_seaorm(&plan).unwrap();

    for (engine, array_stem) in [
        ("postgres", "POSTGRES"),
        ("mysql", "MYSQL"),
        ("sqlite", "SQLITE"),
    ] {
        let canonical_up = read_canonical(engine, "integer", "up.sql");
        let canonical_down = read_canonical(engine, "integer", "down.sql");

        // Up: substitution first, then the split (the emitter's documented
        // pipeline order, mirrored locally).
        let substituted_up = substitute_names_test_local(&canonical_up, roles_table, join_table);
        let expected_up = split_statements_test_local(&substituted_up);

        // Down: whole-statement extraction from the original canonical text.
        let expected_down =
            derive_down_statements_test_local(&canonical_down, roles_table, join_table);

        let up_array = parse_statement_array(&rendered, &format!("{array_stem}_UP_STATEMENTS"));
        let down_array = parse_statement_array(&rendered, &format!("{array_stem}_DOWN_STATEMENTS"));

        assert_eq!(
            up_array, expected_up,
            "{engine} up array is mis-wired or drifted from the canonical tree"
        );
        assert_eq!(
            down_array, expected_down,
            "{engine} down array is mis-wired or drifted from the canonical tree"
        );
    }

    // Dialect markers: each up array carries its own engine's identity DDL,
    // so a copy-paste between dialects fails even if names would agree.
    let postgres_up = parse_statement_array(&rendered, "POSTGRES_UP_STATEMENTS");
    let mysql_up = parse_statement_array(&rendered, "MYSQL_UP_STATEMENTS");
    let sqlite_up = parse_statement_array(&rendered, "SQLITE_UP_STATEMENTS");

    assert!(
        postgres_up
            .iter()
            .any(|statement| statement.contains("GENERATED ALWAYS AS IDENTITY")),
        "postgres up array must carry the identity-column form"
    );
    assert!(
        mysql_up
            .iter()
            .any(|statement| statement.contains("AUTO_INCREMENT")),
        "mysql up array must carry the auto-increment form"
    );
    assert!(
        !mysql_up
            .iter()
            .any(|statement| statement.contains("GENERATED ALWAYS AS IDENTITY")),
        "mysql up array must not carry the postgres identity clause"
    );
    assert!(
        sqlite_up
            .iter()
            .any(|statement| statement.contains("INTEGER PRIMARY KEY")),
        "sqlite up array must carry the rowid-alias primary key form"
    );
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
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

/// Tests that an explicit join table embedding the roles stem survives the
/// Mongo substitution verbatim: the role document carries the requested
/// name (the template's table-name reference block lives in its module
/// docs), and the mangled variant appears in neither emitted document
/// (WR-01).
#[test]
fn mongo_explicit_join_name_survives_substitution() {
    let plan = RenderPlan {
        backend: Backend::Mongodb,
        role_name: "Privilege".to_string(),
        holder_name: "Customer".to_string(),
        roles_table: "privileges".to_string(),
        join_table: "member_roles_archive".to_string(),
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
    };

    let (role_doc, index_notes) = render_mongo(&plan).unwrap();

    assert!(
        role_doc.contains("member_roles_archive"),
        "explicit join name missing from the mongo role document"
    );
    for (name, document) in [("role_doc", &role_doc), ("index_notes", &index_notes)] {
        assert!(
            !document.contains("member_privileges_archive"),
            "explicit join name mangled by a re-scan in the mongo {name}"
        );
    }
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
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
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
    };

    let rendered = render_all(&plan).unwrap();
    let files = rendered.get("postgres").unwrap();

    let up_file = files
        .iter()
        .find(|file| file.path.ends_with("up.sql"))
        .unwrap();
    let down_file = files
        .iter()
        .find(|file| file.path.ends_with("down.sql"))
        .unwrap();

    // Full-file equality, not just contains (Pitfall 2). Regeneration flows
    // through the library render under ROLIFY_UPDATE_SNAPSHOTS=1 (T-08-17).
    assert_snapshot("custom_privileges_postgres_up.sql", &up_file.content);
    assert_snapshot("custom_privileges_postgres_down.sql", &down_file.content);

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
