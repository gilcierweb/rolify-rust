# Migration Guide: Ruby rolify -> rolify-rust

This guide helps you port applications from the Ruby [rolify](https://github.com/RolifyCommunity/rolify) gem to rolify-rust.

## Overview

| Aspect | Ruby rolify | rolify-rust |
|--------|-------------|-------------|
| **Philosophy** | Role management only (no enforcement) | Same |
| **Scopes** | Global, Class, Instance | Same (three levels) |
| **Match ladder** | Global -> Class -> Instance | Same (non-strict) |
| **Strict mode** | `config.strict_rolify` | `RolifyConfig::builder().strict(true)` |
| **Callbacks** | `before_add`, `after_add`, `before_remove`, `after_remove` | Same, with veto via `Result` |
| **Dynamic shortcuts** | `user.is_admin?` | **Not ported** (use `has_role`) |
| **ORM** | ActiveRecord / Mongoid | Diesel, SQLx, SeaORM, MongoDB |
| **Configuration** | Global class variables | Explicit `RolifyConfig` |

## Setup Comparison

### Ruby (ActiveRecord)

```ruby
# Gemfile
gem 'rolify'

# config/initializers/rolify.rb
Rolify.configure do |config|
  config.role_cname = 'Role'
  config.user_cname = 'User'
  config.join_table_name = 'users_roles'
  config.use_dynamic_shortcuts = false
  config.strict_rolify = false
  config.remove_role_if_empty = true
end

# app/models/user.rb
class User < ApplicationRecord
  rolify
end

# app/models/forum.rb
class Forum < ApplicationRecord
  resourcify
end
```

### Rust (Diesel Example)

```toml
# Cargo.toml
[dependencies]
rolify-core = { path = "../rolify-core" }
rolify-diesel = { path = "../rolify-diesel", features = ["postgres"] }
diesel = { version = "2.3", features = ["postgres", "chrono", "r2d2"] }
```

```rust
// src/models.rs
use rolify_core::config::RolifyConfig;
use rolify_core::resource::Resource;
use rolify_core::role::ResourceId;
use rolify_core::user::RolifyUser;
use rolify_diesel::DieselRoleStore;
use diesel::PgConnection;

struct User {
    id: i64,
    username: String,
    store: DieselRoleStore<PgConnection>,
    conn: PgConnection,
    config: RolifyConfig,
}

impl RolifyUser for User {
    type Store = DieselRoleStore<PgConnection>;
    fn store(&mut self) -> &mut Self::Store { &mut self.store }
    fn rolify_config(&self) -> &RolifyConfig { &self.config }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "User" }
    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut PgConnection) {
        (&mut self.store, &mut self.conn)
    }
}

struct Forum { id: i64, name: String }
impl Resource for Forum {
    fn type_name() -> &'static str { "Forum" }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}
```

## Configuration Migration

| Ruby | Rust |
|------|------|
| `config.role_cname = 'Role'` | `role_table("roles")` |
| `config.user_cname = 'User'` | `rolify_type()` returns `"User"` |
| `config.join_table_name = 'users_roles'` | `join_table("users_roles")` |
| `config.strict_rolify = true` | `strict(true)` |
| `config.remove_role_if_empty = false` | `remove_role_if_empty(false)` |
| `config.before_add { |role| ... }` | `before_add(Arc::new(|record| ...))` |
| `config.after_add { |role| ... }` | `after_add(Arc::new(|record| ...))` |
| `config.before_remove { |role| ... }` | `before_remove(Arc::new(|record| ...))` |
| `config.after_remove { |role| ... }` | `after_remove(Arc::new(|record| ...))` |
| `config.use_dynamic_shortcuts` | **Not supported** |

### Callback Differences

**Ruby (exception-based veto):**
```ruby
config.before_add do |role|
  raise Rolify::RoleNotAllowed, "Cannot add root role" if role.name == 'root'
end
```

**Rust (Result-based veto):**
```rust
use rolify_core::error::RolifyError;
use rolify_core::role::RoleRecord;
use std::sync::Arc;

let config = RolifyConfig::builder()
    .before_add(Arc::new(|record: &RoleRecord| {
        if record.name.as_str() == "root" {
            return Err(RolifyError::CallbackVeto {
                callback: "before_add",
                reason: "Cannot add root role".into(),
            });
        }
        Ok(())
    }))
    .build()?;
```

## Role Assignment

### Ruby

```ruby
# Global
user.add_role(:admin)

# Class-scoped
user.add_role(:moderator, Forum)

# Instance-scoped
user.add_role(:owner, forum)

# Alias
user.grant(:moderator, Forum)
```

### Rust

```rust
use rolify_core::resource::ResourceRef;
use rolify_core::role::RoleName;

// Global
user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;

// Class-scoped
user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;

// Instance-scoped
user.add_role(&RoleName::from("owner"), ResourceRef::Instance("Forum", &forum.resource_id())).await?;

// Alias
user.grant(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
```

## Role Checking

### Ruby

```ruby
# Non-strict (default ladder)
user.has_role?(:admin)                    # Global only
user.has_role?(:admin, Forum)             # Class + Global
user.has_role?(:admin, forum)             # Instance + Class + Global
user.has_role?(:admin, :any)              # Any scope (name only)

# Strict (exact scope)
user.has_strict_role?(:admin)             # Global only
user.has_strict_role?(:admin, Forum)      # Class only
user.has_strict_role?(:admin, forum)      # Instance only
```

### Rust

```rust
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::ResourceId;

// Non-strict (default)
user.has_role(&RoleName::from("admin"), ResourceFilter::Global).await?;
user.has_role(&RoleName::from("admin"), ResourceFilter::Class("Forum")).await?;
user.has_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &forum.resource_id())).await?;
user.has_role(&RoleName::from("admin"), ResourceFilter::Any).await?;

// Strict (exact scope)
user.has_strict_role(&RoleName::from("admin"), ResourceFilter::Global).await?;
user.has_strict_role(&RoleName::from("admin"), ResourceFilter::Class("Forum")).await?;
user.has_strict_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &forum.resource_id())).await?;

// Composite checks
user.has_all_roles(&[
    RoleQuery::with_role(&RoleName::from("admin")),
    RoleQuery::with_role_and_filter(&RoleName::from("moderator"), ResourceFilter::Class("Forum")),
]).await?;

user.has_any_roles(&[
    RoleQuery::with_role(&RoleName::from("admin")),
    RoleQuery::with_role(&RoleName::from("moderator")),
]).await?;

user.only_has_role(&RoleName::from("admin"), ResourceFilter::Global).await?;
```

## Role Removal

### Ruby

```ruby
# Remove specific role
user.remove_role(:admin)
user.remove_role(:moderator, Forum)
user.remove_role(:owner, forum)

# Alias
user.revoke(:admin)
user.revoke(:moderator, Forum)
```

### Rust

```rust
use rolify_core::kernel::RemovalTarget;
use rolify_core::role::ResourceId;

// Remove all scopes for name
user.remove_role(&RoleName::from("admin"), RemovalTarget::NameOnly).await?;

// Remove class-scoped for type
user.remove_role(&RoleName::from("moderator"), RemovalTarget::TypeSweep("Forum")).await?;

// Remove exact instance
user.remove_role(&RoleName::from("owner"), RemovalTarget::Exact("Forum", &forum.resource_id())).await?;

// Alias
user.revoke(&RoleName::from("admin"), RemovalTarget::NameOnly).await?;
```

## Role Queries (Listing)

### Ruby

```ruby
user.roles                    # All roles
user.roles_name               # Role names only
user.has_cached_role?(:admin) # Cached check (no DB)
```

### Rust

```rust
// All role records
let roles = user.roles().await?;  // Vec<RoleRecord>

// Role names only
let names = user.roles_name().await?;  // Vec<RoleName>

// Cached check (zero I/O)
use rolify_core::role::RoleSet;
let snapshot = RoleSet::new(&roles);
let has_admin = snapshot.has_cached_role(&RoleQuery::with_role(&RoleName::from("admin")));
```

## User-Class Finders (Finding Users by Role)

### Ruby

```ruby
# Class methods on User
User.with_role(:admin)
User.with_role(:moderator, Forum)
User.with_any_role(:admin, :moderator)
User.with_all_roles(:admin, :moderator)
User.without_role(:admin)
```

### Rust

```rust
use rolify_core::manager::Rolify;
use rolify_core::query::RoleQuery;
use rolify_core::role::RoleName;

let mut engine = Rolify::new(store, conn, config);

// with_role
let admins = User::with_role(&mut engine, &RoleQuery::with_role(&RoleName::from("admin"))).await?;
let moderators = User::with_role(&mut engine, &RoleQuery::with_role_and_filter(
    &RoleName::from("moderator"),
    ResourceFilter::Class("Forum"),
)).await?;

// with_any_roles
let admins_or_mods = User::with_any_roles(&mut engine, &[
    RoleQuery::with_role(&RoleName::from("admin")),
    RoleQuery::with_role_and_filter(&RoleName::from("moderator"), ResourceFilter::Class("Forum")),
]).await?;

// with_all_roles (intersection)
let both = User::with_all_roles(&mut engine, &[
    RoleQuery::with_role(&RoleName::from("admin")),
    RoleQuery::with_role_and_filter(&RoleName::from("moderator"), ResourceFilter::Class("Forum")),
]).await?;

// without_role
let non_admins = User::without_role(&mut engine, &RoleQuery::with_role(&RoleName::from("admin"))).await?;
```

## Resource-Class Finders (Finding Resources by Role)

### Ruby

```ruby
# Class methods on Forum
Forum.with_role(:admin)
Forum.with_role(:admin, user)
Forum.find_roles(:admin)
Forum.find_roles(:admin, user)
Forum.applied_roles
```

### Rust

```rust
use rolify_core::manager::Rolify;
use rolify_core::role::RoleName;
use rolify_core::resource::Resource;

let mut engine = Rolify::new(store, conn, config);

// with_role (resources where user has role)
let forums = Forum::with_role(&mut engine, &[RoleName::from("admin")], Some(&user.rolify_id())).await?;

// find_roles (roles on resource class)
let roles = Forum::find_roles(&mut engine, Some(&RoleName::from("admin")), Some(&user.rolify_id())).await?;

// applied_roles (class-scoped roles of type family)
let class_roles = Forum::applied_roles(&mut engine, true).await?;  // children = true (include descendants)

// Instance methods
let forum = Forum { id: 1, name: "Rust".into() };
let instance_roles = Forum::roles_of_instance(&mut engine, &forum).await?;
let applied = Forum::applied_roles_of_instance(&mut engine, &forum).await?;
```

## Dynamic Shortcuts (Not Ported)

Ruby rolify provides dynamic methods like `user.is_admin?`, `user.is_moderator_of?(forum)`.

**Rust equivalent:** Use explicit `has_role` calls.

```rust
// Instead of user.is_admin?
user.has_role(&RoleName::from("admin"), ResourceFilter::Global).await?

// Instead of user.is_moderator_of?(forum)
user.has_role(&RoleName::from("moderator"), ResourceFilter::Instance("Forum", &forum.resource_id())).await?

// Helper methods on your wrapper:
impl User {
    pub async fn is_admin(&mut self) -> Result<bool, Error> {
        self.has_role(&RoleName::from("admin"), ResourceFilter::Global).await
    }
    
    pub async fn is_moderator_of(&mut self, forum: &Forum) -> Result<bool, Error> {
        self.has_role(&RoleName::from("moderator"), ResourceFilter::Instance("Forum", &forum.resource_id())).await
    }
}
```

## STI (Single Table Inheritance)

### Ruby

```ruby
class Vehicle < ApplicationRecord
  resourcify
end

class Car < Vehicle
end

# Vehicle.descendant_types_for returns ["Vehicle", "Car"]
```

### Rust

```rust
use rolify_core::resource::Resource;
use rolify_core::role::ResourceId;

struct Vehicle { id: i64 }
impl Resource for Vehicle {
    fn type_name() -> &'static str { "Vehicle" }
    fn descendant_types() -> Vec<&'static str> { vec!["Vehicle", "Car", "Truck"] }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}

struct Car { id: i64 }
impl Resource for Car {
    fn type_name() -> &'static str { "Car" }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}
```

**Important:** `descendant_types` feeds ONLY resource-side finders - never the user-side match ladder (Pitfall 1 guard).

## Database Schema

### Ruby (ActiveRecord Migration)

```ruby
create_table :roles do |t|
  t.string :name
  t.references :resource, polymorphic: true
  t.timestamps
end

create_table :users_roles, id: false do |t|
  t.references :user
  t.references :role
end

add_index :roles, [:name, :resource_type, :resource_id], unique: true
add_index :roles, [:resource_type, :resource_id]
add_index :users_roles, [:user_id, :user_type]
```

### Rust (Diesel Migration)

```sql
-- migrations/20240101000000_create_rolify_tables/up.sql
CREATE TABLE roles (
    id BIGSERIAL PRIMARY KEY,
    name VARCHAR NOT NULL,
    resource_type VARCHAR,
    resource_id VARCHAR,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    UNIQUE (name, resource_type, resource_id)
);

CREATE TABLE users_roles (
    user_id VARCHAR NOT NULL,
    user_type VARCHAR NOT NULL,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, user_type, role_id)
);

CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id);
CREATE INDEX idx_users_roles_user ON users_roles (user_id, user_type);
```

**Key difference:** Rust uses `''` (empty string) sentinel instead of `NULL` for `resource_type`/`resource_id` to make `UNIQUE` constraints work identically across Postgres, MySQL, SQLite.

## Testing

### Ruby (RSpec)

```ruby
RSpec.describe User do
  it "has global admin role" do
    user.add_role(:admin)
    expect(user.has_role?(:admin)).to be true
  end
end
```

### Rust (with rolify-test)

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::query::ResourceFilter;
use rolify_core::role::{RoleName, RoleRecord};
use rolify_test::{InMemoryStore, builders::*};

#[test]
fn test_user_has_global_admin() {
    let mut store = InMemoryStore::new();
    let holder = ResourceId::from(1_i64);
    
    grant(&mut store, &holder, FixtureGrant::global("admin"));
    
    let mut user = TestUser { id: 1, store, conn: (), config: RolifyConfig::default() };
    
    let has_admin = user.has_role(&RoleName::from("admin"), ResourceFilter::Global).await.unwrap();
    assert!(has_admin);
}
```

## Common Pitfalls

### 1. String vs Integer IDs

**Ruby:** `resource_id` can be integer or string (ActiveRecord handles both).

**Rust:** Use `ResourceId::from(42_i64)` or `ResourceId::from("uuid-string")` - both work.

### 2. Case Sensitivity

**Ruby:** Role names are case-sensitive (`'Admin' != 'admin'`).

**Rust:** Same - `RoleName` uses exact byte equality.

### 3. Global Role Override

**Ruby:** Global roles satisfy class/instance queries in non-strict mode.

**Rust:** Same behavior - this is the core match ladder.

### 4. Strict Mode Scope

**Ruby:** `strict_rolify` affects all queries.

**Rust:** Strict only engages for Class/Instance filters, never for Global or Any.

### 5. Method Missing

**Ruby:** `user.is_admin?` works via `method_missing`.

**Rust:** Not supported - use explicit `has_role` calls.

### 6. Configuration Scope

**Ruby:** Global config via `Rolify.configure`.

**Rust:** Explicit `RolifyConfig` passed to each engine/user - no global state.

## Checklist for Migration

- [ ] Choose adapter (Diesel, SQLx, SeaORM, MongoDB)
- [ ] Set up database schema with migrations
- [ ] Implement `RolifyUser` on user type
- [ ] Implement `Resource` on resource types
- [ ] Configure `RolifyConfig` (strict, callbacks, table names)
- [ ] Replace `add_role` / `has_role?` / `remove_role` calls
- [ ] Replace finder methods (`with_role`, `find_roles`, etc.)
- [ ] Port callbacks to `Result`-based veto
- [ ] Replace dynamic shortcuts with explicit helpers
- [ ] Update tests to use `rolify-test` or adapter test utilities
- [ ] Verify strict mode behavior matches expectations
- [ ] Check STI `descendant_types` implementation

## Parity Matrix

See `PARITY.md` in the project root for the complete behavioral parity matrix between Ruby rolify and rolify-rust, including deliberate divergences and their rationale.