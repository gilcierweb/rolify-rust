//! `MongoDB` role document emitter - doc struct + index notes (D-09).
//!
//! Shape strictly from the 05-CONTEXT spec (D-07, D-08):
//! - Fields: name (String), `resource_type` (Option<String>), `resource_id`
//!   (Option<String>), `user_ids` (Vec<ObjectId>)
//! - Unique compound index on (name, `resource_type`, `resource_id`)
//! - Consumer-side `role_ids` with two-sided HABTM removal semantics
//! - Emptiness checked after removal
//!
//! PROVISIONAL: Converges with `rolify-mongodb` `document.rs` when Phase 5
//! executes (D-17). No BSON-type invention beyond the spec.

use crate::error::CliError;
use crate::render::RenderPlan;
use crate::templates;

/// Renders the `MongoDB` role document struct plus index notes.
///
/// Returns a tuple of (`role_doc_content`, `index_notes_content`).
///
/// # Errors
///
/// Never fails in practice; the error type keeps the emitter contract uniform.
pub fn render_mongo(plan: &RenderPlan) -> Result<(String, String), CliError> {
    let template = templates::mongo_docs();

    // Substitute table name references in comments
    // Longest-first: join_table before roles_table
    let substituted = template
        .replace("users_roles", &plan.join_table)
        .replace("roles", &plan.roles_table);

    // Split the template into role_doc (before INDEX_NOTES const) and index_notes
    let (role_doc, index_notes) = split_template(&substituted);

    Ok((role_doc, index_notes))
}

/// Splits the template into `role_doc` (before `INDEX_NOTES` const) and `index_notes`.
fn split_template(template: &str) -> (String, String) {
    // Find the start of the INDEX_NOTES const definition
    if let Some(idx) = template.find("pub const INDEX_NOTES:") {
        // Include any preceding blank line in the role_doc
        let role_doc_end = if idx > 0 && &template[idx - 1..idx] == "\n" {
            // Check if there's a blank line before (two newlines)
            if idx > 1 && &template[idx - 2..idx] == "\n\n" {
                idx - 1 // Include the blank line
            } else {
                idx
            }
        } else {
            idx
        };
        let role_doc = template[..role_doc_end].trim_end().to_string();
        let index_notes = template[role_doc_end..].to_string();
        (role_doc, index_notes)
    } else {
        // Fallback: no INDEX_NOTES found
        (template.to_string(), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Backend;
    use crate::render::RenderPlan;

    #[test]
    fn mongo_renderer_produces_expected_fields() {
        let plan = RenderPlan {
            backend: Backend::Mongodb,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: "roles".to_string(),
            join_table: "users_roles".to_string(),
        };

        let (role_doc, index_notes) = render_mongo(&plan).unwrap();

        // Verify field set frozen to 05-CONTEXT spec (Pitfall 8)
        assert!(role_doc.contains("name: String"), "name field missing");
        assert!(
            role_doc.contains("resource_type: Option<String>"),
            "resource_type field missing"
        );
        assert!(
            role_doc.contains("resource_id: Option<String>"),
            "resource_id field missing"
        );
        assert!(
            role_doc.contains("user_ids: Vec<ObjectId>"),
            "user_ids field missing"
        );

        // No camelCase
        assert!(!role_doc.contains("resourceType"), "camelCase detected");
        assert!(!role_doc.contains("resourceId"), "camelCase detected");
        assert!(!role_doc.contains("userIds"), "camelCase detected");

        // Unique compound index note
        assert!(
            index_notes.contains("Unique compound index"),
            "index note missing"
        );
        assert!(
            index_notes.contains("resource_type") && index_notes.contains("resource_id"),
            "index fields missing"
        );
    }

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

    #[test]
    fn mongo_renderer_provisional_marker_present() {
        let plan = RenderPlan {
            backend: Backend::Mongodb,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: "roles".to_string(),
            join_table: "users_roles".to_string(),
        };

        let (role_doc, _) = render_mongo(&plan).unwrap();

        // PROVISIONAL marker per D-17
        assert!(
            role_doc.contains("PROVISIONAL"),
            "PROVISIONAL marker missing"
        );
        assert!(role_doc.contains("D-17"), "D-17 reference missing");
    }
}
