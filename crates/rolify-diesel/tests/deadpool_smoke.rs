//! deadpool smoke test for the diesel-async rider (D-09).
//!
//! Scope is deliberately narrow and LOCKED (prohibition 3): deadpool gets a
//! compile-check plus exactly one grant-then-check round trip on a single
//! checkout. The parity gate never runs on deadpool: bb8 is the canonical
//! gate pool (D-09); this test only proves the pool feature wires up.
//!
//! Self-contained on purpose: the shared test support module seats bb8, and
//! this leg must compile in the `async,deadpool,postgres` combination
//! without bb8 in the feature graph.
//!
//! Runs: `cargo test -p rolify-diesel --no-default-features --features async,deadpool,postgres --test deadpool_smoke`

#![cfg(all(feature = "async", feature = "deadpool", feature = "postgres"))]

use std::sync::OnceLock;

use diesel::Connection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_diesel::{DieselStore, MIGRATIONS, rows::IdRow};
use testcontainers::ImageExt;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres;
use tokio::sync::OnceCell;

static CONTAINER: OnceCell<testcontainers::ContainerAsync<postgres::Postgres>> =
    OnceCell::const_new();

async fn container() -> &'static testcontainers::ContainerAsync<postgres::Postgres> {
    CONTAINER
        .get_or_init(|| async {
            AsyncRunner::start(postgres::Postgres::default().with_tag("17"))
                .await
                .expect("Docker must be available; postgres:17 will be pulled")
        })
        .await
}

async fn database_url() -> String {
    let container = container().await;
    let host_port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Postgres port mapping");
    format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres")
}

/// Apply the embedded migrations off the async runtime (the sync harness
/// runs inside `spawn_blocking`; legal on every runtime flavor).
async fn run_migrations() {
    let url = database_url().await;
    tokio::task::spawn_blocking(move || {
        let mut conn =
            diesel::pg::PgConnection::establish(&url).expect("sync pg connection for migrations");
        use diesel_migrations::MigrationHarness;
        conn.run_pending_migrations(MIGRATIONS)
            .expect("embedded migrations apply cleanly");
    })
    .await
    .expect("migration task must join");
}

async fn async_conn() -> AsyncPgConnection {
    let url = database_url().await;
    AsyncPgConnection::establish(&url)
        .await
        .expect("async pg connection")
}

#[tokio::test(flavor = "multi_thread")]
async fn deadpool_smoke_single_grant_check_round_trip() {
    run_migrations().await;

    // Fixture tables + a clean roles table on a direct connection.
    let mut conn = async_conn().await;
    for statement in rolify_test::ddl::POSTGRES {
        diesel::sql_query(*statement)
            .execute(&mut conn)
            .await
            .expect("fixture tables");
    }
    diesel::sql_query("TRUNCATE TABLE users_roles, roles RESTART IDENTITY CASCADE")
        .execute(&mut conn)
        .await
        .expect("truncate roles");
    let holder_row: IdRow =
        diesel::sql_query("INSERT INTO users (rolify_type, name) VALUES ($1, $2) RETURNING id")
            .bind::<diesel::sql_types::Text, _>("User")
            .bind::<diesel::sql_types::Text, _>("smoke_user")
            .get_result(&mut conn)
            .await
            .expect("insert smoke holder");
    let user_id = ResourceId::from(holder_row.id);
    drop(conn);

    // Build the deadpool pool over the diesel-async manager: the compile
    // itself is half of the D-09 scope.
    let url = database_url().await;
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
    let pool: Pool<AsyncPgConnection> = Pool::builder(manager)
        .build()
        .expect("deadpool builds over the async manager");

    // One checkout; the Object derefs to the inner AsyncPgConnection.
    let mut checkout = pool.get().await.expect("deadpool checkout");

    let config = RolifyConfig::builder().build().unwrap();
    let mut store = DieselStore::new(&config);

    // The single grant-then-check round trip.
    let admin = store
        .find_or_create_by(
            &mut *checkout,
            &RoleName::from("admin"),
            ResourceRef::Global,
        )
        .await
        .expect("smoke: find_or_create_by global admin");
    assert!(admin.is_global(), "smoke: global admin created");

    let added = store
        .add(&mut *checkout, &user_id, &admin)
        .await
        .expect("smoke: add global admin");
    assert!(added, "smoke: first add creates the link");

    let roles = store
        .where_(
            &mut *checkout,
            &user_id,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            },
        )
        .await
        .expect("smoke: where_ readback");
    assert_eq!(roles.len(), 1, "smoke: exactly one global admin role");
    assert!(roles[0].is_global(), "smoke: readback is the global role");
}
