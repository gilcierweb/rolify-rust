//! Gem fixture matrix (`spec/support/data.rb` 1:1) plus the shared-context
//! data (`spec/rolify/shared_contexts.rb` 1:1) behind the `suite` feature.
//!
//! Everything here is spec data or spec-shaped scaffolding: logins,
//! primary keys, and role names stay LITERAL (the shared contexts look them
//! up; [`RoleName`](rolify_core::role::RoleName) compares byte-exact), while
//! display names seed via `faker-rust` (TEST-04 - behaviorally inert).

use std::marker::PhantomData;

use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::resource::Resource;
use rolify_core::role::{ResourceId, RoleRecord};
use rolify_core::store::{ResourceKey, RoleStore};
use rolify_core::user::RolifyUser;

/// Behaviorally inert display name: faker-seeded (TEST-04). Names are never
/// lookup keys - logins, primary keys, and role names stay literal.
fn display_name(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        faker_rust::internet::username(None),
        faker_rust::number::between(1, 9999)
    )
}

/// A gem user class: its type discriminator plus its rolify configuration
/// (`spec/support/adapters/active_record.rb`).
pub trait UserClass: Sized + Send + Sync + 'static {
    /// The `rolify_type` literal (`Resource::type_name` counterpart).
    fn rolify_type() -> &'static str;

    /// The class's rolify configuration.
    fn config() -> RolifyConfig;
}

/// Default `User` (`active_record.rb:11-13`): default configuration.
pub struct DefaultUser;

impl UserClass for DefaultUser {
    fn rolify_type() -> &'static str {
        "User"
    }

    /// # Panics
    ///
    /// Never in practice: the gem defaults always validate. The `expect`
    /// only satisfies `build()`'s `Result`.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .build()
            .expect("the gem's default configuration always passes validation")
    }
}

/// `StrictUser` (`active_record.rb:25-27`, `rolify strict: true`).
pub struct StrictUserClass;

impl UserClass for StrictUserClass {
    fn rolify_type() -> &'static str {
        "StrictUser"
    }

    /// # Panics
    ///
    /// Never in practice: the hardcoded table names are non-empty.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .strict(true)
            .build()
            .expect("the strict configuration always passes validation")
    }
}

/// `Customer` (`active_record.rb:36-38`, `role_cname "Privilege"`).
pub struct CustomerClass;

impl UserClass for CustomerClass {
    fn rolify_type() -> &'static str {
        "Customer"
    }

    /// # Panics
    ///
    /// Never in practice: the hardcoded table names are non-empty.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .role_table("privileges")
            .join_table("customers_privileges")
            .build()
            .expect("the customer table names always pass validation")
    }
}

/// `Admin::Moderator` (`active_record.rb:53-55`): the `rolify_type` carries
/// the literal namespaced `"Admin::Moderator"` string per D-13, with the
/// `admin_` table prefix (`role_table "admin_rights"`, `join_table
/// "moderators_rights"`).
pub struct AdminModeratorClass;

impl UserClass for AdminModeratorClass {
    fn rolify_type() -> &'static str {
        "Admin::Moderator"
    }

    /// # Panics
    ///
    /// Never in practice: the hardcoded table names are non-empty.
    fn config() -> RolifyConfig {
        RolifyConfig::builder()
            .role_table("admin_rights")
            .join_table("moderators_rights")
            .build()
            .expect("the admin table names always pass validation")
    }
}

/// A fixture holder: one login plus identity over the shared engine,
/// shaped like the Phase-1 `Player` (`store`, `conn`, `config` now live
/// inside the owned [`Rolify`] engine handle, D-06: the same store the
/// D-05 class statics receive through `TestBackend::engine`).
pub struct FixtureUser<C: UserClass, S: RoleStore> {
    login: String,
    id: ResourceId,
    engine: Rolify<S>,
    class_marker: PhantomData<C>,
}

impl<C: UserClass, S: RoleStore> FixtureUser<C, S> {
    /// Seat a holder identity over an engine; the engine carries the store,
    /// the connection, and the `C::config()` configuration (applied by the
    /// caller before `Rolify::new`).
    #[must_use]
    pub fn new(login: impl Into<String>, id: impl Into<ResourceId>, engine: Rolify<S>) -> Self {
        Self {
            login: login.into(),
            id: id.into(),
            engine,
            class_marker: PhantomData,
        }
    }

    /// The holder's login (the shared-context lookup key).
    #[must_use]
    pub fn login(&self) -> &str {
        &self.login
    }

    /// The owned engine handle (the D-05 seam: class statics split it).
    pub(crate) fn engine(&mut self) -> &mut Rolify<S> {
        &mut self.engine
    }

    /// Re-seat the handle on another fixture holder identity; the engine
    /// (store, conn, config) stays shared. Ports the suite's subject
    /// switching (`shared_contexts.rb:2/27/58`, where each context re-binds
    /// the subject to another holder).
    pub fn seat_as(&mut self, login: impl Into<String>, id: ResourceId) {
        self.login = login.into();
        self.id = id;
    }
}

#[maybe_async::maybe_async(AFIT)]
impl<C: UserClass, S: RoleStore> RolifyUser for FixtureUser<C, S>
where
    S::Conn: Sync,
{
    // The `S::Conn: Sync` bound exists so this struct satisfies the
    // `Send + Sync` supertraits of `RolifyUser`: provided-method futures
    // hold `&mut Conn` across `.await` points, and every targeted backend
    // connection type is `Sync`. Store and config access delegate through
    // the owned engine (D-06/D-07: one config source of truth; the `Arc`
    // hooks keep sharing cheap, so no clone is needed for the borrow).
    type Store = S;

    fn store(&mut self) -> &mut Self::Store {
        self.engine.store_with_conn().0
    }

    fn rolify_config(&self) -> &RolifyConfig {
        self.engine.config()
    }

    fn rolify_id(&self) -> ResourceId {
        self.id.clone()
    }

    fn rolify_type() -> &'static str {
        C::rolify_type()
    }

    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut <Self::Store as RoleStore>::Conn) {
        self.engine.store_with_conn()
    }
}

/// `User` fixture holder over any store.
pub type User<S> = FixtureUser<DefaultUser, S>;
/// `StrictUser` fixture holder over any store.
pub type StrictUser<S> = FixtureUser<StrictUserClass, S>;
/// `Customer` fixture holder over any store.
pub type Customer<S> = FixtureUser<CustomerClass, S>;

/// The `Admin` namespace mirror: `Admin::Moderator` in Ruby maps to
/// `admin::Moderator` here, while `rolify_type` keeps the literal
/// `"Admin::Moderator"` string.
pub mod admin {
    use super::{AdminModeratorClass, FixtureUser};

    /// `Admin::Moderator` fixture holder over any store.
    pub type Moderator<S> = FixtureUser<AdminModeratorClass, S>;
}

/// `Forum` fixture resource (`data.rb:17-19`).
pub struct Forum {
    /// Stringified integer primary key.
    pub id: ResourceId,
    /// Display name (faker-seeded, behaviorally inert).
    pub name: String,
}

impl Resource for Forum {
    fn type_name() -> &'static str {
        "Forum"
    }

    fn resource_id(&self) -> ResourceId {
        self.id.clone()
    }
}

/// `Group` fixture resource (`data.rb:21-22`).
pub struct Group {
    /// Stringified integer primary key.
    pub id: ResourceId,
    /// Display name (faker-seeded, behaviorally inert).
    pub name: String,
}

impl Resource for Group {
    fn type_name() -> &'static str {
        "Group"
    }

    fn resource_id(&self) -> ResourceId {
        self.id.clone()
    }
}

/// `Team` fixture resource (`data.rb:24-25`): the string-PK pin
/// (`Team.team_code`, `spec/support/schema.rb`) - `resource_id` comes from
/// `team_code`, never from a surrogate integer.
pub struct Team {
    /// String primary key (`team_code` "1"/"2").
    pub team_code: String,
    /// Display name (faker-seeded, behaviorally inert).
    pub name: String,
}

impl Resource for Team {
    fn type_name() -> &'static str {
        "Team"
    }

    fn resource_id(&self) -> ResourceId {
        ResourceId::from(self.team_code.clone())
    }
}

/// `Organization` fixture resource (`data.rb:27`): the STI base of the
/// RSRC-06 pair, overriding `descendant_types` with `Company`.
pub struct Organization {
    /// Stringified integer primary key.
    pub id: ResourceId,
}

impl Resource for Organization {
    fn type_name() -> &'static str {
        "Organization"
    }

    fn descendant_types() -> Vec<&'static str> {
        vec!["Organization", "Company"]
    }

    fn resource_id(&self) -> ResourceId {
        self.id.clone()
    }
}

/// `Company` fixture resource (`data.rb:28`): the STI descendant
/// (`class Company < Organization`, `active_record.rb:90-92`).
pub struct Company {
    /// Stringified integer primary key.
    pub id: ResourceId,
}

impl Resource for Company {
    fn type_name() -> &'static str {
        "Company"
    }

    fn resource_id(&self) -> ResourceId {
        self.id.clone()
    }
}

/// The only resource identities the portable spec files reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureResource {
    /// `Forum.first` (id "1").
    ForumFirst,
    /// The middle forum (id "2", `data.rb:18`) - referenced by the
    /// `Forum.all` query rows of `resource_spec.rb` (l.45, l.95,
    /// l.177, l.223) as part of the full three-forum fixture.
    ForumSecond,
    /// `Forum.last` (id "3").
    ForumLast,
    /// `Group.first` (id "1").
    GroupFirst,
    /// `Group.last` (id "2").
    GroupLast,
    /// `Team` with `team_code` "1".
    TeamFirst,
    /// `Team` with `team_code` "2".
    TeamLast,
    /// The single `Organization` (id "1").
    Organization,
    /// The single `Company` (id "1").
    Company,
}

/// Instantiated fixture resources in `data.rb:17-27` creation order.
pub struct FixtureResources {
    forums: [Forum; 3],
    groups: [Group; 2],
    teams: [Team; 2],
    organization: Organization,
    company: Company,
}

impl FixtureResources {
    /// Build the full resource matrix: three forums (ids "1"/"2"/"3"), two
    /// groups ("1"/"2"), two teams (`team_code` "1"/"2"), one organization
    /// ("1"), one company ("1").
    #[must_use]
    pub fn new() -> Self {
        Self {
            forums: [
                Forum {
                    id: ResourceId::from(1_i64),
                    name: display_name("forum"),
                },
                Forum {
                    id: ResourceId::from(2_i64),
                    name: display_name("forum"),
                },
                Forum {
                    id: ResourceId::from(3_i64),
                    name: display_name("forum"),
                },
            ],
            groups: [
                Group {
                    id: ResourceId::from(1_i64),
                    name: display_name("group"),
                },
                Group {
                    id: ResourceId::from(2_i64),
                    name: display_name("group"),
                },
            ],
            teams: [
                Team {
                    team_code: "1".to_owned(),
                    name: display_name("team"),
                },
                Team {
                    team_code: "2".to_owned(),
                    name: display_name("team"),
                },
            ],
            organization: Organization {
                id: ResourceId::from(1_i64),
            },
            company: Company {
                id: ResourceId::from(1_i64),
            },
        }
    }

    /// Resolve a resource selector to its `(type, id)` key.
    #[must_use]
    pub fn key(&self, which: FixtureResource) -> ResourceKey {
        match which {
            FixtureResource::ForumFirst => {
                ResourceKey::new(Forum::type_name(), self.forums[0].id.clone())
            }
            FixtureResource::ForumSecond => {
                ResourceKey::new(Forum::type_name(), self.forums[1].id.clone())
            }
            FixtureResource::ForumLast => {
                ResourceKey::new(Forum::type_name(), self.forums[2].id.clone())
            }
            FixtureResource::GroupFirst => {
                ResourceKey::new(Group::type_name(), self.groups[0].id.clone())
            }
            FixtureResource::GroupLast => {
                ResourceKey::new(Group::type_name(), self.groups[1].id.clone())
            }
            FixtureResource::TeamFirst => {
                ResourceKey::new(Team::type_name(), self.teams[0].team_code.clone())
            }
            FixtureResource::TeamLast => {
                ResourceKey::new(Team::type_name(), self.teams[1].team_code.clone())
            }
            FixtureResource::Organization => {
                ResourceKey::new(Organization::type_name(), self.organization.id.clone())
            }
            FixtureResource::Company => {
                ResourceKey::new(Company::type_name(), self.company.id.clone())
            }
        }
    }
}

impl Default for FixtureResources {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixture holder identities in `data.rb:5-8` creation order: `user_class`
/// `.first` is "admin", `.last` is "zombie"; the logins are the
/// `shared_contexts.rb` lookup keys.
#[must_use]
pub fn fixture_holders() -> Vec<(&'static str, ResourceId)> {
    vec![
        ("admin", ResourceId::from(1_i64)),
        ("moderator", ResourceId::from(2_i64)),
        ("god", ResourceId::from(3_i64)),
        ("zombie", ResourceId::from(4_i64)),
    ]
}

/// Which shared scope context to load (`shared_contexts.rb:1/26/50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeContext {
    /// `"global role"`: subject "admin".
    Global,
    /// `"class scoped role"`: subject "moderator".
    Class,
    /// `"instance scoped role"`: subject "god".
    Instance,
}

/// One role grant inside a scope context: a literal name plus its scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextScope {
    /// Global scope (no resource).
    Global,
    /// Class scope over a resource type.
    Class(&'static str),
    /// Instance scope over a resource selector.
    Instance(&'static str, FixtureResource),
}

/// One `(name, scope)` grant inside a scope context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextRoleSpec {
    /// Literal role name (spec data, byte-exact).
    pub name: &'static str,
    /// The grant scope.
    pub scope: ContextScope,
}

/// The loaded role table of one shared scope context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeContextData {
    /// The context's subject login.
    pub subject_login: &'static str,
    /// The roles the subject holds after loading.
    pub roles: Vec<ContextRoleSpec>,
}

fn role_spec(name: &'static str, scope: ContextScope) -> ContextRoleSpec {
    ContextRoleSpec { name, scope }
}

/// Encode the three shared scope contexts 1:1 (`shared_contexts.rb:13-23`
/// global, `l.38-47` class, `l.63-69` instance).
#[must_use]
pub fn scope_context(which: ScopeContext) -> ScopeContextData {
    match which {
        ScopeContext::Global => ScopeContextData {
            subject_login: "admin",
            roles: vec![
                role_spec("admin", ContextScope::Global),
                role_spec("staff", ContextScope::Global),
                role_spec("manager", ContextScope::Class("Group")),
                role_spec("player", ContextScope::Class("Forum")),
                role_spec(
                    "moderator",
                    ContextScope::Instance("Forum", FixtureResource::ForumLast),
                ),
                role_spec(
                    "moderator",
                    ContextScope::Instance("Group", FixtureResource::GroupLast),
                ),
                role_spec(
                    "anonymous",
                    ContextScope::Instance("Forum", FixtureResource::ForumFirst),
                ),
            ],
        },
        ScopeContext::Class => ScopeContextData {
            subject_login: "moderator",
            roles: vec![
                role_spec("manager", ContextScope::Class("Forum")),
                role_spec("player", ContextScope::Class("Forum")),
                role_spec("warrior", ContextScope::Global),
                role_spec(
                    "moderator",
                    ContextScope::Instance("Forum", FixtureResource::ForumLast),
                ),
                role_spec(
                    "moderator",
                    ContextScope::Instance("Group", FixtureResource::GroupLast),
                ),
                role_spec(
                    "anonymous",
                    ContextScope::Instance("Forum", FixtureResource::ForumFirst),
                ),
            ],
        },
        ScopeContext::Instance => ScopeContextData {
            subject_login: "god",
            roles: vec![
                role_spec(
                    "moderator",
                    ContextScope::Instance("Forum", FixtureResource::ForumFirst),
                ),
                role_spec(
                    "anonymous",
                    ContextScope::Instance("Forum", FixtureResource::ForumLast),
                ),
                role_spec("visitor", ContextScope::Class("Forum")),
                role_spec("soldier", ContextScope::Global),
            ],
        },
    }
}

/// Encode `create_other_roles` (`shared_contexts.rb:85-93`): unlinked rows
/// the specs assert are never duplicated by `add_role`.
#[must_use]
pub fn other_role_rows() -> Vec<RoleRecord> {
    vec![
        RoleRecord::global("superhero"),
        RoleRecord::for_class("admin", "Group"),
        RoleRecord::for_instance("admin", "Forum", ResourceId::from(1_i64)),
        RoleRecord::for_class("VIP", "Forum"),
        RoleRecord::for_instance("manager", "Forum", ResourceId::from(3_i64)),
        RoleRecord::for_instance("roomate", "Forum", ResourceId::from(1_i64)),
        RoleRecord::for_instance("moderator", "Group", ResourceId::from(1_i64)),
    ]
}

/// One provisioned holder inside the mixed context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisionSpec {
    /// The holder login to provision.
    pub login: &'static str,
    /// The roles to grant that holder.
    pub roles: Vec<ContextRoleSpec>,
}

/// Encode the mixed context (`shared_contexts.rb:79-82`, consumed by the
/// 02-07 finder suite): `root` is `user_class.first` ("admin"), `modo` is
/// the "moderator" login, `visitor` is `user_class.last` ("zombie"), `owner`
/// provisions `user_class.first` again with the Company instance role.
#[must_use]
pub fn mixed_context() -> Vec<ProvisionSpec> {
    vec![
        ProvisionSpec {
            login: "admin",
            roles: vec![
                role_spec("admin", ContextScope::Global),
                role_spec("staff", ContextScope::Global),
                role_spec("moderator", ContextScope::Class("Group")),
                role_spec(
                    "visitor",
                    ContextScope::Instance("Forum", FixtureResource::ForumLast),
                ),
            ],
        },
        ProvisionSpec {
            login: "moderator",
            roles: vec![
                role_spec("moderator", ContextScope::Class("Forum")),
                role_spec("manager", ContextScope::Class("Group")),
                role_spec(
                    "visitor",
                    ContextScope::Instance("Group", FixtureResource::GroupFirst),
                ),
            ],
        },
        ProvisionSpec {
            login: "zombie",
            roles: vec![role_spec(
                "visitor",
                ContextScope::Instance("Forum", FixtureResource::ForumLast),
            )],
        },
        ProvisionSpec {
            login: "admin",
            roles: vec![role_spec(
                "owner",
                ContextScope::Instance("Company", FixtureResource::Company),
            )],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::forum_first(FixtureResource::ForumFirst, "Forum", "1")]
    #[case::forum_second(FixtureResource::ForumSecond, "Forum", "2")]
    #[case::forum_last(FixtureResource::ForumLast, "Forum", "3")]
    #[case::group_first(FixtureResource::GroupFirst, "Group", "1")]
    #[case::group_last(FixtureResource::GroupLast, "Group", "2")]
    #[case::team_first(FixtureResource::TeamFirst, "Team", "1")]
    #[case::team_last(FixtureResource::TeamLast, "Team", "2")]
    #[case::organization(FixtureResource::Organization, "Organization", "1")]
    #[case::company(FixtureResource::Company, "Company", "1")]
    fn fixture_resource_keys_resolve(
        #[case] which: FixtureResource,
        #[case] expected_type: &str,
        #[case] expected_id: &str,
    ) {
        let resources = FixtureResources::new();
        let key = resources.key(which);
        assert_eq!(key.resource_type, expected_type);
        assert_eq!(key.resource_id.as_str(), expected_id);
    }

    #[test]
    fn user_class_type_names_are_exact_and_distinct() {
        let names = [
            DefaultUser::rolify_type(),
            StrictUserClass::rolify_type(),
            CustomerClass::rolify_type(),
            AdminModeratorClass::rolify_type(),
        ];
        assert_eq!(
            names,
            ["User", "StrictUser", "Customer", "Admin::Moderator"]
        );
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "every class has its own literal");
    }

    #[test]
    fn user_class_configs_pin_the_gem_variants() {
        assert!(!DefaultUser::config().strict());
        assert!(StrictUserClass::config().strict());
        assert_eq!(CustomerClass::config().role_table(), "privileges");
        assert_eq!(CustomerClass::config().join_table(), "customers_privileges");
        assert_eq!(AdminModeratorClass::config().role_table(), "admin_rights");
        assert_eq!(
            AdminModeratorClass::config().join_table(),
            "moderators_rights"
        );
    }

    #[test]
    fn organization_company_form_the_sti_pair() {
        assert_eq!(
            Organization::descendant_types(),
            ["Organization", "Company"]
        );
        assert_eq!(Company::descendant_types(), ["Company"]);
    }

    #[test]
    fn fixture_holders_follow_creation_order() {
        let holders = fixture_holders();
        assert_eq!(holders.len(), 4);
        assert_eq!(holders.first().map(|holder| holder.0), Some("admin"));
        assert_eq!(holders.last().map(|holder| holder.0), Some("zombie"));
    }

    #[test]
    fn team_resource_id_stringifies_the_team_code() {
        let resources = FixtureResources::new();
        assert_eq!(resources.teams[0].resource_id().as_str(), "1");
        assert_eq!(resources.teams[1].resource_id().as_str(), "2");
    }

    #[test]
    fn scope_contexts_carry_the_gem_role_tables() {
        assert_eq!(scope_context(ScopeContext::Global).roles.len(), 7);
        assert_eq!(scope_context(ScopeContext::Class).roles.len(), 6);
        assert_eq!(scope_context(ScopeContext::Instance).roles.len(), 4);
        assert_eq!(other_role_rows().len(), 7);
        assert_eq!(mixed_context().len(), 4);
    }
}
