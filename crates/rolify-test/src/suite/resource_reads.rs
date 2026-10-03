//! `resource_reads` cases: the read side of `resource_spec.rb`
//! (l.284-529, the `.find_role` matrix, instance `#roles`, and instance
//! `#applied_roles`) over the 02-05 resource statics (RSRC-03/04/05/06,
//! Suite-Extension binding module 11).
//!
//! ## Source map
//!
//! * Role table (`resource_spec.rb:20-28`): nine `let!` grants loaded
//!   by [`load_resource_table`] through the seated subjects'
//!   [`RolifyUser::add_role`] (the public path, callbacks included),
//!   exactly as the spec's blocks do: admin holds forum@Forum.first,
//!   godfather@Forum, group@Group.last, grouper@Group.first, and
//!   owner@Company; tourist holds forum@Forum.last plus the sneaky
//!   group@Forum.first; god holds captain@Team("1") and
//!   player@Team("2").
//! * `.find_role` matrix (l.284-479): the `:any` rows collapse into the
//!   typed `None` options (D-16), so `find_roles(None, None)` IS the
//!   gem's `find_roles(:any, :any)` cell; the collapse is noted once
//!   in [`find_roles_any_matrix`].
//! * Instance `#roles` (l.481-496) and `#applied_roles` (l.515-529).
//! * Team string-PK family (l.144-150): the read-side pin that the
//!   `team_code` primary key flows through the catalog query.
//!
//! ## D-19 deferral: the dependent-destroy row
//!
//! `resource_spec.rb:498-511` (deleting a Group instance removes the
//! bound role rows) is a foreign-key/migration concern owned by Phase 3
//! (RSRC-05's FK decision). It is NOT ported here; the deferral is a
//! D-19 parity-matrix entry.
//!
//! ## Set-compare convention (D-04)
//!
//! The gem's `~>` and `match_array` matchers are order-free set
//! compares. Every row assertion here goes through
//! [`assert_record_set`]: same row count, every expected row present.
//! Order is never asserted.
//!
//! [`RolifyUser::add_role`]: rolify_core::user::RolifyUser::add_role

use rolify_core::resource::{Resource, ResourceRef};
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::user::RolifyUser;

use crate::backend::TestBackend;
use crate::fixtures::{
    ContextRoleSpec, ContextScope, FixtureResource, Forum, Group, Organization, Team,
};

/// Assert two row collections as sets (D-04): same length, every
/// expected row present, order ignored. Every expected set in this
/// module has distinct rows, so length-plus-membership is exact set
/// equality.
///
/// # Panics
///
/// Panics with both collections on any count or membership mismatch.
fn assert_record_set(found: &[RoleRecord], expected: &[RoleRecord]) {
    assert_eq!(
        found.len(),
        expected.len(),
        "row count mismatch; found {found:?}, expected {expected:?}"
    );
    for record in expected {
        assert!(
            found.contains(record),
            "missing expected row {record:?} in {found:?}"
        );
    }
}

/// One grant of the `resource_spec.rb:20-28` role table, resolved to
/// owned data so provisioning never holds a backend borrow across a
/// mutable subject borrow.
struct ResolvedGrant {
    /// The fixture login that adds the role.
    login: &'static str,
    /// The literal role name.
    name: RoleName,
    /// The write-scope type column (`None` = global; the table never
    /// uses a global grant).
    type_name: Option<String>,
    /// The write-scope instance id (`None` = class grant).
    resource_id: Option<ResourceId>,
}

/// The role table of `resource_spec.rb:20-28` as grant specs: login,
/// literal name, scope. The sneaky row (l.25) carries the `group` NAME
/// over the FORUM resource; the type string is what the store sees.
///
/// Gem subject labels map to fixture logins: the spec's `tourist` is
/// `User.last` (login "zombie", `data.rb:5-8` creation order) and its
/// `captain` is `User.where(login: "god")`; the labels never appear as
/// fixture logins themselves.
fn resource_table() -> Vec<(&'static str, ContextRoleSpec)> {
    fn grant(
        login: &'static str,
        name: &'static str,
        scope: ContextScope,
    ) -> (&'static str, ContextRoleSpec) {
        (login, ContextRoleSpec { name, scope })
    }
    vec![
        grant(
            "admin",
            "forum",
            ContextScope::Instance("Forum", FixtureResource::ForumFirst),
        ),
        grant("admin", "godfather", ContextScope::Class("Forum")),
        grant(
            "admin",
            "group",
            ContextScope::Instance("Group", FixtureResource::GroupLast),
        ),
        grant(
            "admin",
            "grouper",
            ContextScope::Instance("Group", FixtureResource::GroupFirst),
        ),
        grant(
            "admin",
            "owner",
            ContextScope::Instance("Company", FixtureResource::Company),
        ),
        grant(
            "zombie",
            "forum",
            ContextScope::Instance("Forum", FixtureResource::ForumLast),
        ),
        grant(
            "zombie",
            "group",
            ContextScope::Instance("Forum", FixtureResource::ForumFirst),
        ),
        grant(
            "god",
            "captain",
            ContextScope::Instance("Team", FixtureResource::TeamFirst),
        ),
        grant(
            "god",
            "player",
            ContextScope::Instance("Team", FixtureResource::TeamLast),
        ),
    ]
}

/// Resolve one table grant through the backend's fixture registry to
/// owned `(name, type, id)` data (the same discipline as the private
/// `resolve_scope` in `backend.rs`: resolve everything BEFORE any
/// mutable subject borrow).
fn resolve_grant<B: TestBackend>(
    backend: &B,
    spec: &ContextRoleSpec,
) -> (RoleName, Option<String>, Option<ResourceId>) {
    match spec.scope {
        ContextScope::Global => (RoleName::from(spec.name), None, None),
        ContextScope::Class(type_name) => {
            (RoleName::from(spec.name), Some(type_name.to_owned()), None)
        }
        ContextScope::Instance(type_name, which) => {
            let key = backend.resource(which);
            (
                RoleName::from(spec.name),
                Some(type_name.to_owned()),
                Some(key.resource_id),
            )
        }
    }
}

/// Rebuild the write scope over resolved owned data (the counterpart of
/// the private `scope_ref` in `backend.rs`, mirrored here because this
/// module owns its provisioning table).
fn write_scope<'scope>(
    type_name: Option<&'scope str>,
    resource_id: Option<&'scope ResourceId>,
) -> ResourceRef<'scope> {
    match type_name {
        None => ResourceRef::Global,
        Some(type_name) => match resource_id {
            None => ResourceRef::Class(type_name),
            Some(resource_id) => ResourceRef::Instance(type_name, resource_id),
        },
    }
}

/// Load the `resource_spec.rb:20-28` role table: reset, then grant each
/// row through the seated subject's `add_role` (the public path,
/// callbacks included), exactly as the spec's `let!` blocks do.
///
/// # Errors
///
/// Propagates backend failures from the reset or any grant.
#[maybe_async::maybe_async]
pub async fn load_resource_table<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    backend.reset_roles().await?;
    let mut resolved: Vec<ResolvedGrant> = Vec::new();
    for (login, spec) in resource_table() {
        let (name, type_name, resource_id) = resolve_grant(backend, &spec);
        resolved.push(ResolvedGrant {
            login,
            name,
            type_name,
            resource_id,
        });
    }
    for grant in &resolved {
        let subject = backend.subject(grant.login);
        subject
            .add_role(
                &grant.name,
                write_scope(grant.type_name.as_deref(), grant.resource_id.as_ref()),
            )
            .await?;
    }
    Ok(())
}

/// The `Forum.first` fixture for the instance statics. The display
/// `name` is behaviorally inert (fixtures.rs, TEST-04); only
/// `resource_id` reaches the store.
fn forum_instance<B: TestBackend>(backend: &B, which: FixtureResource) -> Forum {
    Forum {
        id: backend.resource(which).resource_id,
        name: "resource_reads display placeholder".to_owned(),
    }
}

/// The `Group` fixture for the instance statics (`GroupFirst` or
/// `GroupLast`).
fn group_instance<B: TestBackend>(backend: &B, which: FixtureResource) -> Group {
    Group {
        id: backend.resource(which).resource_id,
        name: "resource_reads display placeholder".to_owned(),
    }
}

/// The `Team` fixture for the instance statics: the string `team_code`
/// primary key (the l.144-150 string-PK pin).
fn team_instance<B: TestBackend>(backend: &B, which: FixtureResource) -> Team {
    Team {
        team_code: backend.resource(which).resource_id.as_str().to_owned(),
        name: "resource_reads display placeholder".to_owned(),
    }
}

/// `.find_roles` with neither a name nor a user
/// (`resource_spec.rb:286-319`): every row bound to the class family,
/// any name, any holder.
///
/// The gem's `:any` parity rows (l.299-307, `find_roles(:any, :any)`)
/// are the SAME call in the port: the typed `None` options ARE `:any`
/// (D-16), so one assertion per class carries both cells.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn find_roles_without_name_or_user<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;

    // l.292-297: all four Forum-bound rows, godfather's class row
    // included; the Group-bound group row is out of the family.
    let forum_rows = Forum::find_roles(backend.engine(), None, None).await?;
    assert_record_set(
        &forum_rows,
        &[
            RoleRecord::for_instance("forum", "Forum", 1_i64),
            RoleRecord::for_class("godfather", "Forum"),
            RoleRecord::for_instance("forum", "Forum", 3_i64),
            RoleRecord::for_instance("group", "Forum", 1_i64),
        ],
    );

    // l.313-319: the Group mirror sees its two rows and no Forum row.
    let group_rows = Group::find_roles(backend.engine(), None, None).await?;
    assert_record_set(
        &group_rows,
        &[
            RoleRecord::for_instance("group", "Group", 2_i64),
            RoleRecord::for_instance("grouper", "Group", 1_i64),
        ],
    );
    Ok(())
}

/// `.find_roles` with a name but no user (`resource_spec.rb:333-344`
/// Forum rows, l.372-378 Group rows): the name filter narrows the
/// family read.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn find_roles_with_name<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;

    // l.338-344: forum-named Forum rows only (the sneaky row carries the
    // group name, godfather its own).
    let forum_name = RoleName::from("forum");
    let forum_rows = Forum::find_roles(backend.engine(), Some(&forum_name), None).await?;
    assert_record_set(
        &forum_rows,
        &[
            RoleRecord::for_instance("forum", "Forum", 1_i64),
            RoleRecord::for_instance("forum", "Forum", 3_i64),
        ],
    );

    // l.372-378: group-named Group rows only (the sneaky row is
    // Forum-typed, invisible to the Group family).
    let group_name = RoleName::from("group");
    let group_rows = Group::find_roles(backend.engine(), Some(&group_name), None).await?;
    assert_record_set(
        &group_rows,
        &[RoleRecord::for_instance("group", "Group", 2_i64)],
    );
    Ok(())
}

/// `.find_roles` with a name and a user (`resource_spec.rb:347-354`):
/// the holder join (the `user.roles` branch, `resource_adapter.rb:7`)
/// plus the name filter.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn find_roles_with_user<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let admin_id = backend
        .holder_id("admin")
        .expect("the admin fixture login is always seated");

    // l.349-353: exactly the admin's forum-named Forum row; the admin's
    // other Forum-family row (godfather) fails the name filter.
    let forum_name = RoleName::from("forum");
    let forum_rows =
        Forum::find_roles(backend.engine(), Some(&forum_name), Some(&admin_id)).await?;
    assert_record_set(
        &forum_rows,
        &[RoleRecord::for_instance("forum", "Forum", 1_i64)],
    );
    Ok(())
}

/// The remaining `:any` matrix cells (`resource_spec.rb:357-470`):
/// name-`Some` + user-`None` (l.391-399) and name-`None` + user-`Some`
/// (l.418-425). The both-`None` cells live in
/// [`find_roles_without_name_or_user`]; the gem's `(:any, :any)` cell
/// is the same typed call (D-16, asserted there once).
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn find_roles_any_matrix<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;

    // l.391-399: group-named Group rows with no user filter.
    let group_name = RoleName::from("group");
    let named_group_rows = Group::find_roles(backend.engine(), Some(&group_name), None).await?;
    assert_record_set(
        &named_group_rows,
        &[RoleRecord::for_instance("group", "Group", 2_i64)],
    );

    // l.418-425: the admin's Forum-family rows with the holder join:
    // forum plus godfather, never the tourist's rows.
    let admin_id = backend
        .holder_id("admin")
        .expect("the admin fixture login is always seated");
    let holder_rows = Forum::find_roles(backend.engine(), None, Some(&admin_id)).await?;
    assert_record_set(
        &holder_rows,
        &[
            RoleRecord::for_instance("forum", "Forum", 1_i64),
            RoleRecord::for_class("godfather", "Forum"),
        ],
    );
    Ok(())
}
/// The STI row (`resource_spec.rb:473-478`, RSRC-06): the parent-class
/// finder reaches the descendant-typed row through
/// `descendant_types()`; the user-side ladder NEVER expands
/// descendants (the Phase-1 pin).
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn find_roles_sti_matches_children<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let admin_id = backend
        .holder_id("admin")
        .expect("the admin fixture login is always seated");

    // l.475-477: Organization::find_roles(:owner, admin) sees the
    // Company-bound row through the family ["Organization", "Company"].
    let owner_name = RoleName::from("owner");
    let organization_rows =
        Organization::find_roles(backend.engine(), Some(&owner_name), Some(&admin_id)).await?;
    assert_record_set(
        &organization_rows,
        &[RoleRecord::for_instance("owner", "Company", 1_i64)],
    );
    Ok(())
}

/// `.applied_roles` with both children flags (`resource.rb:36-38`,
/// `resource_adapter.rb:32-38`): class-scoped rows of the family
/// (`children = true`) or of exactly `Self` (`children = false`).
///
/// The fixture table (l.20-28) carries NO class-scoped
/// Organization-family row (the owner row is instance-bound), so both
/// flags yield the empty set there; Forum's only class row is the
/// godfather, and Forum has no descendants, so both flags agree.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn applied_roles_class_level_children_flag<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;

    let forum_with_children = Forum::applied_roles(backend.engine(), true).await?;
    assert_record_set(
        &forum_with_children,
        &[RoleRecord::for_class("godfather", "Forum")],
    );
    let forum_own_type = Forum::applied_roles(backend.engine(), false).await?;
    assert_record_set(
        &forum_own_type,
        &[RoleRecord::for_class("godfather", "Forum")],
    );

    let organization_with_children = Organization::applied_roles(backend.engine(), true).await?;
    assert_record_set(&organization_with_children, &[]);
    let organization_own_type = Organization::applied_roles(backend.engine(), false).await?;
    assert_record_set(&organization_own_type, &[]);
    Ok(())
}

/// Instance `#roles` (`resource_spec.rb:481-496`, RSRC-05): ALL rows
/// bound to the instance regardless of holder (l.488 pairs the admin's
/// forum row with the tourist's sneaky row), class rows excluded.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_roles_are_instance_bound_only<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;

    // l.487-490: both holders' rows on Forum.first, godfather's class
    // row excluded.
    let forum_first = forum_instance(&backend, FixtureResource::ForumFirst);
    let forum_rows = Forum::roles_of_instance(backend.engine(), &forum_first).await?;
    assert_record_set(
        &forum_rows,
        &[
            RoleRecord::for_instance("forum", "Forum", 1_i64),
            RoleRecord::for_instance("group", "Forum", 1_i64),
        ],
    );

    // l.492-496: Group.last sees exactly its own bound row.
    let group_last = group_instance(&backend, FixtureResource::GroupLast);
    let group_rows = Group::roles_of_instance(backend.engine(), &group_last).await?;
    assert_record_set(
        &group_rows,
        &[RoleRecord::for_instance("group", "Group", 2_i64)],
    );
    Ok(())
}

/// Instance `#applied_roles` (`resource_spec.rb:515-529`): the
/// instance-bound rows UNION the class-scoped rows of the family
/// (`self.roles + self.class.applied_roles(true)`,
/// `resource.rb:44-47`).
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn instance_applied_roles_compose<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;

    // l.517-521: forum + sneaky + godfather (the instance rows plus the
    // Forum class row); tourist's forum@Forum.last and the Group rows
    // stay out.
    let forum_first = forum_instance(&backend, FixtureResource::ForumFirst);
    let forum_rows = Forum::applied_roles_of_instance(backend.engine(), &forum_first).await?;
    assert_record_set(
        &forum_rows,
        &[
            RoleRecord::for_instance("forum", "Forum", 1_i64),
            RoleRecord::for_instance("group", "Forum", 1_i64),
            RoleRecord::for_class("godfather", "Forum"),
        ],
    );

    // l.523-527: no Group class row exists, so Group.last composes to
    // its own instance row alone.
    let group_last = group_instance(&backend, FixtureResource::GroupLast);
    let group_rows = Group::applied_roles_of_instance(backend.engine(), &group_last).await?;
    assert_record_set(
        &group_rows,
        &[RoleRecord::for_instance("group", "Group", 2_i64)],
    );
    Ok(())
}

/// The Team string-PK row (the `resource_spec.rb:144-150` family):
/// reads over a model whose primary key is the string `team_code`
/// column, not an integer id. The catalog query must carry the
/// stringified PK untouched.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn team_string_pk_reads<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;

    // The captain grant sits on Team("1") only; player@Team("2") is a
    // different instance.
    let team_first = team_instance(&backend, FixtureResource::TeamFirst);
    let team_rows = Team::roles_of_instance(backend.engine(), &team_first).await?;
    assert_record_set(
        &team_rows,
        &[RoleRecord::for_instance("captain", "Team", "1")],
    );
    Ok(())
}

/// Expand the nine `resource_reads` wrappers for one backend. Wrapper
/// names carry the `resource_reads_` prefix so the 12 binding modules
/// never collide.
#[macro_export]
macro_rules! parity_resource_reads_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_find_roles_without_name_or_user() {
            $crate::suite::resource_reads::find_roles_without_name_or_user::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_find_roles_with_name() {
            $crate::suite::resource_reads::find_roles_with_name::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_find_roles_with_user() {
            $crate::suite::resource_reads::find_roles_with_user::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_find_roles_any_matrix() {
            $crate::suite::resource_reads::find_roles_any_matrix::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_find_roles_sti_matches_children() {
            $crate::suite::resource_reads::find_roles_sti_matches_children::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_applied_roles_class_level_children_flag() {
            $crate::suite::resource_reads::applied_roles_class_level_children_flag::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_instance_roles_are_instance_bound_only() {
            $crate::suite::resource_reads::instance_roles_are_instance_bound_only::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_instance_applied_roles_compose() {
            $crate::suite::resource_reads::instance_applied_roles_compose::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_reads_team_string_pk_reads() {
            $crate::suite::resource_reads::team_string_pk_reads::<$backend>()
                .await
                .unwrap();
        }
    };
}
