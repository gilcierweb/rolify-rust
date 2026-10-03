//! `only_has_role` cases: full port of
//! `shared_examples_for_only_has_role.rb` (l.1-174, 44 `it` rows: 11
//! global, 16 class, 17 instance).
//!
//! ## Porting notes (for the 02-09 D-19 parity-matrix harvest)
//!
//! - **Fresh-subject substitution (D-19):** the spec declares its own
//!   subjects (`User.create(:login => "global_user")` plus one
//!   `add_role`, l.4-8/46-50/105-109); the port seats the clean
//!   fixture holder "zombie" (role-free after `reset_roles`) and adds
//!   the single role - exactly one fresh holder per case, mirroring
//!   the gem's per-context `subject` blocks. The gem's `new_record?`
//!   branch of `has_role?` is REQUIREMENTS out-of-scope; the
//!   behavioral point (a holder with exactly one linked role) is
//!   fully preserved.
//! - **`role_class.create` rows:** the spec's unlinked role rows
//!   (`role_class.create(:name => ..., :resource => ...)` at l.19/63/
//!   69/76/83/125/131/138/145/152/159) port through
//!   [`TestBackend::create_role_row`] - rows that exist in the table
//!   but never link to the holder.
//! - **Param collapse:** the gem runs every row twice (`String` and
//!   `Symbol` via `param_method`); the port passes byte-exact
//!   [`RoleName`] once.
//! - **`ArgumentError` at compile time (D-19):** the typed
//!   `(name, ResourceFilter)` signature makes the gem's invalid
//!   argument shapes unrepresentable (USER-04 / ROADMAP SC-3; the
//!   `ArgumentError` family lives at role.rb:63 and finders.rb:44) -
//!   a compile-time guarantee documented here, never a runtime case.
//! - **Count observation:** role.rb:78's `self.roles.count == 1` ports
//!   to the public surface as `subject.roles_name().len() == 1`
//!   (role.rb:88-90: one name per linked role, identical count - the
//!   02-01 observation convention). The dedicated observation fn
//!   proves the count precondition end to end.
//! - **Self-gating coverage split:** `only_has_role?` routes through
//!   the strict-sensitive `has_role?` (role.rb:77-79 with the
//!   role.rb:25-26 redirect), so under a strict config the
//!   resource-filtered asks answer the strict ladder and the rows
//!   would fail. The gem runs this file only for the non-strict
//!   `User`; the port preserves that split by early-returning `Ok(())`
//!   from every fn when the subject's config is strict. Strict-side
//!   coverage of this predicate is the gem's own split, recorded as a
//!   D-19 entry.
//! - **Any-includes-own-row:** the l.79/141/148 subtleties hold
//!   because `ResourceFilter::Any` matches by name only (the D-02 DB
//!   path), so the holder's own single class/instance row answers
//!   the `:any` ask - green here, pinned in the kernel.
//! - **Size split:** the five other-instance blocks of l.129-163 (the
//!   gem's "with another instance scoped role" sub-context) live in
//!   one private row fn behind `instance_only_false_rows`, keeping
//!   every fn inside the clippy `too_many_lines` budget with all rows
//!   and citations intact.
//! - **Await placement:** every ask binds before it asserts - the
//!   maybe-async `is_sync` rewrite does not descend into `assert!`
//!   token trees.
//!
//! [`RoleName`]: rolify_core::role::RoleName
//! [`TestBackend::create_role_row`]: crate::backend::TestBackend::create_role_row

use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{RoleName, RoleRecord};
use rolify_core::user::RolifyUser;

use crate::backend::TestBackend;
use crate::fixtures::FixtureResource;

/// The global holder's true rows: a zombie holding exactly
/// `global_role` globally answers the global, instance, class, and
/// `:any` asks (`shared_examples_for_only_has_role.rb:10-16`).
///
/// Self-gate: default binding only (`only_has_role` routes through the
/// strict-sensitive `has_role`, role.rb:77-79 via l.25-26; see the
/// module docs for the D-19 note).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, provisioning, or
/// asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_only_true_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let global_role = RoleName::from("global_role");
    let subject = backend.subject("zombie");
    subject.add_role(&global_role, ResourceRef::Global).await?;
    // l.10: the gem's no-resource ask.
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Global)
        .await?;
    assert!(answer);
    // l.13: (global_role, Forum.first) - the global covers every resource row.
    let answer = subject
        .only_has_role(
            &global_role,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    assert!(answer);
    // l.14: (global_role, Forum).
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Class("Forum"))
        .await?;
    assert!(answer);
    // l.15: (global_role, :any).
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Any)
        .await?;
    assert!(answer);
    Ok(())
}

/// The global holder's false rows: unlinked rows, missing names, and
/// mis-scoped asks all fail against the single-role zombie
/// (`shared_examples_for_only_has_role.rb:18-36`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, row inserts,
/// provisioning, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_only_false_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    // l.19: role_class.create(:name => "another_global_role") - unlinked.
    backend
        .create_role_row(RoleRecord::global("another_global_role"))
        .await?;
    let global_role = RoleName::from("global_role");
    let another_global_role = RoleName::from("another_global_role");
    let moderator = RoleName::from("moderator");
    let manager = RoleName::from("manager");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("zombie");
    subject.add_role(&global_role, ResourceRef::Global).await?;
    // l.21: the unlinked row never answers.
    let answer = subject
        .only_has_role(&another_global_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    // l.22: and neither does its :any shape.
    let answer = subject
        .only_has_role(&another_global_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    // l.26: an instance-scoped moderator ask misses.
    let answer = subject
        .only_has_role(
            &moderator,
            ResourceFilter::Instance("Group", &group_first.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.30: a class-scoped manager ask misses.
    let answer = subject
        .only_has_role(&manager, ResourceFilter::Class("Forum"))
        .await?;
    assert!(!answer);
    // l.34: an inexisting global name misses.
    let answer = subject
        .only_has_role(&dummy, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    // l.35: an inexisting instance-scoped name misses.
    let answer = subject
        .only_has_role(
            &dumber,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    assert!(!answer);
    Ok(())
}

/// The global multiple-roles veto: a second global role flips every
/// only-ask false although the first role is still held
/// (`shared_examples_for_only_has_role.rb:38-42`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, provisioning, or
/// asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_only_multiple_roles_vetoes<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let global_role = RoleName::from("global_role");
    let multiple_global_roles = RoleName::from("multiple_global_roles");
    let subject = backend.subject("zombie");
    subject.add_role(&global_role, ResourceRef::Global).await?;
    // l.39: before { subject.add_role "multiple_global_roles" }.
    subject
        .add_role(&multiple_global_roles, ResourceRef::Global)
        .await?;
    // l.41: two roles held, so the count vetoes the ask.
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    Ok(())
}

/// The class holder's true rows: a zombie holding exactly
/// `class_role` at Class(Forum) answers the class, instance, and `:any`
/// asks (`shared_examples_for_only_has_role.rb:52-56`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, provisioning, or
/// asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_only_true_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let class_role = RoleName::from("class_role");
    let subject = backend.subject("zombie");
    subject
        .add_role(&class_role, ResourceRef::Class("Forum"))
        .await?;
    // l.53: (class_role, Forum).
    let answer = subject
        .only_has_role(&class_role, ResourceFilter::Class("Forum"))
        .await?;
    assert!(answer);
    // l.54: (class_role, Forum.first) - the class row covers the instance ask.
    let answer = subject
        .only_has_role(
            &class_role,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    assert!(answer);
    // l.55: (class_role, :any).
    let answer = subject
        .only_has_role(&class_role, ResourceFilter::Any)
        .await?;
    assert!(answer);
    Ok(())
}

/// The class holder's false rows: the global ask, unlinked global and
/// other-class rows, mis-scoped asks, and inexisting names all fail;
/// only the `:any` ask over the holder's own name stays true
/// (`shared_examples_for_only_has_role.rb:58-93`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, row inserts,
/// provisioning, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_only_false_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    // l.63: role_class.create(:name => "global_role") - unlinked.
    backend
        .create_role_row(RoleRecord::global("global_role"))
        .await?;
    // l.69: role_class.create(:name => "another_class_role", resource_type: "Forum").
    backend
        .create_role_row(RoleRecord::for_class("another_class_role", "Forum"))
        .await?;
    // l.76: role_class.create(:name => "class_role", resource_type: "Group").
    backend
        .create_role_row(RoleRecord::for_class("class_role", "Group"))
        .await?;
    // l.83: role_class.create(:name => "another_class_role", resource_type: "Group").
    backend
        .create_role_row(RoleRecord::for_class("another_class_role", "Group"))
        .await?;
    let class_role = RoleName::from("class_role");
    let global_role = RoleName::from("global_role");
    let another_class_role = RoleName::from("another_class_role");
    let dummy = RoleName::from("dummy");
    let dumber = RoleName::from("dumber");
    let subject = backend.subject("zombie");
    subject
        .add_role(&class_role, ResourceRef::Class("Forum"))
        .await?;
    // l.59: a class-scoped role never answers the global ask.
    let answer = subject
        .only_has_role(&class_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    // l.64: the unlinked global row never answers.
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    // l.71: another class role on the same resource misses.
    let answer = subject
        .only_has_role(&another_class_role, ResourceFilter::Class("Forum"))
        .await?;
    assert!(!answer);
    // l.72: and so does its :any shape.
    let answer = subject
        .only_has_role(&another_class_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    // l.78: the same name on another resource misses the Class(Group) ask.
    let answer = subject
        .only_has_role(&class_role, ResourceFilter::Class("Group"))
        .await?;
    assert!(!answer);
    // l.79: but :any includes the zombie's own class row - the gem's truthy row.
    let answer = subject
        .only_has_role(&class_role, ResourceFilter::Any)
        .await?;
    assert!(answer);
    // l.85: another name on another resource misses.
    let answer = subject
        .only_has_role(&another_class_role, ResourceFilter::Class("Group"))
        .await?;
    assert!(!answer);
    // l.86: and so does its :any shape.
    let answer = subject
        .only_has_role(&another_class_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    // l.91: an inexisting class-scoped name misses.
    let answer = subject
        .only_has_role(&dummy, ResourceFilter::Class("Forum"))
        .await?;
    assert!(!answer);
    // l.92: an inexisting global name misses.
    let answer = subject
        .only_has_role(&dumber, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    Ok(())
}

/// The class multiple-roles veto: a second global role flips the
/// class, instance, and `:any` asks false although `class_role` is
/// still held (`shared_examples_for_only_has_role.rb:95-101`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, provisioning, or
/// asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn class_only_multiple_roles_vetoes<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let class_role = RoleName::from("class_role");
    let multiple_class_roles = RoleName::from("multiple_class_roles");
    let subject = backend.subject("zombie");
    subject
        .add_role(&class_role, ResourceRef::Class("Forum"))
        .await?;
    // l.96: before { subject.add_role "multiple_class_roles" }.
    subject
        .add_role(&multiple_class_roles, ResourceRef::Global)
        .await?;
    // l.98: two roles held: the class ask is vetoed.
    let answer = subject
        .only_has_role(&class_role, ResourceFilter::Class("Forum"))
        .await?;
    assert!(!answer);
    // l.99: the instance ask too.
    let answer = subject
        .only_has_role(
            &class_role,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.100: and the :any ask as well.
    let answer = subject
        .only_has_role(&class_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    Ok(())
}

/// The instance holder's true rows: a zombie holding exactly
/// `instance_role` at Forum.first answers the exact-instance and `:any`
/// asks (`shared_examples_for_only_has_role.rb:111-114`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, provisioning, or
/// asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_only_true_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let instance_role = RoleName::from("instance_role");
    let subject = backend.subject("zombie");
    subject
        .add_role(
            &instance_role,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    // l.112: (instance_role, Forum.first).
    let answer = subject
        .only_has_role(
            &instance_role,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    assert!(answer);
    // l.113: (instance_role, :any).
    let answer = subject
        .only_has_role(&instance_role, ResourceFilter::Any)
        .await?;
    assert!(answer);
    Ok(())
}

/// The instance holder's false rows: the global and class asks, the
/// unlinked global row, and the five other-instance blocks of
/// l.129-163 (same resource other name, other instance same name,
/// other type same name, same type other name, other type other name)
/// all fail - except the `:any` asks over the holder's own name at
/// l.141 and l.148 (`shared_examples_for_only_has_role.rb:116-163`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, row inserts,
/// provisioning, or asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_only_false_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    // l.125: role_class.create(:name => "global_role") - unlinked.
    backend
        .create_role_row(RoleRecord::global("global_role"))
        .await?;
    let instance_role = RoleName::from("instance_role");
    let global_role = RoleName::from("global_role");
    let subject = backend.subject("zombie");
    subject
        .add_role(
            &instance_role,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    // l.117: an instance-scoped role never answers the global ask.
    let answer = subject
        .only_has_role(&instance_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    // l.121: nor the class ask.
    let answer = subject
        .only_has_role(&instance_role, ResourceFilter::Class("Forum"))
        .await?;
    assert!(!answer);
    // l.126: the unlinked global row never answers.
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    instance_other_role_blocks(&mut backend).await?;
    Ok(())
}

/// The five "with another instance scoped role" blocks (l.129-163).
/// Size split only; every row keeps its gem-line citation (see the
/// module docs).
#[maybe_async::maybe_async]
async fn instance_other_role_blocks<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let group_last = backend.resource(FixtureResource::GroupLast);
    // l.131: role_class.create(:name => "another_instance_role", resource: Forum.first).
    backend
        .create_role_row(RoleRecord::for_instance(
            "another_instance_role",
            "Forum",
            forum_first.resource_id.clone(),
        ))
        .await?;
    // l.138: role_class.create(:name => "moderator", resource: Forum.last).
    backend
        .create_role_row(RoleRecord::for_instance(
            "moderator",
            "Forum",
            forum_last.resource_id.clone(),
        ))
        .await?;
    // l.145: role_class.create(:name => "moderator", resource: Group.last).
    backend
        .create_role_row(RoleRecord::for_instance(
            "moderator",
            "Group",
            group_last.resource_id.clone(),
        ))
        .await?;
    // l.152: role_class.create(:name => "another_instance_role", resource: Forum.last).
    backend
        .create_role_row(RoleRecord::for_instance(
            "another_instance_role",
            "Forum",
            forum_last.resource_id.clone(),
        ))
        .await?;
    // l.159: role_class.create(:name => "another_instance_role", resource: Group.first).
    backend
        .create_role_row(RoleRecord::for_instance(
            "another_instance_role",
            "Group",
            group_first.resource_id.clone(),
        ))
        .await?;
    let instance_role = RoleName::from("instance_role");
    let another_instance_role = RoleName::from("another_instance_role");
    let subject = backend.subject("zombie");
    // l.133: same resource, different name: the exact-instance ask misses.
    let answer = subject
        .only_has_role(
            &another_instance_role,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.134: its :any shape misses too.
    let answer = subject
        .only_has_role(&another_instance_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    // l.140: another instance of the same type with the same name: the Forum.last ask misses.
    let answer = subject
        .only_has_role(
            &instance_role,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.141: but :any includes the zombie's own instance row - the gem's truthy row.
    let answer = subject
        .only_has_role(&instance_role, ResourceFilter::Any)
        .await?;
    assert!(answer);
    // l.147: another type with the same name: the Group.last ask misses.
    let answer = subject
        .only_has_role(
            &instance_role,
            ResourceFilter::Instance("Group", &group_last.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.148: but :any includes the zombie's own instance row - the gem's truthy row.
    let answer = subject
        .only_has_role(&instance_role, ResourceFilter::Any)
        .await?;
    assert!(answer);
    // l.154: same type, another name at another instance: misses.
    let answer = subject
        .only_has_role(
            &another_instance_role,
            ResourceFilter::Instance("Forum", &forum_last.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.155: its :any shape misses too.
    let answer = subject
        .only_has_role(&another_instance_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    // l.161: another type, another name: misses.
    let answer = subject
        .only_has_role(
            &another_instance_role,
            ResourceFilter::Instance("Group", &group_first.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.162: its :any shape misses too.
    let answer = subject
        .only_has_role(&another_instance_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    Ok(())
}

/// The instance multiple-roles veto: a second instance role flips the
/// exact-instance and `:any` asks false although `instance_role` is
/// still held (`shared_examples_for_only_has_role.rb:166-171`).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, provisioning, or
/// asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_only_multiple_roles_vetoes<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let instance_role = RoleName::from("instance_role");
    let multiple_instance_roles = RoleName::from("multiple_instance_roles");
    let subject = backend.subject("zombie");
    subject
        .add_role(
            &instance_role,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    // l.167: before { subject.add_role "multiple_instance_roles", Forum.first }.
    subject
        .add_role(
            &multiple_instance_roles,
            ResourceRef::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    // l.169: two roles held: the exact-instance ask is vetoed.
    let answer = subject
        .only_has_role(
            &instance_role,
            ResourceFilter::Instance("Forum", &forum_first.resource_id),
        )
        .await?;
    assert!(!answer);
    // l.170: the :any ask too.
    let answer = subject
        .only_has_role(&instance_role, ResourceFilter::Any)
        .await?;
    assert!(!answer);
    Ok(())
}

/// The role.rb:77-79 observation pin: `only_has_role?` is
/// `has_role?(name, resource) && roles.count == 1`, so the count is
/// the veto half of the predicate. With exactly one linked role the
/// count reads 1 and the ask is true; after a second grant the count
/// reads 2 and every only-ask turns false although `has_role` still
/// answers true (USER-05).
///
/// Self-gate: default binding only (see the module docs).
///
/// # Errors
///
/// Propagates backend failures from the build, reset, provisioning, or
/// asks.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn only_has_role_observes_single_role_count<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    if backend.subject("admin").rolify_config().strict() {
        return Ok(());
    }
    backend.reset_roles().await?;
    let global_role = RoleName::from("global_role");
    let second_role = RoleName::from("second_role");
    let subject = backend.subject("zombie");
    subject.add_role(&global_role, ResourceRef::Global).await?;
    // role.rb:78: roles.count == 1 while the ask is true.
    let names = subject.roles_name().await?;
    assert_eq!(names.len(), 1);
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Global)
        .await?;
    assert!(answer);
    // Second grant: the count vetoes although has_role still answers true.
    subject.add_role(&second_role, ResourceRef::Global).await?;
    let names = subject.roles_name().await?;
    assert_eq!(names.len(), 2);
    let answer = subject
        .only_has_role(&global_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    let answer = subject
        .only_has_role(&second_role, ResourceFilter::Global)
        .await?;
    assert!(!answer);
    Ok(())
}

/// Expand the 10 `only_has_role` wrappers for one backend. Wrapper
/// names carry the `only_has_role_` prefix so the 12 binding modules
/// never collide.
#[macro_export]
macro_rules! parity_only_has_role_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_global_only_true_rows() {
            $crate::suite::only_has_role::global_only_true_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_global_only_false_rows() {
            $crate::suite::only_has_role::global_only_false_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_global_only_multiple_roles_vetoes() {
            $crate::suite::only_has_role::global_only_multiple_roles_vetoes::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_class_only_true_rows() {
            $crate::suite::only_has_role::class_only_true_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_class_only_false_rows() {
            $crate::suite::only_has_role::class_only_false_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_class_only_multiple_roles_vetoes() {
            $crate::suite::only_has_role::class_only_multiple_roles_vetoes::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_instance_only_true_rows() {
            $crate::suite::only_has_role::instance_only_true_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_instance_only_false_rows() {
            $crate::suite::only_has_role::instance_only_false_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_instance_only_multiple_roles_vetoes() {
            $crate::suite::only_has_role::instance_only_multiple_roles_vetoes::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn only_has_role_only_has_role_observes_single_role_count() {
            $crate::suite::only_has_role::only_has_role_observes_single_role_count::<$backend>()
                .await
                .unwrap();
        }
    };
}
