//! Type-level seam bridging the driver's two handle trees (D-12).
//!
//! Why this module exists (RESEARCH Pitfall 3, Assumption A7 - the riskiest
//! mechanical point of this crate): `maybe-async` rewrites function bodies
//! and strips `.await` in sync mode, but it never rewrites FIELD TYPES, and
//! async `mongodb::Collection<T>` / `mongodb::Database` vs their
//! `mongodb::sync::*` counterparts are distinct Rust types. The store
//! therefore names handles only through the aliases below, and the
//! operations that differ (execute actions via `.await` vs via `run()`)
//! live behind per-mode cfg arms (see `index::ensure_role_index`).
//!
//! The sync path mirrors the driver 1:1: `mongodb::sync` wraps the async
//! client on the driver's own internal runtime (thread offloading), tokio
//! stays linked in sync builds, and library code never calls `block_on`.

use crate::document::RoleDoc;

/// Database handle for the active driver mode (D-16: `Database` + validated
/// collection names is the whole handle surface of the store).
#[cfg(not(feature = "sync"))]
pub type DatabaseHandle = mongodb::Database;

/// Sync-mode database handle (`mongodb::sync::Database` wraps the async
/// one; cloning is an `Arc` bump in both modes).
#[cfg(feature = "sync")]
pub type DatabaseHandle = mongodb::sync::Database;

/// Roles collection handle for the active driver mode.
#[cfg(not(feature = "sync"))]
pub type RoleCollectionHandle = mongodb::Collection<RoleDoc>;

/// Sync-mode roles collection handle.
#[cfg(feature = "sync")]
pub type RoleCollectionHandle = mongodb::sync::Collection<RoleDoc>;

/// Resolve the typed handle for a collection of `RoleDoc`s by validated
/// name. Both driver modes expose `Database::collection::<T>(name)` with
/// the same shape, so this one body serves both cfg arms.
pub(crate) fn collection_for(database: &DatabaseHandle, name: &str) -> RoleCollectionHandle {
    database.collection::<RoleDoc>(name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn seam_types_are_constructible() {
        // Compilation of the aliases against both driver paths is the seam
        // proof (RESEARCH A7); the store shell in Task 2 constructs them.
        fn assert_clone<T: Clone>() {}
        assert_clone::<bson::oid::ObjectId>();
    }
}
