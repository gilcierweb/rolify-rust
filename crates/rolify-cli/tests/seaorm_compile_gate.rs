//! Compile gate for the checked-in expected SeaORM migration (T-06-04, D-16).
//!
//! The expected file is compiled INTO this test binary via a `#[path]` module
//! so `sea-orm-migration` 2.0.4 typechecks it: a template edit that breaks the
//! `MigrationTrait` shape fails HERE, at compile time, instead of in a consumer
//! build. The trait-bound assertion below additionally proves the included
//! `Migration` struct actually implements `MigrationTrait`, and the drift
//! suite (tests/drift.rs) pins the rendered bytes to this exact file.

#[path = "expected/seaorm_migration.rs"]
mod expected_migration;

#[test]
fn expected_migration_implements_migration_trait() {
    fn assert_migration_trait<T: sea_orm_migration::MigrationTrait>() {}
    assert_migration_trait::<expected_migration::Migration>();
}
