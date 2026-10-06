//! Pure writer: writes the rendered plan to disk.
//!
//! Dry-run short-circuits before any filesystem call. Fail-if-exists default
//!
//! names every colliding file; --force overwrites.

use crate::error::CliError;
use crate::render::FileEntry;
use std::fs;
use std::path::PathBuf;

/// Options controlling the write behavior.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    pub out_dir: PathBuf,
    pub force: bool,
    pub dry_run: bool,
}

/// Writes the rendered plan to disk.
///
/// Returns Ok(()) on success. On collision without --force, returns
/// CliError::AlreadyExists naming every colliding file.
pub fn write_plan(
    rendered: &std::collections::BTreeMap<String, Vec<FileEntry>>,
    options: &WriteOptions,
) -> Result<(), CliError> {
    // Collect all target paths first
    let mut all_paths = Vec::new();
    for (_engine, files) in rendered {
        for file in files {
            all_paths.push(options.out_dir.join(&file.path));
        }
    }

    // Check for collisions
    if !options.force {
        let mut collisions = Vec::new();
        for path in &all_paths {
            if path.exists() {
                collisions.push(path.display().to_string());
            }
        }
        if !collisions.is_empty() {
            return Err(CliError::AlreadyExists {
                files: collisions.join(", "),
            });
        }
    }

    // Dry-run: print the file plan and exit
    if options.dry_run {
        println!("DRY RUN - would create:");
        for (engine, files) in rendered {
            println!("  Engine: {engine}");
            for file in files {
                println!("    {}", options.out_dir.join(&file.path).display());
            }
        }
        return Ok(());
    }

    // Write all files
    for (_engine, files) in rendered {
        for file in files {
            let target_path = options.out_dir.join(&file.path);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target_path, &file.content)?;
        }
    }

    Ok(())
}
