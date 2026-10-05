//! `SeaORM` adapter error type.
//!
//! `Error` is the backend-specific error currency for `rolify-seaorm`.
//! It wraps `SeaORM`'s native error and provides `From<RolifyError>` so
//! config-validation failures flow through adapter APIs uniformly.
//! The enum is `#[non_exhaustive]` to allow future variants without
//! breaking changes. `Send + Sync` is asserted for the public API.

use rolify_core::error::RolifyError;
use thiserror::Error;

/// Errors returned by the `SeaORM` adapter.
///
/// # Variants
///
/// - `Db` - wraps `sea_orm::DbErr` (query execution, connection, schema
///   errors). The inner error preserves `SeaORM`'s full diagnostic
///   information; the `find_or_create` race arm classifies portable
///   unique violations via `DbErr::sql_err()` (a unique-key or
///   foreign-key constraint violation across `MySQL`, Postgres and
///   `SQLite`), instead of string-matching driver messages.
/// - `Core` - wraps `RolifyError` from `rolify-core` (invalid config,
///   callback veto, role not found). This path is taken when config
///   validation fails before any SQL is executed.
///
/// Both variants implement `Send + Sync + 'static` as required by the
/// `RoleStore` / `ResourceStore` SPI contracts
/// (`rolify-core/src/store.rs`).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// `SeaORM` execution or connection error.
    #[error("seaorm error: {0}")]
    Db(#[from] sea_orm::DbErr),

    /// Core rolify configuration or semantic error.
    #[error("core error: {0}")]
    Core(#[from] RolifyError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::DbErr;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn error_is_send_sync() {
        assert_send_sync::<Error>();
    }

    #[test]
    fn variants_render() {
        let db_err = DbErr::Custom("test".to_owned());
        let err = Error::Db(db_err);
        assert!(err.to_string().starts_with("seaorm error:"));

        let core_err = RolifyError::InvalidConfig {
            reason: "bad table".to_owned(),
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
    fn from_db_err_works() {
        let db_err = DbErr::RecordNotFound("role".to_owned());
        let err: Error = db_err.into();
        assert!(matches!(err, Error::Db(DbErr::RecordNotFound(_))));
    }
}
