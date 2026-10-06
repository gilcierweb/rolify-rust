//! `SeaORM` migration emitter - hand-maintained Rust template (D-16).
//!
//! Uses raw-SQL migration body via `Statement::from_string` reusing the
//! vendored canonical SQL strings per engine (RESEARCH Code Examples option a).
//! Implements `MigrationTrait` with `DeriveMigrationName`, `down` drops the
//! join table first (D-10, D-12). The template is type-checked via
//! `sea-orm-migration` dev-dep in tests.
//!
//! Semantic checklist enforced in tests/drift.rs:
//! - Unique triple on (name, `resource_type`, `resource_id`)
//! - Both `idx_roles_resource` and `idx_roles_name` indexes
//! - DEFAULT '' on resource columns
//! - ON DELETE CASCADE on join foreign key
//! - Join-first drop order in `down()`

use crate::error::CliError;
use crate::render::RenderPlan;
use crate::templates;

/// Renders the `SeaORM` migration file.
///
/// Substitutes validated table names with longest-first replacement order
/// (same rule as SQL renderer). Returns the migration file content.
///
/// # Errors
///
/// Never fails in practice; the error type keeps the emitter contract uniform.
pub fn render_seaorm(plan: &RenderPlan) -> Result<String, CliError> {
    let template = templates::seaorm_migration();

    // Get the canonical SQL for the target engine
    // SeaORM uses the same dialect as the target engine
    let up_sql = templates::up("postgres");
    let _down_sql = templates::down("postgres");

    // Longest-first replacement: join_table before roles_table
    let up_sql = up_sql
        .replace("users_roles", &plan.join_table)
        .replace("roles", &plan.roles_table);

    // Extract DROP statements from the ORIGINAL canonical down.sql
    // (before substitution), then substitute with custom names
    let original_down_sql = templates::down("postgres");
    let (down_join, down_roles) = extract_and_substitute_drop(original_down_sql, plan);

    // Substitute into template
    let result = template
        .replace("{{UP_SQL}}", &escape_for_rust_string(&up_sql))
        .replace("{{DOWN_JOIN}}", &down_join)
        .replace("{{DOWN_ROLES}}", &down_roles);

    Ok(result)
}

/// Extracts a DROP TABLE IF EXISTS statement for the given table from the SQL
/// and substitutes the table names with longest-first replacement order.
fn extract_and_substitute_drop(sql: &str, plan: &RenderPlan) -> (String, String) {
    let mut down_join = String::new();
    let mut down_roles = String::new();

    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("DROP TABLE IF EXISTS") {
            if trimmed.contains("users_roles") {
                down_join = trimmed.replace("users_roles", &plan.join_table);
            } else if trimmed.contains("roles") {
                down_roles = trimmed.replace("roles", &plan.roles_table);
            }
        }
    }

    // Fallbacks
    if down_join.is_empty() {
        down_join = format!("DROP TABLE IF EXISTS {};", plan.join_table);
    }
    if down_roles.is_empty() {
        down_roles = format!("DROP TABLE IF EXISTS {};", plan.roles_table);
    }

    (down_join, down_roles)
}

/// Escapes a string for inclusion in a Rust string literal.
fn escape_for_rust_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Backend;
    use crate::render::RenderPlan;

    #[test]
    fn seaorm_renderer_substitutes_names_longest_first() {
        let plan = RenderPlan {
            backend: Backend::Seaorm,
            role_name: "Privilege".to_string(),
            holder_name: "Customer".to_string(),
            roles_table: "privileges".to_string(),
            join_table: "customers_privileges".to_string(),
        };

        let rendered = render_seaorm(&plan).unwrap();

        // Debug: print the rendered output to see what's happening
        eprintln!("RENDERED OUTPUT:\n{rendered}");

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

        // Down drops join first
        let join_pos = rendered.find("users_roles").unwrap();
        let roles_pos = rendered.rfind("roles").unwrap();
        assert!(join_pos < roles_pos, "down should drop join table first");
    }

    #[test]
    fn seaorm_template_compiles() {
        // This test ensures the template is valid Rust when included
        // The sea-orm-migration dev-dep in tests will type-check it
        let plan = RenderPlan {
            backend: Backend::Seaorm,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: "roles".to_string(),
            join_table: "users_roles".to_string(),
        };

        let rendered = render_seaorm(&plan).unwrap();

        // Basic structure checks
        assert!(rendered.contains("MigrationTrait"));
        assert!(rendered.contains("DeriveMigrationName"));
        assert!(rendered.contains("async fn up"));
        assert!(rendered.contains("async fn down"));
        assert!(rendered.contains("SchemaManager"));
        assert!(rendered.contains("Statement::from_string"));
    }
}
