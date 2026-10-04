//! `SQLx` adapter error type.
//!
//! `Error` is the backend-specific error currency for `rolify-sqlx`.
//! It wraps `SQLx`'s native error and provides `From<RolifyError>` so
//! config-validation failures flow through adapter APIs uniformly.
//! The enum is `#[non_exhaustive]` to allow future variants without
//! breaking changes. `Send + Sync` is asserted for the public API.

use rolify_core::error::RolifyError;
use sqlx::Error as SqlxError;
use thiserror::Error;

/// Errors returned by the `SQLx` adapter.
///
/// # Variants
///
/// - `Sqlx`: wraps `sqlx::Error` (query execution, connection, pool, and
///   migration errors). The inner error preserves `SQLx`'s full diagnostic
///   information, including portable unique-violation detection through
///   `DatabaseError::code()` (PG `23505` / MySQL `23000` SQLSTATE with the
///   native `1062` also accepted / SQLite `2067`).
/// - `Core`: wraps `RolifyError` from `rolify-core` (invalid config,
///   callback veto, role not found). This path is taken when config
///   validation fails before any SQL is executed.
///
/// Both variants implement `Send + Sync + 'static` as required by the
/// `RoleStore` / `ResourceStore` SPI contracts (`store.rs:113`).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// `SQLx` execution, connection, pool, or migration error.
    #[error("sqlx error: {0}")]
    Sqlx(#[from] SqlxError),

    /// Core rolify configuration or semantic error.
    #[error("core error: {0}")]
    Core(#[from] RolifyError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Error as SqlxError;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn error_is_send_sync() {
        assert_send_sync::<Error>();
    }

    #[test]
    fn variants_render() {
        let sqlx_err = SqlxError::ColumnNotFound("name".into());
        let err = Error::Sqlx(sqlx_err);
        assert!(err.to_string().starts_with("sqlx error:"));

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
    fn from_sqlx_error_works() {
        let sqlx_err = SqlxError::RowNotFound;
        let err: Error = sqlx_err.into();
        assert!(matches!(err, Error::Sqlx(SqlxError::RowNotFound)));
    }
}
