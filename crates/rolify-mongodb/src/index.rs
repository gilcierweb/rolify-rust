//! Idempotent ensure-index for the roles collection (D-10).
//!
//! The gem declares the same index in its Mongoid template
//! (`rolify/lib/generators/rolify/templates/role-mongoid.rb:9-14`):
//! `index({name, resource_type, resource_id}, {unique: true})`. In Mongo
//! there is no versioned migration: convergence is calling `create_index`
//! at store setup, which is a server-side no-op when the identical index
//! already exists (idempotence for free). The unique compound admits each
//! `(name, resource_type, resource_id)` combination at most once,
//! with-null included (unique-with-null semantics, RESEARCH Pitfall 4):
//! exactly one `(admin, null, null)` document exists, which is the D-07
//! dedupe the duplicate-key race (D-14, code 11000) relies on.

use mongodb::IndexModel;
use mongodb::options::IndexOptions;

use crate::collection::CollectionHandle;
use crate::error::Error;

/// Unique keys of the roles index. Defined here once so the document
/// shape (`document::RoleDoc`) and the index can never drift apart.
const INDEX_KEYS: [(&str, i32); 3] = [("name", 1), ("resource_type", 1), ("resource_id", 1)];

/// Build the unique compound index model. `IndexModel` is
/// `#[non_exhaustive]`, so the builder is mandatory; struct literals are
/// impossible outside the driver crate.
#[must_use]
pub fn role_index_model() -> IndexModel {
    let mut keys = bson::Document::new();
    for (field, order) in INDEX_KEYS {
        keys.insert(field, order);
    }
    IndexModel::builder()
        .keys(keys)
        .options(IndexOptions::builder().unique(true).build())
        .build()
}

/// Ensure the unique compound index on `collection`.
///
/// In async collections this awaits the `create_index` action; the sync
/// seam (D-12) swaps the collection type and the driver runs the same
/// action via `run()`. Both paths are idempotent server-side, so callers
/// (store setup, tests) invoke this freely.
///
/// # Errors
///
/// Returns [`Error::Mongo`] when the server rejects the index (conflicting
/// existing index, permission failure, connection error).
#[cfg(not(feature = "sync"))]
pub async fn ensure_role_index(collection: &CollectionHandle) -> Result<(), Error> {
    collection.create_index(role_index_model()).await?;
    Ok(())
}

/// Sync counterpart of the async `ensure_role_index` (D-12 seam): the
/// driver's own `run()` executes the action on its internal runtime -
/// library code never calls `block_on` (Anti-Patterns). Idempotent
/// server-side, exactly like the async arm.
///
/// # Errors
///
/// Returns [`Error::Mongo`] when the server rejects the index.
#[cfg(feature = "sync")]
pub fn ensure_role_index(collection: &CollectionHandle) -> Result<(), Error> {
    collection.create_index(role_index_model()).run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bson::Bson;

    #[test]
    fn index_model_has_unique_compound_keys_in_order() {
        let model = role_index_model();
        let keys: Vec<(&str, &Bson)> = model
            .keys
            .iter()
            .map(|(key, value)| (key.as_str(), value))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("name", &Bson::Int32(1)),
                ("resource_type", &Bson::Int32(1)),
                ("resource_id", &Bson::Int32(1)),
            ]
        );
        let options = model.options.expect("index options present");
        assert_eq!(options.unique, Some(true));
    }
}
