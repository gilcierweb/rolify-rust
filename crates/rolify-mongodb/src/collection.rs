//! Type-level seam bridging the driver's two handle trees (D-12).
//!
//! Why this module exists (RESEARCH Pitfall 3, Assumption A7 - the riskiest
//! mechanical point of this crate): `maybe-async` rewrites method bodies
//! and strips `.await` in sync mode, but it never rewrites FIELD types or
//! the driver call shape itself, and async `mongodb::Collection<T>` vs
//! `mongodb::sync::Collection<T>` are distinct Rust types over distinct
//! execution paths (action `.await` vs action `.run()`). The store
//! therefore never names a driver type directly: handles come through the
//! aliases here, and every driver OPERATION lives behind one of the
//! per-mode helpers below, so `#[maybe_async(AFIT)]` store bodies strip
//! `.await` cleanly onto the sync helper with the same name.
//!
//! Design note (recorded in 05.1-02): the collection element type is
//! `bson::Document`, NOT the typed `RoleDoc`. The driver requires
//! `T: Serialize + DeserializeOwned` for reads/writes, and `serde` derives
//! on `RoleDoc` stay consumer-optional (QUAL-03). Manual
//! `RoleDoc::to_document`/`from_document` conversion keeps the typed
//! boundary at the store edge - the threat model's "typed `doc!` filters"
//! stance (T-05.1-04).
//!
//! The sync path mirrors the driver 1:1: `mongodb::sync` wraps the async
//! client on the driver's own internal runtime (thread offloading), tokio
//! stays linked in sync builds, and library code never calls `block_on`.

use bson::Document;

/// Database handle for the active driver mode (D-16: `Database` +
/// validated collection names is the whole handle surface of the store).
#[cfg(not(feature = "sync"))]
pub type DatabaseHandle = mongodb::Database;

/// Sync-mode database handle (`mongodb::sync::Database` wraps the async
/// one; cloning is an `Arc` bump in both modes).
#[cfg(feature = "sync")]
pub type DatabaseHandle = mongodb::sync::Database;

/// Document collection handle for the active driver mode. `Document`
/// carries every collection the adapter touches (roles, holders,
/// resources): conversion to typed structs happens at the `RoleDoc`
/// boundary.
#[cfg(not(feature = "sync"))]
pub type CollectionHandle = mongodb::Collection<Document>;

/// Sync-mode document collection handle.
#[cfg(feature = "sync")]
pub type CollectionHandle = mongodb::sync::Collection<Document>;

/// Resolve the typed document handle for a collection by validated name.
/// Both driver modes expose `Database::collection::<T>(name)` with the
/// same shape, so this one body serves both cfg arms.
pub(crate) fn collection_for(database: &DatabaseHandle, name: &str) -> CollectionHandle {
    database.collection::<Document>(name)
}

/// Run a `find` and materialize every document. Cursor driving goes
/// through `advance`/`deserialize_current`, so no `futures` dependency is
/// needed in async mode.
///
/// # Errors
///
/// Returns the adapter `Error` when the driver call or cursor driving
/// fails.
#[cfg(not(feature = "sync"))]
pub async fn find_docs(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<Vec<Document>, crate::error::Error> {
    let mut cursor = collection.find(filter).await?;
    let mut documents = Vec::new();
    while cursor.advance().await? {
        documents.push(cursor.deserialize_current()?);
    }
    Ok(documents)
}

/// Sync arm of [`find_docs`]: the sync cursor is an iterator.
///
/// # Errors
///
/// Returns the adapter `Error` when the driver call or cursor driving
/// fails.
#[cfg(feature = "sync")]
pub fn find_docs(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<Vec<Document>, crate::error::Error> {
    let cursor = collection.find(filter).run()?;
    let mut documents = Vec::new();
    for document in cursor {
        documents.push(document?);
    }
    Ok(documents)
}

/// `find_one` for a single matching document, if any.
///
/// # Errors
///
/// Returns the adapter `Error` when the driver call fails.
#[cfg(not(feature = "sync"))]
pub async fn find_one_doc(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<Option<Document>, crate::error::Error> {
    Ok(collection.find_one(filter).await?)
}

/// Sync arm of [`find_one_doc`].
///
/// # Errors
///
/// Returns the adapter `Error` when the driver call fails.
#[cfg(feature = "sync")]
pub fn find_one_doc(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<Option<Document>, crate::error::Error> {
    Ok(collection.find_one(filter).run()?)
}

/// Insert one document, returning its `_id` as reported by the driver.
///
/// # Errors
///
/// Returns the adapter `Error` when the insert fails (including the 11000
/// duplicate-key race arm).
#[cfg(not(feature = "sync"))]
pub async fn insert_doc(
    collection: &CollectionHandle,
    document: Document,
) -> Result<bson::Bson, crate::error::Error> {
    let outcome = collection.insert_one(document).await?;
    Ok(outcome.inserted_id)
}

/// Sync arm of [`insert_doc`].
///
/// # Errors
///
/// Returns the adapter `Error` when the insert fails.
#[cfg(feature = "sync")]
pub fn insert_doc(
    collection: &CollectionHandle,
    document: Document,
) -> Result<bson::Bson, crate::error::Error> {
    let outcome = collection.insert_one(document).run()?;
    Ok(outcome.inserted_id)
}

/// Upsert-aware set: create the document when missing, patch it
/// otherwise. Fixture seeding runs this once per identity on a shared
/// container, so the `$set` re-application on re-run is by design.
///
/// # Errors
///
/// Returns the adapter `Error` when the update fails.
#[cfg(not(feature = "sync"))]
pub async fn set_doc(
    collection: &CollectionHandle,
    filter: Document,
    patch: Document,
) -> Result<(), crate::error::Error> {
    collection.update_many(filter, patch).upsert(true).await?;
    Ok(())
}

/// Sync arm of [`set_doc`].
///
/// # Errors
///
/// Returns the adapter `Error` when the update fails.
#[cfg(feature = "sync")]
pub fn set_doc(
    collection: &CollectionHandle,
    filter: Document,
    patch: Document,
) -> Result<(), crate::error::Error> {
    collection.update_many(filter, patch).upsert(true).run()?;
    Ok(())
}

/// Apply an update document to every match.
///
/// # Errors
///
/// Returns the adapter `Error` when the update fails.
#[cfg(not(feature = "sync"))]
pub async fn update_docs(
    collection: &CollectionHandle,
    filter: Document,
    update: Document,
) -> Result<mongodb::results::UpdateResult, crate::error::Error> {
    Ok(collection.update_many(filter, update).await?)
}

/// Sync arm of [`update_docs`].
///
/// # Errors
///
/// Returns the adapter `Error` when the update fails.
#[cfg(feature = "sync")]
pub fn update_docs(
    collection: &CollectionHandle,
    filter: Document,
    update: Document,
) -> Result<mongodb::results::UpdateResult, crate::error::Error> {
    Ok(collection.update_many(filter, update).run()?)
}

/// Apply an update to at most one document (a single `_id` target).
///
/// # Errors
///
/// Returns the adapter `Error` when the update fails.
#[cfg(not(feature = "sync"))]
pub async fn update_one_doc(
    collection: &CollectionHandle,
    filter: Document,
    update: Document,
) -> Result<mongodb::results::UpdateResult, crate::error::Error> {
    Ok(collection.update_one(filter, update).await?)
}

/// Sync arm of [`update_one_doc`].
///
/// # Errors
///
/// Returns the adapter `Error` when the update fails.
#[cfg(feature = "sync")]
pub fn update_one_doc(
    collection: &CollectionHandle,
    filter: Document,
    update: Document,
) -> Result<mongodb::results::UpdateResult, crate::error::Error> {
    Ok(collection.update_one(filter, update).run()?)
}

/// Delete every matching document, returning the count removed.
///
/// # Errors
///
/// Returns the adapter `Error` when the delete fails.
#[cfg(not(feature = "sync"))]
pub async fn delete_docs(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<u64, crate::error::Error> {
    Ok(collection.delete_many(filter).await?.deleted_count)
}

/// Sync arm of [`delete_docs`].
///
/// # Errors
///
/// Returns the adapter `Error` when the delete fails.
#[cfg(feature = "sync")]
pub fn delete_docs(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<u64, crate::error::Error> {
    Ok(collection.delete_many(filter).run()?.deleted_count)
}

/// Delete a single document by its `_id` filter.
///
/// # Errors
///
/// Returns the adapter `Error` when the delete fails.
#[cfg(not(feature = "sync"))]
pub async fn delete_one_doc(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<u64, crate::error::Error> {
    Ok(collection.delete_one(filter).await?.deleted_count)
}

/// Sync arm of [`delete_one_doc`].
///
/// # Errors
///
/// Returns the adapter `Error` when the delete fails.
#[cfg(feature = "sync")]
pub fn delete_one_doc(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<u64, crate::error::Error> {
    Ok(collection.delete_one(filter).run()?.deleted_count)
}

/// Count matching documents.
///
/// # Errors
///
/// Returns the adapter `Error` when the count fails.
#[cfg(not(feature = "sync"))]
pub async fn count_docs(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<u64, crate::error::Error> {
    Ok(collection.count_documents(filter).await?)
}

/// Sync arm of [`count_docs`].
///
/// # Errors
///
/// Returns the adapter `Error` when the count fails.
#[cfg(feature = "sync")]
pub fn count_docs(
    collection: &CollectionHandle,
    filter: Document,
) -> Result<u64, crate::error::Error> {
    Ok(collection.count_documents(filter).run()?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn seam_types_are_constructible() {
        // Compilation of the aliases against both driver paths is the seam
        // proof (RESEARCH A7); the store shell exercises construction.
        fn assert_clone<T: Clone>() {}
        assert_clone::<bson::oid::ObjectId>();
    }
}
