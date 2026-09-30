//! Query-side types: [`ResourceFilter`] (read scopes) and [`RoleQuery`]
//! (a borrowed name plus a filter - the port of the gem's
//! `{name:, resource:}` query hash).

use crate::role::{ResourceId, RoleName};

/// Scope restriction for role **queries**.
///
/// Port of the gem's `nil | Class | instance | :any` resource argument in
/// `build_query` (`role_adapter.rb:106-121`). `Any` exists only on the query
/// side - writes use [`crate::resource::ResourceRef`], which has no `Any`.
///
/// # Example
///
/// ```
/// use rolify_core::query::ResourceFilter;
/// use rolify_core::role::ResourceId;
///
/// let global = ResourceFilter::Global;
/// let id = ResourceId::from(7_i64);
/// let instance = ResourceFilter::Instance("Forum", &id);
/// assert!(matches!(instance, ResourceFilter::Instance("Forum", _)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceFilter<'a> {
    /// `resource = nil` - global rows only.
    Global,
    /// `resource = Forum` - the class (type) scope; global rows also satisfy
    /// the non-strict ladder (the "global override").
    Class(&'a str),
    /// `resource = forum` - one specific instance; global and class rows of
    /// the same type also satisfy the non-strict ladder.
    Instance(&'a str, &'a ResourceId),
    /// `resource = :any` - name-only match, **including** global rows. This
    /// is the gem's DB path (`build_query:107` short-circuits to `name = ?`),
    /// ratified as D-02 - see the crate docs / parity matrix.
    Any,
}

/// One role lookup: a borrowed role name plus a scope filter.
///
/// Borrow-first (OQ-4): the query never allocates - `name` is `&'a RoleName`,
/// committed; store impls decide whether to clone.
///
/// # Example
///
/// ```
/// use rolify_core::query::{ResourceFilter, RoleQuery};
/// use rolify_core::role::RoleName;
///
/// let name = RoleName::from("admin");
/// let query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
/// assert!(matches!(query.filter, ResourceFilter::Any));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct RoleQuery<'a> {
    /// The role name to match (byte-exact).
    pub name: &'a RoleName,
    /// The scope restriction (see [`ResourceFilter`]).
    pub filter: ResourceFilter<'a>,
}

impl<'a> RoleQuery<'a> {
    /// `has_role(name)` with the gem's default `resource = nil` - a
    /// global-scope query.
    ///
    /// ```
    /// use rolify_core::query::{ResourceFilter, RoleQuery};
    /// use rolify_core::role::RoleName;
    ///
    /// let name = RoleName::from("admin");
    /// let query = RoleQuery::with_role(&name);
    /// assert!(matches!(query.filter, ResourceFilter::Global));
    /// ```
    #[must_use]
    pub fn with_role(name: &'a RoleName) -> Self {
        Self {
            name,
            filter: ResourceFilter::Global,
        }
    }

    /// `has_role(name, resource)` with an explicit scope filter.
    ///
    /// ```
    /// use rolify_core::query::{ResourceFilter, RoleQuery};
    /// use rolify_core::role::{ResourceId, RoleName};
    ///
    /// let name = RoleName::from("moderator");
    /// let id = ResourceId::from(3_i64);
    /// let query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Instance("Forum", &id));
    /// assert!(matches!(query.filter, ResourceFilter::Instance("Forum", _)));
    /// ```
    #[must_use]
    pub fn with_role_and_filter(name: &'a RoleName, filter: ResourceFilter<'a>) -> Self {
        Self { name, filter }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn resource_filter_has_exactly_four_variants() {
        let id = ResourceId::from(1_i64);
        let filters = [
            ResourceFilter::Global,
            ResourceFilter::Class("Forum"),
            ResourceFilter::Instance("Forum", &id),
            ResourceFilter::Any,
        ];
        assert_eq!(filters.len(), 4);
    }

    #[test]
    fn with_role_defaults_to_global_filter() {
        let name = RoleName::from("admin");
        let query = RoleQuery::with_role(&name);
        assert!(matches!(query.filter, ResourceFilter::Global));
        assert!(core::ptr::eq(
            core::ptr::from_ref(query.name),
            core::ptr::from_ref(&name)
        ));
    }

    #[test]
    fn with_role_and_filter_keeps_both_halves_borrowed() {
        let name = RoleName::from("manager");
        let id = ResourceId::from(42_i64);
        let query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Instance("Forum", &id));
        assert!(matches!(
            query.filter,
            ResourceFilter::Instance("Forum", rid) if rid.as_str() == "42"
        ));
        assert!(core::ptr::eq(
            core::ptr::from_ref(query.name),
            core::ptr::from_ref(&name)
        ));
    }
}
