//! D-08-08 behavioral guard: `MongoDB` storage stays canonical strings
//! inside `user_ids` regardless of the configured SQL holder id kind.
//!
//! The `mongo_unchanged` compile-only regression test (src/lib.rs) proves
//! the crate builds untouched by Phase 08; this suite pins the BEHAVIOR
//! the decision locks: for every `HolderIdKind` the same store writes the
//! holder id as a canonical BSON string inside the role document's
//! `user_ids` (never `Int32`/`Int64` for the integer kind, never `Binary`
//! for the uuid kind), and the string query path round-trips unchanged.
//!
//! Runs: `cargo test -p rolify-mongodb --test holder_kind` (async, driver
//! default). The store's write path is one `maybe_async` body, so sync
//! mode exercises the identical logic; dual-mode parity is owned by
//! `parity_mongo.rs`. This file is async-mode-only, like
//! `tracer_mongo.rs`.

#![cfg(not(feature = "sync"))]

use bson::{Bson, Document, doc};
use rolify_core::config::{HolderIdKind, RolifyConfig};
use rolify_core::kernel::RemovalTarget;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_mongodb::MongoStore;
use rolify_mongodb::collection::{CollectionHandle, DatabaseHandle, find_one_doc};
use testcontainers::ImageExt;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mongo::Mongo;

/// Dev image pin: `mongo:8.2` (mirrors tests/support/mod.rs; the original
/// 8.0 pin cannot start on kernel 6.19+ hosts, SERVER-121912).
const MONGO_TAG: &str = "8.2";

/// Per-kind matrix: (kind, fresh database name, canonical holder id).
/// Every case gets its own database, so the two tests never share state
/// and need no cross-test serialization.
#[allow(clippy::type_complexity)]
fn kind_matrix() -> [(HolderIdKind, &'static str, &'static str); 3] {
    [
        (HolderIdKind::Integer, "rolify_holder_kind_integer", "42"),
        (
            HolderIdKind::Uuid,
            "rolify_holder_kind_uuid",
            "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8",
        ),
        (HolderIdKind::String, "rolify_holder_kind_string", "user-1"),
    ]
}

/// A `Database` handle against the shared container (started once per
/// test binary).
async fn mongo_database(name: &str) -> DatabaseHandle {
    static CONTAINER: tokio::sync::OnceCell<testcontainers::ContainerAsync<Mongo>> =
        tokio::sync::OnceCell::const_new();
    let container = CONTAINER
        .get_or_init(|| async {
            Mongo::default()
                .with_tag(MONGO_TAG)
                .start()
                .await
                .expect("mongo container starts")
        })
        .await;
    let port = container
        .get_host_port_ipv4(27017)
        .await
        .expect("mongo port is mapped");
    let client = mongodb::Client::with_uri_str(format!("mongodb://127.0.0.1:{port}/"))
        .await
        .expect("container URI parses");
    client.database(name)
}

/// Build a store carrying `kind` over a fresh per-kind database.
async fn store_for(
    kind: HolderIdKind,
    database_name: &str,
) -> (MongoStore, DatabaseHandle, RolifyConfig) {
    let database = mongo_database(database_name).await;
    let config = RolifyConfig::builder()
        .holder_id_kind(kind)
        .build()
        .expect("config builds for every holder id kind");
    let store = MongoStore::new(&database, &config);
    store.ensure_indexes().await.expect("ensure indexes");
    (store, database, config)
}

/// Grant global `admin` to the canonical holder for `kind`, returning
/// the granted role record.
async fn grant_admin(
    store: &mut MongoStore,
    kind: HolderIdKind,
    canonical: &str,
) -> rolify_core::role::RoleRecord {
    let mut conn = ();
    let admin = store
        .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
        .await
        .expect("find_or_create_by global admin");
    let holder = ResourceId::from(canonical);
    assert!(
        store
            .add(&mut conn, &holder, &admin)
            .await
            .expect("add admin"),
        "first add inserts a link for {kind:?}"
    );
    admin
}

/// D-08-08 storage shape: for every kind, the role document carries the
/// holder id inside `user_ids` as a canonical BSON STRING - the persisted
/// document shape never branches on the configured SQL kind.
#[tokio::test(flavor = "multi_thread")]
async fn user_ids_stay_canonical_strings_for_every_kind() {
    for (kind, database_name, canonical) in kind_matrix() {
        let (mut store, database, config) = store_for(kind, database_name).await;
        grant_admin(&mut store, kind, canonical).await;

        let roles: CollectionHandle = database.collection::<Document>(config.role_table());
        let document = find_one_doc(&roles, doc! { "name": "admin" })
            .await
            .expect("read role document")
            .expect("role document exists");
        let user_ids = document.get_array("user_ids").expect("user_ids array");
        assert_eq!(user_ids.len(), 1, "one holder link for {kind:?}");
        match &user_ids[0] {
            Bson::String(stored) => assert_eq!(
                stored, canonical,
                "holder id must stay the canonical string for {kind:?}"
            ),
            other => panic!(
                "holder id for {kind:?} must be the canonical string \
                 {canonical:?}, got {other:?} (native storage is forbidden by D-08-08)"
            ),
        }
    }
}

/// D-08-08 query path: the grant/check/revoke lifecycle round-trips the
/// canonical holder id unchanged under every kind's config - the string
/// bind path already covers integer and uuid holders.
#[tokio::test(flavor = "multi_thread")]
async fn holder_lifecycle_round_trips_for_every_kind() {
    for (kind, database_name, canonical) in kind_matrix() {
        let lifecycle_database = format!("{database_name}_lifecycle");
        let (mut store, _database, _config) = store_for(kind, &lifecycle_database).await;
        grant_admin(&mut store, kind, canonical).await;

        let mut conn = ();
        let holder = ResourceId::from(canonical);
        assert_eq!(
            store
                .roles_of(&mut conn, &holder)
                .await
                .expect("roles_of")
                .len(),
            1,
            "roles_of lists the granted role for {kind:?}"
        );

        let outcome = store
            .remove(
                &mut conn,
                &holder,
                &RoleName::from("admin"),
                RemovalTarget::NameOnly,
                true,
            )
            .await
            .expect("remove admin");
        assert_eq!(outcome.removed_links, 1, "removed one link for {kind:?}");
        assert_eq!(
            store
                .roles_of(&mut conn, &holder)
                .await
                .expect("roles_of after remove")
                .len(),
            0,
            "no roles remain for {kind:?}"
        );
    }
}
