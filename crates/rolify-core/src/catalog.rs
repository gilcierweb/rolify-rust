//! The resource-catalog read query (D-15/D-16): [`RoleCatalogQuery`] over a
//! typed [`CatalogScope`], feeding `find_roles` and `applied_roles`
//! (RSRC-01/02/06).
//!
//! ## D-15 fine-shape resolution note (D-19 feed)
//!
//! The struct is closed per D-15: one adapter method
//! (`RoleStore::roles_matching`) takes the whole query, so filter semantics
//! live in the pure kernel once (documented on the SPI member). The
//! `InstanceOnly` payload carries the id `Option` because that is where the
//! gem's WHERE would inline it (planner-discretion resolution of D-15's
//! `Instance` scope).
//!
//! ## Dropped-`Global` note (D-19 feed)
//!
//! D-15's sketch named a `Global` scope variant, but resource-side catalog
//! reads never return global rows: the type filter structurally excludes
//! them (`resource_type IN types` never matches `None`,
//! `resource_adapter.rb:8` asymmetry). The enum is closed without it on
//! purpose.

use crate::role::{ResourceId, RoleName};

/// One resource-catalog read: a borrowed type family plus optional
/// name/scope/holder restrictions. Borrow-first: the query never
/// allocates; store impls decide whether to clone.
///
/// `None` is the typed `:any` (D-16): `name: None` means any name,
/// `holder: None` means any holder (the gem's `role_class` branch).
///
/// # Example
///
/// ```
/// use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
///
/// let query = RoleCatalogQuery::for_types(&["Forum"]);
/// assert!(matches!(query.scope, CatalogScope::ClassAndInstance));
/// assert!(query.name.is_none());
/// ```
#[derive(Clone, Copy, Debug)]
pub struct RoleCatalogQuery<'query> {
    /// Resource type family (`descendant_types()`: STI-inclusive).
    pub types: &'query [&'query str],
    /// Byte-exact name restriction (`None` = `:any`).
    pub name: Option<&'query RoleName>,
    /// Scope restriction (never global; see the module docs).
    pub scope: CatalogScope<'query>,
    /// Holder restriction (`None` = `:any`, the `role_class` branch).
    pub holder: Option<&'query ResourceId>,
}

impl<'query> RoleCatalogQuery<'query> {
    /// Query over a type family: every scope, any name, any holder.
    ///
    /// ```
    /// use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    ///
    /// let query = RoleCatalogQuery::for_types(&["Forum", "Group"]);
    /// assert_eq!(query.types.len(), 2);
    /// ```
    #[must_use]
    pub fn for_types(types: &'query [&'query str]) -> Self {
        Self {
            types,
            name: None,
            scope: CatalogScope::ClassAndInstance,
            holder: None,
        }
    }

    /// Restrict to one role name (the gem's `role_name != :any` branch).
    ///
    /// ```
    /// use rolify_core::catalog::RoleCatalogQuery;
    /// use rolify_core::role::RoleName;
    ///
    /// let name = RoleName::from("admin");
    /// let query = RoleCatalogQuery::for_types(&["Forum"]).with_name(&name);
    /// assert!(query.name.is_some());
    /// ```
    #[must_use]
    pub fn with_name(mut self, name: &'query RoleName) -> Self {
        self.name = Some(name);
        self
    }

    /// Restrict to rows linked to one holder (the gem's `user.roles`
    /// branch, `resource_adapter.rb:7`).
    ///
    /// ```
    /// use rolify_core::catalog::RoleCatalogQuery;
    /// use rolify_core::role::ResourceId;
    ///
    /// let holder = ResourceId::from(1_i64);
    /// let query = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&holder);
    /// assert!(query.holder.is_some());
    /// ```
    #[must_use]
    pub fn with_holder(mut self, holder: &'query ResourceId) -> Self {
        self.holder = Some(holder);
        self
    }

    /// Restrict to class-scoped rows (`resource_id IS NULL`).
    ///
    /// ```
    /// use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    ///
    /// let query = RoleCatalogQuery::for_types(&["Forum"]).class_only();
    /// assert!(matches!(query.scope, CatalogScope::ClassOnly));
    /// ```
    #[must_use]
    pub fn class_only(mut self) -> Self {
        self.scope = CatalogScope::ClassOnly;
        self
    }

    /// Restrict to instance-scoped rows, optionally one instance.
    ///
    /// ```
    /// use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    /// use rolify_core::role::ResourceId;
    ///
    /// let id = ResourceId::from(1_i64);
    /// let query = RoleCatalogQuery::for_types(&["Forum"]).instance_only(Some(&id));
    /// assert!(matches!(
    ///     query.scope,
    ///     CatalogScope::InstanceOnly { .. }
    /// ));
    /// ```
    #[must_use]
    pub fn instance_only(mut self, resource_id: Option<&'query ResourceId>) -> Self {
        self.scope = CatalogScope::InstanceOnly { resource_id };
        self
    }
}

/// Scope restriction for catalog reads. No global variant by design (see
/// the module docs): resource-side reads structurally exclude global rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogScope<'query> {
    /// Class and instance rows within `types` (the gem's default
    /// `resource_type IN (...)` branch).
    ClassAndInstance,
    /// Only `resource_id IS NULL` rows (the gem's `applied_roles` branch).
    ClassOnly,
    /// Only instance rows, optionally one instance (the gem's instance
    /// `roles` read).
    InstanceOnly {
        /// The instance to match (`None` = every instance row in `types`).
        resource_id: Option<&'query ResourceId>,
    },
}
