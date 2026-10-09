# MongoDB Adapter Guide

The `rolify-mongodb` crate provides an async-first adapter using the official MongoDB Rust driver 3.9+. It also supports the driver's official `sync` feature for synchronous usage.

## Installation

```toml
# Cargo.toml
[dependencies]
rolify-core = { path = "../rolify-core" }
rolify-mongodb = { path = "../rolify-mongodb" }  # async by default
mongodb = "3.9"  # async on tokio by default

# For sync mode:
# rolify-core = { path = "../rolify-core", features = ["is_sync"] }
# rolify-mongodb = { path = "../rolify-mongodb", features = ["sync"] }
# mongodb = { version = "3.9", features = ["sync"] }  # sync wraps async internally
tokio = { version = "1.53", features = ["full"] }
```

**Feature matrix:**

| Feature | Mode | Runtime |
|---------|------|---------|
| (default) | async | tokio (required even for sync) |
| `sync` | sync | tokio (driver wraps async) |
| `bson-3` | async/sync | Use bson 3.x instead of 2.x |

**Note:** The MongoDB driver requires `tokio` even in sync mode (it wraps async internally).

## Database Setup

### Collections

One store manages the role collection plus your holder/resource collections (two-sided embedded links; names configurable via `RolifyConfig`). There is no separate join collection: a role document embeds its holder ids, and a holder document embeds its role ids.

```javascript
// role collection - one document per role
{
  _id: ObjectId,
  name: "admin",
  resource_type: null,                        // explicit BSON null at global scope
  resource_id: null,                          // explicit BSON null at global/class scope
  user_ids: ["1", "8f14e45f-ea1b-4c6f-b0d0-e59b80d12611"]  // stringified holder PKs
}

// your holder documents (e.g. users) carry the back-link
{ _id: ObjectId, /* your fields... */, role_ids: [ObjectId] }
```

**Holder id typing:** holder ids are stored in canonical string form (integer `"1"` or a UUID string), mirroring the SQL adapters' `VARCHAR(191) user_id`; integer, UUID, and string holder PKs all fit the same document shape.

### Indexes

```javascript
// Created idempotently by store.ensure_indexes() (call it during setup)
db.roles.createIndex({ name: 1, resource_type: 1, resource_id: 1 }, { unique: true });
```

### Sentinel Values

Unlike the SQL adapters (which store `''`), MongoDB stores explicit BSON `null` at global/class scope; the unique compound index admits exactly one `(name, null, null)` document:

```javascript
// Global role
{ name: "admin", resource_type: null, resource_id: null }

// Class-scoped
{ name: "moderator", resource_type: "Forum", resource_id: null }

// Instance-scoped
{ name: "owner", resource_type: "Forum", resource_id: "7" }
```

## Basic Usage (Async)

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_mongodb::MongoStore;
use mongodb::{Client, Database, options::ClientOptions};

struct Player {
    id: i64,
    store: MongoStore,
    db: Database,
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = MongoStore;

    fn store(&mut self) -> &mut MongoStore {
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

    fn store_with_conn(&mut self) -> (&mut MongoStore, &mut Database) {
        (&mut self.store, &mut self.db)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_options = ClientOptions::parse("mongodb://localhost:27017").await?;
    let client = Client::with_options(client_options)?;
    let db = client.database("myapp");

    let config = RolifyConfig::default();
    let store = MongoStore::new(&db, &config);
    store.ensure_indexes().await?;

    let mut player = Player { id: 1, store, db, config };

    player.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;
    let is_admin = player.has_role(&RoleName::from("admin"), ResourceFilter::Global).await?;
    assert!(is_admin);

    Ok(())
}
```

## Basic Usage (Sync)

```rust
// Cargo.toml: sync mode via rolify-core/is_sync + rolify-mongodb/sync
// rolify-core = { path = "../rolify-core", features = ["is_sync"] }
// rolify-mongodb = { path = "../rolify-mongodb", features = ["sync"] }
// mongodb = { version = "3.9", features = ["sync"] }

use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_mongodb::MongoStore; // sync feature: same store type over mongodb::sync::Database
use mongodb::sync::{Client, Database};
use mongodb::options::ClientOptions;

struct Player {
    id: i64,
    store: MongoStore,
    db: Database,
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = MongoStore;

    fn store(&mut self) -> &mut MongoStore { &mut self.store }
    fn rolify_config(&self) -> &RolifyConfig { &self.config }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "Player" }
    fn store_with_conn(&mut self) -> (&mut MongoStore, &mut Database) {
        (&mut self.store, &mut self.db)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_options = ClientOptions::parse("mongodb://localhost:27017")?;
    let client = Client::with_options(client_options)?;
    let db = client.database("myapp");

    let config = RolifyConfig::default();
    let store = MongoStore::new(&db, &config);
    store.ensure_indexes()?;  // Blocking call under the sync feature

    let mut player = Player { id: 1, store, db, config };

    player.add_role(&RoleName::from("admin"), ResourceRef::Global)?;
    let is_admin = player.has_role(&RoleName::from("admin"), ResourceFilter::Global)?;
    assert!(is_admin);

    Ok(())
}
```

## Using with Rolify Engine

```rust
use rolify_core::manager::Rolify;
use rolify_mongodb::MongoStore;
use mongodb::Database;

let client = Client::with_options(ClientOptions::parse("mongodb://localhost:27017").await?)?;
let db = client.database("myapp");
let config = RolifyConfig::default();
let store = MongoStore::new(&db, &config);
store.ensure_indexes().await?;

let mut engine = Rolify::new(store, db, config);

// Resource-side finders
use rolify_core::resource::Resource;
use rolify_core::role::RoleName;

struct Forum { id: i64 }
impl Resource for Forum {
    fn type_name() -> &'static str { "Forum" }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}

let forums = Forum::with_role(&mut engine, &[RoleName::from("moderator")], Some(&ResourceId::from(1))).await?;
```

## Custom Table Names (Collection Names)

```rust
use rolify_core::config::RolifyConfig;
use rolify_mongodb::MongoStore;

let config = RolifyConfig::builder()
    .role_table("privileges")    // roles collection name
    .build()?;

// Gem parity: role_cname 'Privilege' -> tableize -> "privileges".
let store = MongoStore::new(&db, &config);
// Or bypass config with an explicit name (re-validated against the same allow-list):
let store = MongoStore::with_role_collection(&db, "privileges");
```

MongoDB has no join table: memberships live inside the role document's `user_ids` array (and the consumer document's `role_ids`), so `join_table` does not apply to this adapter.

## STI Support

```rust
use rolify_core::resource::Resource;
use rolify_core::role::ResourceId;

struct Vehicle { id: i64 }
impl Resource for Vehicle {
    fn type_name() -> &'static str { "Vehicle" }
    fn descendant_types() -> Vec<&'static str> { vec!["Vehicle", "Car", "Truck"] }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}
```

## Transaction Support

```rust
use mongodb::{Client, Database, ClientSession};

async fn atomic_role_grant(client: &Client, user_id: i64) -> Result<(), mongodb::error::Error> {
    let mut session = client.start_session().await?;
    session.start_transaction(None).await?;

    let db = client.database("myapp");
    let config = RolifyConfig::default();
    let store = MongoStore::new(&db, &config);
    let mut engine = Rolify::new(store, db, config);
    
    // Pass session through store (if supported) or use session-aware methods
    let mut user = UserAdapter { id: user_id, engine: &mut engine, session: &mut session };
    user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;
    user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
    
    session.commit_transaction().await?;
    Ok(())
}

struct UserAdapter<'a> {
    id: i64,
    engine: &'a mut Rolify<MongoStore>,
    session: &'a mut ClientSession,
}

impl<'a> RolifyUser for UserAdapter<'a> {
    type Store = MongoStore;
    fn store(&mut self) -> &mut MongoStore { self.engine.store_with_conn().0 }
    fn rolify_config(&self) -> &RolifyConfig { self.engine.config() }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "Player" }
    fn store_with_conn(&mut self) -> (&mut MongoStore, &mut Database) {
        self.engine.store_with_conn()
    }
}
```

## Connection Pooling

The MongoDB driver manages connection pooling internally:

```rust
use mongodb::options::ClientOptions;

let mut options = ClientOptions::parse("mongodb://localhost:27017").await?;
options.max_pool_size = Some(20);
options.min_pool_size = Some(5);
options.max_idle_time = Some(std::time::Duration::from_secs(600));
options.server_selection_timeout = Some(std::time::Duration::from_secs(30));

let client = Client::with_options(options)?;
```

## TLS/SSL Configuration

```rust
use mongodb::options::{ClientOptions, StreamAddress};
use mongodb::options::tls::TlsOptions;

let mut options = ClientOptions::parse("mongodb://localhost:27017").await?;

// For TLS
options.tls = Some(TlsOptions::default());
// Or with custom certs:
// options.tls = Some(TlsOptions::builder().ca_file("ca.pem").build());

let client = Client::with_options(options)?;
```

## BSON 3 Support

```toml
# Cargo.toml
rolify-mongodb = { path = "../rolify-mongodb" }
mongodb = { version = "3.9", features = ["bson-3"] }  # opt into the bson 3.x line on the driver
```

```rust
// Uses bson 3.x types (different API)
// The adapter handles version differences internally
```

## Performance Tips

1. **Indexes**: Create the recommended indexes during setup.

2. **Projection**: The adapter uses projections to fetch only needed fields.

3. **Batch operations**: `has_any_roles` / `with_any_roles` use `$in` for single round-trips.

4. **Connection pooling**: Configure `max_pool_size` / `min_pool_size`.

5. **Read concern/write concern**: Adjust for your consistency requirements.

## Troubleshooting

### "Tokio runtime required"

Even in sync mode, the MongoDB driver needs a Tokio runtime:

```rust
// Sync mode still needs tokio
#[tokio::main]
fn main() { ... }  // Works for both async and sync
```

### "Unique constraint violation"

The adapter uses `find_one_and_update` with upsert for `find_or_create_by`.

### "Index build failed"

Run `store.ensure_indexes().await?` after connecting. Ensure no duplicate data exists.

### "BSON serialization error"

Enable `serde` feature on `mongodb` crate and ensure your types implement `Serialize`/`Deserialize`.

### Sync/Async mismatch

**Error:** `the trait bound ... is not satisfied`
**Cause:** Mixing sync/async features incorrectly.
**Fix:** 
- Async: `rolify-core` (no `is_sync`), `rolify-mongodb` (no `sync`), `mongodb` with `tokio`
- Sync: `rolify-core` with `is_sync`, `rolify-mongodb` with `sync`, `mongodb` with `sync`

## Migration from Ruby rolify (Mongoid)

| Ruby (Mongoid) | MongoDB Adapter |
|----------------|-----------------|
| `include Rolify::Mongoid` | `impl RolifyUser for User` |
| `user.add_role(:admin)` | `user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?` |
| `user.has_role?(:admin, forum)` | `user.has_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &forum.resource_id())).await?` |
| `Forum.with_role(:admin)` | `Forum::with_role(&mut engine, &[RoleName::from("admin")], None).await?` |
| `rolify :role_cname => 'Privilege'` | `role_table("privileges")` |
| `rolify :role_join_table_name => 'users_privileges'` | `join_table("users_privileges")` |
| `Mongoid::Config.clients` | `ClientOptions::parse(...)` |
| `field :name, type: String` | BSON serialization via `serde` |

**Key differences:**
- Mongoid is an ODM; the adapter uses the raw driver
- No `method_missing` shortcuts (`is_admin?`) - use explicit `has_role`
- Explicit async/await (or sync blocking calls)
- Collection names configurable via `RolifyConfig`