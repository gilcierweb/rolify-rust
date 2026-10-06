//! Concurrency race suite for `rolify-mongodb` (D-14): two parallel
//! grants of the SAME role name/scope converge on EXACTLY one role
//! document, and racing links produce one `true` + one `false` via
//! `$addToSet` idempotence - the unique compound index (explicit null
//! scopes included) is the arbiter. No multi-doc transactions (D-14).
//!
//! Design mirrors the diesel race template (04-10 repair): every
//! contention step rendezvouses on a `tokio::sync::Barrier` (no sleeps),
//! store clones are per-task handles over the shared `Database`, and each
//! scope kind runs three iterations. Async-mode only: in sync builds the
//! same store code paths run through the driver's internal runtime, and
//! the crate-level `sync` suite already proves semantic duality.
//!
//! Runs: `cargo test -p rolify-mongodb --features suite --test concurrency`

#![cfg(all(feature = "suite", not(feature = "sync")))]

mod support;

use std::sync::Arc;

use bson::doc;
use rolify_core::config::RolifyConfig;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_mongodb::MongoStore;
use rolify_mongodb::collection::{delete_docs, find_docs, update_docs};
use rolify_mongodb::document::{ObjectId, RoleDoc};
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::DefaultUser;
use tokio::sync::Barrier;

use crate::support::MongoBackend;

/// Race iterations per scope kind (the diesel template's count).
const RACE_ITERATIONS: usize = 3;

/// The contested scope kinds: one per scope level.
#[derive(Clone)]
enum ScopeKind {
    Global,
    Class(&'static str),
    Instance(&'static str, ResourceId),
}

impl ScopeKind {
    fn to_resource_ref(&self) -> ResourceRef<'_> {
        match self {
            ScopeKind::Global => ResourceRef::Global,
            ScopeKind::Class(type_name) => ResourceRef::Class(type_name),
            ScopeKind::Instance(type_name, id) => ResourceRef::Instance(type_name, id),
        }
    }
}

/// Build an independent store handle (same database, fresh counter) so
/// each racing task drives its own handles.
async fn store_for(database: &mongodb::Database) -> MongoStore {
    let config = RolifyConfig::default();
    let store = MongoStore::new(database, &config)
        .for_holder_collection("users")
        .register_resource_collection("Forum", "forums");
    store.ensure_indexes().await.expect("ensure_indexes");
    store
}

/// Reset role documents only (the racing grants' collision surface), and
/// re-arm the holder docs' links so each iteration starts empty.
async fn clear_roles(database: &mongodb::Database) {
    let roles = database.collection::<bson::Document>("roles");
    delete_docs(&roles, doc! {}).await.expect("clear roles");
    let holders = database.collection::<bson::Document>("users");
    update_docs(
        &holders,
        doc! {},
        doc! { "$set": { "role_ids": Vec::<ObjectId>::new() } },
    )
    .await
    .expect("clear holder links");
}

async fn assert_role_document_count(database: &mongodb::Database, expected: u64) {
    let roles = database.collection::<bson::Document>("roles");
    let count = rolify_mongodb::collection::count_docs(&roles, doc! {})
        .await
        .expect("count documents");
    assert_eq!(count, expected);
}

/// Two tasks race `find_or_create_by` on the same triple; exactly one
/// document must exist after both settle (11000 catch + re-read).
async fn race_find_or_create(scope: ScopeKind, label: &str) {
    let mut backend = MongoBackend::<DefaultUser>::build()
        .await
        .expect("backend builds");
    backend.reset_roles().await.expect("clean slate");
    let database = crate::support::mongo_database().await;

    for iteration in 0..RACE_ITERATIONS {
        clear_roles(&database).await;

        let name = RoleName::from(format!("raced_{label}_{iteration}"));
        let barrier = Arc::new(Barrier::new(2));
        let left_store = store_for(&database).await;
        let right_store = store_for(&database).await;

        let name_a = name.clone();
        let name_b = name.clone();
        let scope_a = scope.clone();
        let scope_b = scope.clone();
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);

        let (record_a, record_b): (rolify_core::role::RoleRecord, rolify_core::role::RoleRecord) = {
            let mut left = left_store;
            let mut right = right_store;
            let handle_a = tokio::spawn(async move {
                let mut conn = ();
                barrier_a.wait().await;
                left.find_or_create_by(&mut conn, &name_a, scope_a.to_resource_ref())
                    .await
                    .expect("find_or_create_by left")
            });
            let handle_b = tokio::spawn(async move {
                let mut conn = ();
                barrier_b.wait().await;
                right
                    .find_or_create_by(&mut conn, &name_b, scope_b.to_resource_ref())
                    .await
                    .expect("find_or_create_by right")
            });
            (
                handle_a.await.expect("task a joins"),
                handle_b.await.expect("task b joins"),
            )
        };

        assert_eq!(
            record_a, record_b,
            "both racers observe the SAME winner record ({label} iteration {iteration})"
        );
        assert_role_document_count(&database, 1).await;
    }
}

/// Two tasks race `add` of the same holder to the same role; `$addToSet`
/// idempotence resolves the loser to `false`, exactly one link exists.
async fn race_add(scope: ScopeKind, label: &str) {
    let mut backend = MongoBackend::<DefaultUser>::build()
        .await
        .expect("backend builds");
    backend.reset_roles().await.expect("clean slate");
    let database = crate::support::mongo_database().await;
    let holder = backend.holder_id("admin").expect("fixture holder resolves");

    for iteration in 0..RACE_ITERATIONS {
        clear_roles(&database).await;

        let name = RoleName::from(format!("linked_{label}_{iteration}"));
        let mut seed_store = store_for(&database).await;
        let record = {
            let mut conn = ();
            seed_store
                .find_or_create_by(&mut conn, &name, scope.to_resource_ref())
                .await
                .expect("seed role")
        };

        let barrier = Arc::new(Barrier::new(2));
        let left_store = store_for(&database).await;
        let right_store = store_for(&database).await;
        let record_a = record.clone();
        let record_b = record.clone();
        let holder_a = holder.clone();
        let holder_b = holder.clone();
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);

        let (added_a, added_b): (bool, bool) = {
            let mut left = left_store;
            let mut right = right_store;
            let handle_a = tokio::spawn(async move {
                let mut conn = ();
                barrier_a.wait().await;
                left.add(&mut conn, &holder_a, &record_a)
                    .await
                    .expect("add left")
            });
            let handle_b = tokio::spawn(async move {
                let mut conn = ();
                barrier_b.wait().await;
                right
                    .add(&mut conn, &holder_b, &record_b)
                    .await
                    .expect("add right")
            });
            (
                handle_a.await.expect("task a joins"),
                handle_b.await.expect("task b joins"),
            )
        };

        let mut outcomes = [added_a, added_b];
        outcomes.sort_unstable();
        assert_eq!(
            outcomes,
            [false, true],
            "exactly one racer wins the link ({label} iteration {iteration})"
        );

        // Both sides carry exactly one entry (two-sided HABTM proof).
        let roles = database.collection::<bson::Document>("roles");
        let docs = find_docs(&roles, doc! {}).await.expect("read roles");
        assert_eq!(docs.len(), 1, "unique role document remains");
        let doc = RoleDoc::from_document(&docs[0]).expect("role doc parses");
        assert_eq!(doc.user_ids, vec![holder.as_str().to_owned()]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn race_find_or_create_converges_globally() {
    race_find_or_create(ScopeKind::Global, "global").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn race_find_or_create_converges_on_class() {
    race_find_or_create(ScopeKind::Class("Forum"), "class").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn race_find_or_create_converges_on_instance() {
    race_find_or_create(
        ScopeKind::Instance("Team", ResourceId::from("1")),
        "instance",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn race_add_is_idempotent_globally() {
    race_add(ScopeKind::Global, "global").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn race_add_is_idempotent_on_class() {
    race_add(ScopeKind::Class("Forum"), "class").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn race_add_is_idempotent_on_instance() {
    race_add(
        ScopeKind::Instance("Team", ResourceId::from("1")),
        "instance",
    )
    .await;
}
