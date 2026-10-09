# Core Concepts

This document explains the fundamental types and concepts in rolify-rust.

## RoleName

`RoleName` is a newtype wrapper around `String` with **exact byte equality** — no case-folding, no trimming.

```rust
use rolify_core::role::RoleName;

let admin = RoleName::from("admin");
let Admin = RoleName::from("Admin");  // Different!
assert_ne!(admin, Admin);

let admin2 = RoleName::new("admin");
assert_eq!(admin, admin2);
```

**Why byte-exact?** The Ruby gem compares with `role.name == args[:name].to_s` — normalization would be a cross-adapter drift and potential spoofing vector.

## ResourceId

`ResourceId` is an opaque, stringified primary key. Both integer and string PKs share one representation:

```rust
use rolify_core::role::ResourceId;

// Integer PKs
assert_eq!(ResourceId::from(42_i64), ResourceId::from(42_u64));
assert_eq!(ResourceId::from(42_i64).as_str(), "42");

// String PKs (e.g., UUIDs, slugs)
let uuid = ResourceId::from("550e8400-e29b-41d4-a716-446655440000");
assert_eq!(uuid.as_str(), "550e8400-e29b-41d4-a716-446655440000");
```

## RoleRecord

A persisted role row with three scope columns:

| Scope | `resource_type` | `resource_id` |
|-------|-----------------|---------------|
| Global | `None` | `None` |
| Class | `Some("Forum")` | `None` |
| Instance | `Some("Forum")` | `Some("7")` |

```rust
use rolify_core::role::{RoleRecord, ResourceId};

// Global role
let global = RoleRecord::global("admin");
assert!(global.is_global());

// Class-scoped role
let class = RoleRecord::for_class("moderator", "Forum");
assert!(class.is_class_scoped_to("Forum"));

// Instance-scoped role
let instance = RoleRecord::for_instance("owner", "Forum", 7_i64);
assert!(instance.is_instance_scoped_to("Forum", &ResourceId::from(7_i64)));
```

### Physical Storage Note

Adapters translate `None` to the sentinel empty string `''` for both columns so that `UNIQUE(name, resource_type, resource_id)` deduplicates identically on Postgres, MySQL, and SQLite. This diverges from the gem (which stores `NULL`); the parity matrix records this.

## ResourceRef (Write Scope)

Used when **granting/removing** roles — the write-side scope enum:

```rust
use rolify_core::resource::ResourceRef;
use rolify_core::role::ResourceId;

let global = ResourceRef::Global;
let class = ResourceRef::Class("Forum");
let instance = ResourceRef::Instance("Forum", &ResourceId::from(7_i64));
```

**No `Any` variant** — a role cannot be granted "at whatever scope." `Any` is query-only.

## ResourceFilter (Read Scope)

Used when **querying** roles — the read-side scope enum:

```rust
use rolify_core::query::ResourceFilter;
use rolify_core::role::ResourceId;

let global = ResourceFilter::Global;
let class = ResourceFilter::Class("Forum");
let instance = ResourceFilter::Instance("Forum", &ResourceId::from(7_i64));
let any = ResourceFilter::Any;  // Query-only: name match across ALL scopes
```

## RoleQuery

A borrowed role name plus a scope filter — the port of the gem's `{name:, resource:}` query hash:

```rust
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::RoleName;

let name = RoleName::from("admin");

// Global query (default)
let query = RoleQuery::with_role(&name);
assert!(matches!(query.filter, ResourceFilter::Global));

// Explicit filter
let query = RoleQuery::with_role_and_filter(&name, ResourceFilter::Any);
```

**Borrow-first design** — the query never allocates; `name` is `&RoleName`. Store implementations decide whether to clone.

## The Match Ladder (Kernel)

The pure kernel (`crate::kernel`) implements the gem's `build_query`/`where_`/`find_cached` logic with zero I/O. Two entry points:

### Non-Strict (Default)

```rust
use rolify_core::kernel::where_;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{RoleName, RoleRecord, ResourceId};

let rows = vec![
    RoleRecord::global("admin"),
    RoleRecord::for_class("moderator", "Forum"),
    RoleRecord::for_instance("owner", "Forum", 7_i64),
];

let admin = RoleName::from("admin");
let moderator = RoleName::from("moderator");
let owner = RoleName::from("owner");
let forum_7 = ResourceId::from(7_i64);

// Global query matches global only
assert!(!where_(&rows, &RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum"))).is_empty());  // global covers class
assert!(where_(&rows, &RoleQuery::with_role_and_filter(&admin, ResourceFilter::Instance("Forum", &forum_7))).is_empty());  // Wait - global DOES cover instance in non-strict!

// Class query matches class + global
assert!(!where_(&rows, &RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Instance("Forum", &forum_7))).is_empty());  // class covers instance
assert!(where_(&rows, &RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Global)).is_empty());  // reverse never holds

// Instance query matches instance only (plus global/class via ladder)
assert!(!where_(&rows, &RoleQuery::with_role_and_filter(&owner, ResourceFilter::Instance("Forum", &forum_7))).is_empty());
assert!(where_(&rows, &RoleQuery::with_role_and_filter(&owner, ResourceFilter::Class("Forum"))).is_empty());  // instance is not class

// Any matches everything by name
assert!(!where_(&rows, &RoleQuery::with_role_and_filter(&admin, ResourceFilter::Any)).is_empty());
```

### Strict

```rust
use rolify_core::kernel::where_strict;

let rows = vec![
    RoleRecord::global("admin"),
    RoleRecord::for_class("moderator", "Forum"),
];

let moderator = RoleName::from("moderator");

// Exact scope only
assert!(!where_strict(&rows, &RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Class("Forum"))).is_empty());
assert!(where_strict(&rows, &RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Instance("Forum", &ResourceId::from(7_i64)))).is_empty());
assert!(where_strict(&rows, &RoleQuery::with_role_and_filter(&moderator, ResourceFilter::Global)).is_empty());

// Global still matches exactly global
let admin = RoleName::from("admin");
assert!(!where_strict(&rows, &RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global)).is_empty());
```

## RoleSet (Zero-I/O Cached Snapshot)

`RoleSet` borrows pre-fetched role rows and answers membership questions purely via the kernel — **zero I/O by signature** (no store handle in the API):

```rust
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::role::{RoleName, RoleRecord, RoleSet, ResourceId};

let rows = vec![
    RoleRecord::global("admin"),
    RoleRecord::for_class("manager", "Forum"),
    RoleRecord::for_instance("moderator", "Forum", 7_i64),
];
let set = RoleSet::new(&rows);

let admin = RoleName::from("admin");
let manager = RoleName::from("manager");
let forum_7 = ResourceId::from(7_i64);

// Non-strict cached (gem's has_cached_role?)
assert!(set.has_cached_role(&RoleQuery::with_role_and_filter(&admin, ResourceFilter::Instance("Forum", &forum_7))));

// Strict cached (gem's has_strict_cached_role?)
assert!(set.has_strict_cached_role(&RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum"))));

// Snapshot filters
let global_roles = set.global();
let class_roles = set.class_scoped(Some("Forum"));
let instance_roles = set.instance_scoped(Some("Forum"), Some(&forum_7));

// Composite checks (gem's has_all_roles?, has_any_role?, only_has_role?)
let queries = vec![
    RoleQuery::with_role(&admin),
    RoleQuery::with_role_and_filter(&manager, ResourceFilter::Class("Forum")),
];
assert!(set.has_all_cached(&queries));
assert!(set.has_any_cached(&[RoleQuery::with_role(&RoleName::from("ghost")), RoleQuery::with_role(&admin)]));
assert!(set.only_has_cached(&RoleQuery::with_role(&admin)));  // false - 3 roles total
```

## Resource Trait (Resource-Side Operations)

Implement `Resource` on your domain types (e.g., `Forum`, `Group`) to get resource-side finders:

```rust
use rolify_core::resource::Resource;
use rolify_core::role::ResourceId;

struct Forum {
    id: i64,
}

impl Resource for Forum {
    fn type_name() -> &'static str {
        "Forum"
    }
    fn resource_id(&self) -> ResourceId {
        ResourceId::from(self.id)
    }
}
```

### Resource-Side Methods

| Method | Description |
|--------|-------------|
| `find_roles(rolify, name?, user?)` | Class-level: roles matching name/user on this type family |
| `applied_roles(rolify, children)` | Class-level: class-scoped roles of type family (children = include descendants) |
| `roles_of_instance(rolify, instance)` | Instance-level: ALL rows bound to that instance (any holder) |
| `applied_roles_of_instance(rolify, instance)` | Instance-level: instance-bound UNION class-scoped of type family |
| `with_role(rolify, names, user?)` | Class-level: resource keys holding any of `names` |
| `without_role(rolify, names, user?, universe)` | Class-level: universe minus `with_role` matches |

### STI Support

```rust
struct Vehicle { id: i64 }
impl Resource for Vehicle {
    fn type_name() -> &'static str { "Vehicle" }
    fn descendant_types() -> Vec<&'static str> { vec!["Vehicle", "Car"] }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}

struct Car { id: i64 }
impl Resource for Car {
    fn type_name() -> &'static str { "Car" }
    fn resource_id(&self) -> ResourceId { ResourceId::from(self.id) }
}

// Vehicle::descendant_types() returns ["Vehicle", "Car"]
// Car::descendant_types() returns ["Car"] (default)
```

**Important:** `descendant_types` feeds ONLY resource-side finders — never the user-side match ladder (Pitfall 1 guard).

## RolifyUser Trait (User-Side Operations)

Implement `RolifyUser` on your user/account type:

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::user::RolifyUser;
use rolify_core::store::RoleStore;

struct Player {
    id: i64,
    store: MyStore,
    conn: MyConn,
    config: RolifyConfig,
}

impl RolifyUser for Player {
    type Store = MyStore;

    fn store(&mut self) -> &mut MyStore { &mut self.store }
    fn rolify_config(&self) -> &RolifyConfig { &self.config }
    fn rolify_id(&self) -> ResourceId { ResourceId::from(self.id) }
    fn rolify_type() -> &'static str { "Player" }
    fn store_with_conn(&mut self) -> (&mut MyStore, &mut MyConn) {
        (&mut self.store, &mut self.conn)
    }
}
```

### Provided Methods

| Method | Gem Equivalent | Description |
|--------|----------------|-------------|
| `add_role(name, scope)` | `add_role` | Grant role (idempotent: row dedupe + link guard) |
| `grant(name, scope)` | `grant` | Alias for `add_role` |
| `remove_role(name, target)` | `remove_role` | Revoke by `RemovalTarget` |
| `revoke(name, target)` | `revoke` | Alias for `remove_role` |
| `has_role(name, filter)` | `has_role?` | Non-strict check (respects config strict gate) |
| `has_strict_role(name, filter)` | `has_strict_role?` | Direct strict check (no gate) |
| `has_cached_role(snapshot, query)` | `has_cached_role?` | Zero-I/O check over borrowed rows |
| `has_all_roles(queries)` | `has_all_roles?` | All queries match (early exit) |
| `has_any_roles(queries)` | `has_any_role?` | Any query matches (single OR-folded round-trip) |
| `only_has_role(name, filter)` | `only_has_role?` | Has role AND exactly one role total |
| `roles_name()` | `roles_name` | All role names linked to this holder |

### Finder Methods (Class-Level)

| Method | Gem Equivalent |
|--------|----------------|
| `with_role(rolify, query)` | `with_role` |
| `without_role(rolify, query)` | `without_role` |
| `with_all_roles(rolify, queries)` | `with_all_roles` |
| `with_any_roles(rolify, queries)` | `with_any_roles` |

## Rolify Engine Handle

`Rolify<S>` bundles a store, its connection, and the single `RolifyConfig` source of truth:

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::manager::Rolify;
use rolify_test::InMemoryStore;

let mut engine = Rolify::new(InMemoryStore::new(), (), RolifyConfig::default());
let config = engine.config();  // The ONE config source
let (store, conn) = engine.store_with_conn();
```

Consumers implementing `RolifyUser::rolify_config` point at a clone of this same instance (cheap — hooks are `Arc`).

## RemovalTarget

Specifies what to revoke when removing roles:

```rust
use rolify_core::kernel::RemovalTarget;
use rolify_core::role::ResourceId;

RemovalTarget::NameOnly           // All scopes for this name
RemovalTarget::TypeSweep("Forum") // All class-scoped for this type
RemovalTarget::Exact("Forum", &ResourceId::from(7_i64))  // Exact instance
```

## Error Handling

All operations return `Result<T, E>` where `E` is the store's error type (implements `From<RolifyError>`). The shared `RolifyError` enum:

```rust
use rolify_core::error::RolifyError;

RolifyError::Store { source }           // Backend-specific error
RolifyError::CallbackVeto { callback, reason }  // before_add/before_remove veto
RolifyError::InvalidConfig { reason }   // Config validation failure
RolifyError::RoleNotFound { name }      // Role row not found
RolifyError::ResourceNotFound { type, id }  // Resource not found
```

## Dual Mode (Sync/Async)

All traits use `#[maybe_async::maybe_async(AFIT)]` — one source of truth for both modes:

| Mode | Feature | Return Type |
|------|---------|-------------|
| async (default) | *(none)* | `impl Future<Output = T> + Send` |
| sync | `is_sync` | `T` |

The macro mechanically strips futures/awaits under `is_sync`. The pure kernel and all value types are mode-agnostic.

```rust
// This code compiles in BOTH modes
#[maybe_async::maybe_async]
async fn example<U: RolifyUser>(user: &mut U) -> Result<bool, U::Store::Error> {
    user.has_role(&RoleName::from("admin"), ResourceFilter::Global).await
}
```