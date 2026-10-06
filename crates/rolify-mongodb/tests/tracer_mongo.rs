//! Tracer (05.1-02): the end-to-end happy path on a real `mongo:8.0`
//! container in async mode - `ensure_indexes`, then grant/check/revoke one
//! role at each scope through PUBLIC `RolifyUser` methods (`add_role` /
//! `has_role` / `remove_role`), mirroring the gem's Mongoid flow
//! (`adapters/mongoid/role_adapter.rb`).
//!
//! Runs: `cargo test -p rolify-mongodb --features suite --test tracer_mongo`

#![cfg(all(feature = "suite", not(feature = "sync")))]

mod support;

use rolify_core::kernel::RemovalTarget;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::DefaultUser;

use crate::support::MongoBackend;

/// One happy path through every scope level, end to end. The line budget
/// is allowed here: a single grant/check/revoke narrative is the point of
/// the tracer.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn grant_check_revoke_lifecycle_on_real_mongo() {
    let mut backend = MongoBackend::<DefaultUser>::build()
        .await
        .expect("backend builds (container, fixtures, ensure_indexes)");
    backend.reset_roles().await.expect("clean slate");

    // Global scope: grant -> check (Global + Any) -> revoke.
    let admin = RoleName::from("admin");
    {
        let subject = backend.subject("admin");
        subject
            .add_role(&admin, ResourceRef::Global)
            .await
            .expect("grant global role");
        assert!(
            subject
                .has_role(&admin, ResourceFilter::Global)
                .await
                .expect("has_role global")
        );
        assert!(
            subject
                .has_role(&admin, ResourceFilter::Any)
                .await
                .expect("has_role any")
        );
    }

    // Class scope: grant -> class covers instance in the non-strict ladder.
    let manager = RoleName::from("manager");
    {
        let subject = backend.subject("admin");
        subject
            .add_role(&manager, ResourceRef::Class("Forum"))
            .await
            .expect("grant class role");
        let forum_one = ResourceId::from(1_i64);
        assert!(
            subject
                .has_role(&manager, ResourceFilter::Class("Forum"))
                .await
                .expect("has_role class")
        );
        assert!(
            subject
                .has_role(&manager, ResourceFilter::Instance("Forum", &forum_one))
                .await
                .expect("class row satisfies instance query (kernel ladder)")
        );
        assert_eq!(
            backend.role_row_count().await.expect("count after grants"),
            2,
            "two distinct role documents so far (unique compound, explicit null scope)"
        );
    }

    // Instance scope: string PK (Team.team_code) round trip.
    let moderator = RoleName::from("moderator");
    let team_one = backend.resource(rolify_test::fixtures::FixtureResource::TeamFirst);
    {
        let subject = backend.subject("admin");
        subject
            .add_role(
                &moderator,
                ResourceRef::Instance("Team", &team_one.resource_id),
            )
            .await
            .expect("grant instance role with string PK");
        assert!(
            subject
                .has_role(
                    &moderator,
                    ResourceFilter::Instance("Team", &team_one.resource_id),
                )
                .await
                .expect("has_role instance")
        );

        // Strict off vs strict on corner: with strict mode the global
        // override must NOT fire (kernel semantics preserved over BSON).
        assert!(
            subject
                .has_role(&moderator, ResourceFilter::Global)
                .await
                .is_ok_and(|present| !present),
            "a scoped role never satisfies the global ladder branch"
        );
    }

    // Revoke the class role: name+class sweep pulls both sides, and the
    // (remove_role_if_empty) destroy fires after the last link vanishes.
    {
        let subject = backend.subject("admin");
        let outcome = subject
            .remove_role(&manager, RemovalTarget::TypeSweep("Forum"))
            .await
            .expect("revoke class role");
        assert_eq!(outcome.removed_links, 1);
        assert_eq!(outcome.removed_roles.len(), 1, "orphan sweep after removal");
        assert!(
            subject
                .has_role(&manager, ResourceFilter::Class("Forum"))
                .await
                .is_ok_and(|present| !present)
        );
    }

    // And the store is consistent: 2 role docs remain (global + instance),
    // candidate re-grant of the same triple stays idempotent.
    {
        let subject = backend.subject("admin");
        subject
            .add_role(&admin, ResourceRef::Global)
            .await
            .expect("re-grant global is idempotent");
        assert_eq!(
            backend.role_row_count().await.expect("final count"),
            2,
            "grant idempotence keeps exactly one (admin, null, null) document"
        );
    }
}
