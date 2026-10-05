//! `MongoDB` adapter error type.
//!
//! `Error` is the backend-specific error currency for `rolify-mongodb`.
//! It wraps the driver's native error and provides `From<RolifyError>` so
//! config-validation failures flow through adapter APIs uniformly.
//! The enum is `#[non_exhaustive]` to allow future variants without
//! breaking changes. `Send + Sync` is asserted for the public API.

use mongodb::error::{ErrorKind, WriteFailure};
use rolify_core::error::RolifyError;
use thiserror::Error;

/// Errors returned by the `MongoDB` adapter.
///
/// # Variants
///
/// - `Mongo` — wraps `mongodb::error::Error` (query execution, connection,
///   index convergence errors). The duplicate-key race arm is
///   `ErrorKind::Write(WriteFailure::WriteError(e))` with `e.code == 11000`
///   (D-14, the Mongo mirror of the diesel `DatabaseErrorKind::UniqueViolation`
///   path); both driver enums are `#[non_exhaustive]`, so the predicate below
///   keeps the `matches!` wildcard behavior for the driver's future variants.
/// - `Core` — wraps `RolifyError` from `rolify-core` (invalid config,
///   callback veto, role not found). This path is taken when config
///   validation fails before any BSON is sent.
///
/// Both variants implement `Send + Sync + 'static` as required by the
/// `RoleStore` / `ResourceStore` SPI contracts (`store.rs:112`).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// `MongoDB` execution or connection error.
    #[error("mongo error: {0}")]
    Mongo(#[from] mongodb::error::Error),

    /// Core rolify configuration or semantic error.
    #[error("core error: {0}")]
    Core(#[from] RolifyError),
}

/// True when `error` is a duplicate-key write failure (Mongo code 11000).
///
/// This is the D-14 race arm of the gem's `find_or_create`
/// (`rolify/lib/rolify/adapters/mongoid/role_adapter.rb:45-51` region:
/// insert, rescue duplicate, re-read); on `true` store methods re-read the
/// role document instead of surfacing the error. Both driver enums are
/// `#[non_exhaustive]`, so the predicate keeps the `matches!` wildcard
/// behavior for the driver's future variants.
#[must_use]
pub fn is_duplicate_key(error: &mongodb::error::Error) -> bool {
    is_duplicate_key_kind(&error.kind)
}

/// Kind-level predicate behind [`is_duplicate_key`], split out because the
/// driver exposes no public constructor from `ErrorKind` to its `Error`
/// (3.x API), so tests exercise the kind directly.
fn is_duplicate_key_kind(kind: &ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::Write(WriteFailure::WriteError(write_error)) if write_error.code == 11000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn error_is_send_sync() {
        assert_send_sync::<Error>();
    }

    #[test]
    fn variants_render() {
        let mongo_err = mongodb::error::Error::custom("test failure");
        let err = Error::Mongo(mongo_err);
        assert!(err.to_string().starts_with("mongo error:"));

        let core_err = RolifyError::InvalidConfig {
            reason: "bad collection".into(),
        };
        let err = Error::Core(core_err);
        assert_eq!(
            err.to_string(),
            "core error: invalid rolify configuration: bad collection"
        );
    }

    #[test]
    fn from_rolify_error_works() {
        let core_err = RolifyError::RoleNotFound {
            name: "admin".into(),
        };
        let err: Error = core_err.into();
        assert!(matches!(err, Error::Core(RolifyError::RoleNotFound { .. })));
    }

    #[test]
    fn from_mongo_error_works() {
        let mongo_err = mongodb::error::Error::custom("connection refused");
        let err: Error = mongo_err.into();
        assert!(matches!(err, Error::Mongo(_)));
    }

    #[test]
    fn duplicate_key_arm_matches_code_11000() {
        // `WriteError` is `#[non_exhaustive]`, so construct it the way the
        // driver itself does: by deserializing a server reply document.
        let reply = bson::doc! {
            "code": 11000,
            "codeName": "DuplicateKey",
            "errmsg": "E11000 duplicate key error collection: roles",
        };
        let write_error: mongodb::error::WriteError =
            bson::from_document(reply).expect("write error reply deserializes");
        let kind = ErrorKind::Write(WriteFailure::WriteError(write_error));
        assert!(is_duplicate_key_kind(&kind));

        let other_kind = ErrorKind::Custom(std::sync::Arc::new("other"));
        assert!(!is_duplicate_key_kind(&other_kind));

        let custom_error = mongodb::error::Error::custom("test failure");
        assert!(!is_duplicate_key(&custom_error));
    }
}
