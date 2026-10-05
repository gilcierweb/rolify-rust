//! Binding-local query-count backstop (TEST-05): the explicit store
//! counter starts at zero, increments per round trip, reset works, the
//! `RoleSet` cached predicate path stays at zero, and `where_any` is
//! exactly one query. Mirrors the diesel `query_counting.rs` backstop.
//!
//! Run: `cargo test -p rolify-seaorm --features postgres,suite --test query_counting`

#![cfg(all(feature = "postgres", feature = "suite"))]

mod support;

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::RoleSet;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_core::user::RolifyUser;
use rolify_test::backend::TestBackend;

use crate::support::SeaormBackend;

#[tokio::test]
async fn counter_backstop_starts_increments_resets() {
    let mut backend = SeaormBackend::<rolify_test::fixtures::DefaultUser>::build()
        .await
        .expect("backend build");
    backend.reset_query_count();
    assert_eq!(backend.query_count(), Some(0), "counter starts at zero");

    let holder = backend.holder_id("admin").expect("admin fixture");
    let (store, conn) = backend.engine().store_with_conn();
    store
        .where_(
            conn,
            &holder,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            },
        )
        .await
        .expect("where_ query");
    assert_eq!(
        backend.query_count(),
        Some(1),
        "where_ increments the counter exactly once"
    );

    backend.reset_query_count();
    assert_eq!(backend.query_count(), Some(0), "reset works");
}

#[tokio::test]
async fn cached_predicates_issue_zero_queries() {
    let mut backend = SeaormBackend::<rolify_test::fixtures::DefaultUser>::build()
        .await
        .expect("backend build");
    let holder = backend.holder_id("admin").expect("admin fixture");

    // Warm the snapshot: grant, then read the holder's rows once.
    backend
        .subject("admin")
        .add_role(
            &RoleName::from("admin"),
            rolify_core::resource::ResourceRef::Global,
        )
        .await
        .expect("grant");
    let (store, conn) = backend.engine().store_with_conn();
    let rows = store.roles_of(conn, &holder).await.expect("roles_of load");
    let role_set = RoleSet::new(&rows);

    backend.reset_query_count();

    let admin = RoleName::from("admin");
    let results = [
        role_set.has_cached_role(&RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Global,
        )),
        role_set.has_cached_role(&RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Class("Forum"),
        )),
    ];
    assert_eq!(results, [true, true], "cached reads see the snapshot");
    assert_eq!(
        backend.query_count(),
        Some(0),
        "cached predicates issue zero queries"
    );
}

#[tokio::test]
async fn where_any_is_exactly_one_round_trip() {
    let mut backend = SeaormBackend::<rolify_test::fixtures::DefaultUser>::build()
        .await
        .expect("backend build");
    backend.reset_query_count();
    let holder = backend.holder_id("admin").expect("admin fixture");
    let id = ResourceId::from(7_i64);
    let name = RoleName::from("admin");
    let (store, conn) = backend.engine().store_with_conn();
    store
        .where_any(
            conn,
            &holder,
            &[
                RoleQuery {
                    name: &name,
                    filter: ResourceFilter::Global,
                },
                RoleQuery {
                    name: &name,
                    filter: ResourceFilter::Instance("Forum", &id),
                },
            ],
        )
        .await
        .expect("where_any query");
    assert_eq!(
        backend.query_count(),
        Some(1),
        "where_any with two queries is exactly one round trip"
    );
}
