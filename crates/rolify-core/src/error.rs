//! Public error type for the rolify workspace.
//!
//! `RolifyError` is the crate's single fallible-API currency (QUAL-02): consumers
//! can `match` on variants, and every adapter crate converts it into its own
//! backend error enum via `From<RolifyError>`. Boxed opaque error types are
//! never used in a public API.

/// Error type for all rolify-core operations.
///
/// The enum is `#[non_exhaustive]`: new variants may be added in later minor
/// releases without a breaking change.
///
/// # Example
///
/// ```
/// use rolify_core::error::RolifyError;
///
/// let err = RolifyError::CallbackVeto {
///     callback: "before_add",
///     reason: "policy says no".into(),
/// };
/// assert!(matches!(err, RolifyError::CallbackVeto { .. }));
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RolifyError {
    /// The requested role does not exist (at the queried scope).
    #[error("role not found: {name}")]
    RoleNotFound {
        /// The role name that was looked up, byte-exact.
        name: String,
    },

    /// A `before_*` callback returned `Err`, vetoing the operation (CONF-05).
    /// The corresponding `after_*` hook is skipped when this is raised.
    #[error("callback `{callback}` vetoed the operation: {reason}")]
    CallbackVeto {
        /// Which hook vetoed (`before_add` or `before_remove`).
        callback: &'static str,
        /// Human-readable reason supplied by the hook.
        reason: String,
    },

    /// The `RolifyConfig` carries an inconsistent or unsupported combination.
    #[error("invalid rolify configuration: {reason}")]
    InvalidConfig {
        /// What is wrong with the configuration.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn rolify_error_is_send_sync() {
        assert_send_sync::<RolifyError>();
    }

    #[test]
    fn variants_render() {
        let err = RolifyError::RoleNotFound {
            name: "admin".into(),
        };
        assert_eq!(err.to_string(), "role not found: admin");
        let err = RolifyError::CallbackVeto {
            callback: "before_add",
            reason: "nope".into(),
        };
        assert_eq!(
            err.to_string(),
            "callback `before_add` vetoed the operation: nope"
        );
        let err = RolifyError::InvalidConfig {
            reason: "empty table name".into(),
        };
        assert_eq!(
            err.to_string(),
            "invalid rolify configuration: empty table name"
        );
    }
}
