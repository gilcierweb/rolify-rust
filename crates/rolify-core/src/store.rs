//! Adapter SPI - the soft-sealed [`RoleStore`] trait.
//!
//! Port of the gem's abstract adapter contract
//! (`rolify/lib/rolify/adapters/base.rb`, `RoleAdapterBase`) into one sealed
//! per-backend trait. The full method surface (`add`/`remove`/`where_strict`/
//! `exists`, plus `ResourceStore`) lands in 01-04 - nothing is invented that
//! the gem doesn't have.

// `Future` is named in the trait signatures in async mode only; maybe-async
// strips the `impl Future` return type in `is_sync` mode.
#[cfg(not(feature = "is_sync"))]
use core::future::Future;

use crate::error::RolifyError;
use crate::query::RoleQuery;
use crate::resource::ResourceRef;
use crate::role::{RoleName, RoleRecord};

#[doc(hidden)] pub mod seal {
    /// Marker supertrait gating `RoleStore` impls (soft seal - see module docs
    /// of [`crate::store`]).
    pub trait Sealed {}
}
pub use seal::Sealed;

/// The role-row storage SPI that backend adapters implement
/// (`rolify-diesel`, `rolify-sqlx`, `rolify-seaorm`, `rolify-mongodb`).
///
/// Sealing contract (soft seal - a hard `sealed`-crate seal cannot work
/// across crates, and `embedded-hal` 1.0 uses the same doc-hidden public
/// marker): implementations exist **only** inside this workspace (today:
/// `rolify-test::InMemoryStore`; from Phase 3: the adapter crates). External
/// impls are possible but unsupported.
///
/// Static dispatch by design: `type Conn` makes the trait non-`dyn`
/// (E0038) on purpose - adapters are generic parameters, not trait objects.
#[maybe_async::maybe_async(AFIT)]
pub trait RoleStore: Sealed + Send + Sync + 'static {
    /// Backend connection/pool handle.
    type Conn;

    /// Backend error - one `thiserror` enum per adapter, converting from
    /// [`RolifyError`] so core-level failures (callback veto, invalid config)
    /// flow through adapter errors uniformly.
    type Error: core::error::Error + Send + Sync + From<RolifyError> + 'static;

    /// Gem `find_or_create_by` (`role_adapter.rb`): role-row dedupe on the
    /// exact `(name, resource_type, resource_id)` triple - idempotent-add
    /// level 1 (the link-guard level 2 lives in `RolifyUser::add_role`).
    fn find_or_create_by(
        &mut self,
        conn: &mut Self::Conn,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send;

    /// Non-strict ladder query - port of `build_query`
    /// (`role_adapter.rb:106-121`); semantics fixed by
    /// [`crate::kernel::where_`].
    fn where_(
        &self,
        conn: &mut Self::Conn,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send;
}
