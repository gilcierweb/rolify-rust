//! Pure writer: writes the rendered plan to disk.
//!
//! Dry-run prints the file plan and returns before any filesystem call, so
//! a preview works whether or not the target tree already exists. A write
//! run without --force fails naming every colliding file; --force
//! overwrites.

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
/// Dry-run prints the file plan and returns before any filesystem call, so
/// it previews even over an existing tree. On collision without --force,
/// returns `CliError::AlreadyExists` naming every colliding file.
///
/// # Errors
///
/// Returns `CliError::AlreadyExists` on collisions without `--force`,
/// `CliError::Io` on filesystem failures, and `CliError::Core` never in
/// practice (kept for contract uniformity).
pub fn write_plan(
    rendered: &std::collections::BTreeMap<String, Vec<FileEntry>>,
    options: &WriteOptions,
) -> Result<(), CliError> {
    // Dry-run previews the plan and returns before any filesystem call:
    // previewing over an existing tree is exactly when a preview is
    // useful, so the collision scan must not run first (WR-04).
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

    // Collect all target paths, then scan for collisions (write path only)
    let mut all_paths = Vec::new();
    for files in rendered.values() {
        for file in files {
            all_paths.push(options.out_dir.join(&file.path));
        }
    }

    // Fail-if-exists default: a write run without --force names every
    // colliding file instead of touching anything.
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

    // Write all files
    for files in rendered.values() {
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
