//! Test support for `rolify-mongodb`: a shared `mongo` testcontainer,
//! fixture collections, and `MongoBackend` for the suite binding in BOTH
//! driver modes (D-13: the identical `parity_suite!` runs async default
//! and under `sync` + `rolify-core/is_sync`).
//!
//! - Async mode: `AsyncRunner` startup on a `tokio::sync::OnceCell`
//!   (the Phase 4 A2 shape), `mongodb::Client` handles.
//! - Sync mode: `SyncRunner` startup on a `std::sync::OnceLock`,
//!   `mongodb::sync::Client` handles (thread offloading; never
//!   `block_on` in this crate's library or suite code).
//! - Fixture holders are seeded once per class collection with
//!   stringified ids (`holder_id`), the class discriminator
//!   (`rolify_type`), and an empty `role_ids` array (D-08 consumer side).
//! - Fixture resources are seeded once from
//!   [`rolify_test::fixtures::mongo_resource_identities`] with stringified
//!   `resource_id` fields (integer PKs stringified, `team_code` kept;
//!   RESEARCH Q3).
//! - `reset_roles` deletes the roles collection and clears `role_ids`
//!   on every holder doc, mirroring `role_class.destroy_all` +
//!   `roles = []` (`shared_contexts.rb:14-15`).
//! - A process-wide suite lock is held for the backend's lifetime so
//!   suite cases never wipe each other's state on the shared container.

#![cfg(feature = "suite")]

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use bson::{Document, doc};
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::{ResourceKey, RoleStore, Sealed};
use rolify_core::user::RolifyUser;
use rolify_mongodb::collection::{
    CollectionHandle, DatabaseHandle, count_docs, delete_docs, insert_doc, set_doc, update_docs,
};
use rolify_mongodb::document::{ObjectId, RoleDoc};
use rolify_mongodb::{Error, MongoStore};
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::{
    FixtureResource, FixtureResources, UserClass, fixture_holders, mongo_holder_collection,
    mongo_resource_identities,
};
use testcontainers::ImageExt;
use testcontainers_modules::mongo::Mongo;

use maybe_async::maybe_async;

/// Dev image pin: `mongo:8.2` (mirrors docker-compose.yml). The original
/// D-14 pin (`mongo:8.0`) cannot start on kernel 6.19+ hosts
/// (SERVER-121912), so local gates and CI both run 8.2.
const MONGO_TAG: &str = "8.2";

#[cfg(not(feature = "sync"))]
use testcontainers::ContainerAsync;

/// Async container: started once per test binary through `AsyncRunner`.
#[cfg(not(feature = "sync"))]
async fn mongo_container() -> &'static ContainerAsync<Mongo> {
    use testcontainers::runners::AsyncRunner;
    static CONTAINER: tokio::sync::OnceCell<ContainerAsync<Mongo>> =
        tokio::sync::OnceCell::const_new();
    CONTAINER
        .get_or_init(|| async {
            Mongo::default()
                .with_tag(MONGO_TAG)
                .start()
                .await
                .expect("mongo container starts")
        })
        .await
}

/// Sync container: started once via the blocking runner.
#[cfg(feature = "sync")]
fn mongo_container() -> &'static testcontainers::Container<Mongo> {
    use testcontainers::runners::SyncRunner;
    static CONTAINER: std::sync::OnceLock<testcontainers::Container<Mongo>> =
        std::sync::OnceLock::new();
    CONTAINER.get_or_init(|| {
        Mongo::default()
            .with_tag(MONGO_TAG)
            .start()
            .expect("mongo container starts")
    })
}

/// A `Database` handle against the shared container.
#[maybe_async]
pub async fn mongo_database() -> DatabaseHandle {
    let container = mongo_container().await;
    #[cfg(not(feature = "sync"))]
    let port = container
        .get_host_port_ipv4(27017)
        .await
        .expect("mongo port is mapped");
    #[cfg(feature = "sync")]
    let port = container
        .get_host_port_ipv4(27017)
        .expect("mongo port is mapped");
    #[cfg(not(feature = "sync"))]
    let client = mongodb::Client::with_uri_str(format!("mongodb://127.0.0.1:{port}/"))
        .await
        .expect("container URI parses");
    #[cfg(feature = "sync")]
    let client = mongodb::sync::Client::with_uri_str(format!("mongodb://127.0.0.1:{port}/"))
        .expect("container URI parses");
    client.database("rolify_test")
}

/// The holder collection for class `C` (gem `user_cname` mapping).
fn holder_collection<C: UserClass>() -> &'static str {
    mongo_holder_collection(C::rolify_type()).unwrap_or_else(|| {
        panic!(
            "no fixture holder collection for class {}",
            C::rolify_type()
        )
    })
}

/// Seed one holder document per fixture login; idempotent via upsert so
/// `build` can re-run over a shared container.
#[maybe_async]
async fn seed_holders(database: &DatabaseHandle, holder_collection: &str, rolify_type: &str) {
    let holders = database.collection::<Document>(holder_collection);
    for (_login, holder_id) in fixture_holders() {
        set_doc(
            &holders,
            doc! { "holder_id": holder_id.as_str() },
            doc! {
                "$set": {
                    "holder_id": holder_id.as_str(),
                    "rolify_type": rolify_type,
                    "role_ids": Vec::<ObjectId>::new(),
                },
            },
        )
        .await
        .expect("seed holder doc");
    }
}

/// Seed the canonical resource collections from the shared identity map.
#[maybe_async]
async fn seed_resources(database: &DatabaseHandle) {
    for (collection_name, ids) in mongo_resource_identities() {
        let collection: CollectionHandle = database.collection::<Document>(collection_name);
        for resource_id in ids {
            set_doc(
                &collection,
                doc! { "resource_id": resource_id.as_str() },
                doc! { "$set": { "resource_id": resource_id.as_str() } },
            )
            .await
            .expect("seed resource doc");
        }
    }
}

// Process-wide suite lock (serializes suite cases per binary): a spin
// flag so the guard type is `Send+Sync` in both driver modes (the diesel
// backend's SuiteGuard pattern).
static SUITE_SERIAL: AtomicBool = AtomicBool::new(false);

/// Held for one backend's lifetime; releases on drop.
pub struct SuiteGuard {
    flag: &'static AtomicBool,
}

impl SuiteGuard {
    /// Acquire the process-wide lock, spinning (liveness-only, no timers).
    fn acquire() -> Self {
        while SUITE_SERIAL
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            #[cfg(not(feature = "sync"))]
            std::hint::spin_loop();
            #[cfg(feature = "sync")]
            std::hint::spin_loop();
        }
        Self {
            flag: &SUITE_SERIAL,
        }
    }
}

impl Drop for SuiteGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// The Mongo backend for the ported suite: one `MongoStore` per class
/// (roles collection named by `C::config().role_table()`, holder
/// collection by class, D-09), seeded fixture holders/resources, and the
/// engine the subject and finders share.
pub struct MongoBackend<C: UserClass> {
    _serial: SuiteGuard,
    subject: MongoSubject<C>,
    counter: std::sync::Arc<AtomicUsize>,
    fixture_holders: Vec<(&'static str, ResourceId)>,
    resources: FixtureResources,
    database: DatabaseHandle,
}

/// The suite subject over Mongo: holder identity plus the shared engine
/// (mirrors the diesel backend's local subject; `FixtureUser::engine` is
/// `pub(crate)` to rolify-test, so adapters seat their own).
pub struct MongoSubject<C: UserClass> {
    login: String,
    holder: ResourceId,
    engine: Rolify<MongoStore>,
    config: RolifyConfig,
    class_marker: PhantomData<C>,
}

impl<C: UserClass> MongoBackend<C> {
    fn roles_collection(&self) -> CollectionHandle {
        self.database
            .collection::<Document>(self.subject.rolify_config().role_table())
    }

    fn holders_collection(&self) -> CollectionHandle {
        self.database
            .collection::<Document>(holder_collection::<C>())
    }
}

impl<C: UserClass> Sealed for MongoBackend<C> {}

#[maybe_async(AFIT)]
impl<C: UserClass> TestBackend for MongoBackend<C> {
    type Store = MongoStore;
    type Subject = MongoSubject<C>;
    type Error = Error;

    /// Build a backend: hold the suite lock, start the container (once),
    /// seed fixtures, ensure the unique index, seat the default subject.
    fn build() -> impl std::future::Future<Output = Result<Self, Self::Error>> + Send {
        async move {
            let serial = SuiteGuard::acquire();
            let database = mongo_database().await;
            let resources = FixtureResources::new();
            let config = C::config();
            let store = MongoStore::new(&database, &config)
                .for_holder_collection(holder_collection::<C>())
                .register_resource_collection("Forum", "forums")
                .register_resource_collection("Group", "groups")
                .register_resource_collection("Team", "teams")
                .register_resource_collection("Organization", "organizations")
                .register_resource_collection("Company", "companies");
            store.ensure_indexes().await?;
            let counter = store.query_counter_probe();
            // Seedings are idempotent rewrites (also re-arms the counter
            // to zero for the first guard case).
            seed_holders(&database, holder_collection::<C>(), C::rolify_type()).await;
            seed_resources(&database).await;
            store.reset_query_count();
            let engine = Rolify::new(store, (), config.clone());
            let subject = MongoSubject {
                login: "admin".to_owned(),
                holder: ResourceId::from(1_i64),
                engine,
                config,
                class_marker: PhantomData,
            };
            Ok(Self {
                _serial: serial,
                subject,
                counter,
                fixture_holders: fixture_holders(),
                resources,
                database,
            })
        }
    }

    fn subject(&mut self, login: &str) -> &mut Self::Subject {
        let holder = self
            .holder_id(login)
            .expect("unknown fixture login: expected admin, moderator, god, or zombie");
        self.subject.login.clear();
        self.subject.login.push_str(login);
        self.subject.holder = holder;
        &mut self.subject
    }

    fn holder_id(&self, login: &str) -> Option<ResourceId> {
        self.fixture_holders
            .iter()
            .find(|(known, _)| *known == login)
            .map(|(_, id)| id.clone())
    }

    fn resource(&self, which: FixtureResource) -> ResourceKey {
        self.resources.key(which)
    }

    fn reset_roles(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async move {
            delete_docs(&self.roles_collection(), doc! {}).await?;
            update_docs(
                &self.holders_collection(),
                doc! {},
                doc! { "$set": { "role_ids": Vec::<ObjectId>::new() } },
            )
            .await?;
            Ok(())
        }
    }

    fn create_role_row(
        &mut self,
        record: RoleRecord,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async move {
            // `role_class.create`: an UNLINKED row (no consumer write).
            insert_doc(
                &self.roles_collection(),
                RoleDoc::from_record(&record).to_document(),
            )
            .await?;
            Ok(())
        }
    }

    fn grant_to(
        &mut self,
        login: &str,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async move {
            let holder = self
                .holder_id(login)
                .expect("unknown fixture login: expected admin, moderator, god, or zombie");
            let (store, conn) = RolifyUser::store_with_conn(&mut self.subject);
            let role = store.find_or_create_by(&mut *conn, name, scope).await?;
            let _ = store.add(&mut *conn, &holder, &role).await?;
            Ok(())
        }
    }

    fn role_row_count(
        &mut self,
    ) -> impl std::future::Future<Output = Result<usize, Self::Error>> + Send {
        async move {
            let count = count_docs(&self.roles_collection(), doc! {}).await?;
            Ok(usize::try_from(count).unwrap_or(usize::MAX))
        }
    }

    fn reset_query_count(&mut self) {
        self.counter.store(0, Ordering::Relaxed);
    }

    fn query_count(&self) -> Option<usize> {
        Some(self.counter.load(Ordering::Relaxed))
    }

    fn engine(&mut self) -> &mut Rolify<Self::Store> {
        &mut self.subject.engine
    }
}

#[maybe_async(AFIT)]
impl<C: UserClass> RolifyUser for MongoSubject<C> {
    type Store = MongoStore;

    fn store(&mut self) -> &mut Self::Store {
        self.engine.store_with_conn().0
    }

    fn rolify_config(&self) -> &RolifyConfig {
        &self.config
    }

    fn rolify_id(&self) -> ResourceId {
        self.holder.clone()
    }

    fn rolify_type() -> &'static str {
        C::rolify_type()
    }

    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut <Self::Store as RoleStore>::Conn) {
        self.engine.store_with_conn()
    }
}
