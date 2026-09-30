//! Write-side scope enum [`ResourceRef`] and the [`Resource`] consumer trait.

use crate::role::ResourceId;

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
