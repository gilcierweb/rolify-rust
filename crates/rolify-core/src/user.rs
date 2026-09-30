//! [`RolifyUser`] - the consumer trait carrying the gem's `Role` concern
//! logic (`rolify/lib/rolify/role.rb`) as provided methods, written once.
//!
//! Storage steps delegate to the adapter SPI ([`crate::store::RoleStore`]);
//! every behavioral knob (strict mode, callbacks, table names) flows through
//! the required [`RolifyUser::rolify_config`] seam.

// `Future` is named in the provided signatures in async mode only;
// maybe-async strips the `impl Future` return type in `is_sync` mode.
#[cfg(not(feature = "is_sync"))]
use core::future::Future;

use crate::config::RolifyConfig;
use crate::query::{ResourceFilter, RoleQuery};
use crate::role::RoleName;
use crate::store::RoleStore;

/// Implement on your user/account type to give it roles.
///
/// Three required members: the store accessor, the pinned config seam, and a
/// split borrow handing the SPI `(&mut store, &mut conn)` at once (two
/// independent `&mut self` accessors could never do that in safe Rust - one
/// method must split the disjoint fields). The provided methods then carry
/// the gem's concern logic - the `method_missing` shortcuts of `role.rb` are
/// intentionally **not** ported; call `has_role(RoleName::from("admin"))`.
#[maybe_async::maybe_async(AFIT)]
pub trait RolifyUser: Send + Sync + 'static {
    /// The store owning this user's role rows (static dispatch - the SPI is
    /// not `dyn`-able by design).
    type Store: RoleStore;

    /// Borrow the store.
    fn store(&mut self) -> &mut Self::Store;

    /// Borrow the resolved configuration (pinned seam - all strict/callback
    /// routing keys off this from 01-03/01-04 onward).
    fn rolify_config(&self) -> &RolifyConfig;

    /// Borrow store and connection as one disjoint split - the SPI's
    /// `(&Store, &mut Conn)` call shape requires both at once.
    fn store_with_conn(&mut self)
        -> (&mut Self::Store, &mut <Self::Store as RoleStore>::Conn);

    /// `has_role?(name)` on the global scope (gem `resource = nil`).
    ///
    /// Strict-sensitive routing (`rolify_config().strict_engages_for(...)`)
    /// and resource-scoped variants complete in 01-04 once the full SPI
    /// surface exists; the seam is already on the trait.
    fn has_role(
        &mut self,
        name: &RoleName,
    ) -> impl Future<Output = Result<bool, <Self::Store as RoleStore>::Error>> + Send {
        async move {
            let query = RoleQuery { name, filter: ResourceFilter::Global };
            let (store, conn) = self.store_with_conn();
            let rows = store.where_(conn, &query).await?;
            Ok(!rows.is_empty())
        }
    }
}
