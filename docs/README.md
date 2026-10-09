# rolify-rust Documentation

Welcome to the documentation for **rolify-rust** — a Rust port of the [rolify](https://github.com/RolifyCommunity/rolify) Ruby gem for role management (RBAC) without authorization enforcement.

## Quick Links

| Document | Description |
|----------|-------------|
| [Getting Started](getting-started.md) | 5-minute quickstart guide |
| [Core Concepts](core-concepts.md) | Deep dive into roles, resources, queries, kernel |
| [Tutorial](tutorial.md) | Complete walkthrough building a forum app |
| [Configuration](configuration.md) | Strict mode, callbacks, table names, sharing config |
| [Migration from Ruby](migration-from-ruby.md) | Porting guide from Ruby rolify |

## Adapter Guides

| Adapter | Database | Mode | Guide |
|---------|----------|------|-------|
| **Diesel** | PostgreSQL, MySQL, SQLite | Sync + Async | [diesel.md](adapters/diesel.md) |
| **SQLx** | PostgreSQL, MySQL, SQLite | Async only | [sqlx.md](adapters/sqlx.md) |
| **SeaORM** | PostgreSQL, MySQL | Async only | [seaorm.md](adapters/seaorm.md) |
| **MongoDB** | MongoDB 4.4+ | Sync + Async | [mongodb.md](adapters/mongodb.md) |

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      rolify-core                            │
│  ┌─────────┐ ┌────────┐ ┌─────────┐ ┌────────┐ ┌────────┐  │
│  │  Role   │ │ Query  │ │Resource │ │ Kernel │ │ Config │  │
│  │  Types  │ │ Types  │ │  Types  │ │ (Pure) │ │ Builder│  │
│  └─────────┘ └────────┘ └─────────┘ └────────┘ └────────┘  │
│         ▲           ▲           ▲           ▲               │
│         └───────────┼───────────┼───────────┘               │
│                     ▼                                       │
│            ┌─────────────────┐                             │
│            │   RolifyUser    │  (Consumer trait)           │
│            │   + Resource    │                             │
│            └─────────────────┘                             │
└─────────────────────────────────────────────────────────────┘
                           ▲
          ┌────────────────┼────────────────┐
          ▼                ▼                ▼
    ┌───────────┐    ┌───────────┐    ┌───────────┐
    │  Diesel   │    │   SQLx    │    │  SeaORM   │
    │  Adapter  │    │  Adapter  │    │  Adapter  │
    └───────────┘    └───────────┘    └───────────┘
          ▲                ▲                ▲
          └────────────────┼────────────────┘
                           ▼
                    ┌───────────┐
                    │  MongoDB  │
                    │  Adapter  │
                    └───────────┘
```

## Key Features

- **Three Scope Levels**: Global, Class/Resource Type, Instance
- **Match Ladder**: Global → Class → Instance (configurable strict mode)
- **Zero-I/O Caching**: `RoleSet` for borrowed-row membership checks
- **Dual Mode**: Sync and async from single source via `maybe-async`
- **No Global State**: Explicit `RolifyConfig` passed everywhere
- **Callbacks with Veto**: `before_add`/`remove` can abort via `Result`
- **Four Adapters**: Diesel, SQLx, SeaORM, MongoDB — same API
- **No Authorization Enforcement**: You decide what roles mean

## Sync vs Async

```toml
# Async (default) - works with sqlx, sea-orm, mongodb, diesel-async
rolify-core = { version = "0.1" }

# Sync - works with diesel (sync), mongodb (sync feature)
rolify-core = { version = "0.1", features = ["is_sync"] }
```

**One mode per build graph** — features unify across the entire dependency graph.

## Community

- **GitHub**: [github.com/rolify-rust/rolify-rust](https://github.com/rolify-rust/rolify-rust)
- **Issues**: Report bugs and request features
- **Discussions**: Ask questions and share patterns

## License

MIT OR Apache-2.0