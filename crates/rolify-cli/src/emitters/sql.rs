//! Single SQL renderer for Diesel and Sqlx (D-11 identity by construction).
//!
//! Name substitution flows through the shared re-scan-free helper
//! (`emitters::substitute_table_names`, D-14), so this renderer can never
//! mangle an explicit join table embedding the roles stem. Names are already
//! validated by `validate_identifier`.

use crate::emitters::substitute_table_names;
use crate::error::CliError;
use crate::render::RenderPlan;
use crate::templates::{down, up};

/// Renders the SQL migrations for the given plan and engine.
///
/// Both the up and the down script go through the re-scan-free substitution
/// helper: the template is split on the canonical join sentinel first, the
/// roles stem is replaced per original segment, and the requested join name
/// is joined in last (WR-01).
///
/// # Errors
///
/// Returns `CliError::Core` when the engine is not one of postgres, mysql, sqlite.
pub fn render_sql(plan: &RenderPlan, engine: &str) -> Result<(String, String), CliError> {
    let template_engine = match engine {
        "postgres" => "postgres",
        "mysql" => "mysql",
        "sqlite" => "sqlite",
        _ => {
            return Err(CliError::Core(
                rolify_core::error::RolifyError::InvalidConfig {
                    reason: format!("unknown engine for SQL renderer: {engine}"),
                },
            ));
        }
    };

    let up_sql = substitute_table_names(up(template_engine), plan);
    let down_sql = substitute_table_names(down(template_engine), plan);

    Ok((up_sql, down_sql))
}
