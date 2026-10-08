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

    // Split the template into role_doc (complete Rust source) and
    // index_notes (pure Markdown)
    let (role_doc, index_notes) = split_template(&substituted);

    Ok((role_doc, index_notes))
}

/// Splits the template into `role_doc` (complete Rust source) and
/// `index_notes` (pure Markdown).
///
/// The boundary is the Markdown level-one heading that opens the notes
/// section: a Rust line never starts with `# ` (attribute lines open with
/// `#[`), so the first such line is unambiguous. Both halves end with
/// exactly one trailing newline: the role doc closes on the consumer
/// struct instead of an orphaned doc comment, and the notes open on the
/// heading instead of a blank line or a `const` wrapper (WR-02).
fn split_template(template: &str) -> (String, String) {
    // The heading is preceded by a newline; +1 lands on the heading start.
    let Some(heading_start) = template
        .find("\n# ")
        .map(|newline_position| newline_position + 1)
    else {
        // No Markdown heading: the whole template is the role doc and the
        // notes half is empty. The drift suite's well-formedness test fails
        // loudly on the empty notes, so a missing boundary cannot ship.
        return (format!("{}\n", template.trim_end()), String::new());
    };

    let role_doc = format!("{}\n", template[..heading_start].trim_end());
    let index_notes = format!("{}\n", template[heading_start..].trim_end());
    (role_doc, index_notes)
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
