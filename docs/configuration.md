# Configuration Guide

rolify-rust uses `RolifyConfig` as the single source of truth for all behavioral knobs. This replaces the Ruby gem's global `@@` class variables.

## Creating Configuration

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::error::RolifyError;
use std::sync::Arc;
use rolify_core::role::RoleRecord;

let config = RolifyConfig::builder()
    .strict(true)
    .remove_role_if_empty(false)
    .role_table("custom_roles")
    .join_table("users_custom_roles")
    .before_add(Arc::new(|record: &RoleRecord| {
        // Veto logic
        Ok(())
    }))
    .after_add(Arc::new(|record: &RoleRecord| {
        // Notification logic
    }))
    .before_remove(Arc::new(|record: &RoleRecord| {
        // Veto logic
        Ok(())
    }))
    .after_remove(Arc::new(|record: &RoleRecord| {
        // Notification logic
    }))
    .build()?;  // Returns Result<RolifyConfig, RolifyError>
```

## Configuration Options

### Strict Mode

```rust
// Default: false (gem default: opt-in via `strict_rolify`)
let config = RolifyConfig::builder().strict(true).build()?;
```

When `strict = true`:
- Class/Instance queries use **exact scope matching only**
- Global roles do NOT cover class/instance queries
- Class roles do NOT cover instance queries
- `ResourceFilter::Any` still matches by name only (no strict gate)

When `strict = false` (default):
- The full override ladder applies (Global -> Class -> Instance)
- Global roles satisfy Class and Instance queries
- Class roles satisfy Instance queries of the same type

### Remove Role If Empty

```rust
// Default: true (gem default: true via `configure.rb:5`)
let config = RolifyConfig::builder().remove_role_if_empty(false).build()?;
```

When `true` (default): When the last membership of a role row is revoked, the role row itself is deleted.

When `false`: Role rows persist even with zero members.

### Table Names

```rust
// Defaults: role_table = "roles", join_table = "users_roles"
let config = RolifyConfig::builder()
    .role_table("privileges")
    .join_table("users_privileges")
    .build()?;
```

Gem mapping: `rolify :role_cname => 'Privilege'` sets `role_table_name = "privileges"` and derives `role_join_table_name = "users_privileges"` (user table + "_" + role table). Rust has no tableize conventions, so both names are given explicitly here; the values are identical.

**Validation rules** (D-08 identifier allow-list):
- Must start with ASCII letter (A-Z, a-z) or underscore (_)
- Contain only ASCII alphanumeric or underscore
- Must not be empty

```rust
// These will fail validation:
RolifyConfig::builder().role_table("").build();           // Error: empty
RolifyConfig::builder().role_table("123invalid").build(); // Error: starts with digit
RolifyConfig::builder().role_table("roles-table").build(); // Error: hyphen not allowed

// These pass:
RolifyConfig::builder().role_table("roles").build();
RolifyConfig::builder().role_table("_private_roles").build();
RolifyConfig::builder().role_table("roles_v2").build();
```

### Lifecycle Callbacks

All callbacks receive `&RoleRecord` (the would-be or affected role row).

#### before_add / before_remove (Veto Hooks)

```rust
use std::sync::Arc;
use rolify_core::error::RolifyError;
use rolify_core::role::RoleRecord;

let config = RolifyConfig::builder()
    .before_add(Arc::new(|record: &RoleRecord| {
        // Veto granting "root" role
        if record.name.as_str() == "root" {
            return Err(RolifyError::CallbackVeto {
                callback: "before_add",
                reason: "root role is reserved for system".into(),
            });
        }
        // Veto instance-scoped roles on sensitive resources
        if record.resource_type.as_deref() == Some("Billing") {
            return Err(RolifyError::CallbackVeto {
                callback: "before_add",
                reason: "Billing roles require approval workflow".into(),
            });
        }
        Ok(())
    }))
    .before_remove(Arc::new(|record: &RoleRecord| {
        // Prevent removing last global admin
        if record.name.as_str() == "admin" && record.is_global() {
            // In real app: check if other admins exist
            return Err(RolifyError::CallbackVeto {
                callback: "before_remove",
                reason: "cannot remove last global admin".into(),
            });
        }
        Ok(())
    }))
    .build()?;
```

**Veto contract:** Returning `Err` aborts the operation BEFORE any store mutation. The corresponding `after_*` hook never runs.

#### after_add / after_remove (Notification Hooks)

```rust
let config = RolifyConfig::builder()
    .after_add(Arc::new(|record: &RoleRecord| {
        // Audit log, metrics, notifications
        tracing::info!(role = %record.name, scope_type = ?record.resource_type, scope_id = ?record.resource_id, "Role granted");
        // Send to audit service, etc.
    }))
    .after_remove(Arc::new(|record: &RoleRecord| {
        tracing::info!(role = %record.name, scope_type = ?record.resource_type, scope_id = ?record.resource_id, "Role revoked");
    }))
    .build()?;
```

**Notification contract:** Returns `()` - cannot veto. Runs only after successful store mutation.

## Using Configuration

### With Rolify Engine

```rust
use rolify_core::manager::Rolify;
use rolify_test::InMemoryStore;

let config = RolifyConfig::builder().strict(true).build()?;
let mut engine = Rolify::new(InMemoryStore::new(), (), config);

// The engine's config is the single source of truth
assert!(engine.config().strict());
assert_eq!(engine.config().role_table(), "roles");
```

### With RolifyUser

```rust
impl RolifyUser for MyUser {
    type Store = MyStore;

    fn rolify_config(&self) -> &RolifyConfig {
        &self.config  // Point at a clone of the engine's config
    }
    // ...
}
```

**Cheap cloning:** `RolifyConfig` holds `Arc` hooks - cloning is just an `Arc` increment.

### Checking Strict Engagement

```rust
use rolify_core::query::ResourceFilter;

let config = RolifyConfig::builder().strict(true).build()?;

// Strict engages for Class and Instance only
assert!(config.strict_engages_for(&ResourceFilter::Class("Forum")));
assert!(config.strict_engages_for(&ResourceFilter::Instance("Forum", &ResourceId::from(1))));

// Strict does NOT engage for Global or Any
assert!(!config.strict_engages_for(&ResourceFilter::Global));
assert!(!config.strict_engages_for(&ResourceFilter::Any));
```

This is the exact logic used by `RolifyUser::has_role` to decide which store method to call.

## Default Configuration

```rust
let config = RolifyConfig::default();
// Equivalent to:
let config = RolifyConfig::builder()
    .strict(false)
    .remove_role_if_empty(true)
    .role_table("roles")
    .join_table("users_roles")
    .build()
    .unwrap();
```

## Sharing Configuration Across Components

```rust
use std::sync::Arc;

// Single config instance shared everywhere
let shared_config = Arc::new(RolifyConfig::builder().strict(true).build()?);

// Engine uses it
let engine = Rolify::new(store, conn, (*shared_config).clone());

// Users point to it
struct MyUser {
    config: Arc<RolifyConfig>,
    // ...
}

impl RolifyUser for MyUser {
    fn rolify_config(&self) -> &RolifyConfig {
        &self.config
    }
    // ...
}

// All components see the same config changes (if you mutate via interior mutability)
// or you can replace the Arc atomically for hot config reloads
```

## Configuration for Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rolify_test::InMemoryStore;

    fn test_config() -> RolifyConfig {
        RolifyConfig::builder()
            .strict(false)
            .remove_role_if_empty(true)
            .role_table("test_roles")
            .join_table("test_users_roles")
            .build()
            .unwrap()
    }

    #[test]
    fn test_with_config() {
        let mut store = InMemoryStore::new();
        let config = test_config();
        let mut engine = Rolify::new(store, (), config);
        // ...
    }
}
```

## Migration from Ruby rolify Config

| Ruby rolify | rolify-rust |
|-------------|-------------|
| `rolify` on `User` (generator `rails g rolify Role User`) | consumer implements `rolify_type()` returning `"User"` |
| `rolify :role_cname => 'Privilege'` | `role_table("privileges")` (table name) |
| `rolify :role_join_table_name => 'users_privileges'` | `join_table("users_privileges")` |
| `config.use_dynamic_shortcuts` | N/A (not ported; no `method_missing`) |
| `rolify :strict => true` | `strict(true)` |
| `config.remove_role_if_empty = false` | `remove_role_if_empty(false)` |
| `rolify :before_add => :hook` | `before_add(Arc::new(...))` |
| `rolify :after_add => :hook` | `after_add(Arc::new(...))` |
| `rolify :before_remove => :hook` | `before_remove(Arc::new(...))` |
| `rolify :after_remove => :hook` | `after_remove(Arc::new(...))` |

**Key differences:**
- No global mutable state - config is explicit and passed around
- Callbacks use `Arc<dyn Fn>` for thread-safety
- Veto is explicit `Result` return, not exception-based
- Table names validated at build time