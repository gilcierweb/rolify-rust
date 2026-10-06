//! Emitter dispatch - single renderer for diesel and sqlx (D-11).

pub mod mongo;
pub mod seaorm;
pub mod sql;

use crate::args::Backend;
use crate::error::CliError;
use crate::render::RenderPlan;

/// Renders the up and down migrations for the given backend and plan.
///
/// Diesel and Sqlx share the same SQL renderer (D-11 identity by construction).
/// This function uses the postgres template as default; the per-engine iteration
/// happens in `render_all` which calls `render_sql` directly.
/// `SeaORM` and `MongoDB` have their own emitters.
///
/// # Errors
///
/// Returns `CliError::Core` when rendering fails for the given backend.
pub fn render(backend: &Backend, plan: &RenderPlan) -> Result<(String, String), CliError> {
    match backend {
        Backend::Diesel | Backend::Sqlx => sql::render_sql(plan, "postgres"),
        Backend::Seaorm => {
            let content = seaorm::render_seaorm(plan)?;
            Ok((content, String::new())) // SeaORM has single file, down is embedded
        }
        Backend::Mongodb => Err(CliError::Core(
            rolify_core::error::RolifyError::InvalidConfig {
                reason: "MongoDB emitter returns (role_doc, index_notes) tuple, use render_mongo directly".into(),
            },
        )),
    }
}

/// Renders the `MongoDB` role document and index notes.
///
/// # Errors
///
/// Propagates the inner mongo renderer's error.
pub fn render_mongo(plan: &RenderPlan) -> Result<(String, String), CliError> {
    mongo::render_mongo(plan)
}
