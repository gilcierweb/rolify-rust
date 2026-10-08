# Coming from Ruby rolify

You know the gem; this guide maps what you know onto idiomatic Rust so you
never have to open the Ruby source. It covers the renamed and removed
surface (the alias migration table), worked grant/check/revoke examples at
each scope level with the real import paths, and the Mongoid-to-MongoDB
notes. Every deliberate behavioral difference underneath is labeled in
[PARITY.md](PARITY.md); this guide stays on the "how do I write it" level.

## Mental-model shifts (read these five first)

1. **Traits, not mixins.** `include Rolify::Role` becomes
   `impl RolifyUser for User`. Class methods (`User.with_role`) become
   provided associated functions on the same trait
   (`User::with_role(&mut rolify, &query)`). There is nothing to inherit;
   there is a trait to implement, once, on your own types.
2. **No global configuration.** `Rolify.configure` and
   `Rolify.resource_types` do not exist. A `RolifyConfig` builder value
   (strict mode, empty-role cleanup, hooks) is owned by the `Rolify<S>`
   handle you construct per request or task. See PARITY.md Entry 19.
3. **Typed scopes, not runtime sentinels.** The gem's
   `nil | Class | instance | :any` position becomes two compile-time types:
   `ResourceRef` for writes (`Global`, `Class`, `Instance`) and
   `ResourceFilter` for reads (plus `Any`). Invalid argument shapes are
   compile errors here, not `ArgumentError` at runtime.
4. **Finders return id lists.** `User.with_role(:admin)` returns
   `Vec<ResourceId>`, not a chainable relation. You filter your own table
   with `IN`. Order is unspecified; compare as sets. See PARITY.md
   Entry 20.
5. **Names are byte-exact, ids are text.** Role names are never normalized
   (no case folding, no trimming). Resource ids are stored as text, so
   integer PKs and string PKs (`teams.team_code` in the gem specs, UUIDs)
   compare uniformly.

## Alias migration table

One concept, one name: only `grant` (for `add_role`) and `revoke` (for
`remove_role`) survive as aliases, matching the gem's own `alias_method`
lines. Everything else maps to its canonical call:

| Ruby (gem) | Rust (this workspace) | Notes |
|------------|-----------------------|-------|
| `user.add_role(:admin)` | `user.add_role(&RoleName::from("admin"), ResourceRef::Global).await` | Same semantics, typed scope |
| `user.grant(:admin)` | `user.grant(&RoleName::from("admin"), ResourceRef::Global).await` | Kept alias (`role.rb:23`) |
| `user.remove_role(:admin)` | `user.remove_role(&name, RemovalTarget::NameOnly).await` | No-arg remove sweeps every scope, like the gem |
| `user.revoke(:admin)` | `user.revoke(&name, RemovalTarget::NameOnly).await` | Kept alias (`role.rb:85`) |
| `user.has_no_role(:admin)` (deprecated) | `user.remove_role(&name, RemovalTarget::NameOnly).await` | Deprecated alias, use the canonical remove |
| `user.has_role?(:admin)` | `user.has_role(&name, ResourceFilter::Global).await` | Returns `Result<bool, _>`; `?` it or unwrap in tests |
| `expect(user).to have_role(:admin)` (RSpec `have_role` matcher, `lib/rolify/matchers.rb`) | `user.assert_has_role(&name, ResourceFilter::Global, "context").await` | In tests: the `RoleAssertions` family; in code: `has_role` |
| `expect(user).not_to have_role(:admin)` | `user.assert_has_no_role(&name, ResourceFilter::Global, "context").await` | Symmetric negative, same message shape |
| `user.is_admin?` / `user.is_moderator_of?(forum)` (dynamic shortcuts) | `user.has_role(&RoleName::from("admin"), ResourceFilter::Global).await` | Not ported (no `method_missing`); `has_role` is the promoted idiom |
| `User.with_role(:admin)` | `User::with_role(&mut rolify, &query)` | Returns `Vec<ResourceId>`; strict-aware except `:any` |
| `Forum.with_role(:mod)` / `Forum.find_as` / `Forum.find_multiple_as` / `Forum.with_roles` | `Forum::with_role(&mut rolify, &name, user)` | Dropped aliases, one canonical name |
| `Forum.without_role(:mod)` / `Forum.except_as` / `Forum.except_multiple_as` / `Forum.without_roles` | `Forum::without_role(&mut rolify, &name, user, candidates)` | Dropped aliases; you supply the `candidates` universe (PARITY.md Entry 21) |
| `Forum.find_roles` / `Forum.applied_roles` | `Forum::find_roles(&mut rolify, name, user)` / `Forum::applied_roles(&mut rolify, children)` | Same pair, associated functions |
| `Rolify.configure { ... strict ... }` | `RolifyConfig::builder().strict(true).build()` | Owned value, no globals |
| `Rolify.resource_types` | `Resource::type_name` / `descendant_types()` on your types | Associated items, no global registry |
| New (unsaved) record checks | `RoleSet` plus `has_cached_role` family (zero I/O) | The gem's `new_record?` path has no direct port; check the snapshot explicitly (PARITY.md Entry 1) |
| `user.roles.global` / `.class_scoped` / `.instance_scoped` | `roles.global()` / `.class_scoped(..)` / `.instance_scoped(..)` on `RoleSet` | Snapshot-side filters, same result sets (PARITY.md Entry 22) |

## Worked examples per scope

Imports used below (real paths):

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
```

`player` is any type implementing `RolifyUser`; `forum_id` is a
`ResourceId`. Async shown; the sync build (`is_sync` feature) compiles the
same calls to plain blocking calls (drop the `.await`s).

### Global scope: site-wide admin

```rust
let admin = RoleName::from("admin");

// Grant: one idempotent call (find-or-create the row, then link).
player.add_role(&admin, ResourceRef::Global).await.unwrap();

// Check: a global grant satisfies any narrower lookup (global override).
let held = player.has_role(&admin, ResourceFilter::Global).await.unwrap();
assert!(held);

// Revoke: with no scope given, every scope with this name is swept.
player.remove_role(&admin, RemovalTarget::NameOnly).await.unwrap();
assert!(!player.has_role(&admin, ResourceFilter::Global).await.unwrap());
```

### Class scope: moderator of every forum

```rust
let moderator = RoleName::from("moderator");
let forum_id = ResourceId::from(1_i64);

// Grant against the class, not an instance.
player.add_role(&moderator, ResourceRef::Class("Forum")).await.unwrap();

// Check at class scope, and at instance scope (class covers instance).
assert!(player.has_role(&moderator, ResourceFilter::Class("Forum")).await.unwrap());
assert!(player.has_role(&moderator, ResourceFilter::Instance("Forum", &forum_id)).await.unwrap());

// Revoke just this class scope; other scopes with the same name survive.
player.remove_role(&moderator, RemovalTarget::TypeSweep("Forum")).await.unwrap();
```

### Instance scope: moderator of one forum

```rust
let moderator = RoleName::from("moderator");
let forum_id = ResourceId::from(1_i64);
let other_id = ResourceId::from(2_i64);

// Grant against one instance.
player
    .add_role(&moderator, ResourceRef::Instance("Forum", &forum_id))
    .await
    .unwrap();

// Check: exact instance hits; a sibling instance misses.
assert!(player.has_role(&moderator, ResourceFilter::Instance("Forum", &forum_id)).await.unwrap());
assert!(!player.has_role(&moderator, ResourceFilter::Instance("Forum", &other_id)).await.unwrap());

// Revoke exactly this row.
let target = RemovalTarget::Exact("Forum", &forum_id);
player.remove_role(&moderator, target).await.unwrap();
```

### The same three checks in tests

```rust
use rolify_test::RoleAssertions;

player.assert_has_role(&admin, ResourceFilter::Global, "seeded admin").await;
player.assert_has_no_role(&moderator, ResourceFilter::Instance("Forum", &forum_id), "never granted").await;
```

The `assert_has_role` family (plus strict, all, any, only, and names
variants, each with a symmetric `assert_has_no_*` negative and a zero-I/O
`assert_cached_*` twin) is the port of the RSpec `have_role` matcher. Every
method panics with the full context (holder id, expected scope, complete
held-role list); store failures panic through the same message with a
`store_error` line instead of returning `Err`.

## Mongoid to MongoDB notes

If you used rolify with Mongoid, the MongoDB adapter (`rolify-mongodb`) is
your home. The document shape mirrors the Mongoid generator template
(`lib/generators/mongoid/rolify_generator.rb` plus its `README-mongoid`):

- **Role documents** carry `name`, `resource_type`, and `resource_id`, with
  absent scope stored as explicit BSON null (mirroring Mongoid's `nil`, not
  the SQL `''` sentinel; see PARITY.md Entry 24). Integer PKs and UUIDs are
  stringified at the adapter boundary, so they travel as BSON strings.
- **Two-sided id arrays.** The role document holds `user_ids`; your consumer
  document gains `role_ids`. Granting updates both sides (`$addToSet`);
  revoking pulls from both (`$pull`). The adapter writes your collection on
  your behalf; the field names follow the adapter's document module.
- **Unique compound index.** `(name, resource_type, resource_id)` is created
  idempotently at store setup (no migration step, no manual index). Races
  resolve via duplicate-key (11000) catch plus re-read, the document twin of
  the SQL unique-violation path.
- **Emptiness after removal.** `remove_role_if_empty` (default on, same as
  the gem) checks emptiness AFTER the removal sweep: when the last member
  leaves, the role document itself is deleted. Orphan cleanup is
  converge-by-design, not a migration concern.
- **Sync, if you need it.** The adapter is async-first with an opt-in `sync`
  feature mirroring the official driver's own duality
  (`sync = ["is_sync", "mongodb/sync"]`). The full suite runs in both modes.

## Where to go next

- [PARITY.md](PARITY.md): the complete divergence ledger with gem
  file-and-line cites and pinning tests. Start with entries 16-19 (aliases,
  dynamic shortcuts, STI, global config) if you are mid-migration.
- The crate rustdoc (`cargo doc -p rolify-core --open`): every public item
  carries a runnable example, and the doctests run in both sync and async
  builds.
- The ported suite behind the `suite` feature of `rolify-test`: the same
  behavioral contract every adapter proves, readable as executable
  documentation.
