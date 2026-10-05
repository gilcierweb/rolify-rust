//! Native `sea-orm-migration` migrator (D-04).
//!
//! The `Migrator` is exported for consumers and tests to run explicitly -
//! this crate NEVER migrates on import or store construction (established
//! lock, diesel D-11 precedent). Apply with:
//!
//! ```ignore
//! use sea_orm_migration::MigratorTrait;
//! use rolify_seaorm::Migrator;
//!
//! Migrator::up(&conn, None).await?;
//! ```

pub mod m20261003_000001_create_roles;

use sea_orm_migration::prelude::*;

/// The `rolify-seaorm` migration set, in apply order.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20261003_000001_create_roles::Migration)]
    }
}
