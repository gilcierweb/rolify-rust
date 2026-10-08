use anyhow::Result;
use clap::Parser;
use rolify_cli::args::{Cli, Command};
use rolify_cli::render::{RenderPlan, render_all};
use rolify_cli::writer::{WriteOptions, write_plan};
use std::path::PathBuf;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Generate(args) => {
            // Default join table derives as holder plural + _ + roles_table,
            // exactly the derivation documented in the args.rs help text:
            // the derived value is computed BEFORE validation and then flows
            // as the single RenderPlan join_table into SQL and scaffolding
            // alike (CR-01, D-05).
            let join_table = args.join_table.unwrap_or_else(|| {
                format!(
                    "{}_{}",
                    args.holder_name.to_lowercase() + "s",
                    args.roles_table
                )
            });

            // Validate identifiers before any rendering
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&args.roles_table)?;
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&join_table)?;
            // Also validate role_name and holder_name (they become table name fragments)
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&args.role_name)?;
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&args.holder_name)?;

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
