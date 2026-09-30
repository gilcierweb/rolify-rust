# rolify-rust

A Rust port of the Ruby [rolify](https://github.com/RolifyCommunity/rolify) gem:
minimalist role management (scoped RBAC storage and queries) with **no
authorization enforcement**, integrated with any authentication or
authorization stack through idiomatic persistence adapters.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust edition 2024](https://img.shields.io/badge/edition-2024-orange.svg)](https://doc.rust-lang.org/edition-guide/rust-2024/)
[![MSRV 1.85+](https://img.shields.io/badge/MSRV-1.85+-green.svg)](#msrv-policy)
[![Clippy all + pedantic](https://img.shields.io/badge/clippy-all%20%2B%20pedantic-yellow.svg)](#quality-gates)

> Status: active development. `rolify-core` (pure semantics kernel) and the
> `rolify-test` in-memory reference store are implemented and tested in both
> sync and async modes. The four backend adapters and the CLI generator are
> scaffolded and land per the [roadmap](#roadmap).

## Table of contents

- [What it does](#what-it-does)
- [Scope model](#scope-model)
- [Workspace layout](#workspace-layout)
- [Tech stack](#tech-stack)
- [Architecture](#architecture)
- [Requirements](#requirements)
- [Quick start](#quick-start)
- [Local setup](#local-setup)
- [Docker and databases](#docker-and-databases)
- [Databases](#databases)
- [Testing](#testing)
- [Configuration](#configuration)
- [Error handling](#error-handling)
- [Sync and async duality](#sync-and-async-duality)
- [MSRV policy](#msrv-policy)
- [Roadmap](#roadmap)
- [Ruby rolify parity](#ruby-rolify-parity)
- [Quality gates](#quality-gates)
- [Contributing](#contributing)
- [License](#license)

## What it does

`rolify-rust` answers one question: **does this user hold this role, at this
scope?** It stores role assignments and resolves scoped queries with the
gem's precedence semantics. It deliberately does not decide what a role is
allowed to do; enforcement stays in your application (the same boundary the
Ruby gem keeps with CanCanCan, Pundit, or Authority).

Core capabilities (target v1 surface):

- `add_role` / `grant` and `remove_role` / `revoke` across three scopes,
  idempotent at both the role-row and the assignment level.
- `has_role`, `has_strict_role`, cached (`has_cached_role`) and set
  predicates (`has_all_roles`, `has_any_role`, `only_has_role`).
- Class finders (`with_role`, `with_all_roles`, `with_any_role`,
  `without_role`) and the resource-side API (`resourcify`, `applied_roles`,
  `find_roles`).
- Configurable strict mode, empty-role cleanup, and before/after hooks.
- One shared behavioral suite (ported from the gem's `shared_examples`)
  that every adapter must pass, so backends cannot drift semantically.

Out of scope (same as the gem, plus Rust-specific exclusions):

- Authorization enforcement.
- Dynamic shortcuts (`is_admin?`, `is_moderator_of?`): Rust has no
  `method_missing`; the idiomatic equivalent is
  `has_role(&RoleName::from("admin"))`.
- Additional ORMs or databases beyond the four adapters (post-v1).

## Scope model

Every role row carries `(name, resource_type, resource_id)`:

| Scope    | `resource_type` | `resource_id` | Example                          |
|----------|-----------------|---------------|----------------------------------|
| Global   | `NULL`          | `NULL`        | `admin` everywhere               |
| Class    | `"Forum"`       | `NULL`        | `moderator` of every forum       |
| Instance | `"Forum"`       | `"1"`         | `moderator` of forum #1 only     |

The non-strict match ladder (ported verbatim from
`role_adapter.rb` `build_query`):

- A **global** query matches global rows only.
- A **class** query matches global rows (override) plus class rows of that type.
- An **instance** query matches global rows, class rows of that type, and the
  exact instance row.
- An **`:any`** query matches by name only, including global rows (the gem's
  database path; recorded as deliberate divergence D-02).
- **Strict mode** disables the override: only the exact scope matches, and it
  engages solely for class/instance queries, never for global or `:any`.

Role names compare **byte-exact**: no case folding, no trimming, ever.

## Workspace layout

Cargo workspace of seven crates (`resolver = "3"`, edition 2024):

| Crate           | Role                                              | Status        |
|-----------------|---------------------------------------------------|---------------|
| `rolify-core`   | Backend-free types, pure kernel, sealed `RoleStore` SPI, `RolifyUser` consumer trait, `RolifyConfig`, `RolifyError` | Implemented (kernel + SPI) |
| `rolify-test`   | `InMemoryStore` reference implementation plus (later) consumer assertion helpers | Implemented (store) |
| `rolify-diesel` | Sync-first reference adapter (Diesel 2.3), optional async via diesel-async | Scaffolded (Phase 3) |
| `rolify-sqlx`   | Async-only adapter, hand-written SQL (SQLx 0.9)   | Scaffolded (Phase 4) |
| `rolify-seaorm` | Async-only adapter (SeaORM 2.0 entities)          | Scaffolded (Phase 5) |
| `rolify-mongodb`| Async-first adapter mirroring the driver's sync feature (MongoDB driver 3.9) | Scaffolded (Phase 5) |
| `rolify-cli`    | `rails g rolify` equivalent: emits migrations plus scaffolding per backend | Scaffolded (Phase 6) |

```text
rolify-rust/
  Cargo.toml            # workspace: shared deps, lint table, resolver v3
  Cargo.lock
  README.md             # this file
  docker-compose.yml    # optional persistent dev databases (tests use testcontainers)
  .env.example          # database URLs and ports (copy to .env)
  rust-toolchain.toml   # pinned toolchain (workspace-wide max MSRV)
  crates/
    rolify-core/src/    # lib, kernel, role, query, resource, store, user, config, error
    rolify-test/src/    # InMemoryStore
    rolify-diesel/src/
    rolify-sqlx/src/
    rolify-seaorm/src/
    rolify-mongodb/src/
    rolify-cli/src/
```

## Tech stack

### Core (workspace foundation)

| Technology   | Version | Purpose |
|--------------|---------|---------|
| Rust         | edition 2024, MSRV 1.85 baseline | All workspace crates |
| `thiserror`  | 2.0.21  | Public error enums (`RolifyError` per crate); consumers can `match` on variants |
| `maybe-async`| 0.2.11  | Single-source sync/async traits via the `is_sync` feature flag |
| `async-trait`| 0.1.92  | Dyn-compatible async traits where object safety is required |
| `serde`      | 1.0.229 | Opt-in feature: serialize `RoleName` and resource types (SeaORM derives, MongoDB documents) |
| `clap`       | 4.6.7   | `rolify-cli` argument parsing (`derive` feature) |
| `tokio`      | 1.53.1  | Async runtime for tests and async adapters (feature-gated, never a hard dependency of `rolify-core`) |

### Persistence adapters

| Backend      | Crate            | Driver                | Sync / async story |
|--------------|------------------|-----------------------|--------------------|
| Diesel       | `rolify-diesel`  | `diesel` 2.3.13 (+ `diesel_migrations` 2.3.2) | Sync-first reference adapter; optional async via `diesel-async` 0.9.2 (`bb8` 0.9 / `deadpool` 0.13 pools) |
| SQLx         | `rolify-sqlx`    | `sqlx` 0.9.0 (`runtime-tokio` + `tls-rustls-ring`) | Async-only; sync SQL users are served by `rolify-diesel` |
| SeaORM       | `rolify-seaorm`  | `sea-orm` 2.0.4 (SeaQuery 1.0, SQLx 0.9) | Async-only; built on plain entities, never on SeaORM's enforcement-oriented `rbac` module |
| MongoDB      | `rolify-mongodb` | `mongodb` 3.9.1 (bson 2.15 default, opt-in `bson-3`) | Async-first; mirrors the driver's official `sync` feature |

### Test and dev tooling

| Tool | Version | Purpose |
|------|---------|---------|
| `rstest` | 0.27.0 | Parametrized and fixture tests; the port of RSpec `shared_examples` |
| `faker-rust` | 0.1 (author's 7.x line) | Test-data generation (dev-dependency) |
| `pretty_assertions` | 1.4.1 | Readable assertion diffs |
| `assert_cmd` + `predicates` | 2.2.2 / 3.1.4 | `rolify-cli` end-to-end tests |
| `testcontainers` + `testcontainers-modules` | 0.27.3 / 0.15.0 | Real Postgres, MySQL, and MongoDB in integration tests (requires Docker; `blocking` module feature for sync tests) |
| `cargo-nextest` | 0.9.146 | Optional parallel test runner |
| `cargo-semver-checks` | 0.50.0 | CI semver lint per published crate |

## Architecture

Business rules live exactly once, in pure code, with no I/O:

```text
                    +------------------+
                    |  rolify-core     |
                    |                  |
  value types       |  RoleName        |
  RoleRecord        |  ResourceId      |
  RoleQuery         |  ResourceFilter  |
  ResourceRef       |  Resource        |
                    +--------+---------+
                             |
  pure predicates   +--------v---------+     adapter SPI (soft-sealed,
  (gem match ladder,|  kernel          |     static dispatch, never
  zero I/O)         |  where_          |---- dyn): RoleStore
                    |  strict_engages  |     (find_or_create_by, where_)
                    +--------+---------+
                             |
  consumer logic    +--------v---------+     +-- rolify-diesel --+
  (provided methods)|  RolifyUser      |<----|  rolify-sqlx      |
  on your types     |  (has_role...)   |     |  rolify-seaorm    |
                    +--------+---------+     +-- rolify-mongodb -+
                             |
  config + errors   +--------v---------+
                    |  RolifyConfig    |
                    |  RolifyError     |
                    +------------------+
```

Key decisions:

1. **Kernel purity.** The match ladder lives once in `rolify-core::kernel`.
   Adapters translate queries mechanically; the in-memory reference store
   delegates to the same predicates, so cached-vs-queried consistency holds
   by construction.
2. **Sealed SPI, static dispatch.** `RoleStore` uses associated types
   (`Conn`, `Error`) and is intentionally not `dyn`-compatible. Adapters
   implement it only for their own local store wrapper types, never for
   foreign pool types (orphan-rule conformance).
3. **Single-source sync/async.** Public traits are written in desugared
   `-> impl Future<Output = T> + Send` form; `maybe-async` strips futures
   under the `is_sync` feature. One mode per build graph.
4. **Per-crate MSRV.** Each crate declares the floor its driver requires
   (see [MSRV policy](#msrv-policy)); the toolchain file pins the maximum.
5. **Byte-exact names, stringified ids.** Role names are never normalized;
   resource ids are stored as text so integer and string primary keys
   compare uniformly (the gem's `teams.team_code` string-PK precedent).

## Requirements

- Rust 1.94 or newer for a full-workspace checkout (the highest per-crate
  floor; see [MSRV policy](#msrv-policy)). Managed automatically by
  `rust-toolchain.toml` via rustup.
- Docker Engine 24+ (daemon only) for adapter integration tests from
  Phase 3 on: `testcontainers` starts ephemeral Postgres, MySQL, and
  MongoDB automatically during `cargo test`. No Docker is needed today:
  the current suite runs with zero containers.
- Docker Compose v2 is optional: persistent, inspectable databases for
  interactive work (see [Docker and databases](#docker-and-databases)).
- `cargo-nextest` (optional) for partitioned test runs.

Check your environment:

```sh
rustc --version
cargo --version
docker --version
docker compose version
```

## Quick start

Add the crates you need (versions will follow at first publish; until then
depend by path or git):

```toml
[dependencies]
rolify-core = "0.1"
rolify-diesel = "0.1" # or rolify-sqlx, rolify-seaorm, rolify-mongodb
```

Minimal end-to-end example against the backend-free store (no Docker):

```rust
use rolify_core::config::RolifyConfig;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_test::InMemoryStore;

#[tokio::main]
async fn main() {
    // Gem defaults: non-strict, clean up roles left without members.
    let config = RolifyConfig::builder().build();
    assert!(!config.strict());
    assert!(config.remove_role_if_empty());

    let mut store = InMemoryStore::new();
    let mut conn = ();
    let admin = RoleName::from("admin");
    let forum_id = ResourceId::from(1_i64);

    // Grant roles at each scope (idempotent find-or-create).
    store.find_or_create_by(&mut conn, &admin, ResourceRef::Global).await.unwrap();
    store
        .find_or_create_by(&mut conn, &RoleName::from("moderator"), ResourceRef::Class("Forum"))
        .await
        .unwrap();
    store
        .find_or_create_by(
            &mut conn,
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &forum_id),
        )
        .await
        .unwrap();

    // A global "admin" satisfies an instance-scoped lookup (global override).
    let query =
        RoleQuery::with_role_and_filter(&admin, ResourceFilter::Instance("Forum", &forum_id));
    let rows = store.where_(&mut conn, &query).await.unwrap();
    assert_eq!(rows.len(), 1);

    // Role names are byte-exact: no case folding, no trimming.
    assert!(RoleName::from("Admin") != RoleName::from("admin"));
}
```

The sync build compiles the same traits to plain blocking calls:

```rust
// with `--features rolify-core/is_sync,rolify-test/is_sync`
store.find_or_create_by(&mut conn, &admin, ResourceRef::Global).unwrap();
let rows = store.where_(&mut conn, &query).unwrap();
```

## Local setup

```sh
git clone <this-repo>
cd rolify-rust

# Toolchain resolves itself from rust-toolchain.toml via rustup.
rustc --version

# Build the workspace.
cargo build --workspace

# Run the full test suite (async mode, the default).
cargo test --workspace

# Run the full test suite in sync mode.
cargo test --workspace --features rolify-core/is_sync

# Lints and formatting (must be clean before review).
cargo clippy --workspace --all-targets
cargo fmt --check

# API docs with doctests.
cargo test --doc -p rolify-core
cargo doc --workspace --no-deps --open
```

With `cargo-nextest` installed, `cargo nextest run --workspace` runs the
same suites partitioned per backend.

## Docker and databases

A library ships no service, so there is no application image and no
production container here. No `Dockerfile` is shipped on purpose: a library
produces no runnable artifact (a `release` image would contain only rlibs),
CI duties (fmt, clippy, test matrix) belong in workflow files rather than
in an image every contributor must rebuild, and the pinned toolchain
already comes from `rust-toolchain.toml` via rustup.

Docker plays exactly two roles:

1. **Test databases (required from Phase 3 on).** Adapter integration
   suites use `testcontainers`: `cargo test` starts ephemeral Postgres,
   MySQL, and MongoDB containers by itself and removes them afterwards.
   The only requirement is a running Docker daemon; no Compose, no manual
   setup, no leftover state.
2. **Persistent dev databases (optional, via Compose).** Ephemeral test
   containers vanish after the run, which makes them useless when you need
   to inspect state between runs: hand-writing migrations, running
   `diesel-cli`, verifying `rolify-cli` output, or debugging adapter SQL
   in `psql`/`mongosh`. For that, `docker-compose.yml` provides
   long-lived Postgres, MySQL, and MongoDB with healthchecks and named
   volumes.

```sh
# Optional: start persistent databases for interactive work.
cp .env.example .env   # adjust ports and credentials once
docker compose up -d

# Inspect health and logs.
docker compose ps
docker compose logs -f postgres

# Connectivity checks.
docker exec -it rolify-postgres psql -U rolify -d rolify -c 'select version();'
docker exec -it rolify-mysql mysql -uroot -prolify -e 'select version();'
docker exec -it rolify-mongo mongosh -u rolify -p rolify --eval "db.adminCommand('ping')"

# Full reset (drops volumes).
docker compose down -v
```

| File | Purpose |
|------|---------|
| `docker-compose.yml` | `postgres`, `mysql`, `mongo` services with healthchecks and named volumes; ports and credentials overridable from `.env` |
| `.env.example` | Documents every variable Compose and the adapters honor |

## Databases

For interactive work, Compose exposes the same engines the adapters
target (automated tests instead get ephemeral equivalents from
testcontainers, so the suite never depends on Compose):

| Service    | Image          | Default port | Default credentials / db | Connection string |
|------------|----------------|--------------|--------------------------|-------------------|
| `postgres` | `postgres:17`  | 5432         | `rolify` / `rolify` / `rolify` | `postgres://rolify:rolify@localhost:5432/rolify` |
| `mysql`    | `mysql:8.4`    | 3306         | `root` / `rolify`, db `rolify` | `mysql://root:rolify@localhost:3306/rolify` |
| `mongo`    | `mongo:8.0`    | 27017        | `rolify` / `rolify`, db `rolify` | `mongodb://rolify:rolify@localhost:27017/rolify` |

All values are overridable via `.env` (see `.env.example`):

| Variable | Default | Used by |
|----------|---------|---------|
| `POSTGRES_PORT` | `5432` | Compose `postgres` port mapping |
| `POSTGRES_USER` / `POSTGRES_PASSWORD` / `POSTGRES_DB` | `rolify` | Compose `postgres`, `DATABASE_URL_POSTGRES` |
| `MYSQL_PORT` | `3306` | Compose `mysql` port mapping |
| `MYSQL_ROOT_PASSWORD` / `MYSQL_DATABASE` | `rolify` / `rolify` | Compose `mysql`, `DATABASE_URL_MYSQL` |
| `MONGO_PORT` | `27017` | Compose `mongo` port mapping |
| `MONGO_INITDB_ROOT_USERNAME` / `MONGO_INITDB_ROOT_PASSWORD` | `rolify` | Compose `mongo`, `DATABASE_URL_MONGO` |
| `DATABASE_URL_POSTGRES` / `DATABASE_URL_MYSQL` / `DATABASE_URL_MONGO` | (derived from above) | Adapter examples, migration runs |

Migration notes per backend:

- SQL backends ship the gem's `roles` table shape (`name`,
  `resource_type`, `resource_id`) with composite indexes
  `(resource_type, resource_id)` and `(name)`; every class/global query
  branch uses `IS NULL`, never `= NULL`. Migrations are emitted by
  `rolify-cli` (Phase 6) and applied with each backend's native runner
  (`diesel_migrations`, `sqlx::migrate!`, SeaORM migrator).
- MongoDB stores role documents with the same three fields plus a unique
  compound index on `(name, resource_type, resource_id)`.

## Testing

| Layer | Command | Docker needed |
|-------|---------|---------------|
| Unit + doctests, async (default) | `cargo test --workspace` | No |
| Unit + doctests, sync mode | `cargo test --workspace --features rolify-core/is_sync` | No |
| Per-crate | `cargo test -p rolify-core` | No |
| Adapter integration (testcontainers) | `cargo test -p rolify-diesel --features postgres` (per-backend features as they land) | Yes |
| Partitioned runs | `cargo nextest run --workspace` | Only for adapter suites |

Conventions:

- Until the Phase 3 adapters land, the entire suite runs with zero Docker;
  the `Yes` rows above apply to adapter crates as they land.

- New logic ships with tests; `rolify-core` and `rolify-test` must pass in
  **both** modes.
- Backend behavior is pinned by the ported `shared_examples` suite; every
  adapter runs the identical suite so semantic drift is structurally
  detectable.
- Query-count guards hold on real databases (cached predicates issue zero
  queries); readiness waits are used in test harnesses, never fixed sleeps.

## Configuration

`RolifyConfig` replaces the gem's global class variables with an explicit,
`Send + Sync` builder. Defaults mirror the gem (strict off, empty-role
cleanup on):

```rust
use rolify_core::config::RolifyConfig;

let config = RolifyConfig::builder()
    .strict(true)              // exact-scope matching, no global override
    .remove_role_if_empty(false) // keep role rows with zero members
    .build();
```

- `strict(true)`: class/instance queries match the exact scope only. The
  gate engages solely for class/instance filters, never for global or
  `:any` (verified identical at the gem's three call sites).
- `remove_role_if_empty(true, default)`: deleting the last assignment also
  deletes the role row itself.
- `before_add` / `after_add` / `before_remove` / `after_remove` hooks arrive
  as configurable callbacks whose `Err` vetoes the operation (the gem's
  raising-hook semantics as `Result`).

## Error handling

One introspectable enum per crate, built on `thiserror` 2. Never `anyhow`
in a public API:

```rust
use rolify_core::RolifyError;

match operation() {
    Err(RolifyError::RoleNotFound { name }) => { /* ... */ }
    Err(RolifyError::CallbackVeto { callback, reason }) => { /* ... */ }
    Err(RolifyError::InvalidConfig { reason }) => { /* ... */ }
    result => result,
}
```

Every adapter defines its own backend error enum with
`From<RolifyError>`, so core-level failures flow through adapter errors
uniformly and consumers keep matching on variants.

## Sync and async duality

One source of truth, two compilations, selected by a single feature:

| Mode | Feature | Trait shape | For |
|------|---------|-------------|-----|
| Async (default) | none | `fn f(..) -> impl Future<Output = T> + Send` | SQLx, SeaORM, MongoDB, diesel-async |
| Sync | `rolify-core/is_sync` | `fn f(..) -> T` | Diesel sync, MongoDB `sync` feature |

Rules:

- One mode per build graph: features unify across a single `cargo`
  invocation, so never mix a sync consumer with an async adapter.
- No `block_on` in library code, ever.
- Async-only adapters (`rolify-sqlx`, `rolify-seaorm`) emit `compile_error!`
  on invalid sync/async feature combinations.
- Sync SQL users are served by `rolify-diesel`; SQLx and SeaORM stay
  async-only by design.

## MSRV policy

Each crate declares the floor its driver requires; the workspace
`rust-version` stays at the 1.85 baseline and per-crate floors rise only
where the ecosystem forces it:

| Crate | `rust-version` | Driven by |
|-------|----------------|-----------|
| `rolify-core`, `rolify-cli`, `rolify-test` | 1.85 | Project baseline (`clap` 4.6.7 fits exactly) |
| `rolify-diesel` | 1.86 | `diesel` 2.3.13 |
| `rolify-mongodb` | 1.88 | `mongodb` 3.9.1 |
| `rolify-sqlx`, `rolify-seaorm` | 1.94 | `sqlx` 0.9.0, `sea-orm` 2.0.4 |

`rust-toolchain.toml` pins 1.94 (the workspace-wide maximum) so a fresh
checkout builds every crate. CI checks each crate against its documented
floor (for example `cargo +1.86 check -p rolify-diesel`).

## Roadmap

Delivery is kernel-first: semantics are locked in memory before any SQL
exists, then each backend proves itself against the same suite.

- [x] Workspace scaffold, lint table, per-crate MSRV floors
- [x] Pure kernel (`where_`, `strict_engages`) with `:any` ratified
- [x] `RoleStore` sealed SPI, `RolifyUser` seam, `RolifyConfig`, `RolifyError`
- [x] `InMemoryStore` reference backend (no Docker)
- [ ] **Phase 1**: Core kernel and trait contracts (in progress)
- [ ] **Phase 2**: Full role API surface plus ported `shared_examples` harness, green in memory
- [ ] **Phase 3**: Diesel reference adapter on real Postgres/MySQL (testcontainers); **v1 parity claim completes here**
- [ ] **Phase 4**: SQLx (async-only) plus diesel-async rider
- [ ] **Phase 5**: SeaORM and MongoDB adapters in parallel; four-backend matrix closes
- [ ] **Phase 6**: `rolify-cli` generator (one canonical schema, per-backend emitters)
- [ ] **Phase 7**: Consumer test matchers, docs.rs/semver/MSRV gates, published parity matrix

## Ruby rolify parity

| Ruby (gem) | Rust (this workspace) |
|------------|-----------------------|
| `user.add_role(:admin)` | `add_role(&RoleName::from("admin"))` (provided method, Phase 2) |
| `user.add_role(:moderator, Forum)` | grant with `ResourceRef::Class("Forum")` |
| `user.add_role(:moderator, forum)` | grant with `ResourceRef::Instance("Forum", &id)` |
| `user.has_role?(:admin)` | `has_role(&RoleName::from("admin"))` |
| `user.has_role?(:moderator, forum)` | query with `ResourceFilter::Instance("Forum", &id)` |
| `user.has_strict_role?(:moderator, forum)` | strict-mode query (exact scope) |
| `User.with_role(:admin, forum)` | `with_role` finder (Phase 2; strict-aware except `:any`) |
| `Forum.find_roles` / `applied_roles` | resource-side API (Phase 2) |
| `rolify strict: true` | `RolifyConfig::builder().strict(true)` |
| `remove_role_if_empty` | `RolifyConfig::builder().remove_role_if_empty(..)` |
| `rails g rolify Role User` | `rolify-cli` (Phase 6) |
| `is_admin?` shortcuts | Not ported (no `method_missing`); use `has_role` |

Deliberate divergences are labeled in code and will be published in the
parity matrix (Phase 7): `:any` follows the gem's database path (includes
global rows); STI is modeled as `Resource::descendant_types()`; dynamic
shortcuts are excluded.

## Quality gates

Every change must satisfy all of these before review:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets   # all + pedantic = warn, unsafe_code = warn, zero unsafe expected
cargo test --workspace
cargo test --workspace --features rolify-core/is_sync
cargo test --doc -p rolify-core
```

Crate conventions: business logic in pure core types, orchestration in
consumer traits, adapters only translate; no logic duplicated across two
files; no single-letter variable names (`self`, `i`/`j` loop indices
excepted); code, identifiers, and comments in English; rustdoc example on
every public item.

## Contributing

1. Pick a roadmap phase and keep changes inside `rolify-rust/`.
2. Mirror the gem's semantics; cite the gem file and line in comments when
   porting behavior.
3. Add or extend tests first (both sync and async modes where applicable).
4. Run the full [quality gates](#quality-gates) suite.
5. Suggest a Conventional Commits title in English (commits are made
   manually by the maintainer; automation never commits).

## License

MIT. See [LICENSE](LICENSE) (to be added at first publish; the workspace
`Cargo.toml` already declares `license = "MIT"`).

### Links



###  Build 

https://gilcierweb.com.br
