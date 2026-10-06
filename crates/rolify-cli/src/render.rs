//! Pure render: builds the file plan (path + bytes) for mirrored trees.
//!
//! No filesystem I/O, no runtime DB access — just path computation and
//! content generation. D-22 mirrored-tree layout with D-19 timestamp stem.

use crate::args::Backend;
use crate::emitters::{render_mongo, seaorm::render_seaorm, sql::render_sql};
use crate::error::CliError;
use std::collections::BTreeMap;

/// Input plan for rendering.
#[derive(Debug, Clone)]
pub struct RenderPlan {
    pub backend: Backend,
    pub role_name: String,
    pub holder_name: String,
    pub roles_table: String,
    pub join_table: String,
}

/// A single file entry in the render output.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub content: String,
}

/// Renders all engines for the given plan, returning a map of engine -> files.
///
/// Each engine gets a mirrored tree under migrations/{engine}/ with the
/// timestamp stem 0000000001_rolify_create_tables containing up.sql and down.sql.
/// SeaORM gets a single migration file under seaorm/.
/// Mongo gets role.rs and INDEX_NOTES.md under mongo/.
pub fn render_all(plan: &RenderPlan) -> Result<BTreeMap<String, Vec<FileEntry>>, CliError> {
    let mut result = BTreeMap::new();

    match plan.backend.as_str() {
        "diesel" | "sqlx" => {
            let engines = vec!["postgres", "mysql", "sqlite"];
            for engine in engines {
                let (up_content, down_content) = render_sql(plan, engine)?;

                let stem = "0000000001_rolify_create_tables";
                let prefix = format!("migrations/{engine}/{stem}");

                let mut files = vec![
                    FileEntry {
                        path: format!("{prefix}/up.sql"),
                        content: up_content,
                    },
                    FileEntry {
                        path: format!("{prefix}/down.sql"),
                        content: down_content,
                    },
                ];

                // Sort for deterministic output
                files.sort_by(|a, b| a.path.cmp(&b.path));

                result.insert(engine.to_string(), files);
            }
        }
        "seaorm" => {
            let content = render_seaorm(plan)?;
            let stem = "0000000001_rolify_create_tables";
            let mut files = vec![FileEntry {
                path: format!("seaorm/{stem}.rs"),
                content,
            }];
            files.sort_by(|a, b| a.path.cmp(&b.path));
            result.insert("seaorm".to_string(), files);
        }
        "mongodb" => {
            let (role_doc, index_notes) = render_mongo(plan)?;
            let mut files = vec![
                FileEntry {
                    path: "mongo/role.rs".to_string(),
                    content: role_doc,
                },
                FileEntry {
                    path: "mongo/INDEX_NOTES.md".to_string(),
                    content: index_notes,
                },
            ];
            files.sort_by(|a, b| a.path.cmp(&b.path));
            result.insert("mongo".to_string(), files);
        }
        _ => {
            return Err(CliError::Core(
                rolify_core::error::RolifyError::InvalidConfig {
                    reason: format!("unknown backend: {}", plan.backend.as_str()),
                },
            ));
        }
    }

    Ok(result)
}
