//! `SeaORM` migration emitter - hand-maintained Rust template (D-16).
//!
//! The emitted migration carries THREE per-dialect statement arrays, each
//! byte-derived at render time from the vendored canonical `up.sql` and
//! `down.sql` for that engine, and `up()`/`down()` select the right array at
//! runtime through a match on the `SchemaManager`'s database backend
//! (Postgres, `MySQL`, `SQLite`; CR-03). `Statement::from_string` only tags
//! a statement with a dialect, it never translates DDL, so every dialect
//! ships its own canonical bytes (D-10, D-16). Statements execute ONE AT
//! A TIME because the Postgres extended protocol rejects multi-command
//! prepared statements. `down()` drops the join table first on every
//! dialect (D-10, D-12). The template is type-checked via the
//! `sea-orm-migration` dev-dep in tests.
//!
//! The pipeline fails loudly on malformed canonical input (WR-05): an
//! unterminated statement tail, a down statement carrying multiple DROP
//! TABLE clauses, and a missing or duplicated drop class all return
//! `CliError::MalformedCanonicalSql` naming the defect instead of silently
//! truncating content or emitting a multi-command prepared statement.
//!
//! Semantic checklist enforced in tests/drift.rs:
//! - Unique triple on (name, `resource_type`, `resource_id`)
//! - Both `idx_roles_resource` and `idx_roles_name` indexes
//! - DEFAULT '' on resource columns
//! - ON DELETE CASCADE on join foreign key
//! - Join-first drop order in every `down()` array

use crate::emitters::{JOIN_TABLE_SENTINEL, ROLES_TABLE_SENTINEL, substitute_table_names};
use crate::error::CliError;
use crate::render::RenderPlan;
use crate::templates;

/// Engines whose canonical trees feed the per-dialect statement arrays.
/// Index-aligned with [`ARRAY_NAME_STEMS`].
const ENGINES: [&str; 3] = ["postgres", "mysql", "sqlite"];

/// Uppercase stems for the emitted const array names, index-aligned with
/// [`ENGINES`] (for engine `postgres` the up array is
/// `POSTGRES_UP_STATEMENTS`).
const ARRAY_NAME_STEMS: [&str; 3] = ["POSTGRES", "MYSQL", "SQLITE"];

/// Renders the `SeaORM` migration file.
///
/// Every per-dialect statement array is byte-derived from the canonical
/// per-engine trees: the up script flows through the shared re-scan-free
/// name substitution and the hardened splitter, and the down statements are
/// extracted from the ORIGINAL canonical down.sql as whole statements and
/// substituted afterwards (WR-05). Returns the migration file content.
///
/// # Errors
///
/// Returns `CliError::MalformedCanonicalSql` when a vendored canonical
/// script is malformed: an unterminated statement tail, a down statement
/// carrying multiple DROP TABLE clauses, or a missing or duplicated drop
/// class (WR-05, never-silently-truncate).
pub fn render_seaorm(plan: &RenderPlan) -> Result<String, CliError> {
    let mut rendered = templates::seaorm_migration().to_owned();

    for (engine, array_stem) in ENGINES.iter().zip(ARRAY_NAME_STEMS) {
        // Up: substitute the validated names first, then split into
        // statements (the template placeholders name the const arrays).
        let up_sql = substitute_table_names(templates::up(engine), plan, engine);
        let up_statements = split_statements(&up_sql)?;
        rendered = rendered.replace(
            &format!("{{{{{array_stem}_UP_STATEMENTS}}}}"),
            &render_array_body(&up_statements),
        );

        // Down: whole-statement extraction on the original canonical text,
        // substitution applied per extracted statement.
        let down_statements = extract_down_statements(templates::down(engine), plan, engine)?;
        rendered = rendered.replace(
            &format!("{{{{{array_stem}_DOWN_STATEMENTS}}}}"),
            &render_array_body(&down_statements),
        );
    }

    Ok(rendered)
}

/// Renders statements as the body of a `&[&str]` const array: one raw-string
/// element per line at rustfmt's canonical top-level-array indent (the
/// checked-in expected snapshot is a compiled `.rs` file, so the emitted
/// bytes must be `cargo fmt`-stable). Raw strings preserve real newlines
/// and never interpret backslash escapes; the canonical SQL contains no
/// double quotes, so no `"` terminator can appear inside a statement body
/// (no hashes needed, per clippy's needless-raw-string-hashes gate).
fn render_array_body(statements: &[String]) -> String {
    statements
        .iter()
        .map(|statement| format!("    r\"{statement}\",")) // -> r"...",
        .collect::<Vec<_>>()
        .join("\n")
}

/// Splits a SQL script into individual statements, breaking on every
/// semicolon that terminates a NON-comment line. Comment lines stay
/// attached to the statement that follows them (valid SQL), and only a
/// NON-comment line can terminate a statement: the canonical mysql header
/// has a prose line ending in a semicolon, which must never split the
/// script.
///
/// Splitting per semicolon (not per line) keeps multi-command prepared
/// statements out of the emitted arrays: a line carrying two commands
/// yields two statements, because the Postgres extended protocol rejects
/// multiple commands inside a single prepared statement.
///
/// Fails loudly on an unterminated tail (WR-05): a script whose remaining
/// content after the last semicolon is non-empty would otherwise be silently
/// dropped, so the error names the leftover content instead.
fn split_statements(sql: &str) -> Result<Vec<String>, CliError> {
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in sql.lines() {
        if line.trim().starts_with("--") {
            current.push_str(line);
            current.push('\n');
            continue;
        }

        let mut remainder = line;
        while let Some(semi) = remainder.find(';') {
            let (head, tail) = remainder.split_at(semi + 1);
            current.push_str(head);
            statements.push(current.trim().to_owned());
            current.clear();
            remainder = tail;
        }
        // Preserve every byte the old line accumulator kept: the
        // post-semicolon remainder (even whitespace-only) and the line
        // break ride along into the next statement buffer, so canonical
        // output stays byte-identical and only same-line multi-commands
        // change shape (they now split instead of merging).
        current.push_str(remainder);
        current.push('\n');
    }

    if !current.trim().is_empty() {
        return Err(CliError::MalformedCanonicalSql {
            detail: format!(
                "unterminated statement tail after the last semicolon: {:?}",
                current.trim()
            ),
        });
    }

    Ok(statements)
}

/// Extracts the join-table and roles-table drop statements as WHOLE
/// semicolon-terminated statements of the original canonical down.sql
/// (before substitution), then substitutes the requested names per
/// statement. Returns the join drop first, the roles drop second (D-10,
/// D-12).
///
/// Classification and the guards below ignore comment lines: a down
/// statement's prose may legitimately mention both table stems, but only its
/// executable clauses decide what it drops.
///
/// Guards (WR-05, never-silently-truncate): a statement carrying more than
/// one DROP TABLE clause would become a multi-command prepared statement; a
/// statement claiming both drop classes cannot be classified; a duplicated
/// class claim would silently drop one of the statements; a missing class
/// means the canonical script no longer matches the extraction contract.
/// All fail loudly instead of synthesizing a fallback.
fn extract_down_statements(
    original_down_sql: &str,
    plan: &RenderPlan,
    engine: &str,
) -> Result<Vec<String>, CliError> {
    let statements = split_statements(original_down_sql)?;

    let mut join_drop: Option<String> = None;
    let mut roles_drop: Option<String> = None;

    for statement in &statements {
        let executable_lines: Vec<&str> = statement
            .lines()
            .filter(|line| !line.trim().starts_with("--"))
            .collect();
        let executable = executable_lines.join("\n");

        let drop_clause_count = executable.matches("DROP TABLE").count();
        if drop_clause_count > 1 {
            return Err(CliError::MalformedCanonicalSql {
                detail: format!(
                    "a single down statement carries {drop_clause_count} DROP TABLE clauses: {statement:?}"
                ),
            });
        }
        if drop_clause_count == 0 {
            // A statement with no DROP TABLE clause drops nothing we own.
            continue;
        }

        let claims_join = executable.contains(JOIN_TABLE_SENTINEL);
        // A bare `str::replace` never re-scans its own insertions, so
        // stripping the join sentinel before the stem check is safe.
        let claims_roles = executable
            .replace(JOIN_TABLE_SENTINEL, "")
            .contains(ROLES_TABLE_SENTINEL);

        match (claims_join, claims_roles) {
            (true, true) => {
                return Err(CliError::MalformedCanonicalSql {
                    detail: format!(
                        "a single down statement claims both the join and the roles drop: {statement:?}"
                    ),
                });
            }
            (true, false) => {
                if join_drop.is_some() {
                    return Err(CliError::MalformedCanonicalSql {
                        detail: format!(
                            "the down script carries more than one join-table drop statement: {statement:?}"
                        ),
                    });
                }
                join_drop = Some(substitute_table_names(statement, plan, engine));
            }
            (false, true) => {
                if roles_drop.is_some() {
                    return Err(CliError::MalformedCanonicalSql {
                        detail: format!(
                            "the down script carries more than one roles-table drop statement: {statement:?}"
                        ),
                    });
                }
                roles_drop = Some(substitute_table_names(statement, plan, engine));
            }
            (false, false) => {
                // A DROP for an unrelated table: not ours to extract.
            }
        }
    }

    let join_drop = join_drop.ok_or_else(|| CliError::MalformedCanonicalSql {
        detail: "the canonical down script carries no join-table drop statement".to_owned(),
    })?;
    let roles_drop = roles_drop.ok_or_else(|| CliError::MalformedCanonicalSql {
        detail: "the canonical down script carries no roles-table drop statement".to_owned(),
    })?;

    // Join first (FK child before parent), roles second (D-10, D-12).
    Ok(vec![join_drop, roles_drop])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Backend;
    use crate::render::RenderPlan;

    fn plan(roles_table: &str, join_table: &str) -> RenderPlan {
        RenderPlan {
            backend: Backend::Seaorm,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: roles_table.to_string(),
            join_table: join_table.to_string(),
            holder_id_kind: rolify_core::config::HolderIdKind::Integer,
            with_holder_fk: false,
        }
    }

    #[test]
    fn seaorm_renderer_substitutes_names_longest_first() {
        let custom_plan = plan("privileges", "customers_privileges");

        let rendered = render_seaorm(&custom_plan).unwrap();

        // Verify longest-first: join table replaced before roles table
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

    #[test]
    fn seaorm_renderer_contains_required_elements() {
        let default_plan = plan("roles", "users_roles");

        let rendered = render_seaorm(&default_plan).unwrap();

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

        // Down drops join first
        let join_pos = rendered.find("users_roles").unwrap();
        let roles_pos = rendered.rfind("roles").unwrap();
        assert!(join_pos < roles_pos, "down should drop join table first");
    }

    #[test]
    fn seaorm_template_compiles() {
        // This test ensures the template is valid Rust when included
        // The sea-orm-migration dev-dep in tests will type-check it
        let default_plan = plan("roles", "users_roles");

        let rendered = render_seaorm(&default_plan).unwrap();

        // Basic structure checks
        assert!(rendered.contains("MigrationTrait"));
        assert!(rendered.contains("DeriveMigrationName"));
        assert!(rendered.contains("async fn up"));
        assert!(rendered.contains("async fn down"));
        assert!(rendered.contains("SchemaManager"));
        assert!(rendered.contains("Statement::from_string"));
    }

    #[test]
    fn seaorm_renderer_emits_three_dialect_arrays() {
        let default_plan = plan("roles", "users_roles");

        let rendered = render_seaorm(&default_plan).unwrap();

        for array_stem in ARRAY_NAME_STEMS {
            assert!(
                rendered.contains(&format!("const {array_stem}_UP_STATEMENTS")),
                "{array_stem} up array missing"
            );
            assert!(
                rendered.contains(&format!("const {array_stem}_DOWN_STATEMENTS")),
                "{array_stem} down array missing"
            );
        }

        // Runtime backend selection with a fallback naming the supported
        // dialects (CR-03), mirroring the rolify-seaorm adapter migration.
        assert!(
            rendered.contains("match manager.get_database_backend()"),
            "runtime backend match missing"
        );
        for variant in [
            "DbBackend::Postgres",
            "DbBackend::MySql",
            "DbBackend::Sqlite",
        ] {
            assert!(
                rendered.contains(variant),
                "{variant} match arm missing from the emitted migration"
            );
        }
        assert!(
            rendered.contains("support Postgres, MySQL, and SQLite"),
            "fallback error must name the supported dialects"
        );
    }

    #[test]
    fn split_statements_rejects_unterminated_tail() {
        let script = "CREATE TABLE roles (id INT);\nSELECT 1";

        let error = split_statements(script).unwrap_err();

        assert!(
            matches!(error, CliError::MalformedCanonicalSql { .. }),
            "unterminated tail must fail loudly, got: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("unterminated statement tail"),
            "error must name the defect: {message}"
        );
        assert!(
            message.contains("SELECT 1"),
            "error must name the tail content: {message}"
        );
    }

    #[test]
    fn split_statements_ignores_semicolons_in_comments() {
        // The canonical mysql header carries a prose line ending in a
        // semicolon; it must ride along, never terminate a statement.
        let script = "-- prose ending with a semicolon;\nCREATE TABLE roles (id INT);";

        let statements = split_statements(script).unwrap();

        assert_eq!(
            statements.len(),
            1,
            "a comment line must never terminate a statement"
        );
        assert!(
            statements[0].starts_with("-- prose ending with a semicolon;"),
            "comment must ride along with its following statement"
        );
        assert!(
            statements[0].ends_with("(id INT);"),
            "statement must run to the real terminator, got: {}",
            statements[0]
        );
    }

    #[test]
    fn split_statements_splits_same_line_commands() {
        // Two commands on one line must never merge into a single prepared
        // statement (the Postgres extended protocol rejects multi-command
        // strings): each semicolon terminates exactly one statement.
        let script = "DROP TABLE IF EXISTS users_roles; DROP TABLE IF EXISTS roles;";

        let statements = split_statements(script).unwrap();

        assert_eq!(
            statements.len(),
            2,
            "same-line commands must split, got: {statements:?}"
        );
        assert!(
            statements[0].ends_with("users_roles;"),
            "first command truncated: {}",
            statements[0]
        );
        assert!(
            statements[1].ends_with("roles;"),
            "second command truncated: {}",
            statements[1]
        );
    }

    #[test]
    fn split_statements_accepts_canonical_scripts() {
        for engine in ENGINES {
            let up_statements = split_statements(templates::up(engine)).unwrap();
            assert_eq!(
                up_statements.len(),
                4,
                "canonical up for {engine} must split into exactly four statements"
            );
            for statement in &up_statements {
                assert!(
                    statement.trim().ends_with(';'),
                    "every up statement for {engine} must be semicolon-terminated"
                );
            }
            let down_statements = split_statements(templates::down(engine)).unwrap();
            assert_eq!(
                down_statements.len(),
                2,
                "canonical down for {engine} must split into exactly two statements"
            );
        }
    }

    #[test]
    fn extract_down_statements_returns_join_first() {
        let default_plan = plan("roles", "users_roles");

        let down_statements =
            extract_down_statements(templates::down("postgres"), &default_plan, "postgres").unwrap();

        assert_eq!(down_statements.len(), 2, "exactly two down statements");
        assert!(
            down_statements[0].contains("DROP TABLE IF EXISTS users_roles;"),
            "join drop must come first, got: {}",
            down_statements[0]
        );
        assert!(
            down_statements[1].contains("DROP TABLE IF EXISTS roles;"),
            "roles drop must come second, got: {}",
            down_statements[1]
        );
    }

    #[test]
    fn extract_down_statements_rejects_multi_drop_statement() {
        // Two DROP TABLE clauses with no semicolon between them cannot be
        // split into separate statements, so the extractor guard must fail
        // loudly instead of emitting a multi-command prepared statement.
        let script = "DROP TABLE IF EXISTS users_roles DROP TABLE IF EXISTS roles;";
        let default_plan = plan("roles", "users_roles");

        let error = extract_down_statements(script, &default_plan, "postgres").unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("2 DROP TABLE clauses"),
            "error must name the multi-drop defect: {message}"
        );
    }

    #[test]
    fn extract_down_statements_rejects_both_classes_in_one_statement() {
        let script = "DROP TABLE IF EXISTS users_roles, roles;";
        let default_plan = plan("roles", "users_roles");

        let error = extract_down_statements(script, &default_plan, "postgres").unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("both the join and the roles drop"),
            "error must name the unclassifiable statement: {message}"
        );
    }

    #[test]
    fn extract_down_statements_rejects_missing_join_drop() {
        let script = "DROP TABLE IF EXISTS roles;";
        let default_plan = plan("roles", "users_roles");

        let error = extract_down_statements(script, &default_plan, "postgres").unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("no join-table drop statement"),
            "error must name the missing join class: {message}"
        );
    }

    #[test]
    fn extract_down_statements_rejects_missing_roles_drop() {
        let script = "DROP TABLE IF EXISTS users_roles;";
        let default_plan = plan("roles", "users_roles");

        let error = extract_down_statements(script, &default_plan, "postgres").unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("no roles-table drop statement"),
            "error must name the missing roles class: {message}"
        );
    }

    #[test]
    fn extract_down_statements_rejects_duplicate_class_claim() {
        let script = "DROP TABLE IF EXISTS users_roles;\nDROP TABLE IF EXISTS users_roles_backup;";
        let default_plan = plan("roles", "users_roles");

        let error = extract_down_statements(script, &default_plan, "postgres").unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("more than one join-table drop statement"),
            "error must name the duplicate claim: {message}"
        );
    }

    #[test]
    fn extract_down_statements_ignores_comment_prose() {
        // The canonical comment mentions both stems in prose; classification
        // must still see exactly one class per statement.
        let script = "-- drop join table first, then roles.\n\nDROP TABLE IF EXISTS users_roles;\nDROP TABLE IF EXISTS roles;";
        let custom_plan = plan("privileges", "customers_privileges");

        let down_statements = extract_down_statements(script, &custom_plan, "postgres").unwrap();

        assert_eq!(down_statements.len(), 2);
        assert!(
            down_statements[0].contains("DROP TABLE IF EXISTS customers_privileges;"),
            "join drop must carry the substituted name: {}",
            down_statements[0]
        );
        assert!(
            down_statements[1].contains("DROP TABLE IF EXISTS privileges;"),
            "roles drop must carry the substituted name: {}",
            down_statements[1]
        );
    }
}
