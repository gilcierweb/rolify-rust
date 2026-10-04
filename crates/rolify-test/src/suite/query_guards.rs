//! Query-count guard cases for TEST-05.
//!
//! These cases exercise the `TestBackend` query-counting hook
//! (`reset_query_count` / `query_count`) to assert that cached
//! predicates execute with zero I/O and the uncached
//! `has_any_roles` persistence path executes exactly one query.
//!
//! The cases early-return `Ok(())` when `backend.query_count()` returns
//! `None`, making them non-breaking for backends without counting
//! support (e.g., `InMemoryStore`).

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{RoleName, RoleRecord, RoleSet};
use rolify_core::store::RoleStore;
use rolify_core::user::RolifyUser;

use crate::backend::{TestBackend, load_scope_context};
use crate::fixtures::{FixtureResource, ScopeContext};

/// Guard case A: cached predicates on a populated `RoleSet` snapshot
/// execute with zero queries.
///
/// Builds a populated `RoleSet` (user with roles loaded), resets the
/// counter, then calls every cached predicate variant and asserts
/// `query_count() == Some(0)` when supported.
///
/// # Errors
///
/// Propagates backend failures from the build, context load,
/// provisioning, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn cached_predicates_zero_queries<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;

    // Add extra roles to have a rich snapshot (global + class + instance)
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let subject = backend.subject("god");
    subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    subject
        .add_role(&RoleName::from("manager"), ResourceRef::Class("Forum"))
        .await?;
    subject
        .add_role(
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &forum_last.resource_id),
        )
        .await?;

    // Snapshot the roles for the cached path (via store.roles_of)
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows: Vec<RoleRecord> = store.roles_of(&mut *conn, &holder).await?;
    let role_set = RoleSet::new(&rows);

    // Reset query counter before cached assertions
    backend.reset_query_count();

    let admin = RoleName::from("admin");
    let manager = RoleName::from("manager");
    let moderator = RoleName::from("moderator");
    let ghost = RoleName::from("ghost");
    let forum_id = &forum_last.resource_id;

    // has_cached_role (non-strict)
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global);
    let _ = role_set.has_cached_role(&query);
    let query = RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum"));
    let _ = role_set.has_cached_role(&query);
    let query =
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Instance("Forum", forum_id));
    let _ = role_set.has_cached_role(&query);
    // Global override visible through cache
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"));
    let _ = role_set.has_cached_role(&query);
    let query =
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Instance("Forum", forum_id));
    let _ = role_set.has_cached_role(&query);
    // Reverse never holds
    let query = RoleQuery::with_role_and_filter(&manager, ResourceFilter::Global);
    let _ = role_set.has_cached_role(&query);
    let query = RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum"));
    let _ = role_set.has_cached_role(&query);
    // Any includes global (D-2)
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Any);
    let _ = role_set.has_cached_role(&query);
    // Unknown
    let query = RoleQuery::with_role_and_filter(&ghost, ResourceFilter::Global);
    let _ = role_set.has_cached_role(&query);

    // has_strict_cached_role
    let query = RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum"));
    let _ = role_set.has_strict_cached_role(&query);
    let query =
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Instance("Forum", forum_id));
    let _ = role_set.has_strict_cached_role(&query);
    // No overrides under strict
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"));
    let _ = role_set.has_strict_cached_role(&query);
    let query =
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Instance("Forum", forum_id));
    let _ = role_set.has_strict_cached_role(&query);

    // has_all_cached
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
    ];
    let _ = role_set.has_all_cached(&queries);
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
        RoleQuery::with_role_and_filter(&ghost, ResourceFilter::Global),
    ];
    let _ = role_set.has_all_cached(&queries);
    let _ = role_set.has_all_cached(&[]);

    // has_any_cached
    let queries = [
        RoleQuery::with_role_and_filter(&ghost, ResourceFilter::Global),
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
    ];
    let _ = role_set.has_any_cached(&queries);
    let queries = [RoleQuery::with_role_and_filter(
        &ghost,
        ResourceFilter::Global,
    )];
    let _ = role_set.has_any_cached(&queries);
    let _ = role_set.has_any_cached(&[]);

    // only_has_cached
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global);
    let _ = role_set.only_has_cached(&query);
    let solo_role = RoleRecord::global("admin");
    let solo_roles = vec![solo_role];
    let solo_set = RoleSet::new(&solo_roles);
    let _ = solo_set.only_has_cached(&query);

    // Assert zero queries if counting is supported
    if let Some(count) = backend.query_count() {
        assert_eq!(
            count, 0,
            "cached predicates must execute with zero queries (got {count})"
        );
    }

    Ok(())
}

/// Guard case B: uncached `has_any_roles` persistence path executes
/// exactly one query (the OR-folded `where_any`).
///
/// Resets the counter, performs the uncached `has_any_roles` on a user
/// with 2+ typed `RoleQuery` entries, asserts `query_count() == Some(1)`
/// (ONE round trip via `where_any` — never N sequential checks).
///
/// # Errors
///
/// Propagates backend failures from the build, context load,
/// provisioning, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn uncached_has_any_roles_one_query<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;

    let forum_last = backend.resource(FixtureResource::ForumLast);
    // Add roles so we have something to query (before getting subject to avoid borrow issues)
    {
        let subject = backend.subject("god");
        subject
            .add_role(&RoleName::from("admin"), ResourceRef::Global)
            .await?;
        subject
            .add_role(&RoleName::from("manager"), ResourceRef::Class("Forum"))
            .await?;
        subject
            .add_role(
                &RoleName::from("moderator"),
                ResourceRef::Instance("Forum", &forum_last.resource_id),
            )
            .await?;
    }

    // Reset query counter before the uncached path
    backend.reset_query_count();

    // Uncached has_any_roles with 2+ queries — should be ONE round trip via where_any
    let ghost = RoleName::from("ghost");
    let admin = RoleName::from("admin");
    let queries = [
        RoleQuery::with_role_and_filter(&ghost, ResourceFilter::Global),
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
    ];
    let subject = backend.subject("god");
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer, "has_any_roles should find the global admin");

    // Assert exactly one query if counting is supported
    if let Some(count) = backend.query_count() {
        assert_eq!(
            count, 1,
            "uncached has_any_roles must execute exactly one query via where_any (got {count})"
        );
    }

    Ok(())
}

/// Expand the 2 query-guard wrappers for one backend.
#[macro_export]
macro_rules! parity_query_guards_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn query_guards_cached_predicates_zero_queries() {
            $crate::suite::query_guards::cached_predicates_zero_queries::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn query_guards_uncached_has_any_roles_one_query() {
            $crate::suite::query_guards::uncached_has_any_roles_one_query::<$backend>()
                .await
                .unwrap();
        }
    };
}
