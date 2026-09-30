//! [`RolifyConfig`] - the builder-produced, `Send + Sync` configuration
//! object that replaces the gem's global `@@` class variables
//! (`lib/rolify/configure.rb`, `lib/rolify.rb:14-36`).

/// Global configuration for a consumer's rolify usage.
///
/// Defaults mirror the gem (`configure.rb:3-5`): strict mode **off**,
/// `remove_role_if_empty` **on**. Callback hooks
/// (`Arc<dyn Fn(&RoleRecord) -> Result<(), RolifyError> + Send + Sync>` with
/// `Err` = veto, CONF-05) arrive in 01-03 - see the `TODO(01-03)` below;
/// the `RolifyUser::rolify_config` seam they plug into already exists.
///
/// ```
/// use rolify_core::config::RolifyConfig;
///
/// let config = RolifyConfig::builder().strict(true).build();
/// assert!(config.strict());
/// assert!(config.remove_role_if_empty()); // gem default (CONF-02)
/// ```
// TODO(01-03): before_add / after_add / before_remove / after_remove hooks.
#[derive(Clone, Debug)]
pub struct RolifyConfig {
    strict: bool,
    remove_role_if_empty: bool,
}

impl RolifyConfig {
    /// Start a builder with the gem's defaults.
    #[must_use]
    pub fn builder() -> RolifyConfigBuilder {
        RolifyConfigBuilder::default()
    }

    /// Strict mode (`self.strict_rolify`): class/instance role queries use
    /// exact-scope matching instead of the override ladder. Default `false`.
    #[must_use]
    pub fn strict(&self) -> bool {
        self.strict
    }

    /// Delete the role row itself when its last membership is revoked
    /// (`remove_role_if_empty`). Default `true`.
    #[must_use]
    pub fn remove_role_if_empty(&self) -> bool {
        self.remove_role_if_empty
    }
}

impl Default for RolifyConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Builder for [`RolifyConfig`]; start with [`RolifyConfig::builder`].
#[derive(Debug, Default)]
pub struct RolifyConfigBuilder {
    strict: Option<bool>,
    remove_role_if_empty: Option<bool>,
}

impl RolifyConfigBuilder {
    /// Enable/disable strict mode (default `false`, CONF-01).
    #[must_use]
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }

    /// Enable/disable empty-role cleanup (default `true`, CONF-02).
    #[must_use]
    pub fn remove_role_if_empty(mut self, yes: bool) -> Self {
        self.remove_role_if_empty = Some(yes);
        self
    }

    /// Resolve the configuration.
    #[must_use]
    pub fn build(self) -> RolifyConfig {
        RolifyConfig {
            strict: self.strict.unwrap_or(false),
            remove_role_if_empty: self.remove_role_if_empty.unwrap_or(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_gem() {
        let config = RolifyConfig::default();
        assert!(!config.strict()); // rolify.rb:35 - strict is opt-in
        assert!(config.remove_role_if_empty()); // configure.rb:5
    }

    #[test]
    fn builder_applies_overrides() {
        let config = RolifyConfig::builder()
            .strict(true)
            .remove_role_if_empty(false)
            .build();
        assert!(config.strict());
        assert!(!config.remove_role_if_empty());
    }

    #[test]
    fn config_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RolifyConfig>();
    }
}
