//! SC-3 compile-and-behavior smoke tests (OQ-1 pinned list). Compiles and
//! runs under BOTH modes via the two cargo invocations
//! `cargo test -p rolify-core` and
//! `cargo test -p rolify-core --features is_sync`.
//!
//! Dyn-compatibility contract being pinned: the **consumer** traits
//! (`Resource`) and the callback closures (`Arc<dyn Fn(&RoleRecord) -> ...>`)
//! ARE dyn-able; the store SPI (`RoleStore`/`ResourceStore`) is intentionally
//! NOT (`type Conn` makes it static-dispatch only - E0038 by design, see
//! [`rolify_core::store`]). If this file ever tries to box the SPI, the
//! compile failure IS the guard doing its job.

use std::sync::Arc;

use rolify_core::config::RolifyConfig;
use rolify_core::error::RolifyError;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::{Resource, ResourceRef};
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::RoleStore;
use rolify_core::user::RolifyUser;
use rolify_test::InMemoryStore;

const fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn store_and_config_are_send_sync() {
    assert_send_sync::<InMemoryStore>();
    assert_send_sync::<RolifyConfig>();
}

/// Consumer domain type used by the smoke tests (a "user" that is also a
/// resource - the gem allows this; only `RolifyUser`/Resource impls matter).
struct Customer {
    id: i64,
    store: InMemoryStore,
    conn: (),
    config: RolifyConfig,
}

impl Customer {
    fn new(id: i64) -> Self {
        Self {
            id,
            store: InMemoryStore::new(),
            conn: (),
            config: RolifyConfig::default(),
        }
    }
}

impl Resource for Customer {
    fn type_name() -> &'static str {
        "Customer"
    }

    fn resource_id(&self) -> ResourceId {
        ResourceId::from(self.id)
    }
}

impl RolifyUser for Customer {
    type Store = InMemoryStore;

    fn store(&mut self) -> &mut InMemoryStore {
        &mut self.store
    }

    fn rolify_config(&self) -> &RolifyConfig {
        &self.config
    }

    fn rolify_id(&self) -> ResourceId {
        ResourceId::from(self.id)
    }

    fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
        (&mut self.store, &mut self.conn)
    }
}

/// The CONF-05 callback hook shape (the canonical `BeforeAddHook`-style alias
/// lands in `config.rs` with 01-03); aliased here so the dyn-compat assertion
/// stays readable.
type TestHook = Arc<dyn Fn(&RoleRecord) -> Result<(), RolifyError> + Send + Sync>;

#[test]
fn consumer_traits_and_callbacks_are_dyn_compatible() {
    let id = faker_rust::number::between(1, 10_000);
    let customer = Customer::new(id);
    // `Resource` stays `dyn`-able (no associated consts - SC-3).
    let _resource: Box<dyn Resource> = Box::new(customer);
    // The 01-03 callback hook type is `dyn`-able and `Send + Sync` (CONF-05).
    let _callback: TestHook = Arc::new(|_record| Ok(()));
}

/// The walking skeleton: a global `admin` role row satisfies
/// `has_role("admin")` through the full stack - `RolifyUser` provided method
/// → soft-sealed `RoleStore` → pure kernel non-strict ladder →
/// `InMemoryStore` - and in async mode the provided-method future is `Send`
/// (proven by `tokio::spawn`).
#[maybe_async::test(
    feature = "is_sync",
    async(not(feature = "is_sync"), tokio::test)
)]
async fn has_role_global_admin_end_to_end() {
    // faker-rust wires test data (TEST-04); the role name is fixed so the
    // assertion is meaningful.
    let admin = RoleName::from("admin");
    let someone_else = RoleName::from(faker_rust::internet::username(None));

    let mut user = Customer::new(faker_rust::number::between(1, 10_000));
    let user_id = user.rolify_id();
    user.store().grant(&user_id, RoleRecord::global(admin.clone()));

    // Level-1 dedupe through the same store (same triple → one row).
    let (store, conn) = user.store_with_conn();
    let first = store
        .find_or_create_by(&mut *conn, &admin, ResourceRef::Global)
        .await
        .unwrap();
    let second = store
        .find_or_create_by(&mut *conn, &admin, ResourceRef::Global)
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(store.rows().len(), 1);

    #[cfg(not(feature = "is_sync"))]
    {
        // Send proof: the provided-method futures cross a spawn boundary.
        let mut spawned_user = Customer::new(faker_rust::number::between(1, 10_000));
        let spawned_id = spawned_user.rolify_id();
        spawned_user
            .store()
            .grant(&spawned_id, RoleRecord::global(RoleName::from("admin")));
        let handle = tokio::spawn(async move {
            spawned_user.has_role(&RoleName::from("admin"), ResourceFilter::Global).await
        });
        let spawned_result = handle.await.unwrap();
        assert!(matches!(spawned_result, Ok(true)));

        // Same Send proof over the write path (add_role provided method).
        let mut write_user = Customer::new(faker_rust::number::between(1, 10_000));
        let write_handle = tokio::spawn(async move {
            write_user
                .add_role(&RoleName::from("editor"), ResourceRef::Global)
                .await
        });
        let write_result = write_handle.await.unwrap();
        assert!(write_result.is_ok());
    }

    // The walking-skeleton predicate, both modes.
    let has_admin = user.has_role(&admin, ResourceFilter::Global).await.unwrap();
    assert!(has_admin, "global admin role must satisfy the global query");

    let has_other = user.has_role(&someone_else, ResourceFilter::Global).await.unwrap();
    assert!(!has_other, "unknown role name must not match (byte-exact)");
}
