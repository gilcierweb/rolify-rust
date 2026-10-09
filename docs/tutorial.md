# Tutorial: Building a Forum Application

This tutorial walks through building a forum application with role-based access control using rolify-rust. We'll cover:

1. Setting up the project
2. Defining domain models
3. Implementing `RolifyUser` and `Resource`
4. Granting and checking roles
5. Using resource-side finders
6. Strict mode and callbacks
7. User-class finders

## Project Setup

```toml
# Cargo.toml
[package]
name = "forum-app"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[dependencies]
rolify-core = { path = "../rolify-core" }
rolify-diesel = { path = "../rolify-diesel", features = ["postgres"] }
rolify-test = { path = "../rolify-test", features = ["suite"] }  # for testing
diesel = { version = "2.3", features = ["postgres", "chrono", "r2d2"] }
diesel_migrations = { version = "2.3", features = ["postgres"] }
tokio = { version = "1.53", features = ["full"] }
anyhow = "1.0"
```

For this tutorial, we'll use the in-memory store from `rolify-test` so you can run everything without a database. Swap in `rolify-diesel`/`rolify-sqlx`/etc. for production.

## Domain Models

```rust
// src/models.rs
use rolify_core::resource::Resource;
use rolify_core::role::ResourceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Forum {
    pub id: i64,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    pub id: i64,
    pub forum_id: i64,
    pub title: String,
    pub author_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: i64,
    pub topic_id: i64,
    pub author_id: i64,
    pub content: String,
}

// Implement Resource for Forum (so we can use resource-side finders)
impl Resource for Forum {
    fn type_name() -> &'static str {
        "Forum"
    }
    fn resource_id(&self) -> ResourceId {
        ResourceId::from(self.id)
    }
}

// STI Example: Vehicle -> Car
#[derive(Debug, Clone)]
pub struct Vehicle {
    pub id: i64,
    pub name: String,
}

impl Resource for Vehicle {
    fn type_name() -> &'static str {
        "Vehicle"
    }
    fn descendant_types() -> Vec<&'static str> {
        vec!["Vehicle", "Car", "Truck"]
    }
    fn resource_id(&self) -> ResourceId {
        ResourceId::from(self.id)
    }
}

#[derive(Debug, Clone)]
pub struct Car {
    pub id: i64,
    pub name: String,
}

impl Resource for Car {
    fn type_name() -> &'static str {
        "Car"
    }
    fn resource_id(&self) -> ResourceId {
        ResourceId::from(self.id)
    }
}
```

## Implementing RolifyUser

```rust
// src/rolify_user.rs
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::user::RolifyUser;
use rolify_test::InMemoryStore;
use crate::models::User;

pub struct RolifyUserWrapper {
    pub user: User,
    pub store: InMemoryStore,
    pub conn: (),
    pub config: RolifyConfig,
}

impl RolifyUser for RolifyUserWrapper {
    type Store = InMemoryStore;

    fn store(&mut self) -> &mut InMemoryStore {
        &mut self.store
    }

    fn rolify_config(&self) -> &RolifyConfig {
        &self.config
    }

    fn rolify_id(&self) -> ResourceId {
        ResourceId::from(self.user.id)
    }

    fn rolify_type() -> &'static str {
        "User"
    }

    fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
        (&mut self.store, &mut self.conn)
    }
}

impl RolifyUserWrapper {
    pub fn new(user: User, config: RolifyConfig) -> Self {
        Self {
            user,
            store: InMemoryStore::new(),
            conn: (),
            config,
        }
    }

    // Convenience methods
    pub async fn make_admin(&mut self) -> Result<RoleRecord, rolify_core::error::RolifyError> {
        self.add_role(&RoleName::from("admin"), ResourceRef::Global).await
    }

    pub async fn make_forum_moderator(&mut self, forum: &Forum) -> Result<RoleRecord, rolify_core::error::RolifyError> {
        self.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await
    }

    pub async fn make_forum_owner(&mut self, forum: &Forum) -> Result<RoleRecord, rolify_core::error::RolifyError> {
        self.add_role(&RoleName::from("owner"), ResourceRef::Instance("Forum", &forum.resource_id())).await
    }

    pub async fn can_moderate_forum(&mut self, forum: &Forum) -> Result<bool, rolify_core::error::RolifyError> {
        self.has_role(
            &RoleName::from("moderator"),
            ResourceFilter::Instance("Forum", &forum.resource_id()),
        ).await
    }

    pub async fn is_admin(&mut self) -> Result<bool, rolify_core::error::RolifyError> {
        self.has_role(&RoleName::from("admin"), ResourceFilter::Global).await
    }
}
```

## Basic Usage

```rust
// src/main.rs
mod models;
mod rolify_user;

use rolify_core::config::RolifyConfig;
use rolify_core::query::ResourceFilter;
use rolify_core::role::RoleName;
use crate::models::{User, Forum};
use crate::rolify_user::RolifyUserWrapper;

# #[cfg(not(feature = "is_sync"))]
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> { run().await }
# #[cfg(feature = "is_sync")]
# fn main() -> Result<(), Box<dyn std::error::Error>> { run() }
#
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Create users
    let alice = User { id: 1, username: "alice".into(), email: "alice@example.com".into() };
    let bob = User { id: 2, username: "bob".into(), email: "bob@example.com".into() };
    let charlie = User { id: 3, username: "charlie".into(), email: "charlie@example.com".into() };

    // Create forums
    let rust_forum = Forum { id: 1, name: "Rust".into(), description: "Rust programming".into() };
    let general_forum = Forum { id: 2, name: "General".into(), description: "General discussion".into() };

    // Wrap users with rolify
    let mut alice = RolifyUserWrapper::new(alice, RolifyConfig::default());
    let mut bob = RolifyUserWrapper::new(bob, RolifyConfig::default());
    let mut charlie = RolifyUserWrapper::new(charlie, RolifyConfig::default());

    // Grant roles
    alice.make_admin().await?;                    // Global admin
    bob.make_forum_moderator(&rust_forum).await?; // Moderator of all Forums
    charlie.make_forum_owner(&rust_forum).await?; // Owner of Rust forum only

    // Check permissions
    assert!(alice.is_admin().await?);
    assert!(!bob.is_admin().await?);

    // Alice (admin) can moderate any forum
    assert!(alice.can_moderate_forum(&rust_forum).await?);
    assert!(alice.can_moderate_forum(&general_forum).await?);

    // Bob (class moderator) can moderate any forum
    assert!(bob.can_moderate_forum(&rust_forum).await?);
    assert!(bob.can_moderate_forum(&general_forum).await?);

    // Charlie (instance owner) can only moderate Rust forum
    assert!(charlie.can_moderate_forum(&rust_forum).await?);
    assert!(!charlie.can_moderate_forum(&general_forum).await?);

    println!("✅ Basic role checks passed!");

    // List all roles for a user
    let alice_roles = alice.roles_name().await?;
    println!("Alice's roles: {:?}", alice_roles);  // [RoleName("admin")]

    let bob_roles = bob.roles_name().await?;
    println!("Bob's roles: {:?}", bob_roles);  // [RoleName("moderator")]

    Ok(())
}
```

## Resource-Side Finders

```rust
// Add to main.rs
use rolify_core::manager::Rolify;
use rolify_core::resource::Resource;
use rolify_core::role::RoleName;

// Create a Rolify engine for resource-side queries
let mut engine = Rolify::new(InMemoryStore::new(), (), RolifyConfig::default());

// Seed some data for resource-side queries
engine.store().grant(&ResourceId::from(1_i64), RoleRecord::for_class("moderator", "Forum"));
engine.store().grant(&ResourceId::from(2_i64), RoleRecord::for_class("moderator", "Forum"));
engine.store().register_resource(ResourceKey { resource_type: "Forum".into(), resource_id: ResourceId::from(1_i64) });
engine.store().register_resource(ResourceKey { resource_type: "Forum".into(), resource_id: ResourceId::from(2_i64) });
engine.store().register_holder("User", 1_i64);
engine.store().register_holder("User", 2_i64);

// Find all forums where user 1 has "moderator" role
let moderator_forums = Forum::with_role(&mut engine, &[RoleName::from("moderator")], Some(&ResourceId::from(1_i64))).await?;
println!("Forums user 1 moderates: {:?}", moderator_forums);

// Find all forums (universe)
let all_forums: Vec<ResourceKey> = vec![
    ResourceKey { resource_type: "Forum".into(), resource_id: ResourceId::from(1_i64) },
    ResourceKey { resource_type: "Forum".into(), resource_id: ResourceId::from(2_i64) },
];

// Forums user 1 does NOT moderate
let non_moderated = Forum::without_role(&mut engine, &[RoleName::from("moderator")], Some(&ResourceId::from(1_i64)), &all_forums).await?;
println!("Forums user 1 doesn't moderate: {:?}", non_moderated);
```

## Strict Mode

```rust
use rolify_core::config::RolifyConfig;

let strict_config = RolifyConfig::builder()
    .strict(true)
    .build()?;

let mut strict_user = RolifyUserWrapper::new(user, strict_config);
strict_user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;

// Strict: exact scope match only
let class_match = strict_user.has_strict_role(&RoleName::from("moderator"), ResourceFilter::Class("Forum")).await?;
assert!(class_match);

let instance_match = strict_user.has_strict_role(&RoleName::from("moderator"), ResourceFilter::Instance("Forum", &ResourceId::from(1_i64))).await?;
assert!(!instance_match);  // class role doesn't match instance in strict mode

let global_match = strict_user.has_strict_role(&RoleName::from("moderator"), ResourceFilter::Global).await?;
assert!(!global_match);  // class role doesn't match global in strict mode

// Non-strict (default) still uses ladder
let normal_user = RolifyUserWrapper::new(user, RolifyConfig::default());
normal_user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
let normal_instance = normal_user.has_role(&RoleName::from("moderator"), ResourceFilter::Instance("Forum", &ResourceId::from(1_i64))).await?;
assert!(normal_instance);  // class covers instance in non-strict
```

## Callbacks (Lifecycle Hooks)

```rust
use std::sync::Arc;
use rolify_core::config::RolifyConfig;
use rolify_core::error::RolifyError;
use rolify_core::role::RoleRecord;

let config = RolifyConfig::builder()
    // Veto: prevent granting "root" role
    .before_add(Arc::new(|record: &RoleRecord| {
        if record.name.as_str() == "root" {
            return Err(RolifyError::CallbackVeto {
                callback: "before_add",
                reason: "root role is reserved".into(),
            });
        }
        Ok(())
    }))
    // Notification: log after role granted
    .after_add(Arc::new(|record: &RoleRecord| {
        println!("Role granted: {} (scope: {:?}/{:?})", record.name, record.resource_type, record.resource_id);
    }))
    // Veto: prevent removing last admin
    .before_remove(Arc::new(|record: &RoleRecord| {
        if record.name.as_str() == "admin" && record.is_global() {
            // In real app, check if this is the last admin
            return Err(RolifyError::CallbackVeto {
                callback: "before_remove",
                reason: "cannot remove last global admin".into(),
            });
        }
        Ok(())
    }))
    // Notification: audit log
    .after_remove(Arc::new(|record: &RoleRecord| {
        println!("Role revoked: {} (scope: {:?}/{:?})", record.name, record.resource_type, record.resource_id);
    }))
    .build()?;

let mut user = RolifyUserWrapper::new(user, config);

// This will be vetoed
let result = user.add_role(&RoleName::from("root"), ResourceRef::Global).await;
assert!(result.is_err());

// This will succeed and trigger after_add
user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
```

## User-Class Finders

```rust
use rolify_core::manager::Rolify;
use rolify_core::query::RoleQuery;
use rolify_core::role::RoleName;

// Create engine with holder registry
let mut engine = Rolify::new(InMemoryStore::new(), (), RolifyConfig::default());
engine.store().register_holder("User", 1_i64);  // Alice
engine.store().register_holder("User", 2_i64);  // Bob
engine.store().register_holder("User", 3_i64);  // Charlie

// Grant roles
engine.store().grant(&ResourceId::from(1_i64), RoleRecord::global("admin"));
engine.store().grant(&ResourceId::from(2_i64), RoleRecord::for_class("moderator", "Forum"));
engine.store().grant(&ResourceId::from(3_i64), RoleRecord::for_instance("owner", "Forum", 1_i64));

// Find all Users with global "admin" role
let admins = RolifyUserWrapper::with_role(&mut engine, &RoleQuery::with_role(&RoleName::from("admin"))).await?;
println!("Admins: {:?}", admins);  // [ResourceId("1")]

// Find all Users with class "moderator" on Forum
let moderators = RolifyUserWrapper::with_role(&mut engine, &RoleQuery::with_role_and_filter(
    &RoleName::from("moderator"),
    ResourceFilter::Class("Forum"),
)).await?;
println!("Moderators: {:?}", moderators);  // [ResourceId("2")]

// Find Users with ANY of these roles
let any_roles = RolifyUserWrapper::with_any_roles(&mut engine, &[
    RoleQuery::with_role(&RoleName::from("admin")),
    RoleQuery::with_role_and_filter(&RoleName::from("moderator"), ResourceFilter::Class("Forum")),
]).await?;
println!("Admins or moderators: {:?}", any_roles);  // [ResourceId("1"), ResourceId("2")]

// Find Users with ALL of these roles (intersection)
let all_roles = RolifyUserWrapper::with_all_roles(&mut engine, &[
    RoleQuery::with_role(&RoleName::from("admin")),
    RoleQuery::with_role_and_filter(&RoleName::from("moderator"), ResourceFilter::Class("Forum")),
]).await?;
println!("Admins AND moderators: {:?}", all_roles);  // [] (no user has both)
```

## Composite Checks

```rust
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::RoleName;
use rolify_core::role::ResourceId;

// has_all_roles - ALL must match (early exit on first miss)
let queries = vec![
    RoleQuery::with_role(&RoleName::from("admin")),
    RoleQuery::with_role_and_filter(&RoleName::from("moderator"), ResourceFilter::Class("Forum")),
];

// alice has admin (global) but not moderator
let has_all = alice.has_all_roles(&queries).await?;
assert!(!has_all);

// has_any_roles - ANY must match (single OR-folded round trip)
let has_any = alice.has_any_roles(&queries).await?;
assert!(has_any);  // has admin

// only_has_role - has the role AND exactly one role total
let solo_user = RolifyUserWrapper::new(User { id: 99, username: "solo".into(), email: "solo@test.com".into() }, RolifyConfig::default());
solo_user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;
let only_admin = solo_user.only_has_role(&RoleName::from("admin"), ResourceFilter::Global).await?;
assert!(only_admin);  // exactly one role

alice.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
let only_admin = alice.only_has_role(&RoleName::from("admin"), ResourceFilter::Global).await?;
assert!(!only_admin);  // now has 2 roles
```

## Revoking Roles

```rust
use rolify_core::kernel::RemovalTarget;

// Remove specific instance role
charlie.remove_role(&RoleName::from("owner"), RemovalTarget::Exact("Forum", &ResourceId::from(1_i64))).await?;

// Remove all class-scoped roles for a type
bob.remove_role(&RoleName::from("moderator"), RemovalTarget::TypeSweep("Forum")).await?;

// Remove all roles with this name (all scopes)
alice.remove_role(&RoleName::from("admin"), RemovalTarget::NameOnly).await?;

// With remove_role_if_empty = true (default), the role row itself is deleted
// when its last membership is revoked
```

## Testing with rolify-test

```rust
// tests/integration_test.rs
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::user::RolifyUser;
use rolify_test::{InMemoryStore, RoleAssertions, builders::*};

#[test]
fn test_admin_can_access_everything() {
    let mut store = InMemoryStore::new();
    let holder = ResourceId::from(1_i64);
    
    // Seed using builders
    grant(&mut store, &holder, FixtureGrant::global("admin"));
    grant(&mut store, &holder, FixtureGrant::class("moderator", "Forum"));
    
    let mut user = TestUser { id: 1, store, conn: (), config: RolifyConfig::default() };
    
    // Global admin covers everything
    assert!(user.has_role(&RoleName::from("admin"), ResourceFilter::Global).await.unwrap());
    assert!(user.has_role(&RoleName::from("admin"), ResourceFilter::Class("Forum")).await.unwrap());
    assert!(user.has_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &ResourceId::from(42))).await.unwrap());
    assert!(user.has_role(&RoleName::from("admin"), ResourceFilter::Any).await.unwrap());
}

#[test]
fn test_strict_mode_blocks_override() {
    let mut store = InMemoryStore::new();
    let holder = ResourceId::from(1_i64);
    grant(&mut store, &holder, FixtureGrant::global("admin"));
    
    let config = RolifyConfig::builder().strict(true).build().unwrap();
    let mut user = TestUser { id: 1, store, conn: (), config };
    
    // Strict: global admin does NOT cover class/instance
    assert!(user.has_strict_role(&RoleName::from("admin"), ResourceFilter::Global).await.unwrap());
    assert!(!user.has_strict_role(&RoleName::from("admin"), ResourceFilter::Class("Forum")).await.unwrap());
    assert!(!user.has_strict_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &ResourceId::from(42))).await.unwrap());
}

#[test]
fn test_resource_side_finders() {
    use rolify_core::manager::Rolify;
    use rolify_core::resource::Resource;
    use rolify_core::role::RoleName;
    use rolify_test::ResourceKey;
    
    let mut engine = Rolify::new(InMemoryStore::new(), (), RolifyConfig::default());
    engine.store().register_resource(ResourceKey { resource_type: "Forum".into(), resource_id: ResourceId::from(1) });
    engine.store().register_resource(ResourceKey { resource_type: "Forum".into(), resource_id: ResourceId::from(2) });
    engine.store().register_holder("User", 1);
    engine.store().register_holder("User", 2);
    
    // User 1 moderates Forum 1
    engine.store().grant(&ResourceId::from(1), RoleRecord::for_class("moderator", "Forum"));
    
    struct Forum;
    impl Resource for Forum {
        fn type_name() -> &'static str { "Forum" }
        fn resource_id(&self) -> ResourceId { ResourceId::from(1) }
    }
    
    let moderated = Forum::with_role(&mut engine, &[RoleName::from("moderator")], Some(&ResourceId::from(1))).await.unwrap();
    assert_eq!(moderated.len(), 1);
    assert_eq!(moderated[0].resource_id.as_str(), "1");
}
```

## Running with a Real Database (Diesel)

```rust
// src/db.rs
use rolify_diesel::DieselRoleStore;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};

pub type DbPool = Pool<ConnectionManager<PgConnection>>;

pub fn create_pool(database_url: &str) -> DbPool {
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    Pool::builder().build(manager).expect("Failed to create pool")
}

pub async fn run_with_diesel(pool: &DbPool) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = pool.get()?;
    
    // Run migrations (rolify-diesel provides them)
    diesel_migrations::run_pending_migrations(&mut conn)?;
    
    // Create store
    let mut store = DieselRoleStore::new(conn);
    
    // Use with RolifyUser...
    Ok(())
}
```

## Summary

Key takeaways:

1. **Three scopes**: Global, Class, Instance — with a predictable override ladder
2. **Two modes**: Non-strict (default, ladder) vs Strict (exact match)
3. **Zero-I/O caching**: `RoleSet` for borrowed-row checks
4. **Dual API**: User-side (`RolifyUser`) + Resource-side (`Resource`) finders
5. **Callbacks**: `before_add`/`after_add`/`before_remove`/`after_remove` with veto semantics
6. **Adapters**: Swap `InMemoryStore` for `DieselRoleStore`, `SqlxRoleStore`, etc. — same API
7. **No authorization enforcement**: rolify-rust only manages roles; you decide what roles mean