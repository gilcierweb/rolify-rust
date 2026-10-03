//! `has_any_role` cases: full port of
//! `shared_examples_for_has_any_role.rb` (l.1-71, 44 `it` rows: 12
//! global l.8-19, 12 class l.28-39, 20 instance l.49-68).
//!
//! ## Porting notes (for the 02-09 D-19 parity-matrix harvest)
//!
//! - **Param collapse:** the gem runs every row twice (`String` and
//!   `Symbol` via `param_method`); the port passes byte-exact
//!   [`RoleName`] once.
//! - **Hash args as queries:** the gem's `{ name:, resource: }` hashes
//!   port as [`RoleQuery`] values (`Forum` as [`ResourceFilter::Class`],
//!   instances as [`ResourceFilter::Instance`], `:any` as
//!   [`ResourceFilter::Any`]); bare string args port as global-scope
//!   queries (`has_role(name)` with the gem's default `resource = nil`),
//!   so the mixed string+hash arg lists of l.57-68 become heterogeneous
//!   `RoleQuery` slices.
//! - **`ArgumentError` at compile time (D-19):** the gem raises
//!   `ArgumentError` for non-string/symbol/hash args (role.rb:63 and
//!   the finders-side twin at finders.rb:44); the typed `&[RoleQuery]`
//!   list makes those shapes unrepresentable (USER-04 / ROADMAP SC-3) -
//!   a compile-time guarantee documented here, never a runtime case.
//! - **No self-gate (strict-safe predicate):** `has_any_role?` has no
//!   strict redirect anywhere - role.rb:69-75 runs the persisted path
//!   through the plain OR-folded `adapter.where(self.roles, *args)`,
//!   and the Phase-1 [`where_any`] store member carries no strict
//!   branch, matching the gem. So unlike the `has_all_roles` and
//!   `only_has_role` modules, every fn here runs UNGATED under BOTH
//!   bindings of the frozen macro: the rows answer identically under a
//!   strict config. D-19 entry.
//! - **Before-block provisioning:** the spec stacks `before`-block
//!   `add_role` calls on top of each scope context (staff global on
//!   Global at l.4-6; player/Class(Forum) plus superhero global on
//!   Class at l.24-26; visitor/Instance(Forum.last) plus
//!   leader/Class(Group) plus warrior global on Instance at l.44-47);
//!   every case fn reproduces its context's before block before
//!   asserting.
//! - **One OR query, boolean only:** the gem's persisted path runs ONE
//!   `WHERE` folding all args (role.rb:73); the port asserts only the
//!   boolean result - the query-count pin is Phase 3 (TEST-05).
//! - **Duplicated gem row:** l.31 byte-duplicates l.30 (a known gem
//!   quirk); the port asserts both rows, each with its own citation.
//! - **Size splits:** the 12-row class matrix and the 20-row instance
//!   matrix live in private row fns (split at the plan's own row-group
//!   seams) behind their one public fn each, keeping every fn inside
//!   the clippy `too_many_lines` budget with all rows and citations
//!   intact.
//! - **Await placement:** every ask binds before it asserts - the
//!   maybe-async `is_sync` rewrite does not descend into `assert!`
//!   token trees.
//!
//! [`RoleName`]: rolify_core::role::RoleName
//! [`RoleQuery`]: rolify_core::query::RoleQuery
//! [`ResourceFilter::Class`]: rolify_core::query::ResourceFilter::Class
//! [`ResourceFilter::Instance`]: rolify_core::query::ResourceFilter::Instance
//! [`ResourceFilter::Any`]: rolify_core::query::ResourceFilter::Any
//! [`where_any`]: rolify_core::store::RoleStore::where_any

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::RoleName;
use rolify_core::user::RolifyUser;

use crate::backend::{TestBackend, load_scope_context};
use crate::fixtures::{FixtureResource, ScopeContext};

/// Global asks over bare names: staff alone, admin+staff, and
/// admin+moderator all hold (one OR hit suffices); dummy+dumber fails
/// (`shared_examples_for_has_any_role.rb:8-11`). The l.4-6 before
/// block re-adds staff on top of the Global context (idempotent).
///
/// No self-gate: `has_any_roles` is strict-safe (see the module docs).
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
pub async fn global_any_single_and_pairs<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let staff = RoleName::from("staff");
    let subject = backend.subject("admin");
    subject.add_role(&staff, ResourceRef::Global).await?;
    // l.8: staff alone.
    let queries = [RoleQuery::with_role(&staff)];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.9: admin OR staff.
    let admin = RoleName::from("admin");
    let queries = [RoleQuery::with_role(&admin), RoleQuery::with_role(&staff)];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.10: admin OR moderator: the global admin matches; one hit suffices.
    let moderator = RoleName::from("moderator");
    let queries = [
        RoleQuery::with_role(&admin),
        RoleQuery::with_role(&moderator),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.11: dummy OR dumber: neither is held globally.
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let queries = [RoleQuery::with_role(&dummy), RoleQuery::with_role(&dumber)];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The global-context mixed-scope OR rows: the global admin matches
/// every filter shape; dummy/dumber rows fail
/// (`shared_examples_for_has_any_role.rb:12-19`, 8 rows).
///
/// No self-gate: `has_any_roles` is strict-safe (see the module docs).
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
pub async fn global_any_mixed_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_last = backend.resource(FixtureResource::GroupLast);
    let staff = RoleName::from("staff");
    let admin = RoleName::from("admin");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("admin");
    subject.add_role(&staff, ResourceRef::Global).await?;
    // l.12: (admin, Forum) OR (admin, Group).
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.13: (admin, :any) OR (admin, Group).
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.14: (admin, Forum) OR (staff, Group.last).
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(
            &staff,
            ResourceFilter::Instance("Group", &group_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.15: (admin, Forum.first) OR (admin, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.16: (admin, Forum.first) OR (dummy, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &dummy,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.17: (admin, Forum.first) OR (dummy, :any).
    let queries = [
        RoleQuery::with_role_and_filter(
            &admin,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.18: (dummy, Forum.first) OR (dumber, :any).
    let queries = [
        RoleQuery::with_role_and_filter(
            &dummy,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(&dumber, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(!answer);
    // l.19: (dummy, :any) OR (dumber, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&dumber, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The class-context OR matrix: player/manager at Class(Forum), the
/// `:any` pairs, the covered dummy rows, and the mixed instance rows
/// (`shared_examples_for_has_any_role.rb:28-39`, 12 rows incl. the
/// gem's l.31 byte-duplicate of l.30). The l.24-26 before block
/// re-adds player/Class(Forum) and adds superhero global.
///
/// No self-gate: `has_any_roles` is strict-safe (see the module docs).
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
pub async fn class_any_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let player = RoleName::from("player");
    let superhero = RoleName::from("superhero");
    let subject = backend.subject("moderator");
    subject
        .add_role(&player, ResourceRef::Class("Forum"))
        .await?;
    subject.add_role(&superhero, ResourceRef::Global).await?;
    class_any_class_filter_rows(&mut backend).await?;
    class_any_mixed_scope_rows(&mut backend).await?;
    Ok(())
}

/// The class-filter OR rows of the class matrix (l.28-35). Size split
/// only; every row keeps its gem-line citation (see the module docs).
#[maybe_async::maybe_async]
async fn class_any_class_filter_rows<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    let manager = RoleName::from("manager");
    let player = RoleName::from("player");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("moderator");
    // l.28: (player, Forum) alone.
    let queries = [RoleQuery::with_role_and_filter(
        &player,
        ResourceFilter::Class("Forum"),
    )];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.29: (manager, Forum) OR (player, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&player, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.30: (manager, Forum) OR (player, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&player, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.31: (manager, Forum) OR (player, :any) - the gem byte-duplicates l.30; ported verbatim.
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&player, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.32: (manager, :any) OR (player, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&player, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.33: (manager, Forum) OR (dummy, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.34: (manager, Forum) OR (dummy, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.35: (dummy, Forum) OR (dumber, Group).
    let queries = [
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&dumber, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The mixed-scope OR rows of the class matrix (l.36-39). Size split
/// only; every row keeps its gem-line citation (see the module docs).
#[maybe_async::maybe_async]
async fn class_any_mixed_scope_rows<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let manager = RoleName::from("manager");
    let moderator = RoleName::from("moderator");
    let warrior = RoleName::from("warrior");
    let subject = backend.subject("moderator");
    // l.36: (manager, Forum.first) OR (manager, Forum.last): the class row covers both.
    let queries = [
        RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.37: (manager, Group) OR (moderator, Forum.first): both miss.
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Group")),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(!answer);
    // l.38: (manager, Forum.first) OR (moderator, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.39: (manager, Forum.last) OR (warrior, Forum.last): warrior is global.
    let queries = [
        RoleQuery::with_role_and_filter(
            &manager,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &warrior,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    Ok(())
}

/// The instance-context OR matrix: the visitor/moderator/dummy rows of
/// l.49-56, the seven warrior/leader triples of l.57-63, and the
/// l.64-68 tail (20 rows). The l.44-47 before block adds
/// visitor/Instance(Forum.last), leader/Class(Group), and warrior
/// global on top of the Instance context.
///
/// No self-gate: `has_any_roles` is strict-safe (see the module docs).
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
pub async fn instance_any_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let visitor = RoleName::from("visitor");
    let leader = RoleName::from("leader");
    let warrior = RoleName::from("warrior");
    let subject = backend.subject("god");
    subject
        .add_role(
            &visitor,
            ResourceRef::Instance("Forum", &forum_last.resource_id),
        )
        .await?;
    subject
        .add_role(&leader, ResourceRef::Class("Group"))
        .await?;
    subject.add_role(&warrior, ResourceRef::Global).await?;
    instance_any_direct_rows(&mut backend).await?;
    instance_any_warrior_leader_triples(&mut backend).await?;
    instance_any_tail_rows(&mut backend).await?;
    Ok(())
}

/// The direct visitor/moderator/dummy OR rows (l.49-56). Size split
/// only; every row keeps its gem-line citation (see the module docs).
#[maybe_async::maybe_async]
async fn instance_any_direct_rows<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let moderator = RoleName::from("moderator");
    let visitor = RoleName::from("visitor");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("god");
    // l.49: (visitor, Forum.last) alone: held at class and instance scope.
    let queries = [RoleQuery::with_role_and_filter(
        &visitor,
        ResourceFilter::Instance("Forum", &forum_last.resource_id),
    )];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.50: (moderator, Forum.first) OR (visitor, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &visitor,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.51: (moderator, :any) OR (visitor, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(
            &visitor,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.52: (moderator, :any) OR (visitor, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.53: (moderator, Forum) OR (visitor, :any): the visitor hit carries the OR.
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.54: (moderator, Forum.first) OR (moderator, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.55: (moderator, Forum.first) OR (dummy, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &dummy,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.56: (dummy, Forum.first) OR (dumber, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(
            &dummy,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &dumber,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The seven warrior/leader triples (l.57-63, all true: the global
/// warrior matches every first-element shape). Size split only; every
/// row keeps its gem-line citation (see the module docs).
#[maybe_async::maybe_async]
async fn instance_any_warrior_leader_triples<B: TestBackend>(
    backend: &mut B,
) -> Result<(), B::Error> {
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let warrior = RoleName::from("warrior");
    let moderator = RoleName::from("moderator");
    let leader = RoleName::from("leader");
    let subject = backend.subject("god");
    // l.57: warrior OR (moderator, Forum.first) OR (leader, Group).
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.58: warrior OR (moderator, :any) OR (leader, Forum).
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.59: warrior OR (moderator, Forum.first) OR (leader, :any).
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.60: warrior OR (moderator, :any) OR (leader, :any).
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Any),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.61: warrior OR (moderator, Forum.last) OR (leader, Forum).
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.62: warrior OR (moderator, Forum.last) OR (leader, Group): both ends match.
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.63: warrior OR (moderator, Forum.last) OR (leader, Group.first).
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &leader,
            ResourceFilter::Instance("Group", &group_first.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    Ok(())
}

/// The tail rows (l.64-68): the two hash-spelled warrior asks, the
/// Forum.first leader row, the dummy row, and the leader-matching
/// closer. Size split only; every row keeps its gem-line citation
/// (see the module docs).
#[maybe_async::maybe_async]
async fn instance_any_tail_rows<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_last = backend.resource(FixtureResource::GroupLast);
    let warrior = RoleName::from("warrior");
    let moderator = RoleName::from("moderator");
    let leader = RoleName::from("leader");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let dumberer = RoleName::from("dumberer");
    let subject = backend.subject("god");
    // l.64: (warrior, Forum) OR (moderator, Forum.last) OR (leader, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&warrior, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.65: (warrior, Forum.first) OR (moderator, Forum.last) OR (leader, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(
            &warrior,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&leader, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.66: warrior OR (moderator, Forum.last) OR (leader, Forum.first).
    let queries = [
        RoleQuery::with_role(&warrior),
        RoleQuery::with_role_and_filter(
            &moderator,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(
            &leader,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        ),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    // l.67: dummy OR (dumber, Forum.last) OR (dumberer, Forum): nothing matches.
    let queries = [
        RoleQuery::with_role(&dummy),
        RoleQuery::with_role_and_filter(
            &dumber,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&dumberer, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(!answer);
    // l.68: (leader, Group.last) OR dummy OR (dumber, Forum.last) OR (dumberer, Forum): leader matches.
    let queries = [
        RoleQuery::with_role_and_filter(
            &leader,
            ResourceFilter::Instance("Group", &group_last.resource_id),
        ),
        RoleQuery::with_role(&dummy),
        RoleQuery::with_role_and_filter(
            &dumber,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        ),
        RoleQuery::with_role_and_filter(&dumberer, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_any_roles(&queries).await?;
    assert!(answer);
    Ok(())
}

/// Expand the 4 `has_any_role` wrappers for one backend. Wrapper names
/// carry the `has_any_role_` prefix so the 12 binding modules never
/// collide.
#[macro_export]
macro_rules! parity_has_any_role_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_any_role_global_any_single_and_pairs() {
            $crate::suite::has_any_role::global_any_single_and_pairs::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_any_role_global_any_mixed_rows() {
            $crate::suite::has_any_role::global_any_mixed_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_any_role_class_any_rows() {
            $crate::suite::has_any_role::class_any_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_any_role_instance_any_rows() {
            $crate::suite::has_any_role::instance_any_rows::<$backend>()
                .await
                .unwrap();
        }
    };
}
