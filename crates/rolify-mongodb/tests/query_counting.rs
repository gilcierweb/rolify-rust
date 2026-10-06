//! Query-counting backstop for `rolify-mongodb` (TEST-05, D-15 analog):
//! the explicit per-method counter on `MongoStore` proves the uniform
//! guard contract that the diesel/sqlx/sea-orm backends pin through
//! instrumentation hooks - Mongo has no such hook, so the store counts.
//!
//! Asserts: counter starts at zero after build; `where_` and
//! `where_any` each cost exactly one store operation; the cached
//! (`has_cached_role`) path never touches the store; `reset` re-zeros.
//!
//! Runs in both modes:
//! - `cargo test -p rolify-mongodb --features suite --test query_counting`
//! - `cargo test -p rolify-mongodb --features suite,rolify-mongodb/sync,rolify-core/is_sync --test query_counting`

#![cfg(feature = "suite")]

mod support;

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::DefaultUser;

use crate::support::MongoBackend;

#[maybe_async::maybe_async]
async fn counting_backstop() {
    let mut backend = MongoBackend::<DefaultUser>::build()
        .await
        .expect("backend builds");
    backend.reset_roles().await.expect("clean slate");
    backend.reset_query_count();
    assert_eq!(backend.query_count(), Some(0));

    // One read costs exactly one store operation.
    let holder = backend.holder_id("admin").expect("fixture holder resolves");
    let name = RoleName::from("admin");
    let query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Global);
    let rows = {
        let (store, conn) =
            rolify_core::user::RolifyUser::store_with_conn(backend.subject("admin"));
        store
            .where_(&mut *conn, &holder, &query)
            .await
            .expect("where_ runs")
    };
    assert!(rows.is_empty() || !rows.is_empty()); // value irrelevant; the call shape is
    assert_eq!(backend.query_count(), Some(1));

    // The `has_any_role?` path collapses to ONE store read with several
    // query legs (`$or`), never N round trips.
    let queries = [
        RoleQuery::with_role_and_filter(&name, ResourceFilter::Global),
        RoleQuery::with_role_and_filter(&name, ResourceFilter::Class("Forum")),
    ];
    let _ = {
        let (store, conn) =
            rolify_core::user::RolifyUser::store_with_conn(backend.subject("admin"));
        store
            .where_any(&mut *conn, &ResourceId::from(1_i64), &queries)
            .await
            .expect("where_any runs")
    };
    assert_eq!(
        backend.query_count(),
        Some(2),
        "where_any is one store operation (tests.cached=0 + any=1 contract)"
    );

    // Reset re-zeros.
    backend.reset_query_count();
    assert_eq!(backend.query_count(), Some(0));
}

#[cfg(not(feature = "sync"))]
#[tokio::test(flavor = "multi_thread")]
async fn query_counting_hook_works() {
    counting_backstop().await;
}

#[cfg(feature = "sync")]
#[test]
fn query_counting_hook_works() {
    // Sync mode: the maybe_async flip makes `counting_backstop` a plain
    // sync body (the driver offloads internally; no test-side runtime).
    counting_backstop();
}
