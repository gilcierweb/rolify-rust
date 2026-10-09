# Getting Started with rolify-rust

rolify-rust is a Rust port of the [rolify](https://github.com/RolifyCommunity/rolify) Ruby gem — a minimalistic role management library (RBAC) **without authorization enforcement**. It provides scoped roles at three levels:

- **Global** — role applies everywhere
- **Class/Resource Type** — role applies to all instances of a type (e.g., all Forums)
- **Instance** — role applies to a specific instance (e.g., Forum #42)

## Installation

Add the core crate and your chosen adapter to `Cargo.toml`:

```toml
[dependencies]
# Core types and traits (required)
rolify-core = { path = "../rolify-core" }  # or from crates.io when published

# Choose ONE adapter:
rolify-diesel = { path = "../rolify-diesel", features = ["postgres"] }  # sync Diesel
# rolify-diesel = { path = "../rolify-diesel", features = ["async", "postgres", "bb8"] }  # async Diesel
rolify-sqlx = { path = "../rolify-sqlx", features = ["postgres"] }  # async runtime + TLS are selected on your sqlx dependency
rolify-seaorm = { path = "../rolify-seaorm", features = ["postgres"] }
rolify-mongodb = { path = "../rolify-mongodb" }

# For testing without a database
rolify-test = { path = "../rolify-test" }
```

### Sync vs Async Mode

rolify-core supports both sync and async through a single feature flag:

| Mode | Cargo Feature | Use With |
|------|---------------|----------|
| async (default) | *(none)* | sqlx, sea-orm, mongodb, diesel-async |
| sync | `is_sync` | diesel (sync), mongodb (sync feature) |

**One mode per build graph.** If any crate in your dependency graph enables `rolify-core/is_sync`, the entire graph becomes sync. Do not mix sync and async in one binary.

```toml
# For sync mode (e.g., with Diesel sync)
rolify-core = { path = "../rolify-core", features = ["is_sync"] }
rolify-diesel = { path = "../rolify-diesel", features = ["postgres"] }
```

## Quick Start (5 Minutes)

### 1. Define Your User Type

Implement `RolifyUser` on your user/account struct:

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::user::RolifyUser;
use rolify_test::InMemoryStore;

struct Player {
    id: i64,
    store: InMemoryStore,
    conn: (),
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = InMemoryStore;

    fn store(&mut self) -> &mut InMemoryStore {
        &mut self.store
    }

    fn rolify_config(&self) -> &RolifyConfig {
        &self.config
    }

    fn rolify_id(&self) -> ResourceId {
        ResourceId::from(self.id)
    }

    fn rolify_type() -> &'static str {
        "Player"
    }

    fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) {
        (&mut self.store, &mut self.conn)
    }
}
```

### 2. Grant Roles

```rust
# #[cfg(not(feature = "is_sync"))]
# #[tokio::main(flavor = "current_thread")]
# async fn main() { usage().await; }
# #[cfg(feature = "is_sync")]
# fn main() { usage(); }
#
# #[maybe_async::maybe_async]
async fn usage() {
    let mut player = Player {
        id: 1,
        store: InMemoryStore::new(),
        conn: (),
        config: RolifyConfig::default(),
    };

    // Global role (applies everywhere)
    player.add_role(&RoleName::from("admin"), ResourceRef::Global).await.unwrap();

    // Class-scoped role (applies to all Forums)
    player.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await.unwrap();

    // Instance-scoped role (applies to Forum #7 only)
    player.add_role(&RoleName::from("owner"), ResourceRef::Instance("Forum", &ResourceId::from(7_i64))).await.unwrap();
}
```

### 3. Check Roles

```rust
# use rolify_core::config::RolifyConfig;
# use rolify_core::query::{ResourceFilter, RoleQuery};
# use rolify_core::resource::ResourceRef;
# use rolify_core::role::{ResourceId, RoleName};
# use rolify_core::user::RolifyUser;
# use rolify_test::InMemoryStore;
# struct Player { id: i64, store: InMemoryStore, conn: (), config: RolifyConfig }
# impl RolifyUser for Player { type Store = InMemoryStore; fn store(&mut self) -> &mut InMemoryStore { &mut self.store } fn rolify_config(&self) -> &RolifyConfig { &self.config } fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) } fn rolify_type() -> &'static str { "Player" } fn store_with_conn(&mut self) -> (&mut InMemoryStore, &mut ()) { (&mut self.store, &mut self.conn) } }
#
# #[cfg(not(feature = "is_sync"))]
# #[tokio::main(flavor = "current_thread")]
# async fn main() { usage().await; }
# #[cfg(feature = "is_sync")]
# fn main() { usage(); }
#
# #[maybe_async::maybe_async]
async fn usage() {
    let mut player = Player {
        id: 1,
        store: InMemoryStore::new(),
        conn: (),
        config: RolifyConfig::default(),
    };
    player.add_role(&RoleName::from("admin"), ResourceRef::Global).await.unwrap();
    player.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await.unwrap();

    // Check global role
    let is_admin = player.has_role(&RoleName::from("admin"), ResourceFilter::Global).await.unwrap();
    assert!(is_admin);

    // Check class-scoped role (global roles also satisfy non-strict checks)
    let is_mod = player.has_role(&RoleName::from("moderator"), ResourceFilter::Class("Forum")).await.unwrap();
    assert!(is_mod);

    // Check instance-scoped role (class roles also satisfy non-strict checks)
    let forum_7 = ResourceId::from(7_i64);
    let has_access = player.has_role(&RoleName::from("moderator"), ResourceFilter::Instance("Forum", &forum_7)).await.unwrap();
    assert!(has_access);  // class role covers instance

    // Strict mode: exact scope match only
    let strict_config = RolifyConfig::builder().strict(true).build().unwrap();
    let mut strict_player = Player { config: strict_config, ..player };
    let strict_mod = strict_player.has_strict_role(&RoleName::from("moderator"), ResourceFilter::Class("Forum")).await.unwrap();
    assert!(strict_mod);
    let strict_global = strict_player.has_strict_role(&RoleName::from("moderator"), ResourceFilter::Global).await.unwrap();
    assert!(!strict_global);  // global doesn't match class in strict mode
}
```

## Role Scope Ladder (The Core Logic)

rolify-rust implements the exact same **match ladder** as the Ruby gem. When checking `has_role(name, resource)`, the logic is:

| Query Scope | Matches Role Scope |
|-------------|-------------------|
| **Global** | Global only |
| **Class** | Class (same type) + Global |
| **Instance** | Instance (same type + id) + Class (same type) + Global |
| **Any** | Any scope (name-only match) |

This means:
- A **global** `admin` role grants access to everything
- A **class-scoped** `moderator` on `Forum` grants access to all Forums
- An **instance-scoped** `owner` on `Forum #7` grants access only to that Forum

### Strict Mode

When `strict = true` in `RolifyConfig`, the ladder collapses to **exact scope matching only**:

| Query Scope | Matches Role Scope |
|-------------|-------------------|
| Global | Global only |
| Class | Class (same type) only |
| Instance | Instance (same type + id) only |
| Any | Any scope (name-only) |

## Next Steps

- [Core Concepts](core-concepts.md) — Deep dive into roles, resources, queries, and the kernel
- [Configuration Guide](configuration.md) — Strict mode, callbacks, table names
- [Adapter Guides](adapters/) — Diesel, SQLx, SeaORM, MongoDB setup
- [Tutorial](tutorial.md) — Complete walkthrough building a forum app
- [Migration from Ruby rolify](migration-from-ruby.md) — Porting guide