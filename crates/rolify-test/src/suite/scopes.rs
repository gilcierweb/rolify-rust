//! Snapshot scope narrows: port of `shared_examples_for_scopes.rb`
//! (l.10-37, SCOP-01/D-17).
//!
//! Each case self-provisions (`reset_roles` then explicit `add_role` calls,
//! mirroring the spec's `let!` blocks) and builds its snapshot through the
//! public path (`roles_of` into `RoleSet`). Set membership compares
//! order-insensitively (D-04). The narrows take no store handle by
//! signature: zero I/O is proven by construction, stated here and
//! exercised in `narrows_issue_no_store_calls` (one snapshot, many
//! queries).

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{RoleName, RoleSet};
use rolify_core::store::RoleStore;
use rolify_core::user::RolifyUser;

use crate::backend::TestBackend;
use crate::fixtures::FixtureResource;

/// `global()` returns exactly the two global rows
/// (`shared_examples_for_scopes.rb:10-15`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_scope_returns_global_roles<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let subject = backend.subject("admin");
    let admin_row = subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    let staff_row = subject
        .add_role(&RoleName::from("staff"), ResourceRef::Global)
        .await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let global = snapshot.global();
    assert_eq!(global.len(), 2);
    assert!(global.iter().any(|row| **row == admin_row));
    assert!(global.iter().any(|row| **row == staff_row));
    Ok(())
}

/// `class_scoped()` filters by type
/// (`shared_examples_for_scopes.rb:17-24`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_scoped_returns_and_filters<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let subject = backend.subject("admin");
    let manager_row = subject
        .add_role(&RoleName::from("manager"), ResourceRef::Class("Group"))
        .await?;
    let moderator_row = subject
        .add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum"))
        .await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let all_classes = snapshot.class_scoped(None);
    assert_eq!(all_classes.len(), 2);
    let group_only = snapshot.class_scoped(Some("Group"));
    assert_eq!(group_only.len(), 1);
    assert!(group_only.iter().any(|row| **row == manager_row));
    let forum_only = snapshot.class_scoped(Some("Forum"));
    assert_eq!(forum_only.len(), 1);
    assert!(forum_only.iter().any(|row| **row == moderator_row));
    Ok(())
}

/// `instance_scoped()` filters by type and id, including the empty-result
/// row (`shared_examples_for_scopes.rb:26-37`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_scoped_returns_and_filters<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_last = backend.resource(FixtureResource::GroupLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let subject = backend.subject("admin");
    let first_visitor_row = subject
        .add_role(
            &RoleName::from("visitor"),
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    let last_visitor_row = subject
        .add_role(
            &RoleName::from("visitor"),
            ResourceRef::Instance("Forum", &forum_last.resource_id),
        )
        .await?;
    let anonymous_row = subject
        .add_role(
            &RoleName::from("anonymous"),
            ResourceRef::Instance("Group", &group_last.resource_id),
        )
        .await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    assert_eq!(snapshot.instance_scoped(None, None).len(), 3);
    let forum_rows = snapshot.instance_scoped(Some("Forum"), None);
    assert_eq!(forum_rows.len(), 2);
    assert!(forum_rows.iter().any(|row| **row == first_visitor_row));
    assert!(forum_rows.iter().any(|row| **row == last_visitor_row));
    let first_only = snapshot.instance_scoped(Some("Forum"), Some(&forum_first.resource_id));
    assert_eq!(first_only.len(), 1);
    assert!(first_only.iter().any(|row| **row == first_visitor_row));
    let last_only = snapshot.instance_scoped(Some("Forum"), Some(&forum_last.resource_id));
    assert_eq!(last_only.len(), 1);
    assert!(last_only.iter().any(|row| **row == last_visitor_row));
    let anonymous_only = snapshot.instance_scoped(Some("Group"), Some(&group_last.resource_id));
    assert_eq!(anonymous_only.len(), 1);
    assert!(anonymous_only.iter().any(|row| **row == anonymous_row));
    assert!(
        snapshot
            .instance_scoped(Some("Group"), Some(&group_first.resource_id))
            .is_empty(),
        "the l.36 empty-result row"
    );
    Ok(())
}

/// Signature-level zero-I/O pin (SC-5): ONE snapshot answers every narrow
/// and every cached predicate — multiple queries, one construction, no
/// store handle anywhere in the narrow/predicate signatures.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn narrows_issue_no_store_calls<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    subject
        .add_role(&RoleName::from("manager"), ResourceRef::Class("Forum"))
        .await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);

    assert_eq!(snapshot.global().len(), 1);
    assert_eq!(snapshot.class_scoped(Some("Forum")).len(), 1);
    assert_eq!(snapshot.instance_scoped(None, None).len(), 0);

    let admin = RoleName::from("admin");
    let manager = RoleName::from("manager");
    let ghost = RoleName::from("ghost");
    let both = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
    ];
    assert!(snapshot.has_all_cached(&both));
    let any_hit = [RoleQuery::with_role(&ghost), RoleQuery::with_role(&admin)];
    assert!(snapshot.has_any_cached(&any_hit));
    assert!(!snapshot.only_has_cached(&RoleQuery::with_role(&admin)));
    Ok(())
}

/// Expand the 4 `scopes` wrappers for one backend. Wrapper names carry the
/// `scopes_` prefix so the 12 binding modules never collide.
#[macro_export]
macro_rules! parity_scopes_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn scopes_global_scope_returns_global_roles() {
            $crate::suite::scopes::global_scope_returns_global_roles::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn scopes_class_scoped_returns_and_filters() {
            $crate::suite::scopes::class_scoped_returns_and_filters::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn scopes_instance_scoped_returns_and_filters() {
            $crate::suite::scopes::instance_scoped_returns_and_filters::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn scopes_narrows_issue_no_store_calls() {
            $crate::suite::scopes::narrows_issue_no_store_calls::<$backend>()
                .await
                .unwrap();
        }
    };
}
