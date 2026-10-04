//! Byte-identity drift guard between the vendored sqlx migration trees and
//! the canonical `rolify-diesel` trees (D-03/D-04/D-05).
//!
//! Plain sync fs test by design: no containers, no async. It runs in every
//! `cargo test` (and therefore enters CI for free, D-03) and fails loudly
//! the moment either tree is missing, a migration is vendored on one side
//! only, or any file diverges by a single byte.
//!
//! Layout mapping (D-04, naming chosen once): the canonical diesel tree uses
//! the directory layout `<engine>/<VERSION>_<DESCRIPTION>/{up,down}.sql`
//! while the vendored sqlx tree uses the flat `MigrationSource` layout
//! `<engine>/<VERSION>_<DESCRIPTION>.sql` (+ `.down.sql`). Byte-identity
//! applies to SQL content; the shared `<VERSION>_<DESCRIPTION>` segment is
//! the dual-compatible name both layouts keep verbatim.
//!
//! D-07 note: pre-1.0 schema changes REWRITE the initial migration rather
//! than append (single file = canonical schema of the moment), so this guard
//! compares the single-version trees exactly as they are.
//!
//! T-04-02: the guard asserts BOTH tree directories exist (the panic
//! message carries both resolved paths) before any comparison, so a moved
//! or deleted canonical tree fails loud instead of vacuously passing an
//! empty intersection.

use std::fs;
use std::path::{Path, PathBuf};

/// The per-engine comparison scope (D-05): both crates support all three
/// engines, so the intersection is the full set. Nothing else is vendored.
const ENGINES: [&str; 3] = ["postgres", "mysql", "sqlite"];

#[test]
fn vendored_migrations_are_byte_identical_to_canonical() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    for engine in ENGINES {
        let canonical_engine_dir =
            manifest_dir
                .join("..")
                .join("rolify-diesel")
                .join("migrations")
                .join(engine);
        let vendored_engine_dir = manifest_dir.join("migrations").join(engine);

        // T-04-02: both trees must exist before anything is compared, with
        // both resolved paths in the failure message.
        assert!(
            canonical_engine_dir.is_dir() && vendored_engine_dir.is_dir(),
            "both migration trees must exist for engine `{engine}`: canonical {} and vendored {}",
            canonical_engine_dir.display(),
            vendored_engine_dir.display(),
        );

        // Direction 1 (canonical-driven): every canonical migration must be
        // vendored on the flat layout, byte-identically, both directions of
        // the file pair.
        for canonical_migration_dir in sorted_entries(&canonical_engine_dir) {
            assert!(
                canonical_migration_dir.is_dir(),
                "canonical migration entry `{}` must be a directory (diesel layout \
                 `<VERSION>_<DESCRIPTION>/`), not a file",
                canonical_migration_dir.display(),
            );
            let migration_name = canonical_migration_dir
                .file_name()
                .expect("canonical migration directory always has a file name")
                .to_string_lossy()
                .into_owned();
            assert_version_segment_is_positive(&migration_name, &canonical_migration_dir);

            let canonical_up = canonical_migration_dir.join("up.sql");
            let canonical_down = canonical_migration_dir.join("down.sql");
            let vendored_up = vendored_engine_dir.join(format!("{migration_name}.sql"));
            let vendored_down = vendored_engine_dir.join(format!("{migration_name}.down.sql"));

            assert!(
                vendored_up.is_file() && canonical_up.is_file(),
                "migration `{migration_name}` (engine `{engine}`) must exist as an up file on \
                 both trees: vendored {} and canonical {}",
                vendored_up.display(),
                canonical_up.display(),
            );
            assert!(
                vendored_down.is_file() && canonical_down.is_file(),
                "migration `{migration_name}` (engine `{engine}`) must exist as a down file on \
                 both trees: vendored {} and canonical {}",
                vendored_down.display(),
                canonical_down.display(),
            );

            assert_byte_equal(&vendored_up, &canonical_up);
            assert_byte_equal(&vendored_down, &canonical_down);
        }

        // Direction 2 (vendored-driven): every vendored flat file must map
        // back to the canonical tree, so stray or extra vendored files can
        // never slip past the canonical-driven direction.
        for vendored_file in sorted_entries(&vendored_engine_dir) {
            assert!(
                vendored_file.is_file(),
                "vendored migration entry `{}` must be a file (sqlx flat layout), not a \
                 directory",
                vendored_file.display(),
            );
            let vendored_name = vendored_file
                .file_name()
                .expect("vendored migration file always has a file name")
                .to_string_lossy()
                .into_owned();
            assert_version_segment_is_positive(&vendored_name, &vendored_file);

            let canonical_counterpart = if let Some(stem) = vendored_name.strip_suffix(".down.sql")
            {
                canonical_engine_dir.join(stem).join("down.sql")
            } else if let Some(stem) = vendored_name.strip_suffix(".sql") {
                canonical_engine_dir.join(stem).join("up.sql")
            } else {
                panic!(
                    "vendored migration `{vendored_name}` (engine `{engine}`) must end in `.sql` \
                     or `.down.sql` (sqlx MigrationSource layout): {}",
                    vendored_file.display()
                );
            };

            assert!(
                canonical_counterpart.is_file(),
                "vendored migration `{vendored_name}` (engine `{engine}`) has no canonical \
                 counterpart at {}",
                canonical_counterpart.display(),
            );
            assert_byte_equal(&vendored_file, &canonical_counterpart);
        }
    }
}

/// Read a directory and return its sorted entries so failure messages are
/// deterministic across platforms (`read_dir` order is arbitrary).
///
/// # Panics
///
/// Panics if the directory cannot be read or an entry cannot be listed.
fn sorted_entries(directory: &Path) -> Vec<PathBuf> {
    let read_dir = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()));
    let mut entries: Vec<PathBuf> = read_dir
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("cannot list {}: {error}", directory.display()))
                .path()
        })
        .collect();
    entries.sort();
    entries
}

/// Assert both files carry exactly the same bytes (D-04 byte-identity).
///
/// # Panics
///
/// Panics if either file cannot be read or the byte vectors differ.
fn assert_byte_equal(vendored: &Path, canonical: &Path) {
    let vendored_bytes = fs::read(vendored)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", vendored.display()));
    let canonical_bytes = fs::read(canonical)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", canonical.display()));
    assert_eq!(
        vendored_bytes,
        canonical_bytes,
        "migration bytes diverged: vendored {} vs canonical {}",
        vendored.display(),
        canonical.display(),
    );
}

/// Assert the `<VERSION>_...` segment of a migration name parses as a
/// positive `i64`, mirroring the sqlx `MigrationSource` filename requirement.
///
/// # Panics
///
/// Panics if the name lacks a `<VERSION>_` segment, the segment does not
/// parse as `i64`, or the parsed value is not greater than zero.
fn assert_version_segment_is_positive(file_name: &str, source_path: &Path) {
    let (version, _description) = file_name
        .split_once('_')
        .unwrap_or_else(|| {
            panic!(
                "migration name `{file_name}` lacks a `<VERSION>_<DESCRIPTION>` segment: {}",
                source_path.display()
            )
        });
    let parsed: i64 = version.parse().unwrap_or_else(|_| {
        panic!(
            "migration name `{file_name}` has version segment `{version}` that does not parse \
             as i64: {}",
            source_path.display()
        )
    });
    assert!(
        parsed > 0,
        "migration name `{file_name}` must carry a version greater than zero: {}",
        source_path.display()
    );
}
