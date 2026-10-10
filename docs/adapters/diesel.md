# Diesel Adapter Guide

The `rolify-diesel` crate provides a synchronous (and optionally async) Diesel 2.3+ adapter for rolify-rust.

## Installation

```toml
# Cargo.toml
[dependencies]
rolify-core = { path = "../rolify-core" }
rolify-diesel = { path = "../rolify-diesel", features = ["postgres"] }  # or "mysql", "sqlite"
diesel = { version = "2.3", features = ["postgres", "chrono", "r2d2"] }
diesel_migrations = { version = "2.3", features = ["postgres"] }

# For async mode (requires diesel-async):
# rolify-diesel = { path = "../rolify-diesel", features = ["async", "postgres", "bb8"] }
# diesel-async = { version = "0.9", features = ["postgres", "bb8"] }
```

**Feature matrix:**

| Feature | Backend | Pool | Mode |
|---------|---------|------|------|
| `postgres` | PostgreSQL | r2d2 (sync) / bb8/deadpool (async) | sync/async |
| `mysql` | MySQL | r2d2 (sync) / bb8/deadpool (async) | sync/async |
| `sqlite` | SQLite | r2d2 (sync) | sync only |
| `async` | Enables diesel-async | bb8 or deadpool | async only |
| `bb8` | bb8 pool for async | bb8 | async |
| `deadpool` | deadpool pool for async | deadpool | async |
| `migrations` | Includes embedded migrations | - | both |

## Database Setup

### Migrations

Run the embedded migrations to create the roles and join tables:

```rust
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");  // or use rolify_diesel::MIGRATIONS

fn run_migrations(pool: &Pool<ConnectionManager<PgConnection>>) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = pool.get()?;
    conn.run_pending_migrations(MIGRATIONS)?;
    Ok(())
}
```

Or use the provided migrations from `rolify-diesel`:

```rust
use rolify_diesel::MIGRATIONS;
use diesel_migrations::MigrationHarness;

let mut conn = pool.get()?;
conn.run_pending_migrations(MIGRATIONS)?;
```

### Schema

The migrations create:

```sql
-- roles table (Postgres dialect; MySQL uses AUTO_INCREMENT + utf8mb4_bin, SQLite uses INTEGER/TEXT)
CREATE TABLE roles (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name          VARCHAR(255) NOT NULL,
    resource_type VARCHAR(191) NOT NULL DEFAULT '',
    resource_id   VARCHAR(191) NOT NULL DEFAULT '',
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT roles_triple_unique UNIQUE (name, resource_type, resource_id)
);
CREATE INDEX idx_roles_resource ON roles (resource_type, resource_id);
CREATE INDEX idx_roles_name ON roles (name);

-- join table (configurable name, default: users_roles)
CREATE TABLE users_roles (
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id)
);
```

### Holder id typing

`role_id` is the library-owned side: always a `BIGINT` FK to `roles(id)`.
`user_id` is the holder side, and its physical type follows the holder id
kind selected at scaffold time by `--holder-id-type` (D-08-04):

| Kind | Postgres | MySQL | SQLite |
|------|----------|-------|--------|
| `integer` (default) | `BIGINT` | `BIGINT` | `INTEGER` |
| `uuid` | `UUID` | `BINARY(16)` | `TEXT` |
| `string` | `VARCHAR(191)` | `VARCHAR(191)` | `VARCHAR(191)` |

The migrations shipped in `rolify-diesel/migrations/` are laid out per
engine, then per kind
(`{engine}/{kind}/0000000001_rolify_create_tables/`), so a consumer applies
exactly the tree matching its configuration. The kind baked into the
scaffolded DDL must match `RolifyConfig::holder_id_kind` at runtime.
`integer` (the default) restores the gem's `t.references` bigint affinity;
`string` keeps the pre-08 `VARCHAR(191)` form. There is no `user_type`
column: the join table links one role to one holder id, like the gem's
HABTM table.

A holder id that does not parse for the configured kind (for example
`"abc"` under `integer`) returns
`rolify_core::Error::InvalidHolderId { expected, got }` before the query
reaches the database (D-08-05). The optional `--with-holder-fk` flag emits
`REFERENCES <holder_table>(<pk>)` for non-string kinds (D-08-07); it is off
by default, matching the gem, which emits no consumer-side FK (Entry 8).
See PARITY.md Entry 29.

**Note:** The adapter uses `''` (empty string) as sentinel for `NULL` in `resource_type`/`resource_id` to make `UNIQUE` constraints work identically across Postgres, MySQL, and SQLite. This diverges from the gem (which uses `NULL`); see parity matrix.

## Basic Usage

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_diesel::DieselStore;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;

type DbPool = Pool<ConnectionManager<PgConnection>>;

struct Player {
    id: i64,
    store: DieselStore,
    conn: PgConnection,
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = DieselStore;

    fn store(&mut self) -> &mut Self::Store {
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

    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut PgConnection) {
        (&mut self.store, &mut self.conn)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")?;
    let pool = create_pool(&database_url)?;
    run_migrations(&pool)?;

    let mut conn = pool.get()?;
    let config = RolifyConfig::default();
    let store = DieselStore::new(&config);

    let mut player = Player { id: 1, store, conn, config };
    // ... use player.add_role(), player.has_role(), etc.
    Ok(())
}
```

## Using with Rolify Engine

```rust
use rolify_core::manager::Rolify;
use rolify_diesel::DieselStore;

let mut conn = pool.get()?;
let config = RolifyConfig::default();
let store = DieselStore::new(&config);

let mut engine = Rolify::new(store, conn, config);

// Resource-side finders
use rolify_core::resource::Resource;
use rolify_core::role::RoleName;

struct Forum { id: i64 }
impl Resource for Forum {
    fn type_name() -> &'static str { "Forum" }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}

// Find forums where user has "moderator" role
let forums = Forum::with_role(&mut engine, &[RoleName::from("moderator")], Some(&ResourceId::from(1))).await?;
```

## Async Mode (diesel-async)

```toml
# Cargo.toml
rolify-diesel = { path = "../rolify-diesel", features = ["async", "postgres", "bb8"] }
diesel-async = { version = "0.9", features = ["postgres", "bb8"] }
tokio = { version = "1.53", features = ["full"] }
```

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_diesel::DieselStore; // async feature: SAME store type, async SPI
use diesel_async::pooled_connection::bb8::Pool;
use diesel_async::AsyncPgConnection;

type AsyncPool = Pool<AsyncPgConnection>;

struct Player {
    id: i64,
    store: DieselStore,
    conn: AsyncPgConnection,
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = DieselStore;

    fn store(&mut self) -> &mut Self::Store { &mut self.store }
    fn rolify_config(&self) -> &RolifyConfig { &self.config }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "Player" }
    fn store_with_conn(&mut self) -> (&mut Self::Store, &mut AsyncPgConnection) {
        (&mut self.store, &mut self.conn)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")?;
    let config = diesel_async::Config::new(&database_url);
    let pool = Pool::builder().build(config).await?;

    let mut conn = pool.get().await?;
    let config = RolifyConfig::default();
    let store = DieselStore::new(&config);

    let mut player = Player { id: 1, store, conn, config };
    player.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;
    Ok(())
}
```

## Connection Pooling

### Sync (r2d2)

```rust
use diesel::r2d2::{ConnectionManager, Pool, PooledConnection};
use diesel::PgConnection;

fn create_pool(url: &str) -> Pool<ConnectionManager<PgConnection>> {
    let manager = ConnectionManager::<PgConnection>::new(url);
    Pool::builder()
        .max_size(15)
        .min_idle(Some(5))
        .build(manager)
        .expect("Failed to create pool")
}

// Per-request checkout: build the store once from config, reuse per request
let config = RolifyConfig::default();
let store = DieselStore::new(&config);
let mut conn = pool.get()?;
```

### Async (bb8)

```rust
use diesel_async::pooled_connection::bb8::Pool;
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;

fn create_async_pool(url: &str) -> Pool<AsyncPgConnection> {
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
    Pool::builder()
        .max_size(15)
        .min_idle(Some(5))
        .build(manager)
        .await
        .expect("Failed to create pool")
}

// Per-request checkout
let config = RolifyConfig::default();
let store = DieselStore::new(&config);
let mut conn = pool.get().await?;
```

## Custom Table Names

```rust
use rolify_core::config::RolifyConfig;
use rolify_diesel::DieselStore;

let config = RolifyConfig::builder()
    .role_table("privileges")
    .join_table("users_privileges")
    .build()?;

// The store reads the table names from the config (defaults: "roles" / "users_roles")
let store = DieselStore::new(&config);
```

The adapter reads table names from `RolifyConfig` at runtime.

## STI (Single Table Inheritance)

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

// Vehicle::descendant_types() returns ["Vehicle", "Car", "Truck"]
// Resource finders will search across all descendant types
```

## Performance Tips

1. **Indexes**: The migrations create indexes on `roles(resource_type, resource_id)` and `roles(name)`, plus the `UNIQUE(user_id, role_id)` constraint on the join table (which doubles as the lookup index). Add more if needed.

2. **Connection reuse**: Check out connection per request/task, not per operation.

3. **Batch operations**: Use `has_any_roles` / `with_any_roles` for OR-folded single round-trips.

4. **Async for high concurrency**: Use `diesel-async` with bb8/deadpool for async workloads.

## Troubleshooting

### "Unique constraint violation"

The `UNIQUE(name, resource_type, resource_id)` constraint prevents duplicate role rows. The adapter's `find_or_create_by` handles this with a `SELECT ... FOR UPDATE` pattern.

### "Relation does not exist"

Run migrations: `conn.run_pending_migrations(MIGRATIONS)?;`

### "Column resource_type/resource_id not found"

Ensure you're using the correct schema. The adapter expects the columns from the migration.

### Sync/Async mismatch

**Error:** `the trait bound ... is not satisfied`
**Cause:** Mixing `rolify-core` (sync) with `rolify-diesel` (async) or vice versa.
**Fix:** Ensure all crates in the graph use the same mode. Either:
- All sync: `rolify-core` without `is_sync`, `rolify-diesel` without `async`
- All async: `rolify-core` with `is_sync`, `rolify-diesel` with `async`

## Migration from Ruby rolify (ActiveRecord)

| Ruby | Diesel |
|------|--------|
| `User.rolify` | `impl RolifyUser for User` |
| `user.add_role(:admin)` | `user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?` |
| `user.has_role?(:admin, forum)` | `user.has_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &forum.resource_id())).await?` |
| `user.roles` | `user.roles_name().await?` |
| `Forum.with_role(:admin)` | `Forum::with_role(&mut engine, &[RoleName::from("admin")], None).await?` |
| `Forum.find_roles(:admin, user)` | `Forum::find_roles(&mut engine, Some(&RoleName::from("admin")), Some(&user.rolify_id())).await?` |
| `rolify :role_cname => 'Privilege'` | `role_table("privileges")` |
| `rolify :role_join_table_name => 'users_privileges'` | `join_table("users_privileges")` |