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
}
