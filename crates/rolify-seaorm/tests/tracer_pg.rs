//! Postgres tracer: end-to-end grant/check/revoke through the public
//! `RolifyUser` methods on a real `postgres:17` container (Migrator up,
//! global + class + instance grants, ladder reads incl. the global
//! override, strict-off semantics, revoke with orphan sweep).
//!
//! Run: `cargo test -p rolify-seaorm --features postgres --test tracer_pg`

#![cfg(feature = "postgres")]

mod support;

use rolify_core::kernel::RemovalTarget;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::DefaultUser;

use crate::support::SeaormBackend;

#[tokio::test]
#[allow(clippy::too_many_lines)] // single end-to-end lifecycle leg mirrors the diesel tracer shape
async fn tracer_grant_check_revoke_lifecycle_on_postgres() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    // Global grant + check (public API surface).
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

    // The global override: global satisfies class/instance reads.
    assert!(
        backend
            .subject("admin")
            .has_role(
                &RoleName::from("admin"),
                ResourceFilter::Instance("Forum", &ResourceId::from(1_i64)),
            )
            .await
            .expect("global override covers instance reads")
    );

    // Idempotent re-grant produces no extra row.
    backend
        .subject("admin")
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await
        .expect("re-grant");
    assert_eq!(
        backend.role_row_count().await.expect("count"),
        1,
        "unique triple keeps one row"
    );

    // Class-scoped grant and reads.
    backend
        .subject("moderator")
        .add_role(&RoleName::from("manager"), ResourceRef::Class("Forum"))
        .await
        .expect("class grant");
    assert!(
        backend
            .subject("moderator")
            .has_role(&RoleName::from("manager"), ResourceFilter::Class("Forum"))
            .await
            .expect("class check")
    );

    // Instance-scoped grant on a different holder stays isolated.
    backend
        .subject("god")
        .add_role(
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &rolify_core::role::ResourceId::from(1_i64)),
        )
        .await
        .expect("instance grant");
    assert!(
        !backend
            .subject("moderator")
            .has_role(
                &RoleName::from("moderator"),
                ResourceFilter::Instance("Forum", &rolify_core::role::ResourceId::from(1_i64)),
            )
            .await
            .expect("isolation across holders")
    );

    // Revoke the global role: link removed and the role row swept
    // (remove_role_if_empty is the gem default in DefaultUser config).
    backend
        .subject("admin")
        .remove_role(&RoleName::from("admin"), RemovalTarget::NameOnly)
        .await
        .expect("revoke");
    assert!(
        !backend
            .subject("admin")
            .has_role(&RoleName::from("admin"), ResourceFilter::Global)
            .await
            .expect("revoked")
    );
    assert_eq!(
        backend.role_row_count().await.expect("count"),
        2,
        "admin row swept; manager plus moderator remain"
    );
}
