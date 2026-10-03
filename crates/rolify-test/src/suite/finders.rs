//! `finders` cases: the full port of `shared_examples_for_finders.rb`
//! (l.1-154) over the 02-07 `RolifyUser` statics.
//!
//! ## Mixed-context dependency
//!
//! Every case builds a fresh backend and loads the mixed context
//! (`shared_contexts.rb:79-82`) through [`provision_mixed_context`]:
//! root is `user_class.first` (login "admin": admin+staff global,
//! moderator/Class(Group), visitor/Instance(ForumLast)), modo is the
//! "moderator" login (moderator/Class(Forum), manager/Class(Group),
//! visitor/Instance(GroupFirst)), visitor is `user_class.last` (login
//! "zombie": visitor/Instance(ForumLast)), and the owner holder reuses
//! `user_class.first` for the Company-bound row (l.82). "god" is a
//! registered holder that never receives a link - the D-02
//! never-rolificated row the `without_role` complement keeps.
//!
//! ## Set-compare contract (D-04)
//!
//! The gem's `eq([ root ])`, `=~`, and `include` rows are all set
//! semantics over unordered relations; every row here compares through
//! [`assert_id_set`]. No ordering guarantee is implemented or tested.
//!
//! ## Self-gating coverage split (the 02-02 contract)
//!
//! The gem toggles `user_class.strict_rolify` for the l.53-72 context
//! only; the port maps that to the strict binding
//! (`in_memory_strict_user`): the seven non-strict case fns
//! early-return under a strict config, the two strict case fns
//! early-return under a non-strict config, so both bindings of the
//! one frozen macro stay green while each row asserts in exactly one
//! binding.
//!
//! ## Porting decisions (for the 02-09 D-19 harvest)
//!
//! - **Param collapse:** the gem runs every row twice (`String` and
//!   `Symbol` via `param_method`, l.1); the port passes byte-exact
//!   [`RoleName`]s once.
//! - **Compile-time presence:** the `respond_to` rows (l.4-5, l.76-77,
//!   l.126, l.140) are method-presence pins; calling the statics
//!   below IS the pin (the `roles.rs` module precedent).
//! - **Weak matchers strengthened where deterministic:** the gem's
//!   `should_not eq([ x ])` complement rows assert the exact set here
//!   (the fixture universe makes the complement deterministic), and
//!   the `should_not be_empty` rows assert non-emptiness, exactly as
//!   written.
//!
//! [`RoleName`]: rolify_core::role::RoleName
//! [`provision_mixed_context`]: crate::backend::provision_mixed_context

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;

use crate::backend::{TestBackend, provision_mixed_context};
use crate::fixtures::FixtureResource;

/// Assert two id lists as SETS (D-04): sort plus dedup on both sides,
/// then equality. The gem's `eq([ root ])` / `=~` / `include` rows are
/// all set semantics; this is the single comparison helper every case
/// consumes. No ordering guarantee is implemented or tested.
///
/// # Panics
///
/// Panics with both collections on any set mismatch.
pub fn assert_id_set(actual: &[ResourceId], expected: &[ResourceId]) {
    let mut sorted_actual = actual.to_vec();
    sorted_actual.sort_unstable();
    sorted_actual.dedup();
    let mut sorted_expected = expected.to_vec();
    sorted_expected.sort_unstable();
    sorted_expected.dedup();
    assert_eq!(
        sorted_actual, sorted_expected,
        "finder results compare as sets (D-04); actual {actual:?} expected {expected:?}"
    );
}

/// Resolve a fixture login to its holder id, panicking on the unknown
/// login (a misspelled login is a harness programming error - the
/// [`TestBackend::subject`] convention).
///
/// # Panics
///
/// Panics when `login` is not a registered fixture holder.
fn fixture_holder<B: TestBackend>(backend: &B, login: &str) -> ResourceId {
    backend
        .holder_id(login)
        .unwrap_or_else(|| panic!("fixture login {login} is registered"))
}

/// Global name-only rows (l.8-12): root holds the only global admin
/// row; the name-only moderator and visitor asks are global-scope and
/// match nobody (root's moderator is Class(Group), modo's is
/// Class(Forum); every visitor row is instance-scoped).
///
/// Self-gate: default binding only (mirrors the gem running this file
/// for `User`, not `StrictUser`).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn with_role_global_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let root = fixture_holder(&backend, "admin");
    let admin = RoleName::from("admin");
    let moderator = RoleName::from("moderator");
    let visitor = RoleName::from("visitor");

    // l.9: with_role("admin") = [ root ]
    let query = RoleQuery::with_role(&admin);
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.10: with_role("moderator") is empty
    let query = RoleQuery::with_role(&moderator);
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(found.is_empty());

    // l.11: with_role("visitor") is empty
    let query = RoleQuery::with_role(&visitor);
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(found.is_empty());
    Ok(())
}

/// Class-scoped rows (l.14-26): on Forum and Group, the non-strict
/// ladder lets root's global admin cover both class asks; modo's
/// moderator/Class(Forum) and root's moderator/Class(Group) answer
/// their own class asks; visitor has no class or global row.
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
pub async fn with_role_class_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let root = fixture_holder(&backend, "admin");
    let modo = fixture_holder(&backend, "moderator");
    let admin = RoleName::from("admin");
    let moderator = RoleName::from("moderator");
    let visitor = RoleName::from("visitor");

    // l.16: with_role("admin", Forum) = [ root ] (global covers)
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"));
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.17: with_role("moderator", Forum) = [ modo ]
    let query = RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum"));
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    // l.18: with_role("visitor", Forum) is empty
    let query = RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Forum"));
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(found.is_empty());

    // l.22: with_role("admin", Group) = [ root ]
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Group"));
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.23: with_role("moderator", Group) = [ root ] (root's
    // moderator/Class(Group) row)
    let query = RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Group"));
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.24: with_role("visitor", Group) is empty
    let query = RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Group"));
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(found.is_empty());
    Ok(())
}

/// Instance-scoped rows (l.28-50): Forum.first, Forum.last,
/// Group.first, and the Company STI fixture. The l.38 row is the
/// class-covers ladder pin: visitor/Instance(ForumLast) is held by
/// root AND visitor. The l.47-49 row rides the owner holder
/// (`user_class.first`, `shared_contexts.rb:82`).
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
pub async fn with_role_instance_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let root = fixture_holder(&backend, "admin");
    let modo = fixture_holder(&backend, "moderator");
    let visitor = fixture_holder(&backend, "zombie");
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let company = backend.resource(FixtureResource::Company);
    let admin = RoleName::from("admin");
    let moderator = RoleName::from("moderator");
    let visitor_name = RoleName::from("visitor");
    let owner = RoleName::from("owner");

    // l.30-32: Forum.first
    let query = RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    let query = RoleQuery::with_role_and_filter(
        &moderator,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    let query = RoleQuery::with_role_and_filter(
        &visitor_name,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(found.is_empty());

    // l.36-38: Forum.last - the l.38 include(root, visitor) row: the
    // visitor/Instance(ForumLast) row is held by root and visitor.
    let query = RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    let query = RoleQuery::with_role_and_filter(
        &moderator,
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    let query = RoleQuery::with_role_and_filter(
        &visitor_name,
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, &[root.clone(), visitor.clone()]);

    // l.42-44: Group.first - root's moderator/Class(Group) covers the
    // instance ask; modo's visitor/Instance(GroupFirst) is exact.
    let query = RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Instance("Group", &group_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    let query = RoleQuery::with_role_and_filter(
        &moderator,
        ResourceFilter::Instance("Group", &group_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    let query = RoleQuery::with_role_and_filter(
        &visitor_name,
        ResourceFilter::Instance("Group", &group_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    // l.47-49: Company.first (STI fixture) - the owner holder is
    // user_class.first, the same fixture identity as root.
    let query = RoleQuery::with_role_and_filter(
        &owner,
        ResourceFilter::Instance("Company", &company.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert_id_set(&found, core::slice::from_ref(&root));
    Ok(())
}

/// The `.without_role` complement matrix (l.79-121): the full-holder
/// universe is {admin, moderator, god, zombie} (D-02 - "god" never
/// received a link and still counts). `should_not eq([ x ])` rows
/// assert the exact complement (the fixture universe makes it
/// deterministic); `should_not be_empty` rows assert non-emptiness.
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
// The 110-line body is the 1:1 port of the gem's l.79-121 complement
// matrix; splitting it would break the per-row spec correspondence.
#[allow(clippy::too_many_lines)]
#[maybe_async::maybe_async]
pub async fn without_role_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let root = fixture_holder(&backend, "admin");
    let modo = fixture_holder(&backend, "moderator");
    let god = fixture_holder(&backend, "god");
    let zombie = fixture_holder(&backend, "zombie");
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let company = backend.resource(FixtureResource::Company);
    let admin = RoleName::from("admin");
    let moderator = RoleName::from("moderator");
    let visitor = RoleName::from("visitor");
    let owner = RoleName::from("owner");

    let without_root = [modo.clone(), god.clone(), zombie.clone()];
    let without_modo = [root.clone(), god.clone(), zombie.clone()];
    let without_root_and_visitor = [modo.clone(), god.clone()];

    // l.80: without_role("admin") excludes root - the other three.
    let query = RoleQuery::with_role(&admin);
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);

    // l.81-82: nobody holds a global moderator or visitor row, so the
    // complements stay non-empty.
    let query = RoleQuery::with_role(&moderator);
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert!(!found.is_empty());
    let query = RoleQuery::with_role(&visitor);
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert!(!found.is_empty());

    // l.87-89: on Forum, the admin complement excludes root and the
    // moderator complement excludes modo.
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"));
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    let query = RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum"));
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_modo);
    let query = RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Forum"));
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert!(!found.is_empty());

    // l.93-95: on Group, both the admin and the moderator complements
    // exclude root (root's global admin and moderator/Class(Group)).
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Group"));
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    let query = RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Group"));
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    let query = RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Group"));
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert!(!found.is_empty());

    // l.101-103: Forum.first
    let query = RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    let query = RoleQuery::with_role_and_filter(
        &moderator,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_modo);
    let query = RoleQuery::with_role_and_filter(
        &visitor,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert!(!found.is_empty());

    // l.107-109: Forum.last - the l.109 row keeps neither root nor
    // visitor (both hold visitor/Instance(ForumLast)).
    let query = RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    let query = RoleQuery::with_role_and_filter(
        &moderator,
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_modo);
    let query = RoleQuery::with_role_and_filter(
        &visitor,
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root_and_visitor);

    // l.113-115: Group.first
    let query = RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Instance("Group", &group_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    let query = RoleQuery::with_role_and_filter(
        &moderator,
        ResourceFilter::Instance("Group", &group_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    let query = RoleQuery::with_role_and_filter(
        &visitor,
        ResourceFilter::Instance("Group", &group_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_modo);

    // l.119: the owner complement excludes the owner holder
    // (user_class.first, the root identity).
    let query = RoleQuery::with_role_and_filter(
        &owner,
        ResourceFilter::Instance("Company", &company.resource_id),
    );
    let found = <B::Subject as RolifyUser>::without_role(backend.engine(), &query).await?;
    assert_id_set(&found, &without_root);
    Ok(())
}

/// The `.with_all_roles` rows (l.125-137): intersect with early exit.
/// l.130 and l.133 are empty because name-only asks are global-scope
/// and nobody holds a global moderator or manager row; l.136 unions
/// the `:any` name-only asks over root and modo.
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
pub async fn with_all_roles_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let root = fixture_holder(&backend, "admin");
    let modo = fixture_holder(&backend, "moderator");
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let admin = RoleName::from("admin");
    let staff = RoleName::from("staff");
    let moderator = RoleName::from("moderator");
    let manager = RoleName::from("manager");
    let visitor = RoleName::from("visitor");

    // l.128: with_all_roles("admin", :staff) = [ root ]
    let queries = [RoleQuery::with_role(&admin), RoleQuery::with_role(&staff)];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.129: + moderator/Class(Group) still = [ root ]
    let queries = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role(&staff),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Group")),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.130: with_all_roles("admin", "moderator") is empty - name-only
    // asks are global scope and nobody holds a global moderator row.
    let queries = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role(&moderator),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert!(found.is_empty());

    // l.131: + staff still misses - root lacks a Class(Forum) moderator.
    let queries = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role(&staff),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert!(found.is_empty());

    // l.132: with_all_roles(moderator/Forum, manager/Group) = [ modo ]
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Group")),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    // l.133: with_all_roles("moderator", :manager) is empty
    let queries = [
        RoleQuery::with_role(&moderator),
        RoleQuery::with_role(&manager),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert!(found.is_empty());

    // l.134: visitor/Instance(ForumLast) + moderator/Class(Group) =
    // [ root ]
    let queries = [
        RoleQuery::with_role_and_filter(
            &visitor,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Group")),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.135: visitor/Instance(GroupFirst) + moderator/Class(Forum) =
    // [ modo ]
    let queries = [
        RoleQuery::with_role_and_filter(
            &visitor,
            ResourceFilter::Instance("Group", &group_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    // l.136: visitor/:any + moderator/:any = [ root, modo ]
    let queries = [
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
    ];
    let found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, &[root, modo]);
    Ok(())
}

/// The `.with_any_role` rows (l.139-152): union with dedup. l.151 is
/// the dedup pin: root and modo match BOTH `:any` asks and appear
/// once each.
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
pub async fn with_any_roles_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let root = fixture_holder(&backend, "admin");
    let modo = fixture_holder(&backend, "moderator");
    let visitor = fixture_holder(&backend, "zombie");
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let admin = RoleName::from("admin");
    let staff = RoleName::from("staff");
    let moderator = RoleName::from("moderator");
    let manager = RoleName::from("manager");
    let visitor_name = RoleName::from("visitor");

    // l.142: with_any_role("admin", :staff) = [ root ]
    let queries = [RoleQuery::with_role(&admin), RoleQuery::with_role(&staff)];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.143: + moderator/Class(Group) still = [ root ]
    let queries = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role(&staff),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Group")),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.144: with_any_role("admin", "moderator") = [ root ]
    let queries = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role(&moderator),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&root));

    // l.145: + staff + moderator/Class(Forum) = [ root, modo ]
    let queries = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role(&staff),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, &[root.clone(), modo.clone()]);

    // l.146: with_any_role(moderator/Forum, manager/Group) = [ modo ]
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Group")),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    // l.147: with_any_role(moderator/Group, manager/Group) =
    // [ root, modo ]
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Group")),
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Group")),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, &[root.clone(), modo.clone()]);

    // l.148: with_any_role("moderator", :manager) is empty
    let queries = [
        RoleQuery::with_role(&moderator),
        RoleQuery::with_role(&manager),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert!(found.is_empty());

    // l.149: visitor/Instance(ForumLast) + moderator/Class(Group) =
    // [ root, visitor ]
    let queries = [
        RoleQuery::with_role_and_filter(
            &visitor_name,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Group")),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, &[root.clone(), visitor.clone()]);

    // l.150: visitor/Instance(GroupFirst) + moderator/Class(Forum) =
    // [ modo ]
    let queries = [
        RoleQuery::with_role_and_filter(
            &visitor_name,
            ResourceFilter::Instance("Group", &group_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, core::slice::from_ref(&modo));

    // l.151: visitor/:any + moderator/:any = [ root, modo, visitor ]
    // (the dedup pin - root and modo match both asks and appear once)
    let queries = [
        RoleQuery::with_role_and_filter(&visitor_name, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
    ];
    let found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &queries).await?;
    assert_id_set(&found, &[root, modo, visitor]);
    Ok(())
}

/// The empty-args rows (finders.rb:16-32/37-48 pin): zero queries
/// return empty id lists - the typed-void of the gem's `parse_args`
/// over no arguments. The zero-store-call probe contract is pinned in
/// `rolify-core`'s `user_flow`; here the observable is the empty
/// result with no panic and no store state change.
///
/// Self-gate: default binding only (the row is config-independent,
/// but the gem runs it in the non-strict context).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn empty_query_lists_return_empty_without_store_calls<B: TestBackend>()
-> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let rows_before = backend.role_row_count().await?;

    let all_found = <B::Subject as RolifyUser>::with_all_roles(backend.engine(), &[]).await?;
    assert!(all_found.is_empty());
    let any_found = <B::Subject as RolifyUser>::with_any_roles(backend.engine(), &[]).await?;
    assert!(any_found.is_empty());

    let rows_after = backend.role_row_count().await?;
    assert_eq!(rows_before, rows_after, "no store state change");
    Ok(())
}

/// Strict instance rows (l.61-65): under the strict binding, the
/// Forum.first instance asks come back empty - strict disables the
/// global/class coverage of instance asks (root's global admin and
/// modo's moderator/Class(Forum) no longer answer).
///
/// Self-gate: strict binding only (the gem's
/// `user_class.strict_rolify = true` toggle rows, l.54-59).
///
/// # Errors
///
/// Propagates backend failures.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn strict_with_role_instance_filters_empty<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if !backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let admin = RoleName::from("admin");
    let moderator = RoleName::from("moderator");

    // l.63: with_role("admin", Forum.first) is empty
    let query = RoleQuery::with_role_and_filter(
        &admin,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(found.is_empty());

    // l.64: with_role("moderator", Forum.first) is empty
    let query = RoleQuery::with_role_and_filter(
        &moderator,
        ResourceFilter::Instance("Forum", &forum_first.resource_id),
    );
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(found.is_empty());
    Ok(())
}

/// Strict `:any` rows (l.67-70): FIND-01 - `Any` never engages the
/// strict gate, so even under the strict binding the name-only admin
/// and moderator asks stay populated.
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
pub async fn strict_with_role_any_stays_populated<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if !backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    provision_mixed_context(&mut backend).await?;
    let admin = RoleName::from("admin");
    let moderator = RoleName::from("moderator");

    // l.68: with_role("admin", :any) is not empty
    let query = RoleQuery::with_role_and_filter(&admin, ResourceFilter::Any);
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(!found.is_empty());

    // l.69: with_role("moderator", :any) is not empty
    let query = RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any);
    let found = <B::Subject as RolifyUser>::with_role(backend.engine(), &query).await?;
    assert!(!found.is_empty());
    Ok(())
}

/// Expand the 9 `finders` wrappers for one backend. Wrapper names
/// carry the `finders_` prefix so the 12 binding modules never
/// collide.
#[macro_export]
macro_rules! parity_finders_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_with_role_global_rows() {
            $crate::suite::finders::with_role_global_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_with_role_class_rows() {
            $crate::suite::finders::with_role_class_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_with_role_instance_rows() {
            $crate::suite::finders::with_role_instance_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_without_role_rows() {
            $crate::suite::finders::without_role_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_with_all_roles_rows() {
            $crate::suite::finders::with_all_roles_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_with_any_roles_rows() {
            $crate::suite::finders::with_any_roles_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_empty_query_lists_return_empty_without_store_calls() {
            $crate::suite::finders::empty_query_lists_return_empty_without_store_calls::<$backend>(
            )
            .await
            .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_strict_with_role_instance_filters_empty() {
            $crate::suite::finders::strict_with_role_instance_filters_empty::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn finders_strict_with_role_any_stays_populated() {
            $crate::suite::finders::strict_with_role_any_stays_populated::<$backend>()
                .await
                .unwrap();
        }
    };
}
