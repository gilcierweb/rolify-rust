//! `MySQL` tracer: the same grant/check/revoke lifecycle plus one
//! resource-side read on a real `mysql:8.4` container (`utf8mb4_bin`
//! collation branch of the native Migrator).
//!
//! Run: `cargo test -p rolify-seaorm --features mysql --test tracer_mysql`

#![cfg(feature = "mysql")]

mod support;

use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::ResourceStore;
use rolify_core::user::RolifyUser;
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::{DefaultUser, FixtureResource};

use crate::support::SeaormBackend;

#[tokio::test]
#[allow(clippy::too_many_lines)] // single end-to-end lifecycle leg mirrors the diesel tracer shape
async fn tracer_grant_check_revoke_lifecycle_on_mysql() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    backend
        .subject("admin")
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await
        .expect("global grant");
    assert!(
        backend
            .subject("admin")
            .has_role(&RoleName::from("admin"), ResourceFilter::Global)
            .await
            .expect("global check")
    );

    // Byte-exact names: `Admin` does not match `admin` under utf8mb4_bin.
    assert!(
        !backend
            .subject("admin")
            .has_role(&RoleName::from("Admin"), ResourceFilter::Global)
            .await
            .expect("byte-exact miss check")
    );

    let forum_one = ResourceId::from(1_i64);
    backend
        .subject("moderator")
        .add_role(
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &forum_one),
        )
        .await
        .expect("instance grant on forum 1");
    assert!(
        backend
            .subject("moderator")
            .has_role(
                &RoleName::from("moderator"),
                ResourceFilter::Instance("Forum", &forum_one),
            )
            .await
            .expect("instance check")
    );

    // Resource-side read: the Forum class row covers instance 1 and every
    // persisted forum id via the class expansion; instance row resolves
    // directly.
    let resources = {
        let (store, conn) = backend.engine().store_with_conn();
        store
            .resources_find(conn, &["Forum"], &RoleName::from("moderator"))
            .await
            .expect("resources_find")
    };
    assert!(
        resources
            .iter()
            .any(|key| key.resource_type == "Forum" && key.resource_id == forum_one),
        "resources_find resolves forum 1, got {resources:?}"
    );

    // Revoke; the default config sweeps the orphaned role row.
    backend
        .subject("moderator")
        .remove_role(
            &RoleName::from("moderator"),
            rolify_core::kernel::RemovalTarget::NameOnly,
        )
        .await
        .expect("revoke");
    assert!(
        !backend
            .subject("moderator")
            .has_role(
                &RoleName::from("moderator"),
                ResourceFilter::Instance("Forum", &forum_one),
            )
            .await
            .expect("revoked")
    );
    let _ = FixtureResource::ForumFirst; // fixture seam referenced for completeness
}
