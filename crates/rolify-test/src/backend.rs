//! Test-facing storage SPI: [`TestBackend`] plus the reference
//! [`InMemoryBackend`], with the shared-context appliers the suite cases
//! consume.

#[cfg(not(feature = "is_sync"))]
use core::future::Future;

use rolify_core::error::RolifyError;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::{ResourceKey, RoleStore, Sealed};
use rolify_core::user::RolifyUser;

use crate::InMemoryStore;
use crate::fixtures::{
    ContextScope, FixtureResource, FixtureResources, FixtureUser, ScopeContext, UserClass,
    fixture_holders, mixed_context, other_role_rows, scope_context,
};

/// The sealed test-facing SPI every suite backend implements (D-12): one
/// `parity_suite!` binding line per backend, shared case bodies everywhere.
///
/// The soft seal reuses [`Sealed`](rolify_core::store::Sealed): workspace-only
/// implementors, the same precedent as the storage SPI. The error-composition
/// bound (`Error: From<Store::Error>`) lets the shared appliers below call
/// the seated subject's provided methods with `?`.
#[maybe_async::maybe_async(AFIT)]
pub trait TestBackend: Sealed + Send + Sync + 'static
where
    Self::Error: From<<Self::Store as RoleStore>::Error>,
{
    /// The role-row store under test.
    type Store: RoleStore;

    /// The seated subject handle (`RolifyUser` over `Store`).
    type Subject: RolifyUser<Store = Self::Store>;

    /// Backend error - one `thiserror` enum per adapter, converting from
    /// [`RolifyError`] so core-level failures (callback veto, invalid
    /// config) flow through uniformly.
    type Error: core::error::Error + Send + Sync + From<RolifyError> + 'static;

    /// A fresh backend carrying the full D-13 fixture set with zero role
    /// rows and zero links. Parameterless by design: adapters bootstrap
    /// their own pools or containers inside it.
    ///
    /// # Errors
    ///
    /// Propagates backend bootstrap failures.
    fn build() -> impl Future<Output = Result<Self, Self::Error>> + Send
    where
        Self: Sized;

    /// Seat the handle on that fixture holder and return it (a SYNC member,
    /// legal under `maybe_async` exactly like
    /// [`RolifyUser::has_cached_role`]); mirrors the `shared_contexts.rb`
    /// subject selection.
    ///
    /// # Panics
    ///
    /// Panics on unknown logins: a misspelled login is a harness
    /// programming error, and failing fast beats asserting against the
    /// wrong holder.
    fn subject(&mut self, login: &str) -> &mut Self::Subject;

    /// Resolve a fixture login to its holder identity, if known.
    #[must_use]
    fn holder_id(&self, login: &str) -> Option<ResourceId>;

    /// Resolve a resource selector to its `(type, id)` key.
    #[must_use]
    fn resource(&self, which: FixtureResource) -> ResourceKey;

    /// The destroy-all plus roles-empty preamble (`role_class.destroy_all`
    /// plus `roles = []`, `shared_contexts.rb:14-15`).
    ///
    /// # Errors
    ///
    /// Propagates backend reset failures.
    fn reset_roles(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// `role_class.create`: insert an unlinked role row.
    ///
    /// # Errors
    ///
    /// Propagates backend insert failures.
    fn create_role_row(
        &mut self,
        record: RoleRecord,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The `provision_user` primitive: `find_or_create_by` plus `add` for
    /// that holder identity. Provisioning goes through the store directly,
    /// skipping config callbacks, which the gem's hook-free spec configs
    /// make behaviorally identical.
    ///
    /// # Errors
    ///
    /// Propagates backend grant failures.
    ///
    /// # Panics
    ///
    /// Panics on unknown logins, like [`TestBackend::subject`].
    fn grant_to(
        &mut self,
        login: &str,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// `role_class.count`. Takes `&mut self` because the store lives inside
    /// the subject handle.
    ///
    /// # Errors
    ///
    /// Propagates backend count failures.
    fn role_row_count(&mut self) -> impl Future<Output = Result<usize, Self::Error>> + Send;
}

/// The reference [`TestBackend`]: [`InMemoryStore`] plus the D-13 fixture
/// set, the implementation Phases 3-5 copy per adapter.
pub struct InMemoryBackend<C: UserClass> {
    subject: FixtureUser<C, InMemoryStore>,
    fixture_holders: Vec<(&'static str, ResourceId)>,
    resources: FixtureResources,
}

impl<C: UserClass> Sealed for InMemoryBackend<C> {}

#[maybe_async::maybe_async(AFIT)]
impl<C: UserClass> TestBackend for InMemoryBackend<C> {
    type Store = InMemoryStore;
    type Subject = FixtureUser<C, InMemoryStore>;
    type Error = RolifyError;

    fn build() -> impl Future<Output = Result<Self, Self::Error>> + Send {
        async move {
            let resources = FixtureResources::new();
            let mut store = InMemoryStore::new();
            for which in [
                FixtureResource::ForumFirst,
                FixtureResource::ForumLast,
                FixtureResource::GroupFirst,
                FixtureResource::GroupLast,
                FixtureResource::TeamFirst,
                FixtureResource::TeamLast,
                FixtureResource::Organization,
                FixtureResource::Company,
            ] {
                store.register_resource(resources.key(which));
            }
            let subject = FixtureUser::new("admin", ResourceId::from(1_i64), store, ());
            Ok(Self {
                subject,
                fixture_holders: fixture_holders(),
                resources,
            })
        }
    }

    /// # Panics
    ///
    /// Panics on unknown logins (see the trait docs).
    fn subject(&mut self, login: &str) -> &mut Self::Subject {
        let holder = self
            .holder_id(login)
            .expect("unknown fixture login: expected admin, moderator, god, or zombie");
        self.subject.seat_as(login, holder);
        &mut self.subject
    }

    fn holder_id(&self, login: &str) -> Option<ResourceId> {
        self.fixture_holders
            .iter()
            .find(|(known, _)| *known == login)
            .map(|(_, id)| id.clone())
    }

    fn resource(&self, which: FixtureResource) -> ResourceKey {
        self.resources.key(which)
    }

    fn reset_roles(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        RolifyUser::store(&mut self.subject).clear();
        async move { Ok(()) }
    }

    fn create_role_row(
        &mut self,
        record: RoleRecord,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.subject.store().insert(record);
        async move { Ok(()) }
    }

    /// # Panics
    ///
    /// Panics on unknown logins (see the trait docs).
    fn grant_to(
        &mut self,
        login: &str,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let holder = self
            .holder_id(login)
            .expect("unknown fixture login: expected admin, moderator, god, or zombie");
        let (store, conn) = RolifyUser::store_with_conn(&mut self.subject);
        async move {
            let role = store.find_or_create_by(&mut *conn, name, scope).await?;
            store.add(&mut *conn, &holder, &role).await?;
            Ok(())
        }
    }

    fn role_row_count(&mut self) -> impl Future<Output = Result<usize, Self::Error>> + Send {
        let count = self.subject.store().rows().len();
        async move { Ok(count) }
    }
}

/// Resolve a context scope to owned `(type, id)` data. Split out so both
/// appliers share the one resolution (DRY): every role's scope resolves
/// through `backend.resource` BEFORE any mutable subject borrow.
fn resolve_scope<B: TestBackend>(
    backend: &B,
    scope: &ContextScope,
) -> (Option<String>, Option<ResourceId>) {
    match scope {
        ContextScope::Global => (None, None),
        ContextScope::Class(type_name) => (Some((*type_name).to_owned()), None),
        ContextScope::Instance(type_name, resource) => {
            let key = backend.resource(*resource);
            (Some((*type_name).to_owned()), Some(key.resource_id))
        }
    }
}

/// Rebuild a write scope over resolved owned data.
fn scope_ref<'scope>(
    type_name: Option<&'scope str>,
    id: Option<&'scope ResourceId>,
) -> ResourceRef<'scope> {
    match type_name {
        None => ResourceRef::Global,
        Some(type_name) => match id {
            None => ResourceRef::Class(type_name),
            Some(id) => ResourceRef::Instance(type_name, id),
        },
    }
}

/// Load one shared scope context (`shared_contexts.rb:1-70`): reset the
/// roles, grant each role through the seated subject's `add_role` (the real
/// public path, callbacks included), then `create_other_roles`.
///
/// # Errors
///
/// Propagates backend failures from the reset, grants, or row inserts.
#[maybe_async::maybe_async]
pub async fn load_scope_context<B: TestBackend>(
    backend: &mut B,
    which: ScopeContext,
) -> Result<(), B::Error> {
    backend.reset_roles().await?;
    let data = scope_context(which);
    let mut resolved: Vec<(RoleName, Option<String>, Option<ResourceId>)> = Vec::new();
    for spec in &data.roles {
        let (type_name, id) = resolve_scope(backend, &spec.scope);
        resolved.push((RoleName::from(spec.name), type_name, id));
    }
    let subject = backend.subject(data.subject_login);
    for (name, type_name, id) in &resolved {
        subject
            .add_role(name, scope_ref(type_name.as_deref(), id.as_ref()))
            .await?;
    }
    create_other_roles(backend).await
}

/// Insert `other_role_rows()` (`shared_contexts.rb:85-93`).
///
/// # Errors
///
/// Propagates backend insert failures.
#[maybe_async::maybe_async]
pub async fn create_other_roles<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    for record in other_role_rows() {
        backend.create_role_row(record).await?;
    }
    Ok(())
}

/// Provision the mixed context (`shared_contexts.rb:79-82`, consumed by the
/// 02-07 finder suite): reset, then `grant_to` per `mixed_context()`.
///
/// # Errors
///
/// Propagates backend failures from the reset or grants.
#[maybe_async::maybe_async]
pub async fn provision_mixed_context<B: TestBackend>(backend: &mut B) -> Result<(), B::Error> {
    backend.reset_roles().await?;
    for provision in mixed_context() {
        for spec in &provision.roles {
            let (type_name, id) = resolve_scope(backend, &spec.scope);
            let name = RoleName::from(spec.name);
            backend
                .grant_to(provision.login, &name, scope_ref(type_name.as_deref(), id.as_ref()))
                .await?;
        }
    }
    Ok(())
}
