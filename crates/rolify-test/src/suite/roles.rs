//! The `roles.rb`-specific rows: the response matrix (l.33-53), the new
//! instance rows (l.81-99), and the `roles_name` accessor (role.rb:88-90).
//! The file's other sections are the other modules' ports, already bound by
//! the frozen macro.
//!
//! ## Response matrix (l.33-53): presence map
//!
//! In Rust, method presence is compile-time; each `respond_to` row names
//! the provided member that satisfies it, and using the member in
//! `response_matrix_is_compile_time` IS the pin:
//!
//! | gem row | port member |
//! |---|---|
//! | `grant` (1, 2 args) | `RolifyUser::grant` (role.rb:23 alias) |
//! | `add_role` (1, 2 args) | `RolifyUser::add_role` |
//! | `has_role?` (1, 2 args) | `RolifyUser::has_role` |
//! | `has_all_roles?` | `RolifyUser::has_all_roles` |
//! | `has_any_role?` | `RolifyUser::has_any_roles` |
//! | `revoke` / `remove_role` (1, 2 args) | `RolifyUser::revoke` / `RolifyUser::remove_role` |
//!
//! ## Deliberate absences (D-19 entries)
//!
//! - `is_admin?` / `is_moderator_of?` are gem `should_not respond_to`
//!   rows: dynamic shortcuts, REQUIREMENTS out-of-scope. The methods simply
//!   do not exist on the trait.
//! - `has_no_role` IS a gem `respond_to` row (roles.rb:47, deprecated alias
//!   of `remove_role`) that the port drops per the REQUIREMENTS out-of-scope
//!   alias table (1 concept = 1 name). No negative-compile test is added:
//!   drift-proof by construction.

use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::RoleName;
use rolify_core::user::RolifyUser;

use crate::backend::TestBackend;
use crate::fixtures::FixtureResource;

/// Every response-matrix presence row executes at least once through a
/// fresh holder, restricted to strict-safe asks (name-only/global filters:
/// `has_role` with a `Global` filter never engages the strict gate, and
/// `has_any_roles` is non-strict by design), because the frozen macro
/// expands this module under the strict binding too.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn response_matrix_is_compile_time<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let subject = backend.subject("admin");
    let admin = RoleName::from("admin");
    let staff = RoleName::from("staff");
    subject.add_role(&admin, ResourceRef::Global).await?;
    subject.grant(&staff, ResourceRef::Global).await?;
    let via_role = subject.has_role(&admin, ResourceFilter::Global).await?;
    assert!(via_role);
    let all_queries = [RoleQuery::with_role(&admin)];
    let all_held = subject.has_all_roles(&all_queries).await?;
    assert!(all_held);
    let any_queries = [RoleQuery::with_role(&admin)];
    let any_held = subject.has_any_roles(&any_queries).await?;
    assert!(any_held);
    subject.revoke(&staff, RemovalTarget::NameOnly).await?;
    subject.remove_role(&admin, RemovalTarget::NameOnly).await?;
    Ok(())
}

/// A fresh holder ("zombie") adds admin global plus moderator at the first
/// forum, then answers the name-only asks
/// (`shared_examples_for_roles.rb:81-99`; the l.92-97 commented-out rows
/// are gem-side exclusions, not port obligations).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn new_instance_role_access<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("zombie");
    let admin = RoleName::from("admin");
    let moderator = RoleName::from("moderator");
    subject.add_role(&admin, ResourceRef::Global).await?;
    subject
        .add_role(
            &moderator,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    let via_role = subject.has_role(&admin, ResourceFilter::Global).await?;
    assert!(via_role);
    let any_queries = [RoleQuery::with_role(&admin)];
    let any_held = subject.has_any_roles(&any_queries).await?;
    assert!(any_held);
    Ok(())
}

/// `roles_name` lists one name per linked role (role.rb:88-90, USER-07).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn roles_name_lists_linked_role_names<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("zombie");
    subject
        .add_role(&RoleName::from("admin"), ResourceRef::Global)
        .await?;
    subject
        .add_role(
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    let names = subject.roles_name().await?;
    assert_eq!(names.len(), 2);
    let mut sorted: Vec<&str> = names.iter().map(RoleName::as_str).collect();
    sorted.sort_unstable();
    assert_eq!(sorted, ["admin", "moderator"]);
    Ok(())
}

/// Expand the 3 `roles` wrappers for one backend. Wrapper names carry the
/// `roles_` prefix so the 12 binding modules never collide.
#[macro_export]
macro_rules! parity_roles_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn roles_response_matrix_is_compile_time() {
            $crate::suite::roles::response_matrix_is_compile_time::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn roles_new_instance_role_access() {
            $crate::suite::roles::new_instance_role_access::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn roles_roles_name_lists_linked_role_names() {
            $crate::suite::roles::roles_name_lists_linked_role_names::<$backend>()
                .await
                .unwrap();
        }
    };
}
