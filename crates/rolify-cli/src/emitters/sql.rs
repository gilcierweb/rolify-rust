//! Single SQL renderer for Diesel and Sqlx (D-11 identity by construction).
//!
//! Uses ordered string replacement: join table stem first (longest-first rule),
//! then roles table stem. Names are already validated by validate_identifier.

use crate::render::RenderPlan;
use crate::templates::{down, up};
use crate::error::CliError;

/// Renders the SQL migrations for the given plan.
///
/// The longest-first replacement rule: replace the join table name (which
/// contains the roles table name as a substring in the derived case) BEFORE
/// replacing the roles table name, to avoid partial replacement.
#[must_use]
pub fn render_sql(plan: &RenderPlan) -> Result<(String, String), CliError> {
    let backend = match plan.backend.as_str() {
        "diesel" => "diesel",
        "sqlx" => "diesel", // sqlx uses the same PostgreSQL dialect templates
        _ => return Err(CliError::Core(rolify_core::error::RolifyError::InvalidConfig {
            reason: format!("unexpected backend for SQL renderer: {}", plan.backend.as_str()),
        })),
    };

    let up_sql = up(backend);
    let down_sql = down(backend);

    // Longest-first: replace join_table before roles_table
    let up_sql = up_sql
        .replace("users_roles", &plan.join_table)
        .replace("roles", &plan.roles_table);
    let down_sql = down_sql
        .replace("users_roles", &plan.join_table)
        .replace("roles", &plan.roles_table);

    Ok((up_sql, down_sql))
}