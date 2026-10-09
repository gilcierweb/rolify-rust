//! Pure render: builds the file plan (path + bytes) for mirrored trees.
//!
//! No filesystem I/O, no runtime DB access - just path computation and
//! content generation. D-22 mirrored-tree layout with D-19 timestamp stem.

use crate::args::{Backend, CliHolderIdKind};
use rolify_core::HolderIdKind;
use crate::emitters::{mongo::render_mongo, seaorm::render_seaorm, sql::render_sql};
use crate::error::CliError;
use crate::templates::scaffolding::{config_example, holder_stub, readme, role_stub};
use std::collections::BTreeMap;

/// Input plan for rendering.
#[derive(Debug, Clone)]
pub struct RenderPlan {
    pub backend: Backend,
    pub role_name: String,
    pub holder_name: String,
    pub roles_table: String,
    pub join_table: String,
    pub holder_id_kind: HolderIdKind,
    pub with_holder_fk: bool,
}

impl RenderPlan {
    /// Creates a RenderPlan from CLI args.
    pub fn from_args(args: crate::args::GenerateArgs) -> Self {
        let join_table = args.join_table.clone().unwrap_or_else(|| {
            let holder_plural = if args.holder_name.ends_with('s') {
                format!("{}_", args.holder_name.to_lowercase())
            } else {
                format!("{}s_", args.holder_name.to_lowercase())
            };
            format!("{}{}", holder_plural, args.roles_table)
        });
        Self {
            backend: args.backend,
            role_name: args.role_name,
            holder_name: args.holder_name,
            roles_table: args.roles_table,
            join_table,
            holder_id_kind: args.holder_id_type.into(),
            with_holder_fk: args.with_holder_fk,
        }
    }
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
/// timestamp stem `0000000001_rolify_create_tables` containing up.sql and down.sql.
/// `SeaORM` gets a single migration file under seaorm/.
/// Mongo gets role.rs and `INDEX_NOTES.md` under mongo/.
/// All backends get scaffolding: `role_stub.rs`, `holder_stub.rs`, `config_example.rs`, README.md
///
/// # Errors
///
/// Returns `CliError::Core` when the backend is unknown or an emitter fails.
pub fn render_all(plan: &RenderPlan) -> Result<BTreeMap<String, Vec<FileEntry>>, CliError> {
    let mut result = BTreeMap::new();

    // Render main migration files
    let migration_files = render_migrations(plan)?;
    result.extend(migration_files);

    // Render scaffolding files for all backends
    let scaffolding_files = render_scaffolding(plan)?;
    for (engine, files) in scaffolding_files {
        result.entry(engine).or_default().extend(files);
    }

    Ok(result)
}

/// Renders migration files for the given backend.
fn render_migrations(plan: &RenderPlan) -> Result<BTreeMap<String, Vec<FileEntry>>, CliError> {
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

/// Renders scaffolding files (stubs, config example, README) for all backends.
fn render_scaffolding(plan: &RenderPlan) -> Result<BTreeMap<String, Vec<FileEntry>>, CliError> {
    let mut result = BTreeMap::new();

    let engines = match plan.backend.as_str() {
        "diesel" | "sqlx" => vec!["postgres", "mysql", "sqlite"],
        "seaorm" => vec!["seaorm"],
        "mongodb" => vec!["mongo"],
        _ => {
            return Err(CliError::Core(
                rolify_core::error::RolifyError::InvalidConfig {
                    reason: format!("unknown backend: {}", plan.backend.as_str()),
                },
            ));
        }
    };

    for engine in engines {
        let scaffolding = generate_scaffolding(plan, engine)?;
        result.insert(engine.to_string(), scaffolding);
    }

    Ok(result)
}

/// Generates scaffolding files for a specific engine.
///
/// The README consumption story keys on the BACKEND the user chose, never on
/// the engine directory the file lands in (D-02, D-20): an `--backend sqlx`
/// run ships the sqlx Migrator story into all three engine directories,
/// and the diesel runner instructions only reach diesel consumers (CR-02).
fn generate_scaffolding(plan: &RenderPlan, engine: &str) -> Result<Vec<FileEntry>, CliError> {
    let readme_name = match plan.backend {
        Backend::Diesel => "README_diesel",
        Backend::Sqlx => "README_sqlx",
        Backend::Seaorm => "README_seaorm",
        Backend::Mongodb => "README_mongodb",
    };
    let readme_content = readme(readme_name)
        .map_err(|error| {
            CliError::Core(rolify_core::error::RolifyError::InvalidConfig {
                reason: format!("unknown README template: {error}"),
            })
        })?
        .replace("{role_name}", &plan.role_name)
        .replace("{holder_name}", &plan.holder_name)
        .replace("{backend}", plan.backend.as_str());

    let holder_id_kind_str = match plan.holder_id_kind {
        HolderIdKind::Integer => "Integer",
        HolderIdKind::Uuid => "Uuid",
        HolderIdKind::String => "String",
    };

    let mut files = vec![
        FileEntry {
            path: format!("{engine}/role_stub.rs"),
            content: role_stub(&plan.role_name, plan.backend.as_str(), &plan.holder_name),
        },
        FileEntry {
            path: format!("{engine}/holder_stub.rs"),
            content: holder_stub(
                &plan.holder_name,
                plan.backend.as_str(),
                &plan.role_name,
                holder_id_kind_str,
            ),
        },
        FileEntry {
            path: format!("{engine}/config_example.rs"),
            content: config_example(
                plan.backend.as_str(),
                &plan.role_name,
                &plan.holder_name,
                &plan.roles_table,
                &plan.join_table,
                holder_id_kind_str,
            ),
        },
        FileEntry {
            path: format!("{engine}/README.md"),
            content: readme_content,
        },
    ];

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}
