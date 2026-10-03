//! `resource_queries` cases: the query side of `resource_spec.rb`
//! (l.30-158, the `.find_multiple_as`/`find_as` rows; l.161-282, the
//! `.except_multiple_as`/`except_as` rows) over the 02-08
//! [`Resource::with_role`]/[`Resource::without_role`] statics
//! (RSRC-01/02/06, Suite-Extension binding module 12).
//!
//! ## Source map
//!
//! * Role table (`resource_spec.rb:20-28`): the SAME nine `let!`
//!   grants 02-05 pins, loaded through [`load_resource_table`] (one
//!   provisioning helper, both modules consume it): admin holds
//!   forum@Forum.first, godfather@Forum, group@Group.last,
//!   grouper@Group.first, owner@Company; tourist (the spec's
//!   `User.last`, login "zombie") holds forum@Forum.last plus the
//!   sneaky group@Forum.first; god (the spec's captain, login "god")
//!   holds captain@Team("1") and player@Team("2").
//! * The query rows l.30-158 map to the `with_role_*` cases, the
//!   complement rows l.161-282 to the `without_role_*` cases, and
//!   [`global_exclusion_asymmetry_labeled`] adds the outline's
//!   explicit labeled pin. The `respond_to(:find_roles)` presence
//!   rows (l.33-34, l.164-165) collapse into compile-time presence:
//!   calling the statics below IS the pin (the `finders.rs`
//!   precedent).
//!
//! ## Alias collapse (D-19)
//!
//! The gem spells one query four ways
//! (`with_role`/`with_roles`/`find_as`/`find_multiple_as`, mirrored
//! by `without_role`/`without_roles`/`except_as`/`except_multiple_as`).
//! The port keeps `with_role`/`without_role` only (REQUIREMENTS
//! out-of-scope: one concept, one name), and the `&[RoleName]`
//! slice collapses the String-vs-Array branches: a one-element slice
//! IS the `find_as(:name)` row, the multi-element slice IS the
//! `find_multiple_as([:names])` row.
//!
//! ## Universe convention (D-18)
//!
//! Every `without_role` case passes the FULL fixture universe of the
//! queried type - the gem's `Forum.all` equivalent (three Forum keys,
//! both Group keys, both Team `team_code` keys, the
//! Organization+Company STI pair). The gem gets that universe for
//! free through `ActiveRecord` (`all_except`,
//! resource_adapter.rb:40-43); the port takes it caller-side and
//! subtracts - the recorded D-18 divergence (a D-19 parity-matrix
//! entry for the 02-09 harvest).
//!
//! ## Set-compare contract (D-04)
//!
//! The gem's `=~`, `should_not =~`, `include`, and `be_empty`
//! matchers are all set semantics over unordered relations; every row
//! assertion goes through [`assert_key_set`] (the
//! `finders::assert_id_set` idiom lifted to `(type, id)` pairs).
//! Weak `should_not =~` complement rows assert the EXACT complement
//! wherever the fixture universe makes it deterministic (the
//! `finders.rs` strengthen-where-deterministic precedent), so the
//! pinned rows here are stronger than the gem's own matchers.
//!
//! ## NOT-PORTED rows (D-19 entries)
//!
//! * l.48-52 and l.179-183 (`should be able to modify the resource`):
//!   save-through-ActiveRecord persistence noise, meaningless over an
//!   in-memory store.
//! * l.63-66 (`Group.last.subgroups.find_as(:group) =~ []`):
//!   `find_as` over a CONSUMER-DOMAIN relation (`subgroups`) is
//!   exactly the shape D-18 excludes - the store never traverses
//!   consumer associations; `with_role` queries the role catalog
//!   only, never consumer relations.
//!
//! ## The asymmetry pin
//!
//! [`global_exclusion_asymmetry_labeled`] is THE explicit
//! global-exclusion pin (resource_adapter.rb:21-23): a GLOBAL row
//! never surfaces resources (its NULL `resource_type` cannot match
//! the family join) while a CLASS-scoped row covers every instance
//! key of the family. The l.45/l.95/l.177 rows pin the same
//! asymmetry implicitly through the fixture's godfather class row.
//!
//! [`Resource::with_role`]: rolify_core::resource::Resource::with_role
//! [`Resource::without_role`]: rolify_core::resource::Resource::without_role
//! [`load_resource_table`]: crate::suite::resource_reads::load_resource_table

use rolify_core::resource::{Resource, ResourceRef};
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::ResourceKey;
use rolify_core::user::RolifyUser;

use crate::backend::TestBackend;
use crate::fixtures::{FixtureResource, Forum, Group, Organization, Team};
use crate::suite::resource_reads::load_resource_table;

/// Assert two resource-key lists as SETS (D-04): sort plus dedup the
/// `(type, id)` pairs on both sides, then equality. The gem's `=~`,
/// `should_not =~`, `include`, and `be_empty` rows are all set
/// semantics over unordered relations; this is the single comparison
/// helper every `resource_queries` case consumes. No ordering
/// guarantee is implemented or tested.
///
/// # Panics
///
/// Panics with both collections on any set mismatch.
pub fn assert_key_set(actual: &[ResourceKey], expected: &[ResourceKey]) {
    let mut sorted_actual: Vec<(&str, &str)> = actual
        .iter()
        .map(|key| (key.resource_type.as_str(), key.resource_id.as_str()))
        .collect();
    sorted_actual.sort_unstable();
    sorted_actual.dedup();
    let mut sorted_expected: Vec<(&str, &str)> = expected
        .iter()
        .map(|key| (key.resource_type.as_str(), key.resource_id.as_str()))
        .collect();
    sorted_expected.sort_unstable();
    sorted_expected.dedup();
    assert_eq!(
        sorted_actual, sorted_expected,
        "resource query results compare as (type, id) sets (D-04); actual {actual:?} expected {expected:?}"
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

/// The full `Forum.all` universe (three keys, `data.rb:17-19`) - the
/// caller-supplied stand-in for the gem's free `Forum.all`
/// (`all_except`, resource_adapter.rb:40-43, D-18).
fn all_forum_keys<B: TestBackend>(backend: &B) -> Vec<ResourceKey> {
    vec![
        backend.resource(FixtureResource::ForumFirst),
        backend.resource(FixtureResource::ForumSecond),
        backend.resource(FixtureResource::ForumLast),
    ]
}

/// The full `Group.all` universe (two keys, `data.rb:21-22`).
fn all_group_keys<B: TestBackend>(backend: &B) -> Vec<ResourceKey> {
    vec![
        backend.resource(FixtureResource::GroupFirst),
        backend.resource(FixtureResource::GroupLast),
    ]
}

/// The full `Team.all` universe (two string `team_code` keys,
/// `data.rb:24-25`).
fn all_team_keys<B: TestBackend>(backend: &B) -> Vec<ResourceKey> {
    vec![
        backend.resource(FixtureResource::TeamFirst),
        backend.resource(FixtureResource::TeamLast),
    ]
}

/// The full `Organization.all` universe under STI (the gem's
/// `organizations` table holds both rows, `data.rb:27-28`): the
/// Organization base key plus the Company descendant key.
fn all_organization_keys<B: TestBackend>(backend: &B) -> Vec<ResourceKey> {
    vec![
        backend.resource(FixtureResource::Organization),
        backend.resource(FixtureResource::Company),
    ]
}

/// The name-only query rows (`resource_spec.rb:36-61`): the
/// `find_as(:name)` cells, one per queried class.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn with_role_name_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_second = backend.resource(FixtureResource::ForumSecond);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_last = backend.resource(FixtureResource::GroupLast);

    // l.41: the forum-named Forum rows sit on Forum.first (admin)
    // and Forum.last (tourist); the godfather class row is a
    // different name and the middle forum holds no forum row.
    let found = Forum::with_role(backend.engine(), &[RoleName::from("forum")], None).await?;
    assert_key_set(&found, &[forum_first.clone(), forum_last.clone()]);

    // l.45: the godfather CLASS row covers every instance key of the
    // family - all three forums (the implicit global-exclusion pin:
    // a class row surfaces every instance; a global row never
    // would).
    let found = Forum::with_role(backend.engine(), &[RoleName::from("godfather")], None).await?;
    assert_key_set(&found, &[forum_first, forum_second, forum_last]);

    // l.59: only Group.last carries a group-named Group row (the
    // sneaky row is Forum-typed, invisible to the Group family).
    let found = Group::with_role(backend.engine(), &[RoleName::from("group")], None).await?;
    assert_key_set(&found, &[group_last]);
    Ok(())
}

/// The multi-name query row (`resource_spec.rb:72-79`): the
/// `find_multiple_as([:group, :grouper])` cell - the Array branch the
/// `&[RoleName]` slice collapses into the same static (D-19).
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn with_role_multi_name_row<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let group_last = backend.resource(FixtureResource::GroupLast);

    // l.77: group matches Group.last, grouper matches Group.first -
    // the deduped union covers both keys.
    let found = Group::with_role(
        backend.engine(),
        &[RoleName::from("group"), RoleName::from("grouper")],
        None,
    )
    .await?;
    assert_key_set(&found, &[group_first, group_last]);
    Ok(())
}

/// The user-filtered query rows (`resource_spec.rb:82-121`): the
/// `find_as(:name, user)` cells - the `in` filter
/// (resource_adapter.rb:27-30) narrowing the name-matched candidates
/// to the keys the user's rows cover.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn with_role_user_filter_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let admin = fixture_holder(&backend, "admin");
    let tourist = fixture_holder(&backend, "zombie");
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_second = backend.resource(FixtureResource::ForumSecond);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_last = backend.resource(FixtureResource::GroupLast);

    // l.87: admin's forum row is bound to Forum.first.
    let found =
        Forum::with_role(backend.engine(), &[RoleName::from("forum")], Some(&admin)).await?;
    assert_key_set(&found, core::slice::from_ref(&forum_first));

    // l.91: tourist's forum row is bound to Forum.last.
    let found =
        Forum::with_role(backend.engine(), &[RoleName::from("forum")], Some(&tourist)).await?;
    assert_key_set(&found, core::slice::from_ref(&forum_last));

    // l.95: admin's godfather CLASS row covers every Forum instance.
    let found = Forum::with_role(
        backend.engine(),
        &[RoleName::from("godfather")],
        Some(&admin),
    )
    .await?;
    assert_key_set(
        &found,
        &[
            forum_first.clone(),
            forum_second.clone(),
            forum_last.clone(),
        ],
    );

    // l.99: tourist holds no godfather row.
    let found = Forum::with_role(
        backend.engine(),
        &[RoleName::from("godfather")],
        Some(&tourist),
    )
    .await?;
    assert!(found.is_empty());

    // l.103 + l.107: the sneaky group-named row binds the tourist to
    // Forum.first - exactly that key, never Forum.last.
    let found =
        Forum::with_role(backend.engine(), &[RoleName::from("group")], Some(&tourist)).await?;
    assert_key_set(&found, &[forum_first]);

    // l.115-119: admin's group row is bound to Group.last, so
    // Group.first stays out.
    let found =
        Group::with_role(backend.engine(), &[RoleName::from("group")], Some(&admin)).await?;
    assert_key_set(&found, &[group_last]);
    Ok(())
}

/// The multi-name user-filtered query rows (`resource_spec.rb:124-141`):
/// the `find_multiple_as([:names], user)` cells.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn with_role_multi_name_user_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let admin = fixture_holder(&backend, "admin");
    let tourist = fixture_holder(&backend, "zombie");
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let group_last = backend.resource(FixtureResource::GroupLast);

    // l.129: the tourist matches forum on Forum.last and group on
    // Forum.first - the union of both.
    let found = Forum::with_role(
        backend.engine(),
        &[RoleName::from("forum"), RoleName::from("group")],
        Some(&tourist),
    )
    .await?;
    assert_key_set(&found, &[forum_first, forum_last]);

    // l.138: the admin matches group on Group.last and grouper on
    // Group.first - the union of both.
    let found = Group::with_role(
        backend.engine(),
        &[RoleName::from("group"), RoleName::from("grouper")],
        Some(&admin),
    )
    .await?;
    assert_key_set(&found, &[group_first, group_last]);
    Ok(())
}

/// The string-PK query row (`resource_spec.rb:144-149`):
/// `find_multiple_as([:captain, :player], captain)` over a model
/// whose primary key is the string `team_code` column, not an
/// integer id - the keys must carry the stringified PK untouched.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn team_string_pk_query<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let god = fixture_holder(&backend, "god");
    let team_first = backend.resource(FixtureResource::TeamFirst);
    let team_last = backend.resource(FixtureResource::TeamLast);

    // l.148: captain rides Team("1"), player rides Team("2") - the
    // union returns both string-keyed teams.
    let found = Team::with_role(
        backend.engine(),
        &[RoleName::from("captain"), RoleName::from("player")],
        Some(&god),
    )
    .await?;
    assert_key_set(&found, &[team_first, team_last]);
    Ok(())
}

/// The STI query row (`resource_spec.rb:152-157`, RSRC-06): the
/// parent-class query runs over `descendant_types()` and finds the
/// descendant-bound row through it - `Organization::with_role` sees
/// the Company-bound owner row.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn sti_query_matches_children<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let admin = fixture_holder(&backend, "admin");
    let company = backend.resource(FixtureResource::Company);

    // l.155: the family ["Organization", "Company"] reaches the
    // Company-bound owner row through the parent-class ask.
    let found =
        Organization::with_role(backend.engine(), &[RoleName::from("owner")], Some(&admin)).await?;
    assert_key_set(&found, &[company]);
    Ok(())
}

/// The name-only complement rows (`resource_spec.rb:167-203`): the
/// `except_as(:name)` cells - the universe minus the `with_role`
/// matches. The weak `should_not =~` rows assert the EXACT complement
/// (the fixture universe makes it deterministic).
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn without_role_name_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let forum_second = backend.resource(FixtureResource::ForumSecond);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let universe = all_forum_keys(&backend);
    let group_universe = all_group_keys(&backend);

    // l.172: the with_role set for "forum" is {Forum.first,
    // Forum.last}, so the exact complement is the middle forum (the
    // gem's `should_not =~ [Forum.first, Forum.last]` row, satisfied
    // a fortiori by the exact set).
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("forum")],
        None,
        &universe,
    )
    .await?;
    assert_key_set(&found, core::slice::from_ref(&forum_second));

    // l.177: the godfather CLASS row covers every instance, so the
    // three-key universe subtracts to nothing.
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("godfather")],
        None,
        &universe,
    )
    .await?;
    assert!(found.is_empty());

    // l.190: the group row sits on Group.last, leaving Group.first.
    let found = Group::without_role(
        backend.engine(),
        &[RoleName::from("group")],
        None,
        &group_universe,
    )
    .await?;
    assert_key_set(&found, &[group_first]);

    // l.201: group plus grouper empty the Group universe.
    let found = Group::without_role(
        backend.engine(),
        &[RoleName::from("group"), RoleName::from("grouper")],
        None,
        &group_universe,
    )
    .await?;
    assert!(found.is_empty());
    Ok(())
}

/// The user-filtered complement rows (`resource_spec.rb:206-245`):
/// the `except_as(:name, user)` cells. The gem duplicates the
/// group/tourist row (l.227 and l.231) - one assertion carries both
/// cells.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
// The body is the 1:1 port of the gem's l.206-245 complement matrix;
// splitting it would break the per-row spec correspondence.
#[allow(clippy::too_many_lines)]
#[maybe_async::maybe_async]
pub async fn without_role_user_filter_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let admin = fixture_holder(&backend, "admin");
    let tourist = fixture_holder(&backend, "zombie");
    let forum_first = backend.resource(FixtureResource::ForumFirst);
    let forum_second = backend.resource(FixtureResource::ForumSecond);
    let forum_last = backend.resource(FixtureResource::ForumLast);
    let group_first = backend.resource(FixtureResource::GroupFirst);
    let universe = all_forum_keys(&backend);
    let group_universe = all_group_keys(&backend);

    // l.211: the admin's forum row binds Forum.first - the universe
    // keeps the other two keys.
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("forum")],
        Some(&admin),
        &universe,
    )
    .await?;
    assert_key_set(&found, &[forum_second.clone(), forum_last.clone()]);

    // l.215: the tourist's forum row binds Forum.last.
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("forum")],
        Some(&tourist),
        &universe,
    )
    .await?;
    assert_key_set(&found, &[forum_first.clone(), forum_second.clone()]);

    // l.219: the admin's godfather class row covers every instance -
    // the universe empties.
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("godfather")],
        Some(&admin),
        &universe,
    )
    .await?;
    assert!(found.is_empty());

    // l.223: the tourist holds no godfather row - the universe stays
    // intact.
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("godfather")],
        Some(&tourist),
        &universe,
    )
    .await?;
    assert_key_set(
        &found,
        &[
            forum_first.clone(),
            forum_second.clone(),
            forum_last.clone(),
        ],
    );

    // l.227 + l.231 (the gem's duplicated row): the sneaky group row
    // binds the tourist to Forum.first.
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("group")],
        Some(&tourist),
        &universe,
    )
    .await?;
    assert_key_set(&found, &[forum_second, forum_last]);

    // l.239-243: the admin's group row binds Group.last - Group.first
    // is the complement.
    let found = Group::without_role(
        backend.engine(),
        &[RoleName::from("group")],
        Some(&admin),
        &group_universe,
    )
    .await?;
    assert_key_set(&found, &[group_first]);
    Ok(())
}

/// The multi-name and special-model complement rows
/// (`resource_spec.rb:248-281`): the `except_multiple_as` cells plus
/// the Team string-PK and Organization STI complements.
///
/// # Errors
///
/// Propagates backend failures from the build, table load, or reads.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn without_role_multi_and_special_rows<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    load_resource_table(&mut backend).await?;
    let admin = fixture_holder(&backend, "admin");
    let tourist = fixture_holder(&backend, "zombie");
    let god = fixture_holder(&backend, "god");
    let forum_second = backend.resource(FixtureResource::ForumSecond);
    let team_last = backend.resource(FixtureResource::TeamLast);
    let organization = backend.resource(FixtureResource::Organization);
    let forum_universe = all_forum_keys(&backend);
    let group_universe = all_group_keys(&backend);
    let team_universe = all_team_keys(&backend);
    let organization_universe = all_organization_keys(&backend);

    // l.253: the tourist matches forum on Forum.last and group on
    // Forum.first - the middle forum remains.
    let found = Forum::without_role(
        backend.engine(),
        &[RoleName::from("forum"), RoleName::from("group")],
        Some(&tourist),
        &forum_universe,
    )
    .await?;
    assert_key_set(&found, &[forum_second]);

    // l.262: the admin holds group on Group.last and grouper on
    // Group.first - the Group universe empties.
    let found = Group::without_role(
        backend.engine(),
        &[RoleName::from("group"), RoleName::from("grouper")],
        Some(&admin),
        &group_universe,
    )
    .await?;
    assert!(found.is_empty());

    // l.272: the god's captain row rides Team("1") - Team("2") stays.
    let found = Team::without_role(
        backend.engine(),
        &[RoleName::from("captain")],
        Some(&god),
        &team_universe,
    )
    .await?;
    assert_key_set(&found, &[team_last]);

    // l.279: the owner row is Company-bound - the Organization base
    // key is the STI complement.
    let found = Organization::without_role(
        backend.engine(),
        &[RoleName::from("owner")],
        Some(&admin),
        &organization_universe,
    )
    .await?;
    assert_key_set(&found, &[organization]);
    Ok(())
}

/// THE global-exclusion asymmetry pin (RSRC-01's labeled case,
/// resource_adapter.rb:21-23): a GLOBAL row never surfaces resources
/// (its NULL `resource_type` cannot match the family join), while a
/// CLASS-scoped row covers every instance key of the family. The
/// fixture provisions the two side by side on a fresh role table - a
/// global-only "watcher" row and a class-scoped "curator" row - so
/// the ONLY difference between the two `with_role` asks is the scope
/// column.
///
/// The complements pin both directions: the class row empties the
/// universe through `without_role`, and the global row excludes
/// nothing.
///
/// # Errors
///
/// Propagates backend failures from the build, reset, or grants.
///
/// # Panics
///
/// Panics when a parity assertion fails.
#[maybe_async::maybe_async]
pub async fn global_exclusion_asymmetry_labeled<B: TestBackend>() -> Result<(), B::Error> {
    let mut backend = B::build().await?;
    backend.reset_roles().await?;
    let watcher = RoleName::from("watcher");
    let curator = RoleName::from("curator");
    backend
        .subject("admin")
        .add_role(&watcher, ResourceRef::Global)
        .await?;
    backend
        .subject("zombie")
        .add_role(&curator, ResourceRef::Class("Forum"))
        .await?;
    let universe = all_forum_keys(&backend);

    // The global row never surfaces: `resource_type IN ("Forum")`
    // cannot match a NULL type.
    let found = Forum::with_role(backend.engine(), core::slice::from_ref(&watcher), None).await?;
    assert!(found.is_empty());

    // The class row covers every instance key of the family.
    let found = Forum::with_role(backend.engine(), core::slice::from_ref(&curator), None).await?;
    assert_key_set(&found, &universe);

    // The class row empties the universe through the complement.
    let found = Forum::without_role(backend.engine(), &[curator], None, &universe).await?;
    assert!(found.is_empty());

    // The global row excludes nothing.
    let found = Forum::without_role(backend.engine(), &[watcher], None, &universe).await?;
    assert_key_set(&found, &universe);
    Ok(())
}

/// Expand the 10 `resource_queries` wrappers for one backend. Wrapper
/// names carry the `resource_queries_` prefix so the binding modules
/// never collide.
#[macro_export]
macro_rules! parity_resource_queries_cases {
    ($backend:ty) => {
        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_with_role_name_rows() {
            $crate::suite::resource_queries::with_role_name_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_with_role_multi_name_row() {
            $crate::suite::resource_queries::with_role_multi_name_row::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_with_role_user_filter_rows() {
            $crate::suite::resource_queries::with_role_user_filter_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_with_role_multi_name_user_rows() {
            $crate::suite::resource_queries::with_role_multi_name_user_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_team_string_pk_query() {
            $crate::suite::resource_queries::team_string_pk_query::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_sti_query_matches_children() {
            $crate::suite::resource_queries::sti_query_matches_children::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_without_role_name_rows() {
            $crate::suite::resource_queries::without_role_name_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_without_role_user_filter_rows() {
            $crate::suite::resource_queries::without_role_user_filter_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_without_role_multi_and_special_rows() {
            $crate::suite::resource_queries::without_role_multi_and_special_rows::<$backend>()
                .await
                .unwrap();
        }

        #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
        async fn resource_queries_global_exclusion_asymmetry_labeled() {
            $crate::suite::resource_queries::global_exclusion_asymmetry_labeled::<$backend>()
                .await
                .unwrap();
        }
    };
}
