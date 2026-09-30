//! rolify-test - test-support crate for the rolify workspace.
//!
//! [`InMemoryStore`] is the reference [`RoleStore`] implementation (TEST-03):
//! it keeps role rows in a `Vec` and delegates **every** matching decision to
//! the pure kernel, so the "cached vs queried" consistency of the gem's
//! semantics is provable by construction instead of by cross-checking two
//! hand-written ladders. It lives here (not in `rolify-core`) from day 1
//! (D-07): core stays zero-backend, and tests/doctests get a real store with
//! no Docker.
//!
//! Orphan-rule note: the `Sealed` + [`RoleStore`] impls below are legal
//! because `InMemoryStore` is local to this crate - the same pattern every
//! backend adapter crate follows (Pitfall 4: never impl core traits for
//! foreign types).

// `Future` is named in the impl signatures in async mode only; maybe-async
// strips the `impl Future` return type in `is_sync` mode.
#[cfg(not(feature = "is_sync"))]
use core::future::Future;

use maybe_async::maybe_async;
use rolify_core::query::RoleQuery;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{RoleName, RoleRecord};
use rolify_core::store::{RoleStore, Sealed};
use rolify_core::RolifyError;

/// In-memory [`RoleStore`] - the workspace's validation target.
///
/// `Conn` is `()`: there is no external connection to manage.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    rows: Vec<RoleRecord>,
}

impl InMemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a role row verbatim (test-setup primitive - no dedupe).
    pub fn insert(&mut self, record: RoleRecord) {
        self.rows.push(record);
    }

    /// All stored rows, in insertion order.
    #[must_use]
    pub fn rows(&self) -> &[RoleRecord] {
        &self.rows
    }
}

impl Sealed for InMemoryStore {}

#[maybe_async(AFIT)]
impl RoleStore for InMemoryStore {
    type Conn = ();
    type Error = RolifyError;

    /// Gem `find_or_create_by` (`role_adapter.rb`) - level-1 dedupe: return
    /// the existing row when the `(name, resource_type, resource_id)` triple
    /// is already present, else insert it.
    fn find_or_create_by(
        &mut self,
        _conn: &mut Self::Conn,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send {
        let (resource_type, resource_id) = match scope {
            ResourceRef::Global => (None, None),
            ResourceRef::Class(type_name) => (Some(type_name.to_owned()), None),
            ResourceRef::Instance(type_name, id) => {
                (Some(type_name.to_owned()), Some(id.clone()))
            }
        };
        let record = if let Some(existing) = self.rows.iter().find(|record| {
            record.name == *name
                && record.resource_type == resource_type
                && record.resource_id == resource_id
        }) {
            existing.clone()
        } else {
            let record = RoleRecord::new(name.clone(), resource_type, resource_id);
            self.rows.push(record.clone());
            record
        };
        async move { Ok(record) }
    }

    /// Non-strict ladder - delegates the decision to the pure kernel.
    fn where_(
        &self,
        _conn: &mut Self::Conn,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let rows: Vec<RoleRecord> = rolify_core::kernel::where_(&self.rows, query)
            .into_iter()
            .cloned()
            .collect();
        async move { Ok(rows) }
    }
}
