//! `MySQL` acceptance pin for the `where_any` holder gate (05 code
//! review CR-01): SQL operator precedence is engine-agnostic, so the
//! second-ladder leak is pinned on this engine too (the sibling
//! Postgres file carries the full CR-01..CR-05 set; `MySQL` coerces the
//! holder-id comparison, so only the precedence pin belongs here).
//!
//! Run: `cargo test -p rolify-seaorm --features mysql,suite --test resource_spi_mysql`

#![cfg(all(feature = "mysql", feature = "suite"))]

mod support;

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::RoleStore;
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::DefaultUser;

use crate::support::SeaormBackend;

/// CR-01 pin on `MySQL`: a role matching ONLY the second ladder and held
/// by another holder must stay invisible to `where_any`.
#[tokio::test]
async fn where_any_second_ladder_stays_gated_on_the_holder() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    let alpha = RoleName::from("alpha");
    let beta = RoleName::from("beta");
    let forum_first = ResourceId::from(1_i64);
    backend
        .grant_to("admin", &alpha, ResourceRef::Global)
        .await
        .expect("admin: global alpha");
    backend
        .grant_to(
            "zombie",
            &beta,
            ResourceRef::Instance("Forum", &forum_first),
        )
        .await
        .expect("zombie: instance beta on Forum first");

    let queries = [
        RoleQuery {
            name: &alpha,
            filter: ResourceFilter::Global,
        },
        RoleQuery {
            name: &beta,
            filter: ResourceFilter::Instance("Forum", &forum_first),
        },
    ];
    let admin_id = backend.holder_id("admin").expect("admin fixture");
    let (store, conn) = backend.engine().store_with_conn();
    let rows = store
        .where_any(conn, &admin_id, &queries)
        .await
        .expect("where_any two ladders");
    assert_eq!(
        rows.len(),
        1,
        "the second ladder must not leak zombie's row: {rows:?}"
    );
    assert!(
        rows.contains(&RoleRecord::global("alpha")),
        "admin's alpha row expected: {rows:?}"
    );
}
