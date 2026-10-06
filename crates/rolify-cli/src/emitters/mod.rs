//! Emitter dispatch for the four backends.
//!
//! Diesel and Sqlx share ONE renderer (`sql::render_sql`), so D-11 identity
//! holds by construction: `render_all` calls it directly per engine. `SeaORM`
//! and `MongoDB` have their own hand-maintained-template emitters.

pub mod mongo;
pub mod seaorm;
pub mod sql;

use crate::error::CliError;
use crate::render::RenderPlan;

/// Renders the `MongoDB` role document and index notes.
///
/// # Errors
///
/// Propagates the inner mongo renderer's error.
pub fn render_mongo(plan: &RenderPlan) -> Result<(String, String), CliError> {
    mongo::render_mongo(plan)
}
