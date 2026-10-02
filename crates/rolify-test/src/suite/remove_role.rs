//! `remove_role` / `revoke` cases: full port of
//! `shared_examples_for_remove_role.rb` (l.3-88).
//!
//! ## Observation port (from the 02-01 `add_role` precedent)
//!
//! - The gem's `subject.roles.size` ports to the public surface as
//!   `subject.roles_name().len()` (`role.rb:88-90`: one name per linked
//!   role, identical count).
//! - The gem's `role_class.count` ports as `backend.role_row_count()`.
//!
//! ## Keep-rows variant
//!
//! The `remove_role_if_empty(false)` case is config-driven (CONF-02): it
//! runs against the concrete `InMemoryBackend<KeepRoleRowsUserClass>`,
//! identical for every backend, so its wrapper ignores `$backend` with a
//! doc note. D-19 entry for the 02-09 harvest.

use rolify_core::config::RolifyConfig;
use rolify_core::error::RolifyError;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::RoleName;
use rolify_core::user::RolifyUser;

use crate::backend::{InMemoryBackend, TestBackend, load_scope_context};
use crate::fixtures::{FixtureResource, ScopeContext, UserClass};

/// Removing the global "admin" by name drops exactly one link, and the
/// global ask answers false afterwards
/// (`shared_examples_for_remove_role.rb:4-8`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn remove_global_role_drops_link<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let subject = backend.subject("admin");
    let admin = RoleName::from("admin");
    let before = subject.roles_name().await?.len();
    subject.remove_role(&admin, RemovalTarget::NameOnly).await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after + 1);
    let via_query = subject.has_role(&admin, ResourceFilter::Global).await?;
    assert!(!via_query);
    Ok(())
}

/// Removing "manager" by name drops its single Class("Group") link
/// (`shared_examples_for_remove_role.rb:10-14`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn remove_class_role_by_name_drops_link<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let subject = backend.subject("admin");
    let name = RoleName::from("manager");
    let before = subject.roles_name().await?.len();
    subject.remove_role(&name, RemovalTarget::NameOnly).await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after + 1);
    let via_query = subject
        .has_role(&name, ResourceFilter::Class("Group"))
        .await?;
    assert!(!via_query);
    Ok(())
}

/// THE -2 CASE (`shared_examples_for_remove_role.rb:16-21`): a name-only
/// remove sweeps every scope, dropping BOTH instance-scoped "moderator"
/// links (Forum.last AND Group.last).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn name_only_remove_sweeps_all_scopes_minus_two<B: TestBackend>() -> Result<(), B::Error>
{
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_last = backend.resource(FixtureResource::GroupLast);
    let subject = backend.subject("admin");
    let name = RoleName::from("moderator");
    let before = subject.roles_name().await?.len();
    subject.remove_role(&name, RemovalTarget::NameOnly).await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after + 2);
    let forum_filter = ResourceFilter::Instance("Forum", &forum_last.resource_id);
    let via_forum = subject.has_role(&name, forum_filter).await?;
    assert!(!via_forum);
    let group_filter = ResourceFilter::Instance("Group", &group_last.resource_id);
    let via_group = subject.has_role(&name, group_filter).await?;
    assert!(!via_group);
    Ok(())
}

/// Removing the never-held "superhero" changes nothing
/// (`shared_examples_for_remove_role.rb:23-25`; the row exists only in
/// `other_role_rows()`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn remove_never_held_role_is_noop<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let subject = backend.subject("admin");
    let name = RoleName::from("superhero");
    let before = subject.roles_name().await?.len();
    subject.remove_role(&name, RemovalTarget::NameOnly).await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// A row held by another user survives the subject's remove
/// (`shared_examples_for_remove_role.rb:27-36`): "zombie" holds "staff", so
/// the subject's name-only remove destroys no row.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn shared_row_survives_when_held_by_another_user<B: TestBackend>() -> Result<(), B::Error>
{
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    backend
        .subject("zombie")
        .add_role(&RoleName::from("staff"), ResourceRef::Global)
        .await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("admin");
    let name = RoleName::from("staff");
    subject.remove_role(&name, RemovalTarget::NameOnly).await?;
    let via_query = subject.has_role(&name, ResourceFilter::Global).await?;
    assert!(!via_query);
    let after = backend.role_row_count().await?;
    assert_eq!(before, after);
    Ok(())
}

/// An orphaned row is destroyed under the default `remove_role_if_empty`
/// (`shared_examples_for_remove_role.rb:38-44`, CONF-02).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn orphaned_row_destroyed_when_remove_role_if_empty<B: TestBackend>()
-> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let name = RoleName::from("nobody");
    backend
        .subject("admin")
        .add_role(&name, ResourceRef::Global)
        .await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("admin");
    subject.remove_role(&name, RemovalTarget::NameOnly).await?;
    let after = backend.role_row_count().await?;
    assert_eq!(before, after + 1);
    Ok(())
}

/// A class filter never removes the global "warrior" row
/// (`shared_examples_for_remove_role.rb:48-50`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_filter_never_removes_global<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let subject = backend.subject("moderator");
    let name = RoleName::from("warrior");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(&name, RemovalTarget::TypeSweep("Forum"))
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// A class filter removes the matching class link
/// (`shared_examples_for_remove_role.rb:52-56`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_filter_removes_matching_class_link<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let subject = backend.subject("moderator");
    let name = RoleName::from("manager");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(&name, RemovalTarget::TypeSweep("Forum"))
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after + 1);
    let via_query = subject
        .has_role(&name, ResourceFilter::Class("Forum"))
        .await?;
    assert!(!via_query);
    Ok(())
}

/// A class filter removes only the matching instance link: the `ForumLast`
/// "moderator" link goes, the `GroupLast` one stays
/// (`shared_examples_for_remove_role.rb:58-63`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_filter_removes_only_matching_instance<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let group_last = backend.resource(FixtureResource::GroupLast);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let subject = backend.subject("moderator");
    let name = RoleName::from("moderator");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(&name, RemovalTarget::TypeSweep("Forum"))
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after + 1);
    let forum_filter = ResourceFilter::Instance("Forum", &forum_last.resource_id);
    let via_forum = subject.has_role(&name, forum_filter).await?;
    assert!(!via_forum);
    let group_filter = ResourceFilter::Instance("Group", &group_last.resource_id);
    let via_group = subject.has_role(&name, group_filter).await?;
    assert!(via_group);
    Ok(())
}

/// A class filter on an unheld resource type is a no-op: the subject's
/// "manager" is Class("Forum"), not Class("Group")
/// (`shared_examples_for_remove_role.rb:65-67`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_filter_on_unheld_resource_is_noop<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let subject = backend.subject("moderator");
    let name = RoleName::from("manager");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(&name, RemovalTarget::TypeSweep("Group"))
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// An instance filter never removes the global "soldier" row
/// (`shared_examples_for_remove_role.rb:70-72`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_filter_never_removes_global<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let subject = backend.subject("god");
    let name = RoleName::from("soldier");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(
            &name,
            RemovalTarget::Exact("Group", &group_first.resource_id),
        )
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// An instance filter never removes the class-scoped "visitor" row
/// (`shared_examples_for_remove_role.rb:74-77`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_filter_never_removes_class<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("god");
    let name = RoleName::from("visitor");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(
            &name,
            RemovalTarget::Exact("Forum", &forum_first.resource_id),
        )
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// An instance filter removes the matching instance link
/// (`shared_examples_for_remove_role.rb:79-83`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_filter_removes_matching_instance_link<B: TestBackend>() -> Result<(), B::Error>
{
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("god");
    let name = RoleName::from("moderator");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(
            &name,
            RemovalTarget::Exact("Forum", &forum_first.resource_id),
        )
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after + 1);
    let instance_filter = ResourceFilter::Instance("Forum", &forum_first.resource_id);
    let via_query = subject.has_role(&name, instance_filter).await?;
    assert!(!via_query);
    Ok(())
}

/// An instance filter on another instance is a no-op: "anonymous" lives on
/// `ForumLast`, not `ForumFirst`
/// (`shared_examples_for_remove_role.rb:85-87`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_filter_on_other_instance_is_noop<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("god");
    let name = RoleName::from("anonymous");
    let before = subject.roles_name().await?.len();
    subject
        .remove_role(
            &name,
            RemovalTarget::Exact("Forum", &forum_first.resource_id),
        )
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// Fixture user class with `remove_role_if_empty(false)` (CONF-02 toggle):
/// orphaned rows are kept. The type name is inert here (behavior is
/// config-level).
struct KeepRoleRowsUserClass;

impl UserClass for KeepRoleRowsUserClass {
    fn rolify_type() -> &'static str {
        "User"
    }

    /// # Panics
    ///
    /// Never in practice: the default table names always validate.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .remove_role_if_empty(false)
            .build()
            .expect("the keep-rows configuration always passes validation")
    }
}

/// Under `remove_role_if_empty(false)` the orphaned row is kept while the
/// link is gone (CONF-02 toggle family). Concrete backend: the variant is
/// config-driven and identical for every backend, so the wrapper ignores
/// `$backend`.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn remove_role_if_empty_false_keeps_orphaned_row() -> Result<(), RolifyError> {
    let mut backend: InMemoryBackend<KeepRoleRowsUserClass> = InMemoryBackend::build().await?;
    let name = RoleName::from("nobody");
    backend
        .subject("admin")
        .add_role(&name, ResourceRef::Global)
        .await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("admin");
    subject.remove_role(&name, RemovalTarget::NameOnly).await?;
    let after = backend.role_row_count().await?;
    assert_eq!(before, after);
    let via_query = backend
        .subject("admin")
        .has_role(&name, ResourceFilter::Global)
        .await?;
    assert!(!via_query);
    Ok(())
}

/// Expand the 15 `remove_role` wrappers for one backend. Wrapper names
/// carry the `remove_role_` prefix so the 12 binding modules never collide.
/// The keep-rows wrapper calls its concrete fn and ignores `$backend`: the
/// variant is config-driven and backend-independent.
#[macro_export]
macro_rules! parity_remove_role_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_remove_global_role_drops_link() {
            $crate::suite::remove_role::remove_global_role_drops_link::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_remove_class_role_by_name_drops_link() {
            $crate::suite::remove_role::remove_class_role_by_name_drops_link::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_name_only_remove_sweeps_all_scopes_minus_two() {
            $crate::suite::remove_role::name_only_remove_sweeps_all_scopes_minus_two::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_remove_never_held_role_is_noop() {
            $crate::suite::remove_role::remove_never_held_role_is_noop::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_shared_row_survives_when_held_by_another_user() {
            $crate::suite::remove_role::shared_row_survives_when_held_by_another_user::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_orphaned_row_destroyed_when_remove_role_if_empty() {
            $crate::suite::remove_role::orphaned_row_destroyed_when_remove_role_if_empty::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_class_filter_never_removes_global() {
            $crate::suite::remove_role::class_filter_never_removes_global::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_class_filter_removes_matching_class_link() {
            $crate::suite::remove_role::class_filter_removes_matching_class_link::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_class_filter_removes_only_matching_instance() {
            $crate::suite::remove_role::class_filter_removes_only_matching_instance::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_class_filter_on_unheld_resource_is_noop() {
            $crate::suite::remove_role::class_filter_on_unheld_resource_is_noop::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_instance_filter_never_removes_global() {
            $crate::suite::remove_role::instance_filter_never_removes_global::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_instance_filter_never_removes_class() {
            $crate::suite::remove_role::instance_filter_never_removes_class::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_instance_filter_removes_matching_instance_link() {
            $crate::suite::remove_role::instance_filter_removes_matching_instance_link::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_instance_filter_on_other_instance_is_noop() {
            $crate::suite::remove_role::instance_filter_on_other_instance_is_noop::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn remove_role_remove_role_if_empty_false_keeps_orphaned_row() {
            $crate::suite::remove_role::remove_role_if_empty_false_keeps_orphaned_row()
                .await
                .unwrap();
        }
    };
}
