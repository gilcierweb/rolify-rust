use rolify_core::error::RolifyError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CliError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("file already exists: {files}")]
    AlreadyExists { files: String },

    #[error("core error: {0}")]
    Core(#[from] RolifyError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolify_core::error::RolifyError;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn error_is_send_sync() {
        assert_send_sync::<CliError>();
    }

    #[test]
    fn variants_render() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let err = CliError::Io(io_err);
        assert!(err.to_string().starts_with("I/O error:"));

        let err = CliError::AlreadyExists {
            files: "up.sql".into(),
        };
        assert_eq!(err.to_string(), "file already exists: up.sql");

        let core_err = RolifyError::InvalidConfig {
            reason: "bad table".into(),
        };
        let err = CliError::Core(core_err);
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
        let err: CliError = core_err.into();
        assert!(matches!(
            err,
            CliError::Core(RolifyError::RoleNotFound { .. })
        ));
    }

    #[test]
    fn from_io_error_works() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: CliError = io_err.into();
        assert!(matches!(err, CliError::Io(_)));
    }
}
