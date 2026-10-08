//! Generic fixture builders for consumer integration seeds (TOOL-02, D-12).
//!
//! Deterministic primitives always available: [`FixtureScope`] (the owned
//! scope triple) plus [`FixtureGrant`] (one name plus its scope) plus the
//! [`grant`] and [`grant_all`] free functions. The functions speak only the
//! public [`RoleStore`](rolify_core::store::RoleStore) SPI
//! (`find_or_create_by` plus `add`), so they seed any consumer store, not
//! just the published mock. Holders travel as borrowed
//! [`ResourceId`](rolify_core::role::ResourceId): no concrete `TestUser`
//! or `TestForum` type ships here (D-15, the CONF-03
//! custom-types-free-via-generics philosophy).
//!
//! Dual-mode: the free functions carry `maybe_async` exactly like the SPI,
//! so they compile in both default async and `is_sync` builds with no
//! `block_on` anywhere.

use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::RoleStore;

/// Owned scope for one fixture grant: the write-side triple behind
/// [`ResourceRef`](rolify_core::resource::ResourceRef), owned so grants
/// can live in vectors and presets.
///
/// # Example
///
/// ```
/// use rolify_test::builders::FixtureScope;
///
/// let scope = FixtureScope::Class("Forum".to_owned());
/// assert!(matches!(scope, FixtureScope::Class(_)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixtureScope {
    /// A global grant (no resource).
    Global,
    /// A class-scoped grant over a resource type name.
    Class(String),
    /// An instance-scoped grant over a type name plus a resource id.
    Instance(String, ResourceId),
}

impl FixtureScope {
    /// Borrow as the SPI write scope for `find_or_create_by`.
    #[must_use]
    pub fn scope_ref(&self) -> ResourceRef<'_> {
        match self {
            Self::Global => ResourceRef::Global,
            Self::Class(type_name) => ResourceRef::Class(type_name.as_str()),
            Self::Instance(type_name, resource_id) => {
                ResourceRef::Instance(type_name.as_str(), resource_id)
            }
        }
    }
}

/// One fixture grant: a literal role name plus its owned scope.
///
/// # Example
///
/// ```
/// use rolify_test::builders::{FixtureGrant, FixtureScope};
///
/// let grant = FixtureGrant::global("admin");
/// assert_eq!(grant.name.as_str(), "admin");
/// assert_eq!(grant.scope, FixtureScope::Global);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureGrant {
    /// Literal role name (spec data, byte-exact).
    pub name: RoleName,
    /// The grant scope.
    pub scope: FixtureScope,
}

impl FixtureGrant {
    /// Build from an explicit name plus scope.
    #[must_use]
    pub fn new(name: RoleName, scope: FixtureScope) -> Self {
        Self { name, scope }
    }

    /// A global grant (mirrors [`RoleRecord::global`](rolify_core::role::RoleRecord::global)).
    #[must_use]
    pub fn global(name: impl Into<RoleName>) -> Self {
        Self::new(name.into(), FixtureScope::Global)
    }

    /// A class-scoped grant over a resource type name.
    #[must_use]
    pub fn for_class(name: impl Into<RoleName>, resource_type: impl Into<String>) -> Self {
        Self::new(name.into(), FixtureScope::Class(resource_type.into()))
    }

    /// An instance-scoped grant over a type name plus a resource id.
    #[must_use]
    pub fn for_instance(
        name: impl Into<RoleName>,
        resource_type: impl Into<String>,
        resource_id: impl Into<ResourceId>,
    ) -> Self {
        Self::new(
            name.into(),
            FixtureScope::Instance(resource_type.into(), resource_id.into()),
        )
    }
}

/// Seed one grant against any consumer store: `find_or_create_by` the row
/// for the grant name plus scope, then `add` the holder link (the gem's
/// `role_adapter.rb` level-1 row dedupe plus level-2 link guard).
/// Idempotent: repeating the same grant returns the same row with no
/// extra rows or links.
///
/// # Errors
///
/// Propagates the store errors of `find_or_create_by` / `add`.
///
/// # Example
///
/// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
/// example's own `await`s when `is_sync` is active):
///
/// ```rust
/// use rolify_core::role::ResourceId;
/// use rolify_test::builders::{FixtureGrant, grant};
/// use rolify_test::InMemoryStore;
///
/// # #[cfg(not(feature = "is_sync"))]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() { usage().await; }
/// # #[cfg(feature = "is_sync")]
/// # fn main() { usage(); }
/// #
/// #[maybe_async::maybe_async]
/// async fn usage() {
///     let mut store = InMemoryStore::new();
///     let mut conn = ();
///     let holder = ResourceId::from(7001_i64);
///     let record = grant(&mut store, &mut conn, &holder, &FixtureGrant::global("admin"))
///         .await
///         .unwrap();
///     assert!(record.is_global());
/// }
/// ```
#[maybe_async::maybe_async]
pub async fn grant<Store>(
    store: &mut Store,
    conn: &mut Store::Conn,
    holder: &ResourceId,
    fixture_grant: &FixtureGrant,
) -> Result<RoleRecord, Store::Error>
where
    Store: RoleStore,
{
    let scope = fixture_grant.scope.scope_ref();
    let record = store
        .find_or_create_by(conn, &fixture_grant.name, scope)
        .await?;
    store.add(conn, holder, &record).await?;
    Ok(record)
}

/// Seed several grants for one holder in order, returning the created rows
/// in the same order (one `grant` call per entry).
///
/// # Errors
///
/// Propagates the store errors of the first failing `grant` call.
///
/// # Example
///
/// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
/// example's own `await`s when `is_sync` is active):
///
/// ```rust
/// use rolify_core::role::ResourceId;
/// use rolify_test::builders::{FixtureGrant, grant_all};
/// use rolify_test::InMemoryStore;
///
/// # #[cfg(not(feature = "is_sync"))]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() { usage().await; }
/// # #[cfg(feature = "is_sync")]
/// # fn main() { usage(); }
/// #
/// #[maybe_async::maybe_async]
/// async fn usage() {
///     let mut store = InMemoryStore::new();
///     let mut conn = ();
///     let holder = ResourceId::from(7001_i64);
///     let forum_id = ResourceId::from(3_i64);
///     let fixture_grants = vec![
///         FixtureGrant::for_class("manager", "Forum"),
///         FixtureGrant::for_instance("moderator", "Forum", forum_id),
///     ];
///     let records = grant_all(&mut store, &mut conn, &holder, &fixture_grants)
///         .await
///         .unwrap();
///     assert_eq!(records.len(), 2);
/// }
/// ```
#[maybe_async::maybe_async]
pub async fn grant_all<Store>(
    store: &mut Store,
    conn: &mut Store::Conn,
    holder: &ResourceId,
    fixture_grants: &[FixtureGrant],
) -> Result<Vec<RoleRecord>, Store::Error>
where
    Store: RoleStore,
{
    let mut records = Vec::with_capacity(fixture_grants.len());
    for fixture_grant in fixture_grants {
        let record = grant(store, conn, holder, fixture_grant).await?;
        records.push(record);
    }
    Ok(records)
}

/// One seeded holder inside the standard preset: its fixture login plus
/// its literal holder id plus every grant the preset gives it.
///
/// # Example
///
/// ```
/// use rolify_core::role::ResourceId;
/// use rolify_test::builders::{PresetHolder, standard_preset};
///
/// let preset = standard_preset();
/// assert_eq!(preset[0].login, "preset-admin");
/// assert_eq!(preset[0].holder_id, ResourceId::from(9001_i64));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetHolder {
    /// The holder login (fixture data, lookup key in consumer tests).
    pub login: &'static str,
    /// The holder id (deterministic literal, reproducible seeds).
    pub holder_id: ResourceId,
    /// Every grant the preset gives this holder.
    pub grants: Vec<FixtureGrant>,
}

/// The standard preset (D-13): one call seeding the suite fixture graph
/// (`spec/support/data.rb` shape: a global admin plus a scoped
/// moderator). Fully deterministic: holder logins (`preset-admin`,
/// `preset-moderator`), holder ids (9001, 9002), role names, and scopes
/// are literals, so seeds reproduce exactly (no faker inside).
///
/// * `preset-admin` (id 9001): global `admin` plus class `manager` on
///   `Forum`.
/// * `preset-moderator` (id 9002): instance `moderator` on `Forum` id 3.
///
/// # Example
///
/// ```
/// use rolify_test::builders::standard_preset;
///
/// let preset = standard_preset();
/// assert_eq!(preset.len(), 2);
/// assert_eq!(preset[1].login, "preset-moderator");
/// ```
#[must_use]
pub fn standard_preset() -> Vec<PresetHolder> {
    vec![
        PresetHolder {
            login: "preset-admin",
            holder_id: ResourceId::from(9001_i64),
            grants: vec![
                FixtureGrant::global("admin"),
                FixtureGrant::for_class("manager", "Forum"),
            ],
        },
        PresetHolder {
            login: "preset-moderator",
            holder_id: ResourceId::from(9002_i64),
            grants: vec![FixtureGrant::for_instance(
                "moderator",
                "Forum",
                ResourceId::from(3_i64),
            )],
        },
    ]
}

/// Seed a whole preset against any consumer store: every holder's grants
/// through [`grant_all`], in slice order, returning all created rows in
/// seeding order.
///
/// # Errors
///
/// Propagates the store errors of the first failing `grant_all` call.
///
/// # Example
///
/// Runs live in BOTH modes (the `maybe_async` attribute rewrites the
/// example's own `await`s when `is_sync` is active):
///
/// ```rust
/// use rolify_core::role::ResourceId;
/// use rolify_core::store::RoleStore;
/// use rolify_test::builders::{apply_preset, standard_preset};
/// use rolify_test::InMemoryStore;
///
/// # #[cfg(not(feature = "is_sync"))]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() { usage().await; }
/// # #[cfg(feature = "is_sync")]
/// # fn main() { usage(); }
/// #
/// #[maybe_async::maybe_async]
/// async fn usage() {
///     let mut store = InMemoryStore::new();
///     let mut conn = ();
///     let preset = standard_preset();
///     let records = apply_preset(&mut store, &mut conn, &preset).await.unwrap();
///     assert_eq!(records.len(), 3);
///     let admin_rows = store
///         .roles_of(&mut conn, &ResourceId::from(9001_i64))
///         .await
///         .unwrap();
///     assert_eq!(admin_rows.len(), 2);
/// }
/// ```
#[maybe_async::maybe_async]
pub async fn apply_preset<Store>(
    store: &mut Store,
    conn: &mut Store::Conn,
    preset: &[PresetHolder],
) -> Result<Vec<RoleRecord>, Store::Error>
where
    Store: RoleStore,
{
    let mut records = Vec::new();
    for preset_holder in preset {
        let created =
            grant_all(store, conn, &preset_holder.holder_id, &preset_holder.grants).await?;
        records.extend(created);
    }
    Ok(records)
}

// ---- Faker generators (opt-in `faker` feature, D-14) ----
//
// Second builder layer behind `faker = ["dep:faker-rust"]`, lighter than
// `suite` (which also pulls `rstest`). Built on the same `faker_rust`
// call paths as the suite fixtures (`fixtures.rs` `display_name`):
// `faker_rust::internet::username` plus `faker_rust::number::between`.
// Generated values suit negative-path and uniqueness probes; positive
// assertions pin literal names (T-7-07: generated display values never
// serve as expected lookup keys).

/// A random holder id for uniqueness probes (inert display value, never
/// an expected lookup key).
///
/// # Example
///
/// ```rust
/// use rolify_test::builders::fake_holder_id;
///
/// assert!(!fake_holder_id().as_str().is_empty());
/// ```
#[cfg(feature = "faker")]
#[must_use]
pub fn fake_holder_id() -> ResourceId {
    ResourceId::from(faker_rust::number::between(1, 1_000_000_000))
}

/// A random role name carrying the caller prefix for uniqueness probes
/// (inert display value, never an expected lookup key).
///
/// # Example
///
/// ```rust
/// use rolify_test::builders::fake_role_name;
///
/// assert!(fake_role_name("reviewer").as_str().starts_with("reviewer-"));
/// ```
#[cfg(feature = "faker")]
#[must_use]
pub fn fake_role_name(prefix: &str) -> RoleName {
    RoleName::from(format!(
        "{prefix}-{}-{}",
        faker_rust::internet::username(None),
        faker_rust::number::between(1, 9999)
    ))
}

/// A random forum id for uniqueness probes (inert display value, never
/// an expected lookup key).
///
/// # Example
///
/// ```rust
/// use rolify_test::builders::fake_forum_id;
///
/// assert!(!fake_forum_id().as_str().is_empty());
/// ```
#[cfg(feature = "faker")]
#[must_use]
pub fn fake_forum_id() -> ResourceId {
    ResourceId::from(faker_rust::number::between(1, 1_000_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryStore;
    use rolify_core::store::RoleStore;

    fn holder_id() -> ResourceId {
        ResourceId::from(7001_i64)
    }

    /// Grant creates a global admin row linked to the holder and repeats
    /// idempotently.
    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn grant_creates_global_admin_and_repeats_idempotently() {
        let mut store = InMemoryStore::new();
        let mut conn = ();
        let holder = holder_id();
        let fixture_grant = FixtureGrant::global("admin");
        let first = grant(&mut store, &mut conn, &holder, &fixture_grant)
            .await
            .unwrap();
        assert_eq!(first, rolify_core::role::RoleRecord::global("admin"));
        let second = grant(&mut store, &mut conn, &holder, &fixture_grant)
            .await
            .unwrap();
        assert_eq!(second, first, "repeat grant returns the same row");
        let held = store.roles_of(&mut conn, &holder).await.unwrap();
        assert_eq!(held.len(), 1, "exactly one linked row after repeat");
        assert_eq!(
            store.assertion_len(),
            1,
            "exactly one role row after repeat"
        );
    }

    /// Grant-all seeds class plus instance grants in one call.
    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn grant_all_seeds_class_plus_instance_in_one_call() {
        let mut store = InMemoryStore::new();
        let mut conn = ();
        let holder = holder_id();
        let forum_id = ResourceId::from(3_i64);
        let fixture_grants = vec![
            FixtureGrant::for_class("manager", "Forum"),
            FixtureGrant::for_instance("moderator", "Forum", forum_id.clone()),
        ];
        let records = grant_all(&mut store, &mut conn, &holder, &fixture_grants)
            .await
            .unwrap();
        assert_eq!(records.len(), 2, "one record per grant in order");
        assert_eq!(
            records[0],
            rolify_core::role::RoleRecord::for_class("manager", "Forum")
        );
        assert_eq!(
            records[1],
            rolify_core::role::RoleRecord::for_instance("moderator", "Forum", forum_id)
        );
        let held = store.roles_of(&mut conn, &holder).await.unwrap();
        assert_eq!(held.len(), 2, "both grants linked to the holder");
    }

    #[test]
    fn scope_ref_borrows_each_variant_as_resource_ref() {
        let forum_id = ResourceId::from(3_i64);
        assert_eq!(FixtureScope::Global.scope_ref(), ResourceRef::Global);
        assert_eq!(
            FixtureScope::Class("Forum".to_owned()).scope_ref(),
            ResourceRef::Class("Forum")
        );
        assert_eq!(
            FixtureScope::Instance("Forum".to_owned(), forum_id.clone()).scope_ref(),
            ResourceRef::Instance("Forum", &forum_id)
        );
    }

    /// The standard preset mirrors the suite fixture graph: a global admin
    /// holder plus a scoped moderator holder with deterministic literals
    /// (RED: `standard_preset` does not exist yet).
    #[test]
    fn standard_preset_mirrors_suite_graph_with_deterministic_literals() {
        let preset = standard_preset();
        assert_eq!(preset.len(), 2, "exactly the admin plus moderator holders");
        let admin = &preset[0];
        assert_eq!(admin.login, "preset-admin");
        assert_eq!(admin.holder_id, ResourceId::from(9001_i64));
        assert_eq!(
            admin.grants,
            vec![
                FixtureGrant::global("admin"),
                FixtureGrant::for_class("manager", "Forum"),
            ],
            "admin carries global admin plus class manager on Forum"
        );
        let moderator = &preset[1];
        assert_eq!(moderator.login, "preset-moderator");
        assert_eq!(moderator.holder_id, ResourceId::from(9002_i64));
        assert_eq!(
            moderator.grants,
            vec![FixtureGrant::for_instance(
                "moderator",
                "Forum",
                ResourceId::from(3_i64)
            )],
            "moderator carries the Forum instance grant"
        );
    }

    /// Apply-preset seeds both holders so every grant reads back through
    /// `roles_of` (RED: `apply_preset` does not exist yet).
    #[maybe_async::test(feature = "is_sync", async(not(feature = "is_sync"), tokio::test))]
    async fn apply_preset_seeds_both_holders_readably() {
        let mut store = InMemoryStore::new();
        let mut conn = ();
        let preset = standard_preset();
        let records = apply_preset(&mut store, &mut conn, &preset).await.unwrap();
        assert_eq!(
            records.len(),
            3,
            "two admin grants plus one moderator grant"
        );
        let admin_id = ResourceId::from(9001_i64);
        let admin_rows = store.roles_of(&mut conn, &admin_id).await.unwrap();
        assert_eq!(
            admin_rows,
            vec![
                rolify_core::role::RoleRecord::global("admin"),
                rolify_core::role::RoleRecord::for_class("manager", "Forum"),
            ],
            "admin holder reads back both seeded grants"
        );
        let moderator_id = ResourceId::from(9002_i64);
        let moderator_rows = store.roles_of(&mut conn, &moderator_id).await.unwrap();
        assert_eq!(
            moderator_rows,
            vec![rolify_core::role::RoleRecord::for_instance(
                "moderator",
                "Forum",
                ResourceId::from(3_i64)
            )],
            "moderator holder reads back the instance grant"
        );
    }

    /// Faker generators produce distinct ids and prefixed names across
    /// calls (RED: the `faker` generators do not exist yet).
    #[cfg(feature = "faker")]
    #[test]
    fn faker_generators_produce_distinct_ids_and_prefixed_names() {
        let first_holder = fake_holder_id();
        let second_holder = fake_holder_id();
        assert_ne!(
            first_holder, second_holder,
            "two holder ids collide almost never: {first_holder:?} vs {second_holder:?}"
        );
        let first_name = fake_role_name("reviewer");
        let second_name = fake_role_name("reviewer");
        for generated in [&first_name, &second_name] {
            assert!(
                generated.as_str().starts_with("reviewer-"),
                "prefixed name keeps the caller prefix: {generated:?}"
            );
        }
        assert_ne!(first_name, second_name, "two names collide almost never");
        let forum_id = fake_forum_id();
        assert!(
            !forum_id.as_str().is_empty(),
            "forum id is a usable identifier"
        );
    }
}
