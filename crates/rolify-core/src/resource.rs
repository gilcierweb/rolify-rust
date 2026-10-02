//! Write-side scope enum [`ResourceRef`] and the [`Resource`] consumer
//! trait: the gem-traced discriminators plus the resource-side READ
//! statics (`find_roles`, `applied_roles`, `roles_of_instance`,
//! `applied_roles_of_instance`, RSRC-03..06), all provided and dual-mode
//! (D-10, `maybe_async` AFIT).

// `Future` is named in the provided signatures in async mode only;
// maybe-async strips the `impl Future` return type in `is_sync` mode.
#[cfg(not(feature = "is_sync"))]
use core::future::Future;

use crate::catalog::RoleCatalogQuery;
use crate::manager::Rolify;
use crate::role::{ResourceId, RoleName, RoleRecord};
use crate::store::RoleStore;

/// Scope for role **writes** (`add_role` / `remove_role`).
///
/// Port of the gem's `nil | Class | instance` write argument. There is no
/// `Any` variant - a role cannot be granted "at whatever scope"; `Any` is a
/// query-only concept (see [`crate::query::ResourceFilter`]).
///
/// # Example
///
/// ```
/// use rolify_core::resource::ResourceRef;
/// use rolify_core::role::ResourceId;
///
/// let id = ResourceId::from(7_i64);
/// let scope = ResourceRef::Instance("Forum", &id);
/// assert!(matches!(scope, ResourceRef::Instance("Forum", _)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceRef<'a> {
    /// `resource = nil` - a global role row.
    Global,
    /// `resource = Forum` - a class-scoped row (one per `(name, type)`).
    Class(&'a str),
    /// `resource = forum` - an instance-scoped row (one per
    /// `(name, type, id)`).
    Instance(&'a str, &'a ResourceId),
}

/// Implemented by consumer domain types that can own roles (a Rails model
/// like `Forum`, etc.).
///
/// Dyn-compatibility contract (SC-3): **no associated consts, ever** - the
/// Rust Reference bans them in dyn-compatible traits, and
/// `Box<dyn Resource>` must keep compiling. Discriminators are associated
/// functions carrying `where Self: Sized`.
///
/// # Example
///
/// ```
/// use rolify_core::resource::Resource;
/// use rolify_core::role::ResourceId;
///
/// struct Forum { id: i64 }
///
/// impl Resource for Forum {
///     fn type_name() -> &'static str { "Forum" }
///     fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
/// }
/// ```
///
/// Dual-mode note (D-10): the trait is annotated `maybe_async(AFIT)` the
/// same way as [`crate::store::RoleStore`] - async mode keeps the
/// hand-written `impl Future` statics untouched, `is_sync` mode rewrites
/// their signatures and strips the `async`/`await` tokens. Plain members
/// (`type_name`, `descendant_types`, `resource_id`) are unaffected in
/// both modes, and the attribute adds no supertraits, so dyn
/// compatibility (SC-3) is preserved.
#[maybe_async::maybe_async(AFIT)]
pub trait Resource {
    /// Discriminator - the Ruby class-name equivalent (e.g. `"Forum"`).
    fn type_name() -> &'static str
    where
        Self: Sized;

    /// STI descendants - port of `relation_types_for`
    /// (`adapters/base.rb:27-28`): returns `Self` plus descendant type names.
    /// Default: no STI, just `Self`.
    ///
    /// ```
    /// # use rolify_core::resource::Resource;
    /// # use rolify_core::role::ResourceId;
    /// # struct Vehicle { id: i64 }
    /// # impl Resource for Vehicle {
    /// #     fn type_name() -> &'static str { "Vehicle" }
    /// #     fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
    /// # }
/// struct Car { id: i64 }
/// impl Resource for Car {
///     fn type_name() -> &'static str { "Car" }
///     fn descendant_types() -> Vec<&'static str> { vec!["Vehicle", "Car"] }
///     fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
/// }
/// assert_eq!(Vehicle::descendant_types(), vec!["Vehicle"]); // default
/// assert_eq!(Car::descendant_types(), vec!["Vehicle", "Car"]); // STI
/// ```
    #[must_use]
    fn descendant_types() -> Vec<&'static str>
    where
        Self: Sized,
    {
        vec![Self::type_name()]
    }

    /// Instance identity - the stringified primary key (the
    /// `teams.team_code` string-PK case from the gem's schema).
    fn resource_id(&self) -> ResourceId;

    /// Class-level `find_roles(name?, user?)` (resource.rb:8-10, D-16):
    /// `name`/`user` as typed `Option`s (`None` = the gem's `:any`).
    ///
    /// * `user = None` reads the catalog (`roles_matching` with
    ///   `types = Self::descendant_types()`, resource_adapter.rb:6-11
    ///   branch).
    /// * `user = Some(holder)` reads `roles_of(holder)` and filters
    ///   kernel-side on the type family plus the name
    ///   (`resource_adapter.rb:7` mirror, zero additional SPI).
    ///
    /// A provided assoc fn with `where Self: Sized` (D-09 dyn-safety:
    /// `Box<dyn Resource>` keeps compiling). Resource-side statics never
    /// consult `strict` (the finders.rb:4 vs resource.rb asymmetry).
    ///
    /// # Errors
    ///
    /// Propagates the store errors of `roles_matching` / `roles_of`.
    fn find_roles<S>(
        rolify: &mut Rolify<S>,
        name: Option<&RoleName>,
        user: Option<&ResourceId>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, S::Error>> + Send
    where
        Self: Sized,
        S: RoleStore,
    {
        async move {
            if let Some(holder) = user {
                let (store, conn) = rolify.store_with_conn();
                let rows = store.roles_of(&mut *conn, holder).await?;
                let family = Self::descendant_types();
                Ok(rows
                    .into_iter()
                    .filter(|row| {
                        row.resource_type
                            .as_deref()
                            .is_some_and(|row_type| family.contains(&row_type))
                            && name.is_none_or(|wanted| *wanted == row.name)
                    })
                    .collect())
            } else {
                let family = Self::descendant_types();
                let query = RoleCatalogQuery::for_types(&family);
                let query = match name {
                    Some(wanted) => query.with_name(wanted),
                    None => query,
                };
                let (store, conn) = rolify.store_with_conn();
                let rows = store.roles_matching(&mut *conn, &query).await?;
                Ok(rows)
            }
        }
    }

    /// Class-level `applied_roles(children)` (resource.rb:36-38): the
    /// class-scoped rows of the type family (`children = true`,
    /// resource_adapter.rb:32-38) or of exactly `Self` (`children =
    /// false`, l.34-36).
    ///
    /// Same D-09/D-07 shapes as [`Resource::find_roles`]: static with
    /// `where Self: Sized`, never reads `strict`.
    ///
    /// # Errors
    ///
    /// Propagates the store errors of `roles_matching`.
    fn applied_roles<S>(
        rolify: &mut Rolify<S>,
        children: bool,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, S::Error>> + Send
    where
        Self: Sized,
        S: RoleStore,
    {
        async move {
            let family = if children {
                Self::descendant_types()
            } else {
                vec![Self::type_name()]
            };
            let query = RoleCatalogQuery::for_types(&family).class_only();
            let (store, conn) = rolify.store_with_conn();
            let rows = store.roles_matching(&mut *conn, &query).await?;
            Ok(rows)
        }
    }

    /// Instance `roles` (RSRC-05, `resource_spec` l.488): ALL rows bound to
    /// that instance regardless of holder (the admin's row AND any other
    /// holder's row match). Same D-09 shape as [`Resource::find_roles`]:
    /// static with `where Self: Sized`, never consults `strict`.
    ///
    /// `Self: Sync` because the returned future holds `&Self` across the
    /// store round-trip (SPI futures are `Send`-bound).
    ///
    /// # Errors
    ///
    /// Propagates the store errors of `roles_matching`.
    fn roles_of_instance<S>(
        rolify: &mut Rolify<S>,
        instance: &Self,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, S::Error>> + Send
    where
        Self: Sized + Sync,
        S: RoleStore,
    {
        async move {
            let id = instance.resource_id();
            let types = [Self::type_name()];
            let query = RoleCatalogQuery::for_types(&types).instance_only(Some(&id));
            let (store, conn) = rolify.store_with_conn();
            let rows = store.roles_matching(&mut *conn, &query).await?;
            Ok(rows)
        }
    }

    /// Instance `applied_roles` (resource.rb:44-47): the instance-bound
    /// rows UNION the class-scoped rows of the type family
    /// (`self.roles + self.class.applied_roles(true)`), deduped set-wise
    /// (D-04 order-free). Same D-09 shape as [`Resource::find_roles`]:
    /// static with `where Self: Sized`, never consults `strict`.
    ///
    /// `Self: Sync` for the same reason as
    /// [`Resource::roles_of_instance`]: the future holds `&Self` across
    /// the two composed reads.
    ///
    /// # Errors
    ///
    /// Propagates the store errors of the two composed reads.
    fn applied_roles_of_instance<S>(
        rolify: &mut Rolify<S>,
        instance: &Self,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, S::Error>> + Send
    where
        Self: Sized + Sync,
        S: RoleStore,
    {
        async move {
            let mut combined = Self::roles_of_instance(rolify, instance).await?;
            let class_roles = Self::applied_roles(rolify, true).await?;
            for row in class_roles {
                if !combined.contains(&row) {
                    combined.push(row);
                }
            }
            Ok(combined)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    struct Forum {
        id: i64,
    }

    impl Resource for Forum {
        fn type_name() -> &'static str {
            "Forum"
        }

        fn resource_id(&self) -> ResourceId {
            ResourceId::from(self.id)
        }
    }

    #[test]
    fn descendant_types_defaults_to_self_only() {
        assert_eq!(Forum::descendant_types(), vec!["Forum"]);
    }

    #[test]
    fn resource_trait_stays_dyn_compatible() {
        // No associated consts - `Box<dyn Resource>` must compile (SC-3).
        let forum: Box<dyn Resource> = Box::new(Forum { id: 3 });
        assert_eq!(forum.resource_id().as_str(), "3");
    }

    #[test]
    fn resource_ref_has_exactly_three_variants() {
        let id = ResourceId::from(1_i64);
        let refs = [
            ResourceRef::Global,
            ResourceRef::Class("Forum"),
            ResourceRef::Instance("Forum", &id),
        ];
        assert_eq!(refs.len(), 3);
    }

    // ---- STI shape-only contract (RSRC-06 wiring is Phase 2) ----

    struct Vehicle {
        id: i64,
    }
    impl Resource for Vehicle {
        fn type_name() -> &'static str {
            "Vehicle"
        }
        // STI override: Vehicle has descendant Car (port of
        // `relation_types_for`, `adapters/base.rb:27-28`).
        fn descendant_types() -> Vec<&'static str> {
            vec!["Vehicle", "Car"]
        }
        fn resource_id(&self) -> ResourceId {
            ResourceId::from(self.id)
        }
    }

    struct Car {
        id: i64,
    }
    impl Resource for Car {
        fn type_name() -> &'static str {
            "Car"
        }
        fn resource_id(&self) -> ResourceId {
            ResourceId::from(self.id)
        }
    }

    #[test]
    fn descendant_types_override_returns_base_plus_descendants() {
        assert_eq!(Vehicle::descendant_types(), vec!["Vehicle", "Car"]);
        assert_eq!(Car::descendant_types(), vec!["Car"]);
    }

    #[test]
    fn user_side_matching_never_expands_descendants() {
        // Pitfall 1 drift guard: a `Class(Base)` query must NOT match rows
        // scoped to a descendant type. `descendant_types` feeds ONLY
        // resource-side finders (Phase 2, RSRC-6) - never the ladder.
        let rows = vec![crate::role::RoleRecord::for_instance("vip", "Car", 1_i64)];
        let name = crate::role::RoleName::from("vip");
        let id = ResourceId::from(1_i64);
        let base_query = crate::query::RoleQuery {
            name: &name,
            filter: crate::query::ResourceFilter::Class("Vehicle"),
        };
        assert!(
            crate::kernel::where_(&rows, &base_query).is_empty(),
            "descendant-typed rows are invisible to base-type user queries"
        );
        // sanity: the descendant's own type does match
        let own_query = crate::query::RoleQuery {
            name: &name,
            filter: crate::query::ResourceFilter::Instance("Car", &id),
        };
        assert_eq!(crate::kernel::where_(&rows, &own_query).len(), 1);
    }
}
