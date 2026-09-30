//! [`RolifyConfig`] - the builder-produced, `Send + Sync` configuration
//! object that replaces the gem's global `@@` class variables
//! (`lib/rolify/configure.rb`, `lib/rolify.rb:14-36`).
//!
//! Everything a consumer tunes lives here: strict mode, empty-role cleanup,
//! table names, and the four lifecycle hooks with Result-veto semantics
//! (CONF-05 - see [`RolifyConfig::run_before_add`]).

use std::sync::Arc;

use crate::error::RolifyError;
use crate::query::ResourceFilter;
use crate::role::RoleRecord;

/// A `before_*` hook: returning `Err` **vetoes** the operation - the write
/// is aborted and the corresponding `after_*` hook never runs (CONF-05:
/// deliberate strengthening over the gem, whose HABTM callbacks raise to
/// abort but whose specs never assert the ordering).
pub type BeforeHook = Arc<dyn Fn(&RoleRecord) -> Result<(), RolifyError> + Send + Sync>;

/// An `after_*` hook: notification only - it returns `()` and cannot veto.
pub type AfterHook = Arc<dyn Fn(&RoleRecord) + Send + Sync>;

/// Global configuration for a consumer's rolify usage.
///
/// Defaults mirror the gem: `strict = false` (`rolify.rb:35` - opt-in),
/// `remove_role_if_empty = true` (`configure.rb:5`), table names `roles` /
/// `users_roles` (`rolify.rb:18-22`).
///
/// ```
/// use std::sync::Arc;
/// use rolify_core::config::RolifyConfig;
/// use rolify_core::error::RolifyError;
/// use rolify_core::role::RoleRecord;
///
/// let config = RolifyConfig::builder()
///     .strict(true)
///     .before_add(Arc::new(|record: &RoleRecord| {
///         if record.name.as_str() == "root" {
///             return Err(RolifyError::CallbackVeto {
///                 callback: "before_add",
///                 reason: "root is reserved".into(),
///             });
///         }
///         Ok(())
///     }))
///     .build()?;
/// assert!(config.strict());
/// assert_eq!(config.role_table(), "roles");
/// # Ok::<(), RolifyError>(())
/// ```
#[derive(Clone)]
pub struct RolifyConfig {
    strict: bool,
    remove_role_if_empty: bool,
    role_table: String,
    join_table: String,
    before_add: Option<BeforeHook>,
    before_remove: Option<BeforeHook>,
    after_add: Option<AfterHook>,
    after_remove: Option<AfterHook>,
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

    /// The roles table name (gem `role_cname` table). Default `"roles"`.
    #[must_use]
    pub fn role_table(&self) -> &str {
        &self.role_table
    }

    /// The join table name (gem `"#{user_table}_#{role_table}"` default).
    /// Default `"users_roles"`.
    #[must_use]
    pub fn join_table(&self) -> &str {
        &self.join_table
    }

    /// Whether strict predicates should engage for this filter (CONF-01
    /// plumbing): delegates to [`crate::kernel::strict_engages`] with this
    /// configuration's `strict` flag - true ONLY for Class/Instance filters.
    #[must_use]
    pub fn strict_engages_for(&self, filter: &ResourceFilter<'_>) -> bool {
        crate::kernel::strict_engages(self.strict, filter)
    }

    /// Run the `before_add` hook (if set). `Err` from the hook is the veto:
    /// callers MUST abort the add and skip `run_after_add`.
    ///
    /// # Errors
    ///
    /// Propagates the hook's [`RolifyError`] (typically
    /// [`RolifyError::CallbackVeto`]) when the hook vetoes the operation.
    pub fn run_before_add(&self, record: &RoleRecord) -> Result<(), RolifyError> {
        if let Some(hook) = &self.before_add {
            hook(record)?;
        }
        Ok(())
    }

    /// Run the `before_remove` hook (if set). Same veto contract as
    /// [`RolifyConfig::run_before_add`].
    ///
    /// # Errors
    ///
    /// Propagates the hook's [`RolifyError`] when the hook vetoes the removal.
    pub fn run_before_remove(&self, record: &RoleRecord) -> Result<(), RolifyError> {
        if let Some(hook) = &self.before_remove {
            hook(record)?;
        }
        Ok(())
    }

    /// Run the `after_add` hook (if set). Notification only - never vetoes.
    pub fn run_after_add(&self, record: &RoleRecord) {
        if let Some(hook) = &self.after_add {
            hook(record);
        }
    }

    /// Run the `after_remove` hook (if set). Notification only.
    pub fn run_after_remove(&self, record: &RoleRecord) {
        if let Some(hook) = &self.after_remove {
            hook(record);
        }
    }
}

impl core::fmt::Debug for RolifyConfig {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RolifyConfig")
            .field("strict", &self.strict)
            .field("remove_role_if_empty", &self.remove_role_if_empty)
            .field("role_table", &self.role_table)
            .field("join_table", &self.join_table)
            .field("before_add", &self.before_add.as_ref().map(|_| "<hook>"))
            .field("before_remove", &self.before_remove.as_ref().map(|_| "<hook>"))
            .field("after_add", &self.after_add.as_ref().map(|_| "<hook>"))
            .field("after_remove", &self.after_remove.as_ref().map(|_| "<hook>"))
            .finish()
    }
}

impl Default for RolifyConfig {
    fn default() -> Self {
        Self::builder()
            .build()
            .expect("the gem's default configuration always passes validation")
    }
}

/// Builder for [`RolifyConfig`]; start with [`RolifyConfig::builder`].
///
/// `build()` validates the configuration (currently: table names must be
/// non-empty) and returns `Result` so later phases can add more invariants
/// without a breaking signature change.
#[derive(Default)]
pub struct RolifyConfigBuilder {
    strict: Option<bool>,
    remove_role_if_empty: Option<bool>,
    role_table: Option<String>,
    join_table: Option<String>,
    before_add: Option<BeforeHook>,
    before_remove: Option<BeforeHook>,
    after_add: Option<AfterHook>,
    after_remove: Option<AfterHook>,
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

    /// Override the roles table name (default `"roles"`).
    #[must_use]
    pub fn role_table(mut self, name: &str) -> Self {
        self.role_table = Some(name.to_owned());
        self
    }

    /// Override the join table name (default `"users_roles"`).
    #[must_use]
    pub fn join_table(mut self, name: &str) -> Self {
        self.join_table = Some(name.to_owned());
        self
    }

    /// Register the `before_add` veto hook.
    #[must_use]
    pub fn before_add(mut self, hook: BeforeHook) -> Self {
        self.before_add = Some(hook);
        self
    }

    /// Register the `before_remove` veto hook.
    #[must_use]
    pub fn before_remove(mut self, hook: BeforeHook) -> Self {
        self.before_remove = Some(hook);
        self
    }

    /// Register the `after_add` notification hook.
    #[must_use]
    pub fn after_add(mut self, hook: AfterHook) -> Self {
        self.after_add = Some(hook);
        self
    }

    /// Register the `after_remove` notification hook.
    #[must_use]
    pub fn after_remove(mut self, hook: AfterHook) -> Self {
        self.after_remove = Some(hook);
        self
    }

    /// Resolve the configuration.
    ///
    /// # Errors
    ///
    /// Fails with [`RolifyError::InvalidConfig`] when a table name is empty.
    pub fn build(self) -> Result<RolifyConfig, RolifyError> {
        let role_table = self.role_table.unwrap_or_else(|| "roles".to_owned());
        let join_table = self.join_table.unwrap_or_else(|| "users_roles".to_owned());
        if role_table.is_empty() {
            return Err(RolifyError::InvalidConfig {
                reason: "role_table must not be empty".into(),
            });
        }
        if join_table.is_empty() {
            return Err(RolifyError::InvalidConfig {
                reason: "join_table must not be empty".into(),
            });
        }
        Ok(RolifyConfig {
            strict: self.strict.unwrap_or(false),
            remove_role_if_empty: self.remove_role_if_empty.unwrap_or(true),
            role_table,
            join_table,
            before_add: self.before_add,
            before_remove: self.before_remove,
            after_add: self.after_add,
            after_remove: self.after_remove,
        })
    }
}

impl core::fmt::Debug for RolifyConfigBuilder {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RolifyConfigBuilder")
            .field("strict", &self.strict)
            .field("remove_role_if_empty", &self.remove_role_if_empty)
            .field("role_table", &self.role_table)
            .field("join_table", &self.join_table)
            .field("before_add", &self.before_add.as_ref().map(|_| "<hook>"))
            .field("before_remove", &self.before_remove.as_ref().map(|_| "<hook>"))
            .field("after_add", &self.after_add.as_ref().map(|_| "<hook>"))
            .field("after_remove", &self.after_remove.as_ref().map(|_| "<hook>"))
            .finish()
    }
}

#[cfg(test)]
mod callbacks;
#[cfg(test)]
mod tests;
