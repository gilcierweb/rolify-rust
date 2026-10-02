//! [`Rolify`] - the engine handle: store plus connection plus configuration.
//!
//! `Rolify<S>` mirrors the Phase-1 consumer shape (`Player { store, conn,
//! config }`): construction is cheap per request or task (pool checkout,
//! then `Rolify::new`), there is no global mutable state (QUAL-04), and
//! [`Rolify::config`] is the ONE `RolifyConfig` source of truth (D-06/D-07):
//! consumers implementing [`RolifyUser::rolify_config`] point it at a clone
//! of this same instance (cheap - the hooks are `Arc`). The 02-05/02-07/02-08
//! assoc fns receive `&mut Rolify<_>` and split it through
//! [`Rolify::store_with_conn`].
//!
//! `Rolify<S>` is `Send + Sync` whenever `S::Conn: Sync` ([`RoleStore`]
//! guarantees `Send` on `Conn` and `Send + Sync` on `S`; `RolifyConfig` is
//! `Send + Sync` via `Arc` hooks; all four target backends' connections
//! are `Sync`).
//!
//! No async members: construction and accessors are pure data movement, so
//! this module carries no `maybe_async` attribute.
//!
//! [`RolifyUser::rolify_config`]: crate::user::RolifyUser::rolify_config
//! [`RoleStore`]: crate::store::RoleStore
//!
//! # Example
//!
//! ```rust
//! use rolify_core::config::RolifyConfig;
//! use rolify_core::manager::Rolify;
//! use rolify_test::InMemoryStore;
//!
//! let mut engine = Rolify::new(InMemoryStore::new(), (), RolifyConfig::default());
//! assert!(!engine.config().strict());
//! let (store, _conn) = engine.store_with_conn();
//! assert_eq!(store.assertion_len(), 0);
//! ```

use crate::config::RolifyConfig;
use crate::store::RoleStore;

/// The engine handle owning one store, its connection, and the single
/// configuration (D-06/D-07). See the module docs for the ownership story.
pub struct Rolify<S: RoleStore> {
    store: S,
    conn: S::Conn,
    config: RolifyConfig,
}

impl<S: RoleStore> Rolify<S> {
    /// Bundle a store, its connection, and the resolved configuration.
    /// Cheap per request or task: check a connection out of the pool,
    /// build the handle, run the calls, drop it.
    #[must_use]
    pub fn new(store: S, conn: S::Conn, config: RolifyConfig) -> Self {
        Self {
            store,
            conn,
            config,
        }
    }

    /// The configuration every call on this handle consults (D-07: the one
    /// source of truth - implementations of
    /// [`RolifyUser::rolify_config`](crate::user::RolifyUser::rolify_config)
    /// point at a clone of this same instance).
    #[must_use]
    pub fn config(&self) -> &RolifyConfig {
        &self.config
    }

    /// Split the handle into its store and connection halves - the
    /// delegation seam: the 02-05/02-07/02-08 assoc fns receive
    /// `&mut Rolify<_>` and split through this.
    pub fn store_with_conn(&mut self) -> (&mut S, &mut S::Conn) {
        (&mut self.store, &mut self.conn)
    }
}
