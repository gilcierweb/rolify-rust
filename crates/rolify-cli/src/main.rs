use clap::Parser;
use rolify_cli::args::{Cli, Command};
use rolify_cli::render::{render_all, RenderPlan};
use rolify_cli::writer::{write_plan, WriteOptions};
use anyhow::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Generate(args) => {
            let join_table = args.join_table.unwrap_or_else(|| {
                format!("{}_roles", args.holder_name.to_lowercase() + "s")
            });

            // Validate identifiers before any rendering
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&args.roles_table)?;
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&join_table)?;

            let plan = RenderPlan {
                backend: args.backend,
                role_name: args.role_name,
                holder_name: args.holder_name,
                roles_table: args.roles_table,
                join_table,
            };

            let rendered = render_all(&plan)?;

            let options = WriteOptions {
                out_dir: PathBuf::from(args.out_dir),
                force: args.force,
                dry_run: args.dry_run,
            };

            write_plan(&rendered, &options)?;
        }
    }

    Ok(())
}