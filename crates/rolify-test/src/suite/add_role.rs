//! `add_role` cases: full port of
//! `shared_examples_for_add_role.rb` (l.3-83, 15 cases).
//!
//! ## Porting notes (for the 02-09 D-19 parity-matrix harvest)
//!
//! - **String/Symbol collapse:** the gem runs every case twice (`String`
//!   and `Symbol` params via `param_method`); the port passes byte-exact
//!   [`RoleName`] once - the distinction is unrepresentable by construction.
//! - **`roles.count`:** the gem's `subject.roles.count` observation ports to
//!   the public surface as `subject.roles_name().len()` (`role.rb:88-90`:
//!   one name per linked role, identical count).
//! - **`be_the_same_role`:** the matcher (`rolify/lib/rolify/matchers.rb`)
//!   ports to [`RoleRecord`] field plus scope-predicate assertions
//!   (`is_global`, `is_class_scoped_to`, `is_instance_scoped_to`).
//!
//! [`RoleName`]: rolify_core::role::RoleName
//! [`RoleRecord`]: rolify_core::role::RoleRecord

use rolify_core::resource::ResourceRef;
use rolify_core::role::{RoleName, RoleRecord};
use rolify_core::user::RolifyUser;

use crate::backend::{TestBackend, load_scope_context};
use crate::fixtures::{FixtureResource, ScopeContext};

/// Adding global "root" grows the subject's linked-role count by exactly 1
/// (`shared_examples_for_add_role.rb:4-6`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_add_increases_subject_roles<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let subject = backend.subject("admin");
    let before = subject.roles_name().await?.len();
    subject
        .add_role(&RoleName::from("root"), ResourceRef::Global)
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(after, before + 1);
    Ok(())
}

/// Adding global "moderator" creates exactly one new role row
/// (`shared_examples_for_add_role.rb:8-10`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_add_creates_role_row<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("moderator"), ResourceRef::Global)
        .await?;
    let after = backend.role_row_count().await?;
    assert_eq!(after, before + 1);
    Ok(())
}

/// Adding "expert" with no resource returns the global role
/// (`shared_examples_for_add_role.rb:12-16`). Note the gem's l.13 title says
/// "class scoped" while the call passes no resource: the port asserts the
/// call, not the title.
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_add_returns_the_global_role<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let subject = backend.subject("admin");
    let record = subject
        .add_role(&RoleName::from("expert"), ResourceRef::Global)
        .await?;
    assert_eq!(record.name, RoleName::from("expert"));
    assert!(record.is_global());
    Ok(())
}

/// Adding global "manager" twice leaves the link count unchanged on the
/// second call (`shared_examples_for_add_role.rb:18-22`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_readd_does_not_duplicate_link<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("manager"), ResourceRef::Global)
        .await?;
    let before = subject.roles_name().await?.len();
    subject
        .add_role(&RoleName::from("manager"), ResourceRef::Global)
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// With a pre-created unlinked global "god" row, adding "god" leaves the
/// role-row count unchanged (`shared_examples_for_add_role.rb:24-27`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_add_reuses_existing_role_row<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    backend.create_role_row(RoleRecord::global("god")).await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("admin");
    subject
        .add_role(&RoleName::from("god"), ResourceRef::Global)
        .await?;
    let after = backend.role_row_count().await?;
    assert_eq!(before, after);
    Ok(())
}

/// Adding "supervisor" at `Class("Forum")` grows the subject's linked-role
/// count by exactly 1 (`shared_examples_for_add_role.rb:31-34`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_add_increases_subject_roles<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let subject = backend.subject("moderator");
    let before = subject.roles_name().await?.len();
    subject
        .add_role(&RoleName::from("supervisor"), ResourceRef::Class("Forum"))
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(after, before + 1);
    Ok(())
}

/// Adding "moderator" at `Class("Forum")` creates exactly one new role row
/// (`shared_examples_for_add_role.rb:36-38`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_add_creates_role_row<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("moderator");
    subject
        .add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum"))
        .await?;
    let after = backend.role_row_count().await?;
    assert_eq!(after, before + 1);
    Ok(())
}

/// Adding "boss" at `Class("Forum")` returns the class-scoped role
/// (`shared_examples_for_add_role.rb:40-44`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_add_returns_the_class_scoped_role<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let subject = backend.subject("moderator");
    let record = subject
        .add_role(&RoleName::from("boss"), ResourceRef::Class("Forum"))
        .await?;
    assert_eq!(record.name, RoleName::from("boss"));
    assert!(record.is_class_scoped_to("Forum"));
    Ok(())
}

/// Adding "warrior" at `Class("Forum")` twice leaves the link count
/// unchanged on the second call (`shared_examples_for_add_role.rb:46-50`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_readd_does_not_duplicate_link<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let subject = backend.subject("moderator");
    subject
        .add_role(&RoleName::from("warrior"), ResourceRef::Class("Forum"))
        .await?;
    let before = subject.roles_name().await?.len();
    subject
        .add_role(&RoleName::from("warrior"), ResourceRef::Class("Forum"))
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// With a pre-created unlinked "hacker" `Class("Forum")` row, adding it
/// leaves the role-row count unchanged
/// (`shared_examples_for_add_role.rb:52-55`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_add_reuses_existing_role_row<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    backend
        .create_role_row(RoleRecord::for_class("hacker", "Forum"))
        .await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("moderator");
    subject
        .add_role(&RoleName::from("hacker"), ResourceRef::Class("Forum"))
        .await?;
    let after = backend.role_row_count().await?;
    assert_eq!(before, after);
    Ok(())
}

/// Adding "visitor" at the last forum grows the subject's linked-role count
/// by exactly 1 (`shared_examples_for_add_role.rb:59-61`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_add_increases_subject_roles<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let subject = backend.subject("god");
    let before = subject.roles_name().await?.len();
    subject
        .add_role(
            &RoleName::from("visitor"),
            ResourceRef::Instance("Forum", &forum_last.resource_id),
        )
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(after, before + 1);
    Ok(())
}

/// Adding "member" at the last forum creates exactly one new role row
/// (`shared_examples_for_add_role.rb:63-66`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_add_creates_role_row<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let before = backend.role_row_count().await?;
    let subject = backend.subject("god");
    subject
        .add_role(
            &RoleName::from("member"),
            ResourceRef::Instance("Forum", &forum_last.resource_id),
        )
        .await?;
    let after = backend.role_row_count().await?;
    assert_eq!(after, before + 1);
    Ok(())
}

/// Adding "mate" at the last forum returns the instance-scoped role
/// (`shared_examples_for_add_role.rb:68-70`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_add_returns_the_instance_scoped_role<B: TestBackend>() -> Result<(), B::Error>
{
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let subject = backend.subject("god");
    let record = subject
        .add_role(
            &RoleName::from("mate"),
            ResourceRef::Instance("Forum", &forum_last.resource_id),
        )
        .await?;
    assert_eq!(record.name, RoleName::from("mate"));
    assert!(record.is_instance_scoped_to("Forum", &forum_last.resource_id));
    Ok(())
}

/// Adding "anonymous" at the first forum twice leaves the link count
/// unchanged on the second call (`shared_examples_for_add_role.rb:72-76`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_readd_does_not_duplicate_link<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("god");
    subject
        .add_role(
            &RoleName::from("anonymous"),
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    let before = subject.roles_name().await?.len();
    subject
        .add_role(
            &RoleName::from("anonymous"),
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    let after = subject.roles_name().await?.len();
    assert_eq!(before, after);
    Ok(())
}

/// With a pre-created unlinked "ghost" row at the first forum, adding it
/// leaves the role-row count unchanged
/// (`shared_examples_for_add_role.rb:78-81`).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, grants, or
/// reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_add_reuses_existing_role_row<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    backend
        .create_role_row(RoleRecord::for_instance(
            "ghost",
            "Forum",
            forum_first.resource_id.clone(),
        ))
        .await?;
    let before = backend.role_row_count().await?;
    let subject = backend.subject("god");
    subject
        .add_role(
            &RoleName::from("ghost"),
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    let after = backend.role_row_count().await?;
    assert_eq!(before, after);
    Ok(())
}

/// Expand the 15 `add_role` wrappers for one backend. Wrapper names carry
/// the `add_role_` prefix so the 12 binding modules never collide.
#[macro_export]
macro_rules! parity_add_role_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_global_add_increases_subject_roles() {
            $crate::suite::add_role::global_add_increases_subject_roles::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_global_add_creates_role_row() {
            $crate::suite::add_role::global_add_creates_role_row::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_global_add_returns_the_global_role() {
            $crate::suite::add_role::global_add_returns_the_global_role::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_global_readd_does_not_duplicate_link() {
            $crate::suite::add_role::global_readd_does_not_duplicate_link::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_global_add_reuses_existing_role_row() {
            $crate::suite::add_role::global_add_reuses_existing_role_row::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_class_add_increases_subject_roles() {
            $crate::suite::add_role::class_add_increases_subject_roles::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_class_add_creates_role_row() {
            $crate::suite::add_role::class_add_creates_role_row::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_class_add_returns_the_class_scoped_role() {
            $crate::suite::add_role::class_add_returns_the_class_scoped_role::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_class_readd_does_not_duplicate_link() {
            $crate::suite::add_role::class_readd_does_not_duplicate_link::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_class_add_reuses_existing_role_row() {
            $crate::suite::add_role::class_add_reuses_existing_role_row::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_instance_add_increases_subject_roles() {
            $crate::suite::add_role::instance_add_increases_subject_roles::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_instance_add_creates_role_row() {
            $crate::suite::add_role::instance_add_creates_role_row::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_instance_add_returns_the_instance_scoped_role() {
            $crate::suite::add_role::instance_add_returns_the_instance_scoped_role::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_instance_readd_does_not_duplicate_link() {
            $crate::suite::add_role::instance_readd_does_not_duplicate_link::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn add_role_instance_add_reuses_existing_role_row() {
            $crate::suite::add_role::instance_add_reuses_existing_role_row::<$backend>()
                .await
                .unwrap();
        }
    };
}
