//! `has_all_roles` cases: full port of
//! `shared_examples_for_has_all_roles.rb` (l.1-71, 44 `it` rows: 10
//! global l.5-17, 11 class l.23-36, 23 instance l.42-67).
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
//!   so the mixed string+hash arg lists of l.53-67 become heterogeneous
//!   `RoleQuery` slices.
//! - **`ArgumentError` at compile time (D-19):** the gem raises
//!   `ArgumentError` for non-string/symbol/hash args (role.rb:63); the
//!   typed `&[RoleQuery]` list makes those shapes unrepresentable
//!   (USER-04 / ROADMAP SC-3) - a compile-time guarantee documented
//!   here, never a runtime case.
//! - **Self-gating coverage split:** `has_all_roles` routes every query
//!   through the strict-sensitive `has_role` (role.rb:56-67 with the
//!   role.rb:25-26 redirect), so under a strict config the matrices
//!   answer the strict ladder (a global row no longer covers a class
//!   filter) and the rows would fail. The gem runs this file only for
//!   the non-strict `User`; the port preserves that split by
//!   early-returning `Ok(())` from every matrix fn when the subject's
//!   config is strict, so the strict binding of the frozen macro
//!   exercises the gate instead of failing. Strict-side coverage of
//!   this predicate is the gem's own split, recorded as a D-19 entry.
//! - **Size split of the l.52-67 matrices:** the 16-row soldier/
//!   moderator/visitor matrix lives in two private row fns (the `be(true)`
//!   rows and the `be(false)` rows, the plan's own enumeration) behind
//!   the one public `instance_all_mixed_rows` fn, keeping every fn
//!   inside the clippy `too_many_lines` budget with the spec rows and
//!   their gem-line citations intact.
//! - **Await placement:** every ask binds before it asserts
//!   (`let answer = ...await?; assert!(answer);`) - the maybe-async
//!   `is_sync` rewrite does not descend into `assert!` token trees, so
//!   an `await` inside the assertion macro would break the sync build.
//!
//! [`RoleName`]: rolify_core::role::RoleName
//! [`RoleQuery`]: rolify_core::query::RoleQuery
//! [`ResourceFilter::Class`]: rolify_core::query::ResourceFilter::Class
//! [`ResourceFilter::Instance`]: rolify_core::query::ResourceFilter::Instance
//! [`ResourceFilter::Any`]: rolify_core::query::ResourceFilter::Any

use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::RoleName;
use rolify_core::store::ResourceKey;
use rolify_core::user::RolifyUser;

use crate::backend::{TestBackend, load_scope_context};
use crate::fixtures::{FixtureResource, ScopeContext};

/// The repeated instance ask of this module's rows: one row resource
/// resolved from the fixture matrix, asked as `Instance(type, id)`.
fn instance_filter<'key>(type_name: &'static str, key: &'key ResourceKey) -> ResourceFilter<'key> {
    ResourceFilter::Instance(type_name, &key.resource_id)
}

/// Global-only asks over bare names: staff alone and admin+staff both
/// hold (`shared_examples_for_has_all_roles.rb:5-6`).
///
/// Self-gate: default binding only (the gem runs this file for `User`,
/// not `StrictUser`; see the module docs for the D-19 note).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_all_single_and_pair_true<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let staff = RoleName::from("staff");
    let admin = RoleName::from("admin");
    let subject = backend.subject("admin");
    let queries = [RoleQuery::with_role(&staff)];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    let queries = [RoleQuery::with_role(&admin), RoleQuery::with_role(&staff)];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    Ok(())
}

/// One missing global name breaks the AND: admin+dummy and
/// dummy+dumber both fail (`shared_examples_for_has_all_roles.rb:7-8`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_all_with_missing_false<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let admin = RoleName::from("admin");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("admin");
    let queries = [RoleQuery::with_role(&admin), RoleQuery::with_role(&dummy)];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    let queries = [RoleQuery::with_role(&dummy), RoleQuery::with_role(&dumber)];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The global-context mixed-scope AND rows: the global admin covers
/// class, instance, and `:any` asks; one missing name breaks the AND
/// (`shared_examples_for_has_all_roles.rb:12-17`, 6 rows).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_all_mixed_scope_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Global).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_last = backend.resource(FixtureResource::GroupLast);
    let admin = RoleName::from("admin");
    let staff = RoleName::from("staff");
    let dummy = RoleName::from("dummy");
    let subject = backend.subject("admin");
    // l.12: (admin, Forum) AND (admin, Group): the global admin covers both class filters.
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.13: (admin, :any) AND (admin, Group).
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.14: (admin, Forum) AND (staff, Group.last): the global staff covers the instance ask.
    let queries = [
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&staff, instance_filter("Group", &group_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.15: (admin, Forum.first) AND (admin, Forum.last): the global covers every resource row.
    let queries = [
        RoleQuery::with_role_and_filter(&admin, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&admin, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.16: (admin, Forum.first) AND (dummy, Forum.last): dummy is missing.
    let queries = [
        RoleQuery::with_role_and_filter(&admin, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&dummy, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.17: (admin, Forum.first) AND (dummy, :any): dummy is missing everywhere.
    let queries = [
        RoleQuery::with_role_and_filter(&admin, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Any),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The class-context AND rows over class filters: player and manager
/// both held at Class(Forum), `:any` rows included; one missing name
/// breaks the AND (`shared_examples_for_has_all_roles.rb:23-29`, 7
/// rows).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_all_class_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let manager = RoleName::from("manager");
    let player = RoleName::from("player");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("moderator");
    // l.23: (player, Forum) alone.
    let queries = [RoleQuery::with_role_and_filter(
        &player,
        ResourceFilter::Class("Forum"),
    )];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.24: (manager, Forum) AND (player, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&player, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.25: (manager, :any) AND (player, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&player, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.26: (manager, :any) AND (player, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&player, ResourceFilter::Any),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.27: (manager, Forum) AND (dummy, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.28: (manager, Forum) AND (dummy, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Any),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.29: (dummy, Forum) AND (dumber, Group).
    let queries = [
        RoleQuery::with_role_and_filter(&dummy, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&dumber, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The class-context mixed-scope AND rows: the Class(Forum) row covers
/// instance asks of the same type, the global warrior covers every
/// resource row, and a Class(Group) ask misses
/// (`shared_examples_for_has_all_roles.rb:33-36`, 4 rows).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_all_mixed_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Class).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let manager = RoleName::from("manager");
    let moderator = RoleName::from("moderator");
    let warrior = RoleName::from("warrior");
    let subject = backend.subject("moderator");
    // l.33: (manager, Forum.first) AND (manager, Forum.last): the class row covers instances.
    let queries = [
        RoleQuery::with_role_and_filter(&manager, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&manager, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.34: (manager, Group) AND (moderator, Forum.first): both miss.
    let queries = [
        RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Group")),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.35: (manager, Forum.first) AND (moderator, Forum): the moderator rows are instance-bound.
    let queries = [
        RoleQuery::with_role_and_filter(&manager, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.36: (manager, Forum.last) AND (warrior, Forum.last): warrior is global.
    let queries = [
        RoleQuery::with_role_and_filter(&manager, instance_filter("Forum", &forum_last)),
        RoleQuery::with_role_and_filter(&warrior, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    Ok(())
}

/// The instance-context AND rows over instance filters:
/// `:any`-folded instance rows hold, the instance-bound anonymous
/// misses a Class(Forum) ask, and god's moderator lives at Forum.first
/// only (`shared_examples_for_has_all_roles.rb:42-48`, 7 rows).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_all_instance_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let moderator = RoleName::from("moderator");
    let anonymous = RoleName::from("anonymous");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("god");
    // l.42: (moderator, :any) AND (anonymous, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&anonymous, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.43: (moderator, :any) AND (anonymous, :any).
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&anonymous, ResourceFilter::Any),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.44: (moderator, :any) AND (anonymous, Forum): anonymous is instance-bound, the class filter misses.
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&anonymous, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.45: (moderator, Forum.first) AND (anonymous, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&anonymous, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.46: (moderator, Forum.first) AND (moderator, Forum.last): god holds moderator at Forum.first only.
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.47: (moderator, Forum.first) AND (dummy, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&dummy, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.48: (dummy, Forum.first) AND (dumber, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(&dummy, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&dumber, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// The instance-context soldier/moderator/visitor matrices: the 16
/// heterogeneous string+hash AND rows of l.52-67 (true at l.52, 53,
/// 55, 56, 59, 60, 61, 66, 67; false at l.54, 57, 58, 62, 63, 64, 65).
/// The rows live in two private fns (see the module docs' size note).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, context load, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_all_mixed_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    load_scope_context(&mut backend, ScopeContext::Instance).await?;
    instance_mixed_true_rows(&mut backend).await?;
    instance_mixed_false_rows(&mut backend).await?;
    Ok(())
}

/// The nine `be(true)` rows of the l.52-67 matrices. Size split only;
/// every row keeps its gem-line citation (see the module docs).
#[maybe_async::maybe_async]
async fn instance_mixed_true_rows<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let soldier = RoleName::from("soldier");
    let moderator = RoleName::from("moderator");
    let visitor = RoleName::from("visitor");
    let subject = backend.subject("god");
    // l.52: (visitor, Forum.last) alone: the visitor class row covers the instance ask.
    let queries = [RoleQuery::with_role_and_filter(
        &visitor,
        instance_filter("Forum", &forum_last),
    )];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.53: soldier AND (moderator, Forum.first) AND (visitor, Forum).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.55: soldier AND (moderator, :any) AND (visitor, Forum).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.56: soldier AND (moderator, :any) AND (visitor, :any).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Any),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.59: (soldier, Forum) AND (moderator, Forum.first) AND (visitor, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&soldier, ResourceFilter::Class("Forum")),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.60: (soldier, Forum.first) AND (moderator, Forum.first) AND (visitor, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&soldier, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.61: soldier AND (moderator, Forum.first) AND (visitor, Forum.first).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&visitor, instance_filter("Forum", &forum_first)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.66: (soldier, Forum.first) AND (moderator, Forum.first) AND (visitor, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(&soldier, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&visitor, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    // l.67: (soldier, Forum.first) AND (moderator, :any) AND (visitor, Forum.last).
    let queries = [
        RoleQuery::with_role_and_filter(&soldier, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Any),
        RoleQuery::with_role_and_filter(&visitor, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(answer);
    Ok(())
}

/// The seven `be(false)` rows of the l.52-67 matrices. Size split
/// only; every row keeps its gem-line citation (see the module docs).
#[maybe_async::maybe_async]
async fn instance_mixed_false_rows<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let soldier = RoleName::from("soldier");
    let moderator = RoleName::from("moderator");
    let visitor = RoleName::from("visitor");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let dumberer = RoleName::from("dumberer");
    let manager = RoleName::from("manager");
    let subject = backend.subject("god");
    // l.54: soldier AND (moderator, Forum.last) AND (visitor, Forum): moderator misses at Forum.last.
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_last)),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.57: soldier AND (moderator, Forum.first) AND (visitor, Group).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&visitor, ResourceFilter::Class("Group")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.58: soldier AND (moderator, Forum.first) AND (visitor, Group.first).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&moderator, instance_filter("Forum", &forum_first)),
        RoleQuery::with_role_and_filter(&visitor, instance_filter("Group", &group_first)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.62: dummy AND (dumber, Forum.last) AND (dumberer, Forum).
    let queries = [
        RoleQuery::with_role(&dummy),
        RoleQuery::with_role_and_filter(&dumber, instance_filter("Forum", &forum_last)),
        RoleQuery::with_role_and_filter(&dumberer, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.63: soldier AND dummy AND (dumber, Forum.last) AND (dumberer, Forum).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role(&dummy),
        RoleQuery::with_role_and_filter(&dumber, instance_filter("Forum", &forum_last)),
        RoleQuery::with_role_and_filter(&dumberer, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.64: (manager, Forum.last) AND dummy AND (dumber, Forum.last) AND (dumberer, Forum).
    let queries = [
        RoleQuery::with_role_and_filter(&manager, instance_filter("Forum", &forum_last)),
        RoleQuery::with_role(&dummy),
        RoleQuery::with_role_and_filter(&dumber, instance_filter("Forum", &forum_last)),
        RoleQuery::with_role_and_filter(&dumberer, ResourceFilter::Class("Forum")),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    // l.65: soldier AND (dumber, Forum.last) AND (manager, Forum.last).
    let queries = [
        RoleQuery::with_role(&soldier),
        RoleQuery::with_role_and_filter(&dumber, instance_filter("Forum", &forum_last)),
        RoleQuery::with_role_and_filter(&manager, instance_filter("Forum", &forum_last)),
    ];
    let answer = subject.has_all_roles(&queries).await?;
    assert!(!answer);
    Ok(())
}

/// Expand the 7 `has_all_roles` wrappers for one backend. Wrapper names
/// carry the `has_all_roles_` prefix so the 12 binding modules never
/// collide.
#[macro_export]
macro_rules! parity_has_all_roles_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_all_roles_global_all_single_and_pair_true() {
            $crate::suite::has_all_roles::global_all_single_and_pair_true::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_all_roles_global_all_with_missing_false() {
            $crate::suite::has_all_roles::global_all_with_missing_false::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_all_roles_global_all_mixed_scope_rows() {
            $crate::suite::has_all_roles::global_all_mixed_scope_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_all_roles_class_all_class_rows() {
            $crate::suite::has_all_roles::class_all_class_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_all_roles_class_all_mixed_rows() {
            $crate::suite::has_all_roles::class_all_mixed_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_all_roles_instance_all_instance_rows() {
            $crate::suite::has_all_roles::instance_all_instance_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn has_all_roles_instance_all_mixed_rows() {
            $crate::suite::has_all_roles::instance_all_mixed_rows::<$backend>()
                .await
                .unwrap();
        }
    };
}
