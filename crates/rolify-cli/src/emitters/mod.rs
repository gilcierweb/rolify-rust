//! Emitter dispatch — single renderer for diesel and sqlx (D-11).

pub mod sql;

use crate::args::Backend;
use crate::error::CliError;
use crate::render::RenderPlan;

/// Renders the up and down migrations for the given backend and plan.
///
/// Diesel and Sqlx share the same SQL renderer (D-11 identity by construction).
/// SeaORM and MongoDB will have their own emitters in Plan 02.
pub fn render(backend: Backend, plan: &RenderPlan) -> Result<(String, String), CliError> {
    match backend {
        Backend::Diesel | Backend::Sqlx => sql::render_sql(plan),
        Backend::Seaorm => Err(CliError::Core(
            rolify_core::error::RolifyError::InvalidConfig {
                reason: "SeaORM emitter not yet implemented (Plan 02)".into(),
            },
        )),
        Backend::Mongodb => Err(CliError::Core(
            rolify_core::error::RolifyError::InvalidConfig {
                reason: "MongoDB emitter not yet implemented (Plan 02)".into(),
            },
        )),
    }
}