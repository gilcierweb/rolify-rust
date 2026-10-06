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

// The `Future` import is only referenced by `impl Future` signatures in
// async mode; maybe_async strips them in `is_sync` mode (the rolify-test
// `allow` precedent - the cfg gate sees only this crate's feature while the
// macro reads the package-unified one).
#[cfg(not(feature = "is_sync"))]
#[allow(unused_imports)]
use core::future::Future;

use bson::{Bson, Document, doc};
use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
use rolify_core::config::{RolifyConfig, RolifyConfigBuilder};
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::{
    RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn, Sealed,
};

use crate::collection::{
    CollectionHandle, DatabaseHandle, collection_for, count_docs, delete_docs, delete_one_doc,
    find_docs, find_one_doc, insert_doc, update_docs, update_one_doc,
};
use crate::document::RoleDoc;
use crate::error::{Error, is_duplicate_key};

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

    /// Typed handles are document-level in both modes (see the
    /// [`crate::collection`] seam), so these resolve through the same
    /// helper. `bson::Document` keeps `serde` optional (QUAL-03) while
    /// [`crate::document::RoleDoc`] conversions hold the typed boundary.
    pub(crate) fn roles_collection(&self) -> CollectionHandle {
        collection_for(&self.database, &self.role_collection)
    }

    /// Holder collection handle, when a holder collection is configured.
    /// The SPI consumer-side writes require one (error when unset), while
    /// read paths on role documents never do.
    fn holders_collection(&self) -> Option<CollectionHandle> {
        self.holder_collection
            .as_deref()
            .map(|name| collection_for(&self.database, name))
    }

    /// Resource collection handle for a registered type.
    fn resource_collection(&self, collection_name: &str) -> CollectionHandle {
        collection_for(&self.database, collection_name)
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

    /// Shared handle on the query counter for test backends that must read
    /// the count through a shared reference (the `TestBackend`
    /// `query_count(&self)` shape). The `Arc` keeps reads race-free while a
    /// seated subject holds `&mut self`.
    #[must_use]
    pub fn query_counter_probe(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.query_count)
    }

    pub(crate) fn bump_query_count(&self) {
        self.query_count.fetch_add(1, Ordering::Relaxed);
    }
}

impl Sealed for MongoStore {}

// ---------------------------------------------------------------------------
// RoleStore / ResourceStore SPI over the document seam
// ---------------------------------------------------------------------------
//
// Translation notes (gem anchors):
// * `find_or_create_by` is insert-then-catch-11000-then-re-read (D-14), the
//   gem's `find_or_create_by(:name, :resource_type, :resource_id)` with the
//   unique compound index as the arbiter (`role-mongoid.rb:9-14`). Never
//   pre-check-then-insert (RESEARCH Don't Hand-Roll).
// * Link writes are two-sided with a FIXED order - role side first
//   (`$addToSet` user_ids), consumer side second (`$addToSet` role_ids) -
//   mirroring `mongoid/role_adapter.rb:75-87` (`relation.roles << role`;
//   removal pulls both sides, emptiness checked AFTER removal on the
//   authoritative role side). Half-applied pairs are tolerated by every
//   read path below (they read whichever side they need).
// * All reads keep kernel semantics: byte-exact names, explicit null scope
//   fields (never the SQL `''` sentinel, NEVER a missing field write).

/// Holder pin: documents linked to one holder (array-contains equality
/// over `user_ids`).
fn holder_filter(filter: &mut Document, holder: &ResourceId) {
    filter.insert(FIELDS_USER_IDS, holder.as_str());
}

/// The non-strict ladder as one BSON filter (null-equality, `$or`
/// disjuncts) - mirrors `kernel::where_` (`build_query`,
/// `role_adapter.rb:106-121`):
///
/// | filter | clauses |
/// |---|---|
/// | Global | name AND type null AND id null |
/// | Class(t) | name AND (type null OR (type t AND id null)) |
/// | Instance(t, id) | name AND (type null OR (type t AND id null) OR (type t AND id)) |
/// | Any | name only (`:any` short-circuit, D-02) |
///
/// Scope pins read `{field: null}` which matches null AND missing, so a
/// half-applied legacy write still matches (Pitfall-4 tolerance).
fn ladder_filter(query: &RoleQuery<'_>) -> Document {
    let mut filter = doc! { FIELDS_NAME: query.name.to_string() };
    match &query.filter {
        ResourceFilter::Global => {
            filter.insert(FIELDS_RESOURCE_TYPE, Bson::Null);
            filter.insert(FIELDS_RESOURCE_ID, Bson::Null);
        }
        ResourceFilter::Class(type_name) => {
            filter.insert(
                "$or",
                vec![
                    doc! { FIELDS_RESOURCE_TYPE: Bson::Null },
                    doc! { FIELDS_RESOURCE_TYPE: *type_name, FIELDS_RESOURCE_ID: Bson::Null },
                ],
            );
        }
        ResourceFilter::Instance(type_name, id) => {
            filter.insert(
                "$or",
                vec![
                    doc! { FIELDS_RESOURCE_TYPE: Bson::Null },
                    doc! { FIELDS_RESOURCE_TYPE: *type_name, FIELDS_RESOURCE_ID: Bson::Null },
                    doc! { FIELDS_RESOURCE_TYPE: *type_name, FIELDS_RESOURCE_ID: id.as_str() },
                ],
            );
        }
        ResourceFilter::Any => {}
    }
    filter
}

/// Strict exact-scope filter - mirrors `kernel::where_strict`
/// (`role_adapter.rb:11-26`): global means both null, Class means
/// (type, id null), Instance means the exact pair, Any is name-only
/// (the ratified corner behavior).
fn strict_filter(query: &RoleQuery<'_>) -> Document {
    let mut filter = doc! { FIELDS_NAME: query.name.to_string() };
    match &query.filter {
        ResourceFilter::Global => {
            filter.insert(FIELDS_RESOURCE_TYPE, Bson::Null);
            filter.insert(FIELDS_RESOURCE_ID, Bson::Null);
        }
        ResourceFilter::Class(type_name) => {
            filter.insert(FIELDS_RESOURCE_TYPE, *type_name);
            filter.insert(FIELDS_RESOURCE_ID, Bson::Null);
        }
        ResourceFilter::Instance(type_name, id) => {
            filter.insert(FIELDS_RESOURCE_TYPE, *type_name);
            filter.insert(FIELDS_RESOURCE_ID, id.as_str());
        }
        ResourceFilter::Any => {}
    }
    filter
}

/// The removal sweep filter (kernel `RemovalTarget`): conjunctive, always
/// name-gated, never the OR ladder. `TypeSweep` touches class AND instance
/// rows of that type (no `resource_id` condition).
fn removal_filter(name: &RoleName, target: &RemovalTarget<'_>, holder: &ResourceId) -> Document {
    let mut filter = doc! { FIELDS_NAME: name.to_string() };
    match target {
        RemovalTarget::NameOnly => {}
        RemovalTarget::TypeSweep(type_name) => {
            filter.insert(FIELDS_RESOURCE_TYPE, *type_name);
        }
        RemovalTarget::Exact(type_name, id) => {
            filter.insert(FIELDS_RESOURCE_TYPE, *type_name);
            filter.insert(FIELDS_RESOURCE_ID, id.as_str());
        }
    }
    holder_filter(&mut filter, holder);
    filter
}

/// Exact-triple lookup filter for one role document (the unique index's
/// key: name + explicit-null-able scope pair).
fn triple_filter(record: &RoleRecord) -> Document {
    doc! {
        FIELDS_NAME: record.name.to_string(),
        FIELDS_RESOURCE_TYPE: record.resource_type.as_deref().map_or(Bson::Null, Bson::from),
        FIELDS_RESOURCE_ID: record.resource_id.as_ref().map_or(Bson::Null, |id| Bson::from(id.as_str())),
    }
}

/// Fetch one role document by its unique key triple.
#[maybe_async::maybe_async(AFIT)]
async fn read_role_doc(
    collection: &CollectionHandle,
    record: &RoleRecord,
) -> Result<Option<RoleDoc>, Error> {
    find_one_doc(collection, triple_filter(record))
        .await?
        .map(|document| RoleDoc::from_document(&document))
        .transpose()
}

#[maybe_async::maybe_async(AFIT)]
impl RoleStore for MongoStore {
    type Conn = ();
    type Error = Error;

    fn where_(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let roles = self.roles_collection();
        let mut filter = ladder_filter(query);
        holder_filter(&mut filter, holder);
        self.bump_query_count();
        async move {
            let documents = find_docs(&roles, filter).await?;
            documents
                .iter()
                .map(|document| Ok(RoleDoc::from_document(document)?.to_record()))
                .collect()
        }
    }

    fn where_strict(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let roles = self.roles_collection();
        let mut filter = strict_filter(query);
        holder_filter(&mut filter, holder);
        self.bump_query_count();
        async move {
            let documents = find_docs(&roles, filter).await?;
            documents
                .iter()
                .map(|document| Ok(RoleDoc::from_document(document)?.to_record()))
                .collect()
        }
    }

    fn where_any(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        queries: &[RoleQuery<'_>],
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        // ONE logical round trip: the gem's `build_conditions`
        // `join(' OR ')` collapses to a `$or` of per-query filters.
        let roles = self.roles_collection();
        let disjuncts: Vec<Document> = queries.iter().map(ladder_filter).collect();
        let mut filter = doc! { "$or": disjuncts };
        holder_filter(&mut filter, holder);
        self.bump_query_count();
        async move {
            let documents = find_docs(&roles, filter).await?;
            documents
                .iter()
                .map(|document| Ok(RoleDoc::from_document(document)?.to_record()))
                .collect()
        }
    }

    fn find_or_create_by(
        &mut self,
        _conn: &mut Self::Conn,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send {
        let roles = self.roles_collection();
        let (resource_type, resource_id) = match scope {
            ResourceRef::Global => (None, None),
            ResourceRef::Class(type_name) => (Some(type_name.to_owned()), None),
            ResourceRef::Instance(type_name, id) => (Some(type_name.to_owned()), Some(id.clone())),
        };
        let record = RoleRecord::new(name.clone(), resource_type.clone(), resource_id.clone());
        self.bump_query_count();
        async move {
            let document = RoleDoc::from_record(&record).to_document();
            match insert_doc(&roles, document).await {
                Ok(_) => Ok(record),
                Err(error) => {
                    if let Error::Mongo(mongo_error) = &error
                        && is_duplicate_key(mongo_error)
                    {
                        // D-14 race: a peer inserted the same triple
                        // between our intent and the write; re-read the
                        // winner's document through the unique key.
                        let reread = read_role_doc(&roles, &record).await?;
                        return Ok(reread.map_or(record.clone(), |doc| doc.to_record()));
                    }
                    Err(error)
                }
            }
        }
    }

    fn add(
        &mut self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        role: &RoleRecord,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let roles = self.roles_collection();
        let holders = self.holders_collection();
        self.bump_query_count();
        async move {
            let Some(document) = read_role_doc(&roles, role).await? else {
                return Err(invalid_config(
                    "add: role document must exist before linking (use find_or_create_by first)",
                ));
            };
            let holder_key = holder.as_str().to_owned();
            // Link-guard dedupe (level 2, gem `unless include?`): the
            // atomic filter `{_id, user_ids: {$ne: holder}}` makes the
            // SERVER the arbiter - a racing loser matches nothing and
            // reads back `matched_count == 0` (D-14, Pitfall 5).
            let created = update_one_doc(
                &roles,
                doc! {
                    FIELDS_ID: document.id,
                    FIELDS_USER_IDS: { "$ne": holder_key.clone() },
                },
                doc! { "$addToSet": { FIELDS_USER_IDS: holder_key.clone() } },
            )
            .await?
            .matched_count
                == 1;
            // D-08 fixed write order: role side first, consumer side
            // second (idempotent regardless of outcome).
            if let Some(holders) = holders {
                update_docs(
                    &holders,
                    doc! { FIELDS_HOLDER_ID: holder_key },
                    doc! { "$addToSet": { FIELDS_ROLE_IDS: document.id } },
                )
                .await?;
            }
            Ok(created)
        }
    }

    fn remove(
        &mut self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        name: &RoleName,
        target: RemovalTarget<'_>,
        remove_role_if_empty: bool,
    ) -> impl Future<Output = Result<RemovalOutcome, Self::Error>> + Send {
        let roles = self.roles_collection();
        let holders = self.holders_collection();
        let filter = removal_filter(name, &target, holder);
        let holder_key = holder.as_str().to_owned();
        self.bump_query_count();
        async move {
            let affected: Vec<RoleDoc> = find_docs(&roles, filter)
                .await?
                .iter()
                .map(RoleDoc::from_document)
                .collect::<Result<_, _>>()?;
            let mut removed_links_count = 0_usize;
            let mut removed_roles = Vec::new();
            for affected_doc in &affected {
                // D-08 fixed write order: role side first, consumer side
                // second; emptiness is checked AFTER removal on the role
                // (authoritative) side (`mongoid/role_adapter.rb:75-87`).
                update_one_doc(
                    &roles,
                    doc! { FIELDS_ID: affected_doc.id },
                    doc! { "$pull": { FIELDS_USER_IDS: holder_key.clone() } },
                )
                .await?;
                if let Some(holders) = &holders {
                    update_docs(
                        holders,
                        doc! { FIELDS_HOLDER_ID: holder_key.clone() },
                        doc! { "$pull": { FIELDS_ROLE_IDS: affected_doc.id } },
                    )
                    .await?;
                }
                removed_links_count += 1;
                if remove_role_if_empty {
                    let fresh = find_one_doc(&roles, doc! { FIELDS_ID: affected_doc.id })
                        .await?
                        .map(|document| RoleDoc::from_document(&document))
                        .transpose()?;
                    if let Some(current) = fresh
                        && current.user_ids.is_empty()
                    {
                        delete_one_doc(&roles, doc! { FIELDS_ID: affected_doc.id }).await?;
                        removed_roles.push(affected_doc.to_record());
                    }
                }
            }
            Ok(RemovalOutcome {
                removed_links: removed_links_count,
                removed_roles,
            })
        }
    }

    fn exists(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
        column: ScopeColumn,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let roles = self.roles_collection();
        // `{$ne: null}` matches any non-null field - the
        // `column IS NOT NULL` semantic (`role_adapter.rb:72-74`).
        let field = match column {
            ScopeColumn::ResourceType => FIELDS_RESOURCE_TYPE,
            ScopeColumn::ResourceId => FIELDS_RESOURCE_ID,
        };
        let filter = doc! {
            FIELDS_USER_IDS: holder.as_str(),
            field: { "$ne": Bson::Null },
        };
        self.bump_query_count();
        async move { Ok(count_docs(&roles, filter).await? > 0) }
    }

    fn roles_of(
        &self,
        _conn: &mut Self::Conn,
        holder: &ResourceId,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let roles = self.roles_collection();
        let filter = doc! { FIELDS_USER_IDS: holder.as_str() };
        self.bump_query_count();
        async move {
            let documents = find_docs(&roles, filter).await?;
            documents
                .iter()
                .map(|document| Ok(RoleDoc::from_document(document)?.to_record()))
                .collect()
        }
    }

    fn holders_where(
        &self,
        _conn: &mut Self::Conn,
        holder_types: &[&str],
        query: &RoleQuery<'_>,
        strict: bool,
    ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
        let roles = self.roles_collection();
        let holders = self.holders_collection();
        let role_filter = if strict {
            strict_filter(query)
        } else {
            ladder_filter(query)
        };
        let holder_type_strings: Vec<Bson> = holder_types
            .iter()
            .map(|holder_type| Bson::from(*holder_type))
            .collect();
        self.bump_query_count();
        async move {
            let Some(holders) = holders else {
                return Err(invalid_config(
                    "holders_where requires for_holder_collection(..) on the store",
                ));
            };
            if holder_types.is_empty() {
                // Registry rule: an empty type slice matches nothing (the
                // SPI contract on `holders_where`).
                return Ok(Vec::new());
            }
            let matched_docs = find_docs(&roles, role_filter).await?;
            let mut candidates: Vec<String> = Vec::new();
            for document in &matched_docs {
                for user_id in RoleDoc::from_document(document)?.user_ids {
                    if !candidates.contains(&user_id) {
                        candidates.push(user_id);
                    }
                }
            }
            let holder_docs = find_docs(
                &holders,
                doc! { FIELDS_ROLIFY_TYPE: { "$in": holder_type_strings } },
            )
            .await?;
            let mut found: Vec<ResourceId> = Vec::new();
            for holder_doc in &holder_docs {
                let holder_id = holder_doc.get_str(FIELDS_HOLDER_ID).map_err(|error| {
                    Error::Mongo(mongodb::error::Error::custom(format!("{error}")))
                })?;
                if candidates.iter().any(|candidate| candidate == holder_id) {
                    let id = ResourceId::new(holder_id.to_owned());
                    if !found.contains(&id) {
                        found.push(id);
                    }
                }
            }
            Ok(found)
        }
    }

    fn all_holders(
        &self,
        _conn: &mut Self::Conn,
        holder_types: &[&str],
    ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
        let holders = self.holders_collection();
        let holder_type_strings: Vec<Bson> = holder_types
            .iter()
            .map(|holder_type| Bson::from(*holder_type))
            .collect();
        self.bump_query_count();
        async move {
            let Some(holders) = holders else {
                return Err(invalid_config(
                    "all_holders requires for_holder_collection(..) on the store",
                ));
            };
            if holder_types.is_empty() {
                return Ok(Vec::new());
            }
            let documents = find_docs(
                &holders,
                doc! { FIELDS_ROLIFY_TYPE: { "$in": holder_type_strings } },
            )
            .await?;
            let mut found: Vec<ResourceId> = Vec::new();
            for document in &documents {
                if let Ok(holder_id) = document.get_str(FIELDS_HOLDER_ID) {
                    let id = ResourceId::new(holder_id.to_owned());
                    if !found.contains(&id) {
                        found.push(id);
                    }
                }
            }
            Ok(found)
        }
    }

    fn roles_matching(
        &self,
        _conn: &mut Self::Conn,
        query: &RoleCatalogQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let roles = self.roles_collection();
        let type_strings: Vec<Bson> = query
            .types
            .iter()
            .map(|type_name| Bson::from(*type_name))
            .collect();
        let mut filter = doc! { FIELDS_RESOURCE_TYPE: { "$in": type_strings } };
        if let Some(name) = query.name {
            filter.insert(FIELDS_NAME, name.to_string());
        }
        match &query.scope {
            CatalogScope::ClassAndInstance => {}
            CatalogScope::ClassOnly => {
                filter.insert(FIELDS_RESOURCE_ID, Bson::Null);
            }
            CatalogScope::InstanceOnly { resource_id } => {
                if let Some(id) = resource_id {
                    filter.insert(FIELDS_RESOURCE_ID, id.as_str());
                } else {
                    filter.insert(FIELDS_RESOURCE_ID, doc! { "$ne": Bson::Null });
                }
            }
        }
        if let Some(holder) = query.holder {
            filter.insert(FIELDS_USER_IDS, holder.as_str());
        }
        self.bump_query_count();
        async move {
            let documents = find_docs(&roles, filter).await?;
            documents
                .iter()
                .map(|document| Ok(RoleDoc::from_document(document)?.to_record()))
                .collect()
        }
    }

    fn remove_roles_for_scope(
        &mut self,
        _conn: &mut Self::Conn,
        resource_type: &str,
        resource_id: &ResourceId,
    ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
        let roles = self.roles_collection();
        let holders = self.holders_collection();
        let filter = doc! {
            FIELDS_RESOURCE_TYPE: resource_type,
            FIELDS_RESOURCE_ID: resource_id.as_str(),
        };
        self.bump_query_count();
        async move {
            // Read first so the consumer-side cleanup can pull the exact
            // `_id`s (the SQL foreign-key cascade analog; the pair is read
            // atomically enough for the suite's tearing-down semantics).
            let affected: Vec<RoleDoc> = find_docs(&roles, filter.clone())
                .await?
                .iter()
                .map(RoleDoc::from_document)
                .collect::<Result<_, _>>()?;
            let removed = usize::try_from(delete_docs(&roles, filter).await?).unwrap_or(usize::MAX);
            if let Some(holders) = holders {
                let ids: Vec<Bson> = affected.iter().map(|doc| Bson::from(doc.id)).collect();
                if !ids.is_empty() {
                    update_docs(
                        &holders,
                        doc! {},
                        doc! { "$pull": { FIELDS_ROLE_IDS: { "$in": ids } } },
                    )
                    .await?;
                }
            }
            Ok(removed)
        }
    }
}

#[maybe_async::maybe_async(AFIT)]
impl ResourceStore for MongoStore {
    type Conn = ();
    type Error = Error;

    fn resources_find(
        &self,
        _conn: &mut Self::Conn,
        types: &[&str],
        name: &RoleName,
    ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
        let registry: Vec<(String, String)> = self
            .resource_collections
            .iter()
            .filter(|(type_name, _)| types.contains(&type_name.as_str()))
            .cloned()
            .collect();
        self.bump_query_count();
        let mut members: Vec<(String, Option<String>)> = Vec::new();
        async move {
            // `roles`: rows of `name` of requested types; a null
            // `resource_id` (class row) means "all resources of that type"
            // (gem: `resources += relation.all`, `resource_adapter.rb`).
            let roles = self.roles_collection();
            for (type_name, _collection_name) in &registry {
                let documents = find_docs(
                    &roles,
                    doc! {
                        FIELDS_NAME: name.to_string(),
                        FIELDS_RESOURCE_TYPE: type_name.as_str(),
                    },
                )
                .await?;
                for document in documents {
                    let doc = RoleDoc::from_document(&document)?;
                    members.push((type_name.clone(), doc.resource_id));
                }
            }
            let mut found: Vec<ResourceKey> = Vec::new();
            for (type_name, resource_id) in members {
                let present = registry
                    .iter()
                    .find(|(registered, _)| registered == &type_name)
                    .map(|(_, collection_name)| collection_name.clone());
                let Some(collection_name) = present else {
                    continue;
                };
                match resource_id {
                    None => {
                        let collection = self.resource_collection(&collection_name);
                        let documents = find_docs(&collection, doc! {}).await?;
                        // Holder docs and resource docs both carry their
                        // stringified PK under `resource_id`/`holder_id`.
                        for document in &documents {
                            if let Some(resource_id) =
                                read_string_field(document, FIELDS_RESOURCE_ID)?
                            {
                                let key = ResourceKey::new(
                                    type_name.clone(),
                                    ResourceId::new(resource_id),
                                );
                                if !found.contains(&key) {
                                    found.push(key);
                                }
                            }
                        }
                    }
                    Some(resource_id) => {
                        let key = ResourceKey::new(type_name.clone(), ResourceId::new(resource_id));
                        if !found.contains(&key) {
                            found.push(key);
                        }
                    }
                }
            }
            Ok(found)
        }
    }

    fn in_list(
        &self,
        _conn: &mut Self::Conn,
        candidates: &[ResourceKey],
        holder: &ResourceId,
        names: &[RoleName],
    ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
        let roles = self.roles_collection();
        let name_strings: Vec<Bson> = names
            .iter()
            .map(|name| Bson::from(name.to_string()))
            .collect();
        self.bump_query_count();
        async move {
            let documents = find_docs(
                &roles,
                doc! {
                    FIELDS_USER_IDS: holder.as_str(),
                    FIELDS_NAME: { "$in": name_strings },
                },
            )
            .await?;
            let rows: Vec<RoleDoc> = documents
                .iter()
                .map(RoleDoc::from_document)
                .collect::<Result<_, _>>()?;
            let found = candidates
                .iter()
                .filter(|key| {
                    rows.iter().any(|row| {
                        row.resource_id
                            .as_deref()
                            .is_none_or(|row_id| row_id == key.resource_id.as_str())
                    })
                })
                .cloned()
                .collect();
            Ok(found)
        }
    }
}

/// The gem's scope column names, identical on documents; kept local to the
/// store reads/writes so the schema can never drift from
/// `document::RoleDoc`'s builders.
const FIELDS_ID: &str = "_id";
const FIELDS_NAME: &str = "name";
const FIELDS_RESOURCE_TYPE: &str = "resource_type";
const FIELDS_RESOURCE_ID: &str = "resource_id";
const FIELDS_USER_IDS: &str = "user_ids";
const FIELDS_ROLE_IDS: &str = "role_ids";
const FIELDS_HOLDER_ID: &str = "holder_id";
const FIELDS_ROLIFY_TYPE: &str = "rolify_type";

fn invalid_config(reason: &str) -> Error {
    Error::Core(rolify_core::error::RolifyError::InvalidConfig {
        reason: reason.to_owned(),
    })
}

fn read_string_field(document: &Document, field: &str) -> Result<Option<String>, Error> {
    match document.get(field) {
        Some(Bson::String(value)) => Ok(Some(value.clone())),
        Some(Bson::Null) | None => Ok(None),
        Some(other) => Err(Error::Mongo(mongodb::error::Error::custom(format!(
            "document field `{field}`: expected string, found {other:?}"
        )))),
    }
}

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

    /// D-12 compile proof: in `sync` builds the seam resolves to
    /// `mongodb::sync` handles and `ensure_indexes` is a sync fn (not
    /// `async`), so this pointer type-checks ONLY under the feature flip.
    #[cfg(feature = "sync")]
    #[test]
    fn sync_seam_makes_store_calls_sync() {
        let store = MongoStore::new(&database(), &RolifyConfig::default());
        // The coercion itself is the assertion (an `async` signature would
        // not fit a sync fn pointer); read the binding so it stays
        // side-effectful.
        let probe: fn(&MongoStore) -> Result<(), Error> = MongoStore::ensure_indexes;
        assert!(size_of_val(&probe) > 0);
        assert_eq!(store.role_collection(), "roles");
        assert_eq!(store.query_count(), 0);
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
