//! `has_role` / `has_strict_role` cases: full port of
//! `shared_examples_for_has_role.rb` (l.1-205) plus the strict gating family
//! (`resource_spec.rb` l.541-576, the `#strict` describe the gem uses for
//! `StrictUser`).
//!
//! ## Porting decisions (for the 02-09 D-19 harvest)
//!
//! - **Param collapse:** the gem runs every row twice (`String` and `Symbol`
//!   via `param_method`); the port passes byte-exact [`RoleName`] once.
//! - **Zombie-holder substitution:** the l.118-130 `subject.class.new` rows
//!   pin a second holder instance answering `:any`. The port seats the clean
//!   fixture holder "zombie" (id 4, role-free in every scope context) via
//!   `backend.subject("zombie")`, adds, and asserts. The gem's `new_record?`
//!   path is REQUIREMENTS out-of-scope; the behavioral point (per-holder
//!   role state plus `:any` including resource-present rows) is fully
//!   preserved. D-19 entry.
//! - **Self-gating coverage split:** matrix fns early-return `Ok(())` under
//!   a strict config, strict-family fns early-return under a non-strict
//!   config (each with a doc note). This preserves the gem's exact coverage
//!   split (matrix for `User`, `#strict` for `StrictUser`) under both
//!   bindings of the one frozen macro: the default binding runs the matrix
//!   plus the ungated pin, the strict binding runs the strict family plus
//!   the ungated pin.
//!
//! [`RoleName`]: rolify_core::role::RoleName

use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{RoleName, RoleRecord, RoleSet};
use rolify_core::store::RoleStore;
use rolify_core::user::RolifyUser;

use crate::backend::{TestBackend, load_scope_context};
use crate::fixtures::{FixtureResource, ScopeContext};

/// Global "admin" answers the global ask, query and cached
/// (`shared_examples_for_has_role.rb:4-6`).
///
/// Self-gate: default binding only (mirrors the gem running this file for
/// `User`, not `StrictUser`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_role_answers_global_and_cached<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let admin = RoleName::from("admin");
    let filter = ResourceFilter::Global;
    let subject = backend.subject("admin");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&admin, filter).await?;
    assert!(via_query);
    let query = RoleQuery::with_role_and_filter(&admin, filter);
    assert!(subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// Global "admin" covers instance, class, and `:any` asks, query and cached
/// (`shared_examples_for_has_role.rb:8-16`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_role_covers_resource_requests<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let admin = RoleName::from("admin");
    let subject = backend.subject("admin");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    for filter in [
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ResourceFilter::Class("Forum"),
        ResourceFilter::Any,
    ] {
        let via_query = subject.has_role(&admin, filter).await?;
        assert!(via_query);
        let query = RoleQuery::with_role_and_filter(&admin, filter);
        assert!(subject.has_cached_role(&snapshot, &query));
    }
    Ok(())
}

/// An unlinked global "global" row is not answered, globally or via `:any`
/// (`shared_examples_for_has_role.rb:18-26`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn other_global_row_not_answered<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    backend
        .create_role_row(RoleRecord::global("global"))
        .await?;
    let name = RoleName::from("global");
    let subject = backend.subject("admin");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    for filter in [ResourceFilter::Global, ResourceFilter::Any] {
        let via_query = subject.has_role(&name, filter).await?;
        assert!(!via_query);
        let query = RoleQuery::with_role_and_filter(&name, filter);
        assert!(!subject.has_cached_role(&snapshot, &query));
    }
    Ok(())
}

/// The subject's instance-scoped "moderator" rows (`ForumLast`, `GroupLast`)
/// do not answer an `Instance(GroupFirst)` ask
/// (`shared_examples_for_has_role.rb:28-32`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_scoped_role_not_gotten<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let name = RoleName::from("moderator");
    let filter = ResourceFilter::Instance("Group", &group_first.resource_id);
    let subject = backend.subject("admin");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// The subject's "manager" (Class("Group")) does not answer a
/// Class("Forum") ask (`shared_examples_for_has_role.rb:34-38`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_scoped_role_not_gotten<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let name = RoleName::from("manager");
    let filter = ResourceFilter::Class("Forum");
    let subject = backend.subject("admin");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// Unknown role names answer false, globally and scoped
/// (`shared_examples_for_has_role.rb:40-46`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn inexisting_roles_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("admin");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_dummy = subject.has_role(&dummy, ResourceFilter::Global).await?;
    assert!(!via_dummy);
    let dummy_query = RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Global);
    assert!(!subject.has_cached_role(&snapshot, &dummy_query));
    let scoped = ResourceFilter::Instance("Forum", &forum_first.resource_id);
    let via_dumber = subject.has_role(&dumber, scoped).await?;
    assert!(!via_dumber);
    let scoped_query = RoleQuery::with_role_and_filter(&dumber, scoped);
    assert!(!subject.has_cached_role(&snapshot, &scoped_query));
    Ok(())
}

/// Class "manager" covers the class, instance, and `:any` asks, query and
/// cached (`shared_examples_for_has_role.rb:51-57`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_role_covers_class_instance_and_any<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("moderator").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let name = RoleName::from("manager");
    let subject = backend.subject("moderator");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    for filter in [
        ResourceFilter::Class("Forum"),
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ResourceFilter::Any,
    ] {
        let via_query = subject.has_role(&name, filter).await?;
        assert!(via_query);
        let query = RoleQuery::with_role_and_filter(&name, filter);
        assert!(subject.has_cached_role(&snapshot, &query));
    }
    Ok(())
}

/// A class row never answers the global ask
/// (`shared_examples_for_has_role.rb:60-64`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_role_not_gotten_as_global<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("moderator").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let name = RoleName::from("manager");
    let filter = ResourceFilter::Global;
    let subject = backend.subject("moderator");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// An unlinked global "admin" row is not answered
/// (`shared_examples_for_has_role.rb:66-71`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn unheld_global_row_not_gotten<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("moderator").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    backend.create_role_row(RoleRecord::global("admin")).await?;
    let name = RoleName::from("admin");
    let filter = ResourceFilter::Global;
    let subject = backend.subject("moderator");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// An unlinked same-type other-name row answers neither the class nor the
/// `:any` ask (`shared_examples_for_has_role.rb:74-82`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn same_resource_other_name_not_gotten<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("moderator").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    backend
        .create_role_row(RoleRecord::for_class("member", "Forum"))
        .await?;
    let name = RoleName::from("member");
    let subject = backend.subject("moderator");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    for filter in [ResourceFilter::Class("Forum"), ResourceFilter::Any] {
        let via_query = subject.has_role(&name, filter).await?;
        assert!(!via_query);
        let query = RoleQuery::with_role_and_filter(&name, filter);
        assert!(!subject.has_cached_role(&snapshot, &query));
    }
    Ok(())
}

/// An unlinked same-name other-type row misses the class ask but the holder
/// still answers `:any` by name (the name-only DB path,
/// `shared_examples_for_has_role.rb:84-92`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn other_resource_same_name_any_true<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("moderator").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    backend
        .create_role_row(RoleRecord::for_class("manager", "Group"))
        .await?;
    let name = RoleName::from("manager");
    let subject = backend.subject("moderator");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let class_filter = ResourceFilter::Class("Group");
    let via_class = subject.has_role(&name, class_filter).await?;
    assert!(!via_class);
    let class_query = RoleQuery::with_role_and_filter(&name, class_filter);
    assert!(!subject.has_cached_role(&snapshot, &class_query));
    let via_any = subject.has_role(&name, ResourceFilter::Any).await?;
    assert!(via_any);
    let any_query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
    assert!(subject.has_cached_role(&snapshot, &any_query));
    Ok(())
}

/// An unlinked other-type other-name row answers neither the class nor the
/// `:any` ask (`shared_examples_for_has_role.rb:94-102`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn other_resource_other_name_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("moderator").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    backend
        .create_role_row(RoleRecord::for_class("defenders", "Group"))
        .await?;
    let name = RoleName::from("defenders");
    let subject = backend.subject("moderator");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    for filter in [ResourceFilter::Class("Group"), ResourceFilter::Any] {
        let via_query = subject.has_role(&name, filter).await?;
        assert!(!via_query);
        let query = RoleQuery::with_role_and_filter(&name, filter);
        assert!(!subject.has_cached_role(&snapshot, &query));
    }
    Ok(())
}

/// Unknown role names answer false, scoped and globally
/// (`shared_examples_for_has_role.rb:105-111`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_context_inexisting_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("moderator").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("moderator");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let scoped = ResourceFilter::Class("Forum");
    let via_dummy = subject.has_role(&dummy, scoped).await?;
    assert!(!via_dummy);
    let dummy_query = RoleQuery::with_role_and_filter(&dummy, scoped);
    assert!(!subject.has_cached_role(&snapshot, &dummy_query));
    let via_dumber = subject.has_role(&dumber, ResourceFilter::Global).await?;
    assert!(!via_dumber);
    let dumber_query = RoleQuery::with_role_and_filter(&dumber, ResourceFilter::Global);
    assert!(!subject.has_cached_role(&snapshot, &dumber_query));
    Ok(())
}

/// Instance "moderator" answers the instance and `:any` asks, query and
/// cached (`shared_examples_for_has_role.rb:116-125`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_role_covers_instance_and_any<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let name = RoleName::from("moderator");
    let instance_filter = ResourceFilter::Instance("Forum", &forum_first.resource_id);
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_instance = subject.has_role(&name, instance_filter).await?;
    assert!(via_instance);
    let instance_query = RoleQuery::with_role_and_filter(&name, instance_filter);
    assert!(subject.has_cached_role(&snapshot, &instance_query));
    let via_any = subject.has_role(&name, ResourceFilter::Any).await?;
    assert!(via_any);
    let any_query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
    assert!(subject.has_cached_role(&snapshot, &any_query));
    Ok(())
}

/// A second holder instance answers `:any` for its own grant, query and
/// cached (the `subject.class.new` rows, `shared_examples_for_has_role.rb:
/// 118-130`; zombie-holder substitution, see the module docs).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn second_holder_any_path<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let zombie = backend.subject("zombie");
    zombie
        .add_role(
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    let name = RoleName::from("moderator");
    let holder = zombie.rolify_id();
    let (store, conn) = zombie.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = zombie.has_role(&name, ResourceFilter::Any).await?;
    assert!(via_query);
    let query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
    assert!(zombie.has_cached_role(&snapshot, &query));
    Ok(())
}

/// An instance row never answers the global ask
/// (`shared_examples_for_has_role.rb:133-137`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_role_not_gotten_as_global<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let name = RoleName::from("moderator");
    let filter = ResourceFilter::Global;
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// An instance row never answers the class ask
/// (`shared_examples_for_has_role.rb:139-143`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_role_not_gotten_as_class<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let name = RoleName::from("moderator");
    let filter = ResourceFilter::Class("Forum");
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// An unlinked global "admin" row is not answered
/// (`shared_examples_for_has_role.rb:145-150`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_context_unheld_global_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    backend.create_role_row(RoleRecord::global("admin")).await?;
    let name = RoleName::from("admin");
    let filter = ResourceFilter::Global;
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// An unlinked same-resource other-name row answers neither the instance
/// nor the `:any` ask (`shared_examples_for_has_role.rb:152-161`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn same_resource_other_name_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    backend
        .create_role_row(RoleRecord::for_instance(
            "member",
            "Forum",
            forum_first.resource_id.clone(),
        ))
        .await?;
    let name = RoleName::from("member");
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let instance_filter = ResourceFilter::Instance("Forum", &forum_first.resource_id);
    let via_instance = subject.has_role(&name, instance_filter).await?;
    assert!(!via_instance);
    let instance_query = RoleQuery::with_role_and_filter(&name, instance_filter);
    assert!(!subject.has_cached_role(&snapshot, &instance_query));
    let via_any = subject.has_role(&name, ResourceFilter::Any).await?;
    assert!(!via_any);
    let any_query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
    assert!(!subject.has_cached_role(&snapshot, &any_query));
    Ok(())
}

/// An unlinked same-name other-instance row misses the instance ask but the
/// holder answers `:any` by name
/// (`shared_examples_for_has_role.rb:163-171`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn same_type_other_instance_any_true<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_last = backend.resource(FixtureResource::ForumLast);
    backend
        .create_role_row(RoleRecord::for_instance(
            "moderator",
            "Forum",
            forum_last.resource_id.clone(),
        ))
        .await?;
    let name = RoleName::from("moderator");
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let instance_filter = ResourceFilter::Instance("Forum", &forum_last.resource_id);
    let via_instance = subject.has_role(&name, instance_filter).await?;
    assert!(!via_instance);
    let instance_query = RoleQuery::with_role_and_filter(&name, instance_filter);
    assert!(!subject.has_cached_role(&snapshot, &instance_query));
    let via_any = subject.has_role(&name, ResourceFilter::Any).await?;
    assert!(via_any);
    let any_query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
    assert!(subject.has_cached_role(&snapshot, &any_query));
    Ok(())
}

/// An unlinked same-name other-type row misses the instance ask but the
/// holder answers `:any` by name
/// (`shared_examples_for_has_role.rb:173-181`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn other_type_same_name_any_true<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let group_last = backend.resource(FixtureResource::GroupLast);
    backend
        .create_role_row(RoleRecord::for_instance(
            "moderator",
            "Group",
            group_last.resource_id.clone(),
        ))
        .await?;
    let name = RoleName::from("moderator");
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let instance_filter = ResourceFilter::Instance("Group", &group_last.resource_id);
    let via_instance = subject.has_role(&name, instance_filter).await?;
    assert!(!via_instance);
    let instance_query = RoleQuery::with_role_and_filter(&name, instance_filter);
    assert!(!subject.has_cached_role(&snapshot, &instance_query));
    let via_any = subject.has_role(&name, ResourceFilter::Any).await?;
    assert!(via_any);
    let any_query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
    assert!(subject.has_cached_role(&snapshot, &any_query));
    Ok(())
}

/// Unlinked other-name rows answer neither their instance nor the `:any`
/// ask (`shared_examples_for_has_role.rb:183-201`).
///
/// Self-gate: default binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn other_type_other_name_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("god").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    backend
        .create_role_row(RoleRecord::for_instance(
            "member",
            "Forum",
            forum_last.resource_id.clone(),
        ))
        .await?;
    backend
        .create_role_row(RoleRecord::for_instance(
            "member",
            "Group",
            group_first.resource_id.clone(),
        ))
        .await?;
    let name = RoleName::from("member");
    let subject = backend.subject("god");
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    for filter in [
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ResourceFilter::Instance("Group", &group_first.resource_id),
        ResourceFilter::Any,
    ] {
        let via_query = subject.has_role(&name, filter).await?;
        assert!(!via_query);
        let query = RoleQuery::with_role_and_filter(&name, filter);
        assert!(!subject.has_cached_role(&snapshot, &query));
    }
    Ok(())
}

/// Strict exact instance match (`resource_spec.rb:550-553`): with "forum"
/// at Instance(ForumFirst) and Class("Forum"), the instance ask answers
/// true, query and cached.
///
/// Self-gate: strict binding only (mirrors the gem's `#strict` describe for
/// `StrictUser`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn strict_exact_instance_match<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if !backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("admin");
    let name = RoleName::from("forum");
    subject
        .add_role(
            &name,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    subject.add_role(&name, ResourceRef::Class("Forum")).await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let filter = ResourceFilter::Instance("Forum", &forum_first.resource_id);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// Strict other instance misses (`resource_spec.rb:555-558`).
///
/// Self-gate: strict binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn strict_other_instance_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if !backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let subject = backend.subject("admin");
    let name = RoleName::from("forum");
    subject
        .add_role(
            &name,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    subject.add_role(&name, ResourceRef::Class("Forum")).await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let filter = ResourceFilter::Instance("Forum", &forum_last.resource_id);
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// Strict class match (`resource_spec.rb:560-563`).
///
/// Self-gate: strict binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn strict_class_match_true<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if !backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("admin");
    let name = RoleName::from("forum");
    subject
        .add_role(
            &name,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    subject.add_role(&name, ResourceRef::Class("Forum")).await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let filter = ResourceFilter::Class("Forum");
    let via_query = subject.has_role(&name, filter).await?;
    assert!(via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// Strict `:any` stays non-strict (`resource_spec.rb:565-568`, the
/// FIND-01-family truth: `:any` never engages the strict gate, role.rb:26).
///
/// Self-gate: strict binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn strict_any_stays_non_strict<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if !backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("admin");
    let name = RoleName::from("forum");
    subject
        .add_role(
            &name,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    subject.add_role(&name, ResourceRef::Class("Forum")).await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let via_query = subject.has_role(&name, ResourceFilter::Any).await?;
    assert!(via_query);
    let query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
    assert!(subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// After removing the class role, the strict class ask answers false
/// (`resource_spec.rb:570-574`).
///
/// Self-gate: strict binding only.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn strict_removed_class_role_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if !backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let subject = backend.subject("admin");
    let name = RoleName::from("forum");
    subject
        .add_role(
            &name,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    subject.add_role(&name, ResourceRef::Class("Forum")).await?;
    subject
        .remove_role(&name, RemovalTarget::TypeSweep("Forum"))
        .await?;
    let holder = subject.rolify_id();
    let (store, conn) = subject.store_with_conn();
    let rows = store.roles_of(&mut *conn, &holder).await?;
    let snapshot = RoleSet::new(&rows);
    let filter = ResourceFilter::Class("Forum");
    let via_query = subject.has_role(&name, filter).await?;
    assert!(!via_query);
    let query = RoleQuery::with_role_and_filter(&name, filter);
    assert!(!subject.has_cached_role(&snapshot, &query));
    Ok(())
}

/// `has_strict_role` is the ungated exact-scope path in every config
/// (role.rb:25-26 together with role.rb:43-45, USER-03).
///
/// No self-gate: a global-only holder answers `has_role(name,
/// Class("Group"))` through the ladder override under the default binding
/// but not under the strict binding (where role.rb:26 redirects
/// resource-filtered asks to the strict path), while `has_strict_role`
/// answers the same exact-scope false in both configs.
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn has_strict_role_is_the_ungated_path<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    let subject = backend.subject("admin");
    let name = RoleName::from("steward");
    subject.add_role(&name, ResourceRef::Global).await?;
    let filter = ResourceFilter::Class("Group");
    let strict_config = subject.rolify_config().strict();
    let via_role = subject.has_role(&name, filter).await?;
    assert_eq!(
        via_role, !strict_config,
        "has_role's resource-filtered answer flips per config"
    );
    let via_strict = subject.has_strict_role(&name, filter).await?;
    assert!(
        !via_strict,
        "has_strict_role is exact-scope in every config"
    );
    Ok(())
}

/// Expand the 28 `has_role` wrappers for one backend. Wrapper names carry
/// the `has_role_` prefix so the 12 binding modules never collide.
#[macro_export]
macro_rules! parity_has_role_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_global_role_answers_global_and_cached() {
            $crate::suite::has_role::global_role_answers_global_and_cached::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_global_role_covers_resource_requests() {
            $crate::suite::has_role::global_role_covers_resource_requests::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_other_global_row_not_answered() {
            $crate::suite::has_role::other_global_row_not_answered::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_instance_scoped_role_not_gotten() {
            $crate::suite::has_role::instance_scoped_role_not_gotten::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_class_scoped_role_not_gotten() {
            $crate::suite::has_role::class_scoped_role_not_gotten::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_inexisting_roles_false() {
            $crate::suite::has_role::inexisting_roles_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_class_role_covers_class_instance_and_any() {
            $crate::suite::has_role::class_role_covers_class_instance_and_any::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_class_role_not_gotten_as_global() {
            $crate::suite::has_role::class_role_not_gotten_as_global::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_unheld_global_row_not_gotten() {
            $crate::suite::has_role::unheld_global_row_not_gotten::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_same_resource_other_name_not_gotten() {
            $crate::suite::has_role::same_resource_other_name_not_gotten::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_other_resource_same_name_any_true() {
            $crate::suite::has_role::other_resource_same_name_any_true::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_other_resource_other_name_false() {
            $crate::suite::has_role::other_resource_other_name_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_class_context_inexisting_false() {
            $crate::suite::has_role::class_context_inexisting_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_instance_role_covers_instance_and_any() {
            $crate::suite::has_role::instance_role_covers_instance_and_any::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_second_holder_any_path() {
            $crate::suite::has_role::second_holder_any_path::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_instance_role_not_gotten_as_global() {
            $crate::suite::has_role::instance_role_not_gotten_as_global::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_instance_role_not_gotten_as_class() {
            $crate::suite::has_role::instance_role_not_gotten_as_class::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_instance_context_unheld_global_false() {
            $crate::suite::has_role::instance_context_unheld_global_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_same_resource_other_name_false() {
            $crate::suite::has_role::same_resource_other_name_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_same_type_other_instance_any_true() {
            $crate::suite::has_role::same_type_other_instance_any_true::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_other_type_same_name_any_true() {
            $crate::suite::has_role::other_type_same_name_any_true::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_other_type_other_name_false() {
            $crate::suite::has_role::other_type_other_name_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_strict_exact_instance_match() {
            $crate::suite::has_role::strict_exact_instance_match::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_strict_other_instance_false() {
            $crate::suite::has_role::strict_other_instance_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_strict_class_match_true() {
            $crate::suite::has_role::strict_class_match_true::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_strict_any_stays_non_strict() {
            $crate::suite::has_role::strict_any_stays_non_strict::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_strict_removed_class_role_false() {
            $crate::suite::has_role::strict_removed_class_role_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_role_has_strict_role_is_the_ungated_path() {
            $crate::suite::has_role::has_strict_role_is_the_ungated_path::<$backend>()
                .await
                .unwrap();
        }
    };
}
