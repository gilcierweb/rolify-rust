# SeaORM Adapter Guide

The `rolify-seaorm` crate provides an async-only SeaORM 2.0+ adapter for rolify-rust.

## Installation

```toml
# Cargo.toml
[dependencies]
rolify-core = { path = "../rolify-core" }
rolify-seaorm = { path = "../rolify-seaorm", features = ["postgres"] }  # or "mysql"
sea-orm = { version = "2.0", features = ["sqlx-postgres", "runtime-tokio", "macros"] }
tokio = { version = "1.53", features = ["full"] }
```

**Feature matrix:**

| Feature | Backend |
|---------|---------|
| `postgres` | PostgreSQL (via sqlx-postgres) |
| `mysql` | MySQL (via sqlx-mysql) |
| `sqlite` | SQLite (experimental, sync via rusqlite - out of scope) |

**SeaORM 2.0** requires SeaQuery 1.0 + SQLx 0.9 (verified).

## Database Setup

### Migrations

```rust
use sea_orm_migration::MigratorTrait;
use rolify_seaorm::migration::Migrator;  // Provided by rolify-seaorm

async fn run_migrations(db: &sea_orm::DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    Migrator::up(db, None).await
}
```

### Schema

SeaORM uses entities. The adapter provides entities for `roles` and the join table:

```rust
// rolify_seaorm::entities::roles
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "roles")]  // Configurable via RolifyConfig
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub name: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::users_roles::Entity")]
    UsersRoles,
}

// Join table entity
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "users_roles")]  // Configurable via RolifyConfig
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: String,
    #[sea_orm(primary_key)]
    pub user_type: String,
    #[sea_orm(primary_key)]
    pub role_id: i64,
}
```

**Sentinel note:** The adapter uses `''` (empty string) for `NULL` in `resource_type`/`resource_id` to make `UNIQUE` work identically across backends.

### Generating Entities (Optional)

If you want to customize entities, generate them from your database:

```bash
sea-orm-cli generate entity -o src/entities -u postgres://user:pass@localhost/db
```

Then implement the adapter's `SeaOrmRoleStore` with your custom entities.

## Basic Usage

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_seaorm::SeaOrmRoleStore;
use sea_orm::{Database, DatabaseConnection};

struct Player {
    id: i64,
    store: SeaOrmRoleStore,
    db: DatabaseConnection,
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = SeaOrmRoleStore;

    fn store(&mut self) -> &mut SeaOrmRoleStore {
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

    fn store_with_conn(&mut self) -> (&mut SeaOrmRoleStore, &mut DatabaseConnection) {
        (&mut self.store, &mut self.db)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::connect("postgres://user:pass@localhost/db").await?;
    
    // Run migrations
    rolify_seaorm::migration::Migrator::up(&db, None).await?;
    
    let store = SeaOrmRoleStore::new();
    let config = RolifyConfig::default();
    
    let mut player = Player { id: 1, store, db, config };
    player.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;
    
    Ok(())
}
```

## Using with Rolify Engine

```rust
use rolify_core::manager::Rolify;
use rolify_seaorm::SeaOrmRoleStore;
use sea_orm::DatabaseConnection;

let db = Database::connect("postgres://user:pass@localhost/db").await?;
let store = SeaOrmRoleStore::new();
let config = RolifyConfig::default();

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

## Custom Table Names

```rust
use rolify_core::config::RolifyConfig;

let config = RolifyConfig::builder()
    .role_table("app_roles")
    .join_table("accounts_app_roles")
    .build()?;

// The entities use these names at runtime
let store = SeaOrmRoleStore::with_table_names("app_roles", "accounts_app_roles");
```

Or set via environment/config before generating entities.

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

struct Car { id: i64 }
impl Resource for Car {
    fn type_name() -> &'static str { "Car" }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}
```

## Transaction Support

```rust
use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};

async fn atomic_role_grant(db: &DatabaseConnection, user_id: i64) -> Result<(), sea_orm::DbErr> {
    let tx = db.begin().await?;
    let store = SeaOrmRoleStore::new();
    let config = RolifyConfig::default();
    let mut engine = Rolify::new(store, tx, config);
    
    let mut user = UserAdapter { id: user_id, engine: &mut engine };
    user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?;
    user.add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum")).await?;
    
    engine.conn().commit().await?;
    Ok(())
}

struct UserAdapter<'a> {
    id: i64,
    engine: &'a mut Rolify<SeaOrmRoleStore, DatabaseTransaction>,
}

impl<'a> RolifyUser for UserAdapter<'a> {
    type Store = SeaOrmRoleStore;
    fn store(&mut self) -> &mut SeaOrmRoleStore { &mut self.engine.store }
    fn rolify_config(&self) -> &RolifyConfig { self.engine.config() }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "Player" }
    fn store_with_conn(&mut self) -> (&mut SeaOrmRoleStore, &mut DatabaseTransaction) {
        self.engine.store_with_conn()
    }
}
```

## Connection Pooling

```rust
use sea_orm::{Database, ConnectOptions};
use std::time::Duration;

let mut opt = ConnectOptions::new("postgres://user:pass@localhost/db");
opt.max_connections(20)
    .min_connections(5)
    .connect_timeout(Duration::from_secs(30))
    .idle_timeout(Duration::from_secs(600))
    .max_lifetime(Duration::from_secs(1800))
    .sqlx_logging(true);

let db = Database::connect(opt).await?;
```

## SeaORM's Built-in RBAC (Important)

SeaORM 2.0 ships its own `rbac` feature - **do not confuse it with rolify-rust**:

| Aspect | SeaORM RBAC | rolify-rust |
|--------|-------------|-------------|
| Philosophy | Enforcement-oriented | Role management only |
| Roles per user | Exactly ONE | Multiple |
| Scope | Table-scoped CRUD permissions | Instance-scoped (three levels) |
| Authorization | Query-time auditing | **None** (you enforce) |
| Use case | Full RBAC system | Complement SeaORM RBAC |

**rolify-rust positions itself as complementary to SeaORM's RBAC** - use rolify for flexible role assignment, SeaORM RBAC for permission enforcement, or both.

## Performance Tips

1. **Select related**: Use `find_with_related` for eager loading.

2. **Batch queries**: `has_any_roles` / `with_any_roles` use single OR-folded queries.

3. **Indexes**: Ensure indexes on `roles(resource_type, resource_id)` and join table.

4. **Connection pooling**: Configure via `ConnectOptions`.

## Troubleshooting

### "Entity not found"

Ensure you've run migrations or the tables exist with the expected schema.

### "Unique constraint violation"

The adapter handles `find_or_create_by` with `ON CONFLICT DO NOTHING` (Postgres) / equivalent.

### "Async trait not dyn-compatible"

SeaORM uses async traits. The adapter implements the SPI with `#[maybe_async(AFIT)]` for dual-mode compatibility.

### Feature conflicts

```toml
# Ensure compatible features:
sea-orm = { version = "2.0", features = ["sqlx-postgres", "runtime-tokio", "macros"] }
# NOT: "sqlx-postgres" + "sqlx-mysql" simultaneously (pick one)
```

## Migration from Ruby rolify

| Ruby | SeaORM |
|------|--------|
| `User.rolify` | `impl RolifyUser for User` |
| `user.add_role(:admin)` | `user.add_role(&RoleName::from("admin"), ResourceRef::Global).await?` |
| `user.has_role?(:admin, forum)` | `user.has_role(&RoleName::from("admin"), ResourceFilter::Instance("Forum", &forum.resource_id())).await?` |
| `Forum.with_role(:admin)` | `Forum::with_role(&mut engine, &[RoleName::from("admin")], None).await?` |
| `config.role_cname` | `role_table("roles")` |
| `config.join_table_name` | `join_table("users_roles")` |
| `ActiveRecord::Base.transaction` | `db.begin().await?` + `tx.commit().await?` |

**Key difference:** SeaORM is an async ORM with entities. The adapter provides SeaORM entities for roles/join table; you use SeaORM's query builder for your domain models.