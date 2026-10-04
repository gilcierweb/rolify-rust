//! Diesel adapter error type.
//!
//! `Error` is the backend-specific error currency for `rolify-diesel`.
//! It wraps Diesel's native error and provides `From<RolifyError>` so
//! config-validation failures flow through adapter APIs uniformly.
//! The enum is `#[non_exhaustive]` to allow future variants without
//! breaking changes. `Send + Sync` is asserted for the public API.

use diesel::result::Error as DieselError;
use rolify_core::error::RolifyError;
use thiserror::Error;

/// Errors returned by the Diesel adapter.
///
/// # Variants
///
/// - `Diesel` — wraps `diesel::result::Error` (query execution, connection,
///   migration errors). The inner error preserves Diesel's full diagnostic
///   information including SQLSTATE codes for portable unique-violation
///   detection (`DatabaseErrorKind::UniqueViolation`).
/// - `Core` — wraps `RolifyError` from `rolify-core` (invalid config,
///   callback veto, role not found). This path is taken when config
///   validation fails before any SQL is executed.
///
/// Both variants implement `Send + Sync + 'static` as required by the
/// `RoleStore` / `ResourceStore` SPI contracts (`store.rs:112`).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Diesel execution or connection error.
    #[error("diesel error: {0}")]
    Diesel(#[from] DieselError),

    /// Core rolify configuration or semantic error.
    #[error("core error: {0}")]
    Core(#[from] RolifyError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::result::Error as DieselError;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn error_is_send_sync() {
        assert_send_sync::<Error>();
    }

    #[test]
    fn variants_render() {
        let diesel_err = DieselError::QueryBuilderError("test".into());
        let err = Error::Diesel(diesel_err);
        assert!(err.to_string().starts_with("diesel error:"));

        let core_err = RolifyError::InvalidConfig {
            reason: "bad table".into(),
        };
        let err = Error::Core(core_err);
        assert_eq!(
            err.to_string(),
            "core error: invalid rolify configuration: bad table"
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
    fn from_diesel_error_works() {
        let diesel_err = DieselError::NotFound;
        let err: Error = diesel_err.into();
        assert!(matches!(err, Error::Diesel(DieselError::NotFound)));
    }
}
