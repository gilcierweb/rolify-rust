//! Compile-plus-shape gate for the `README_sqlx` Programmatic Migrator recipe (G-06-2).
//!
//! The recipe builds a `sqlx::migrate::Migrator` programmatically from the
//! rolify-generated trees with the exact sqlx 0.9.0 construction calls (the
//! `ReversibleUp` variant for `up.sql`, the `ReversibleDown` variant for
//! `down.sql`, the five-argument `Migration::new` with `SqlStr` built through
//! the audited-string wrapper, and the vector-based `Migrator::with_migrations`).
//! The helper below replicates those calls against a real generated tree, so
//! sqlx API drift fails HERE at compile time instead of hiding behind prose.
//! Zero database access: the Migrator is only constructed and inspected,
//! never run.

mod common;

use common::{rolify_cli, test_temp_dir};
use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::{AssertSqlSafe, SqlSafeStr};
use std::borrow::Cow;
use std::fs;
use std::path::Path;

/// Replicates the `README_sqlx` recipe construction over one engine tree: one
/// `ReversibleUp` entry from `up.sql` plus one `ReversibleDown` entry from
/// `down.sql` per versioned stem directory, combined through the
/// vector-based `Migrator` constructor.
fn build_migrator_like_recipe(migration_dir: &Path) -> Migrator {
    let mut entries: Vec<_> = fs::read_dir(migration_dir)
        .expect("generated migration dir reads")
        .filter_map(std::result::Result::ok)
        .filter(|dir_entry| match dir_entry.file_type() {
            Ok(file_type) => file_type.is_dir(),
            Err(_) => false,
        })
        .collect();

    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut migrations = Vec::new();
    for dir_entry in entries {
        let stem_dir = dir_entry.path();
        let stem_name = stem_dir
            .file_name()
            .expect("stem dir has a name")
            .to_str()
            .expect("stem dir name is UTF-8");
        let version: i64 = stem_name
            .split('_')
            .next()
            .expect("stem carries a timestamp prefix")
            .parse()
            .expect("timestamp prefix parses as i64");
        let description: Cow<'static, str> = stem_name.to_owned().into();

        let up_sql = fs::read_to_string(stem_dir.join("up.sql")).expect("up.sql reads");
        migrations.push(Migration::new(
            version,
            description.clone(),
            MigrationType::ReversibleUp,
            AssertSqlSafe(up_sql).into_sql_str(),
            false,
        ));

        let down_sql =
            fs::read_to_string(stem_dir.join("down.sql")).expect("down.sql reads");
        migrations.push(Migration::new(
            version,
            description,
            MigrationType::ReversibleDown,
            AssertSqlSafe(down_sql).into_sql_str(),
            false,
        ));
    }

    Migrator::with_migrations(migrations)
}

/// The recipe construction proves its shape against a real generated sqlx
/// tree: two entries at version 1, the up and down reversible types in
/// order, matching descriptions naming the stem, and non-empty SQL carrying
/// the canonical DDL.
#[test]
fn sqlx_recipe_constructs_typed_migrator_from_generated_tree() {
    let work_dir = test_temp_dir();
    let out_dir = work_dir.to_str().expect("temp dir is UTF-8");

    rolify_cli()
        .args([
            "generate",
            "--backend",
            "sqlx",
            "Role",
            "User",
            "--out-dir",
            out_dir,
        ])
        .assert()
        .success();

    let postgres_tree = work_dir.join("migrations/postgres");
    let migrator = build_migrator_like_recipe(&postgres_tree);

    assert_eq!(
        migrator.migrations.len(),
        2,
        "one generated stem yields exactly the up and down entries"
    );

    let up_entry = &migrator.migrations[0];
    let down_entry = &migrator.migrations[1];

    assert_eq!(up_entry.version, 1, "stem timestamp prefix is version 1");
    assert_eq!(down_entry.version, 1, "down entry shares the stem version");
    assert_eq!(
        up_entry.migration_type,
        MigrationType::ReversibleUp,
        "up.sql maps to the reversible-up variant"
    );
    assert_eq!(
        down_entry.migration_type,
        MigrationType::ReversibleDown,
        "down.sql maps to the reversible-down variant"
    );
    assert_eq!(
        up_entry.description, down_entry.description,
        "both entries describe the same stem"
    );
    assert!(
        up_entry.description.contains("rolify_create_tables"),
        "description names the stem"
    );
    assert!(
        !up_entry.sql.as_str().is_empty(),
        "up entry carries SQL"
    );
    assert!(
        !down_entry.sql.as_str().is_empty(),
        "down entry carries SQL"
    );
    assert!(
        up_entry.sql.as_str().contains("CREATE TABLE"),
        "up entry carries the canonical DDL"
    );
}
