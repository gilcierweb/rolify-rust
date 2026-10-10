# SQLx Adapter Guide

The `rolify-sqlx` crate provides an async-only SQLx 0.9+ adapter for rolify-rust. SQLx is not an ORM - the adapter hand-writes role/join SQL queries.

## Installation

```toml
# Cargo.toml
[dependencies]
rolify-core = { path = "../rolify-core" }
rolify-sqlx = { path = "../rolify-sqlx", features = ["postgres"] }
# sqlx 0.9 splits runtime from TLS: pick exactly one of each on the sqlx dep
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio", "tls-rustls-ring", "chrono", "uuid", "json"] }
tokio = { version = "1.53", features = ["full"] }
```

**Feature matrix (sqlx 0.9 splitting):**

| Feature | Purpose |
|---------|---------|
| `postgres` / `mysql` / `sqlite` | Backend driver |
| `runtime-tokio` / `runtime-smol` / `runtime-async-global-executor` | Async runtime |
| `tls-rustls-ring` / `tls-native-tls` / `tls-openssl` | TLS backend |
| `chrono` / `time` / `uuid` / `json` | Type support |

**No sync variant** - sync SQL users are served by `rolify-diesel`.

## Database Setup

### Migrations

```rust
use sqlx::postgres::PgPoolOptions;
use sqlx::migrate::Migrator;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");  // or use rolify_sqlx::MIGRATOR

async fn run_migrations(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    MIGRATOR.run(pool).await
}
```

Or embed migrations:

```rust
use sqlx::migrate::Migrator;
static MIGRATOR: Migrator = sqlx::migrate!();  // reads from ./migrations at compile time
```

### Schema

```sql
-- crates/rolify-sqlx/migrations/postgres/integer/0000000001_rolify_create_tables.sql
-- (physical schema shared with diesel; laid out per engine, then per kind)
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

-- Default join table: users_roles (configurable via RolifyConfig)
CREATE TABLE users_roles (
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    CONSTRAINT users_roles_pair_unique UNIQUE (user_id, role_id)
);
```

### Holder id typing

`role_id` is the library-owned side: always a `BIGINT` FK to `roles(id)`.
`user_id` follows the holder id kind selected at scaffold time by
`--holder-id-type` (D-08-04):

| Kind | Postgres | MySQL | SQLite |
|------|----------|-------|--------|
| `integer` (default) | `BIGINT` | `BIGINT` | `INTEGER` |
| `uuid` | `UUID` | `BINARY(16)` | `TEXT` |
| `string` | `VARCHAR(191)` | `VARCHAR(191)` | `VARCHAR(191)` |

Migrations live under `crates/rolify-sqlx/migrations/{engine}/{kind}/` as
`0000000001_rolify_create_tables.sql` plus a `.down.sql` pair, so a consumer
selects exactly the tree matching its configuration. The kind baked into the
DDL must match `RolifyConfig::holder_id_kind`. For the SQLite `uuid` kind the
column is `TEXT` and UUID binds use the hyphenated string form. `integer`
(the default) restores the gem's `t.references` bigint affinity; `string`
keeps the pre-08 `VARCHAR(191)` form.

A holder id that does not parse for the configured kind (for example `"abc"`
under `integer`) returns `rolify_core::Error::InvalidHolderId { expected, got }`
before the query reaches the database (D-08-05). The optional
`--with-holder-fk` flag emits `REFERENCES <holder_table>(<pk>)` for
non-string kinds (D-08-07); it is off by default, matching the gem, which
emits no consumer-side FK (Entry 8). See PARITY.md Entry 29.

**Sentinel note:** Like Diesel, the adapter uses `''` (empty string) for `NULL` in `resource_type`/`resource_id` to make `UNIQUE` work identically across backends.

## Basic Usage

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_sqlx::SqlxStore;
use sqlx::PgPool;

struct Player {
    id: i64,
    store: SqlxStore<sqlx::Postgres>,
    pool: PgPool,
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = SqlxStore<sqlx::Postgres>;

    fn store(&mut self) -> &mut SqlxStore<sqlx::Postgres> {
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

    fn store_with_conn(&mut self) -> (&mut SqlxStore<sqlx::Postgres>, &mut sqlx::PgConnection) {
        // SQLx uses pool directly; we borrow a connection from the pool
        // For proper usage, see the connection handling section below
        unimplemented!("See connection handling section")
    }
}
```

### Connection Handling (Important)

SQLx doesn't have a synchronous connection object like Diesel. The adapter's `Conn` type is `sqlx::PgConnection` (or `sqlx::MySqlConnection`, etc.), but you typically work with a pool. The recommended pattern:

```rust
use sqlx::{PgPool, Postgres, Transaction};

struct Player {
    id: i64,
    pool: PgPool,
    config: RolifyConfig,
    // We don't store a connection; we acquire one per operation
}

impl Player {
    async fn with_store<F, R>(&mut self, f: F) -> Result<R, sqlx::Error>
    where
        F: for<'c> FnOnce(&'c mut SqlxStore<sqlx::Postgres>, &'c mut sqlx::PgConnection) -> std::pin::Pin<Box<dyn Future<Output = Result<R, sqlx::Error>> + Send + 'c>>,
    {
        let mut conn = self.pool.acquire().await?;
        let mut store = SqlxStore::<sqlx::Postgres>::new(&self.config);
        f(&mut store, &mut conn).await
    }
}

impl RolifyUser for Player {
    type Store = SqlxStore<sqlx::Postgres>;

    fn store(&mut self) -> &mut SqlxStore<sqlx::Postgres> {
        // SqlxStore is small; create on demand from the config
        Box::leak(Box::new(SqlxStore::<sqlx::Postgres>::new(&self.config)))  // Leaks - see the Rolify engine pattern below
    }

    fn rolify_config(&self) -> &RolifyConfig { &self.config }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "Player" }

    fn store_with_conn(&mut self) -> (&mut SqlxStore<sqlx::Postgres>, &mut sqlx::PgConnection) {
        // This is tricky with SQLx because we need a connection
        // Better: use the Rolify engine pattern
        unimplemented!("Use Rolify engine instead")
    }
}
```

### Recommended Pattern: Rolify Engine

```rust
use rolify_core::manager::Rolify;
use rolify_sqlx::SqlxStore;
use sqlx::{PgPool, Acquire};

struct AppState {
    pool: PgPool,
    config: RolifyConfig,
}

impl AppState {
    // Create a Rolify engine per request/task
    async fn rolify_engine(&self) -> Result<Rolify<SqlxStore<sqlx::Postgres>>, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        let store = SqlxStore::<sqlx::Postgres>::new(&self.config);
        Ok(Rolify::new(store, conn, self.config.clone()))
    }
}

// Usage in handlers
async fn make_moderator(state: &AppState, user_id: i64, forum_id: i64) -> Result<(), sqlx::Error> {
    let mut engine = state.rolify_engine().await?;
    let mut user = UserAdapter { id: user_id, engine: &mut engine };
    user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
    Ok(())
}

// User adapter that borrows the engine
struct UserAdapter<'a> {
    id: i64,
    engine: &'a mut Rolify<SqlxStore<sqlx::Postgres>>,
}

impl<'a> RolifyUser for UserAdapter<'a> {
    type Store = SqlxStore<sqlx::Postgres>;

    fn store(&mut self) -> &mut SqlxStore<sqlx::Postgres> { self.engine.store_with_conn().0 }
    fn rolify_config(&self) -> &RolifyConfig { self.engine.config() }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "Player" }
    fn store_with_conn(&mut self) -> (&mut SqlxStore<sqlx::Postgres>, &mut sqlx::PgConnection) {
        self.engine.store_with_conn()
    }
}
```

## Pool Configuration

```rust
use sqlx::postgres::PgPoolOptions;

let pool = PgPoolOptions::new()
    .max_connections(20)
    .min_connections(5)
    .acquire_timeout(std::time::Duration::from_secs(30))
    .idle_timeout(std::time::Duration::from_secs(600))
    .max_lifetime(std::time::Duration::from_secs(1800))
    .connect(&database_url)
    .await?;
```

## Custom Table Names

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_sqlx::SqlxStore;

let config = RolifyConfig::builder()
    .role_table("privileges")
    .join_table("users_privileges")
    .build()?;

// The store reads these from the config (defaults: "roles" / "users_roles")
let store = SqlxStore::<sqlx::Postgres>::new(&config);
let mut engine = Rolify::new(store, conn, config);
```

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
use sqlx::{PgPool, Transaction};

async fn atomic_role_grant(pool: &PgPool, user_id: i64) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let config = RolifyConfig::default();
    let store = SqlxStore::<sqlx::Postgres>::new(&config);
    let mut engine = Rolify::new(store, &mut *tx, config);
    
    let mut user = UserAdapter { id: user_id, engine: &mut engine };
    user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;
    user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
    
    tx.commit().await?;
    Ok(())
}
```

## MySQL / SQLite

```toml
# For MySQL
rolify-sqlx = { path = "../rolify-sqlx", features = ["mysql"] }
sqlx = { version = "0.9", features = ["mysql", "runtime-tokio", "tls-rustls-ring", "chrono"] }

# For SQLite
rolify-sqlx = { path = "../rolify-sqlx", features = ["sqlite"] }
sqlx = { version = "0.9", features = ["sqlite", "runtime-tokio", "chrono"] }
```

```rust
// MySQL
use sqlx::MySqlPool;
let pool = MySqlPool::connect(&database_url).await?;

// SQLite
use sqlx::SqlitePool;
let pool = SqlitePool::connect(&database_url).await?;
```

The adapter handles backend differences in SQL dialect.

## Performance Tips

1. **Prepared statements**: SQLx automatically prepares and caches statements.

2. **Connection pooling**: Configure pool size for your workload.

3. **Batch queries**: Use `has_any_roles` / `with_any_roles` for single OR-folded round-trips.

4. **Indexes**: The shipped migrations already carry `idx_roles_resource`, `idx_roles_name`, and `UNIQUE(user_id, role_id)` on the join table; add more only if your query patterns need them.

5. **Avoid N+1**: Use finder methods (`with_role`, `with_any_roles`) instead of looping.

## Troubleshooting

### "Runtime not found"

```toml
# Ensure exactly ONE runtime feature:
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio", "tls-rustls-ring"] }
# NOT: "runtime-tokio-rustls" (old combined feature, removed in 0.9)
```

### "TLS not configured"

```toml
# Add exactly ONE TLS feature:
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio", "tls-rustls-ring"] }
# Or: "tls-native-tls", "tls-openssl"
```

### "Type not supported"

Add the appropriate feature for your types:
```toml
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio", "tls-rustls-ring", "chrono", "uuid", "json"] }
```

### "Unique constraint violation"

The adapter uses `INSERT ... ON CONFLICT DO NOTHING` (Postgres) or equivalent for `find_or_create_by`.

### Migration errors

Ensure migrations run in order. The adapter expects the exact schema from the provided migrations.

## Migration from Ruby rolify (ActiveRecord)

| Ruby | SQLx |
|------|------|
| `User.rolify` | `impl RolifyUser for User` |
| `user.add_role(:admin)` | `user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?` |
| `user.has_role?(:admin, forum)` | `user.has_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &forum.resource_id())).await?` |
| `Forum.with_role(:admin)` | `Forum::with_role(&mut engine, &[RoleName::from("admin")], None).await?` |
| `rolify :role_cname => 'Privilege'` | `role_table("privileges")` |
| `rolify :role_join_table_name => 'users_privileges'` | `join_table("users_privileges")` |
| `ActiveRecord::Base.transaction` | `pool.begin().await?` + `tx.commit().await?` |

**Key difference:** SQLx is not an ORM - you write the queries (or use the adapter's provided methods). The adapter provides the rolify-specific queries; you handle connections/transactions.