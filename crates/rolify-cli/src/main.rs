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
            let out_dir = args.out_dir.clone();
            let force = args.force;
            let dry_run = args.dry_run;

            // The plan derives (or takes) the join table exactly once
            // (WR-03): validation below checks the SAME value that
            // flows into SQL and scaffolding, never a recomputed twin.
            let plan = RenderPlan::from_args(args);

            // Validate identifiers before any rendering
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&plan.roles_table)?;
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&plan.join_table)?;
            // Also validate role_name and holder_name (they become table name fragments)
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&plan.role_name)?;
            rolify_core::config::RolifyConfigBuilder::validate_identifier(&plan.holder_name)?;

            let rendered = render_all(&plan)?;

            let options = WriteOptions {
                out_dir: PathBuf::from(out_dir),
                force,
                dry_run,
            };

            write_plan(&rendered, &options)?;
        }
    }

    Ok(())
}
