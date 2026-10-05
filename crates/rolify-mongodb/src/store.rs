//! `MongoStore`: the store handle for `rolify-mongodb`.
//!
//! One store per role/consumer collection pair (D-09, mirroring the diesel
//! constructor contract): the store holds a `Database` handle plus the
//! VALIDATED collection names, nothing more (D-16). Full `RoleStore` /
//! `ResourceStore` method bodies land in 05.1-02; this file is the
//! dual-mode shell: constructor validation, registries, ensure-index setup,
//! and the explicit query counter the suite backend delegates to.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rolify_core::config::{RolifyConfig, RolifyConfigBuilder};
use rolify_core::store::Sealed;

use crate::collection::{DatabaseHandle, RoleCollectionHandle, collection_for};
use crate::error::Error;

/// `MongoDB` role store (async first; same type compiles sync via D-12).
///
/// Construct with [`MongoStore::new`] (collection names from
/// `&RolifyConfig`, D-09) and a driver `Database` handle (async mode) or
/// `mongodb::sync::Database` (sync mode) - the same field type switches
/// through the [`crate::collection`] seam. Then chain
/// `.for_holder_collection("users")` and
/// `.register_resource_collection("Forum", "forums")` as needed.
///
/// Index convergence is NEVER hidden in this constructor: `create_index`
/// is a server round-trip, so it lives in the async setup method
/// [`MongoStore::ensure_indexes`] (D-10, RESEARCH Open Question 1),
/// callable from setup and test paths.
#[derive(Debug, Clone)]
pub struct MongoStore {
    database: DatabaseHandle,
    role_collection: String,
    holder_collection: Option<String>,
    resource_collections: Vec<(String, String)>, // (type_name, collection_name)
    /// Explicit per-method query counter (RESEARCH Open Question 2; Mongo
    /// has no diesel-style instrumentation hook). The suite backend reads
    /// this through `query_count`/`reset_query_count` for the TEST-05
    /// guards (`cached` = 0 ops, `has_any` = 1 op).
    query_count: Arc<AtomicUsize>,
}

impl MongoStore {
    /// Create a store over `database` using `config`'s role collection name
    /// (the `roles` default or the config override, D-09).
    ///
    /// The collection name is re-validated against the D-08 allow-list even
    /// though [`RolifyConfig`] validated it at build time, because the
    /// `with_role_collection` path (below) bypasses config: T-05.1-04.
    ///
    /// # Panics
    ///
    /// Panics when the configured role collection name fails the
    /// identifier allow-list (`^[A-Za-z_][A-Za-z0-9_]*$`). A config built
    /// via `RolifyConfig::builder().build()` can never trigger this.
    #[must_use]
    pub fn new(database: &DatabaseHandle, config: &RolifyConfig) -> Self {
        Self::with_role_collection(database, config.role_table())
    }

    /// Create a store with an explicit roles collection name (multi-pair
    /// setups). Validated against the same allow-list config uses.
    ///
    /// # Panics
    ///
    /// Panics when `role_collection` fails the identifier allow-list.
    #[must_use]
    pub fn with_role_collection(database: &DatabaseHandle, role_collection: &str) -> Self {
        RolifyConfigBuilder::validate_identifier(role_collection)
            .expect("role collection name must pass validation");
        Self {
            database: database.clone(),
            role_collection: role_collection.to_owned(),
            holder_collection: None,
            resource_collections: Vec::new(),
            query_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Set the holder (consumer) collection (e.g., "users", "customers")
    /// for `holders_where` / `all_holders`. Validated via the config
    /// allow-list.
    ///
    /// # Panics
    ///
    /// Panics when `holder_collection` fails the identifier allow-list
    /// (`^[A-Za-z_][A-Za-z0-9_]*$`).
    #[must_use]
    pub fn for_holder_collection(mut self, holder_collection: &str) -> Self {
        RolifyConfigBuilder::validate_identifier(holder_collection)
            .expect("holder collection name must pass validation");
        self.holder_collection = Some(holder_collection.to_owned());
        self
    }

    /// Register a resource type for class-scope expansion in
    /// `resources_find`. `type_name` is the STI-ish type string (e.g.,
    /// "Forum"), `collection_name` its Mongo collection (e.g., "forums").
    ///
    /// # Panics
    ///
    /// Panics when `type_name` or `collection_name` fails the identifier
    /// allow-list (`^[A-Za-z_][A-Za-z0-9_]*$`).
    #[must_use]
    pub fn register_resource_collection(
        mut self,
        type_name: &str,
        resource_collection: &str,
    ) -> Self {
        RolifyConfigBuilder::validate_identifier(resource_collection)
            .expect("resource collection name must pass validation");
        RolifyConfigBuilder::validate_identifier(type_name)
            .expect("resource type name must pass validation");
        self.resource_collections
            .push((type_name.to_owned(), resource_collection.to_owned()));
        self
    }

    /// The roles collection name this store was built with.
    #[must_use]
    pub fn role_collection(&self) -> &str {
        &self.role_collection
    }

    /// The holder collection name, if set.
    #[must_use]
    pub fn holder_collection(&self) -> Option<&str> {
        self.holder_collection.as_deref()
    }

    /// Registered `(type_name, collection_name)` resource pairs.
    #[must_use]
    pub fn resource_collections(&self) -> &[(String, String)] {
        &self.resource_collections
    }

    /// Typed handle for the roles collection (seam-swapped driver type).
    pub(crate) fn roles_collection(&self) -> RoleCollectionHandle {
        collection_for(&self.database, &self.role_collection)
    }

    /// Ensure the unique compound index on the roles collection (D-10).
    /// Idempotent: `create_index` on an identical existing index is a
    /// server-side no-op. Call from setup and test paths, never from the
    /// constructor (which stays sync in both driver modes).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Mongo`] when the server rejects the index.
    #[maybe_async::maybe_async(AFIT)]
    pub async fn ensure_indexes(&self) -> Result<(), Error> {
        self.bump_query_count();
        crate::index::ensure_role_index(&self.roles_collection()).await?;
        Ok(())
    }

    /// Number of store operations issued since construction or the last
    /// [`MongoStore::reset_query_count`]. Store methods bump this; the suite
    /// backend reads it for the TEST-05 query-count guards.
    #[must_use]
    pub fn query_count(&self) -> usize {
        self.query_count.load(Ordering::Relaxed)
    }

    /// Reset the explicit query counter (suite `reset_query_count` hook).
    pub fn reset_query_count(&self) {
        self.query_count.store(0, Ordering::Relaxed);
    }

    pub(crate) fn bump_query_count(&self) {
        self.query_count.fetch_add(1, Ordering::Relaxed);
    }
}

impl Sealed for MongoStore {}

#[cfg(test)]
mod tests {
    use super::*;
    use rolify_core::config::RolifyConfig;

    #[cfg(not(feature = "sync"))]
    fn database() -> DatabaseHandle {
        // Test-only handle construction (pure shaping, no I/O until first
        // use; the tracer plan 05.1-02 covers real traffic). The async
        // client's `with_uri_str` is a future in driver 3.x; blocking here
        // is a test path, never library code (RESEARCH Anti-Patterns).
        tokio::runtime::Runtime::new()
            .expect("tokio runtime builds")
            .block_on(mongodb::Client::with_uri_str("mongodb://localhost:27017"))
            .expect("default URI parses")
            .database("rolify_test_shell")
    }

    #[cfg(feature = "sync")]
    fn database() -> DatabaseHandle {
        mongodb::sync::Client::with_uri_str("mongodb://localhost:27017")
            .expect("default URI parses")
            .database("rolify_test_shell")
    }

    #[test]
    fn store_constructs_from_config_with_defaults() {
        let config = RolifyConfig::default();
        let store = MongoStore::new(&database(), &config);
        assert_eq!(store.role_collection(), "roles");
        assert_eq!(store.holder_collection(), None);
        assert!(store.resource_collections().is_empty());
        assert_eq!(store.query_count(), 0);
    }

    #[test]
    fn store_validates_custom_collection_names() {
        let store = MongoStore::with_role_collection(&database(), "tenant_roles")
            .for_holder_collection("users")
            .register_resource_collection("Forum", "forums");
        assert_eq!(store.role_collection(), "tenant_roles");
        assert_eq!(store.holder_collection(), Some("users"));
        assert_eq!(
            store.resource_collections(),
            &[("Forum".to_owned(), "forums".to_owned())]
        );
    }

    #[test]
    fn invalid_names_panic_at_the_allow_list() {
        let check = |name: &str| {
            std::panic::catch_unwind(|| MongoStore::with_role_collection(&database(), name))
                .is_err()
        };
        assert!(check(""));
        assert!(check("1roles"));
        assert!(check("roles$bad"));
        assert!(!check("_roles"));
    }

    #[test]
    fn query_counter_starts_at_zero_and_resets() {
        let store = MongoStore::new(&database(), &RolifyConfig::default());
        assert_eq!(store.query_count(), 0);
        store.bump_query_count();
        store.bump_query_count();
        assert_eq!(store.query_count(), 2);
        store.reset_query_count();
        assert_eq!(store.query_count(), 0);
    }
}
