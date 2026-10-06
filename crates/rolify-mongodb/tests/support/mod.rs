//! Test support for `rolify-mongodb`: a shared `mongo:8.0` testcontainer,
//! fixture collections, and `MongoBackend` for the suite binding.
//!
//! - Shared container per test binary via `tokio::sync::OnceCell`
//!   (`AsyncRunner` startup, the Phase 4 A2 shape).
//! - Fixture holders are seeded once per class collection with stringified
//!   ids (`holder_id`), the class discriminator (`rolify_type`), and an
//!   empty `role_ids` array (D-08 consumer side).
//! - Fixture resources are seeded once with stringified `resource_id`
//!   fields (integer PKs stringified, `team_code` kept as-is; RESEARCH Q3).
//! - `reset_roles` deletes the roles collection and clears `role_ids` on
//!   every holder doc, mirroring `role_class.destroy_all` + `roles = []`
//!   (`shared_contexts.rb:14-15`).

#![cfg(all(feature = "suite", not(feature = "sync")))]

use std::marker::PhantomData;

use bson::{Document, doc};
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::{ResourceKey, RoleStore, Sealed};
use rolify_core::user::RolifyUser;
use rolify_mongodb::document::{ObjectId, RoleDoc};
use rolify_mongodb::{Error, MongoStore};
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::{FixtureResource, FixtureResources, UserClass, fixture_holders};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::mongo::Mongo;

use maybe_async::maybe_async;

/// Dev image pin: `mongo:8.2` (mirrors docker-compose.yml). The original
/// D-14 pin (`mongo:8.0`) cannot start on kernel 6.19+ hosts
/// (SERVER-121912), so local gates and CI both run 8.2.
async fn mongo_container() -> &'static ContainerAsync<Mongo> {
    static CONTAINER: tokio::sync::OnceCell<ContainerAsync<Mongo>> =
        tokio::sync::OnceCell::const_new();
    CONTAINER
        .get_or_init(|| async {
            Mongo::default()
                .with_tag("8.2")
                .start()
                .await
                .expect("mongo container starts")
        })
        .await
}

/// A `Database` handle against the (shared, lazily started) container.
async fn mongo_database() -> mongodb::Database {
    let container = mongo_container().await;
    let port = container
        .get_host_port_ipv4(27017)
        .await
        .expect("mongo port is mapped");
    let client = mongodb::Client::with_uri_str(format!("mongodb://127.0.0.1:{port}/"))
        .await
        .expect("container URI parses");
    client.database("rolify_test")
}

/// The holder collection name per fixture class (the gem's `user_cname`:
/// `users` / `customers` / `moderators`).
fn holder_collection<C: UserClass>() -> &'static str {
    match C::rolify_type() {
        "User" | "StrictUser" => "users",
        "Customer" => "customers",
        "Admin::Moderator" => "moderators",
        other => panic!("no fixture holder collection for class {other}"),
    }
}

/// Seed one holder document per fixture login, identified by stringified
/// PK, carrying the class discriminator; idempotent via upsert on
/// `(holder_id)` so `build` can run repeatedly over a shared container.
async fn seed_holders(database: &mongodb::Database, holder_collection: &str, rolify_type: &str) {
    let holders = database.collection::<Document>(holder_collection);
    for (_login, holder_id) in fixture_holders() {
        let filter = doc! { "holder_id": holder_id.as_str() };
        let update = doc! {
            "$setOnInsert": {
                "holder_id": holder_id.as_str(),
                "rolify_type": rolify_type,
            },
            "$set": { "role_ids": Vec::<ObjectId>::new() },
        };
        holders
            .update_many(filter, update)
            .upsert(true)
            .await
            .expect("seed holder doc");
    }
}

/// Seed the canonical resource collections (`forums` 1-3, `groups` 1-2,
/// `teams` "1"/"2" string keys, `organizations`/`companies` id 1), so the
/// class-scope `resources_find` branch reads the full "relation.all" list.
async fn seed_resources(database: &mongodb::Database, resources: &FixtureResources) {
    let seeds: [(&str, ResourceId); 9] = [
        (
            "forums",
            resources.key(FixtureResource::ForumFirst).resource_id,
        ),
        (
            "forums",
            resources.key(FixtureResource::ForumSecond).resource_id,
        ),
        (
            "forums",
            resources.key(FixtureResource::ForumLast).resource_id,
        ),
        (
            "groups",
            resources.key(FixtureResource::GroupFirst).resource_id,
        ),
        (
            "groups",
            resources.key(FixtureResource::GroupLast).resource_id,
        ),
        (
            "teams",
            resources.key(FixtureResource::TeamFirst).resource_id,
        ),
        (
            "teams",
            resources.key(FixtureResource::TeamLast).resource_id,
        ),
        (
            "organizations",
            resources.key(FixtureResource::Organization).resource_id,
        ),
        (
            "companies",
            resources.key(FixtureResource::Company).resource_id,
        ),
    ];
    for (collection_name, resource_id) in seeds {
        let collection = database.collection::<Document>(collection_name);
        collection
            .update_many(
                doc! { "resource_id": resource_id.as_str() },
                doc! { "$setOnInsert": { "resource_id": resource_id.as_str() } },
            )
            .upsert(true)
            .await
            .expect("seed resource doc");
    }
}

/// The Mongo backend for the ported suite: one `MongoStore` per class
/// (roles collection named by `C::config().role_table()`, holder
/// collection by class, D-09), seeded fixture holders/resources, and the
/// engine the subject and finders share.
pub struct MongoBackend<C: UserClass> {
    subject: MongoSubject<C>,
    counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    fixture_holders: Vec<(&'static str, ResourceId)>,
    resources: FixtureResources,
    database: mongodb::Database,
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
    fn roles_collection(&self) -> mongodb::Collection<Document> {
        self.database
            .collection::<Document>(self.subject.rolify_config().role_table())
    }

    fn holders_collection(&self) -> mongodb::Collection<Document> {
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

    /// Build a backend: start the container (once per binary), seed
    /// fixtures, ensure the unique index, and seat the default subject.
    async fn build() -> Result<Self, Self::Error> {
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
        seed_holders(&database, holder_collection::<C>(), C::rolify_type()).await;
        seed_resources(&database, &resources).await;
        store.reset_query_count();
        let counter = store.query_counter_probe();
        let engine = Rolify::new(store, (), config.clone());
        let subject = MongoSubject {
            login: "admin".to_owned(),
            holder: ResourceId::from(1_i64),
            engine,
            config,
            class_marker: PhantomData,
        };
        Ok(Self {
            subject,
            counter,
            fixture_holders: fixture_holders(),
            resources,
            database,
        })
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

    async fn reset_roles(&mut self) -> Result<(), Self::Error> {
        rolify_mongodb::collection::delete_docs(&self.roles_collection(), doc! {}).await?;
        rolify_mongodb::collection::update_docs(
            &self.holders_collection(),
            doc! {},
            doc! { "$set": { "role_ids": Vec::<ObjectId>::new() } },
        )
        .await?;
        Ok(())
    }

    async fn create_role_row(&mut self, record: RoleRecord) -> Result<(), Self::Error> {
        // `role_class.create`: an UNLINKED row (no consumer write).
        rolify_mongodb::collection::insert_doc(
            &self.roles_collection(),
            RoleDoc::from_record(&record).to_document(),
        )
        .await?;
        Ok(())
    }

    async fn grant_to(
        &mut self,
        login: &str,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> Result<(), Self::Error> {
        let holder = self
            .holder_id(login)
            .expect("unknown fixture login: expected admin, moderator, god, or zombie");
        let (store, conn) = RolifyUser::store_with_conn(&mut self.subject);
        let role = store.find_or_create_by(&mut *conn, name, scope).await?;
        store.add(&mut *conn, &holder, &role).await?;
        Ok(())
    }

    async fn role_row_count(&mut self) -> Result<usize, Self::Error> {
        let count =
            rolify_mongodb::collection::count_docs(&self.roles_collection(), doc! {}).await?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    fn reset_query_count(&mut self) {
        self.subject.store().reset_query_count();
    }

    fn query_count(&self) -> Option<usize> {
        Some(self.counter.load(std::sync::atomic::Ordering::Relaxed))
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
