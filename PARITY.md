# rolify-rust Parity Ledger

The single canonical record of every deliberate divergence between the Ruby
[rolify](https://github.com/RolifyCommunity/rolify) gem (vendored read-only
under `rolify/` at the workspace root) and this Rust port. It ships with the
code under `rolify-rust/` so it is versioned, published on crates.io, and
visible on GitHub and docs.rs. The planning matrix under `.planning/` is
history; this file is the published contract.

## Scope: what parity means here

Parity means behavioral equivalence at the consumer-observable surface:
granting and querying roles across the three scopes (global, class,
instance), with the gem's precedence ladder (global override,
class-covers-instance, `:any` including global rows on the database path,
strict mode disabling overrides). Physical storage formats, framework
plumbing (ActiveRecord relations, Rails generators, RSpec matchers), and
Ruby-only metaprogramming are adapted to idiomatic Rust, and every such
adaptation is labeled below. Anything not listed here is intended to behave
exactly like the gem; if you find an unlisted behavioral difference, it is a
bug, please report it.

## Entry format

Each entry carries:

- **Topic**: the one behavior the entry locks.
- **Gem behavior**: what the gem does, with a `file:line` cite into the
  vendored gem (`rolify/lib/...` or `rolify/spec/...`).
- **rolify-rust behavior**: what this port does instead, with the Rust
  location.
- **Status**: one of `deliberate divergence` (a conscious behavioral or
  physical-format difference), `corner choice` (a semantically sane reading
  of a case the gem leaves unreachable or contradictory), `strengthening`
  (strictly more guarantees than the gem's specs assert), or `exclusion`
  (gem surface intentionally not ported, with the idiomatic replacement).
- **Pinned by**: the test path, suite, or document that locks the entry.
  Absence of API surface is itself the assertion for exclusions, pinned by
  the migration table in `COMING-FROM-RUBY.md`.
- **Review flag**: present only on entries whose behavior lock comes from a
  phase whose container-gated suite was not re-run while writing this
  ledger. A review flag names the owning phase and means: the phase-closure
  sweep must re-check the flagged entry against executed code before any
  publish tag. Unflagged entries are pinned by tests runnable with zero
  Docker (`cargo test -p rolify-test`, `cargo test -p rolify-core`, or the
  crate's unit tests).

## Review-flag legend

`Review flag: Phase NN (reason)` means the entry text below was written from
the owning phase's context decisions plus code inspection, and its full
container-backed proof lives in that phase's suite. The flag is cleared by
re-running that phase's suite green; it never weakens the entry, it only
records which re-verification closes the loop. The string "review flag"
appears here so the ledger is greppable for outstanding flags.

Status vocabulary: `deliberate divergence`, `corner choice`,
`strengthening`, `exclusion`. See the Entry format section above.

---

## Entry 1 - `:any` includes global roles

- **Topic**: scope filter `:any` on `has_role?`-style queries.
- **Gem behavior (two paths, divergent)**:
  - DB path: `lib/rolify/adapters/active_record/role_adapter.rb:106-107` -
    `build_query` short-circuits to `name = ?` when `resource == :any`,
    before any scope condition. Global-role rows DO match.
  - In-memory path: `lib/rolify/role.rb:25-42` (`has_role?`) `new_record?`
    flow requires `r.resource.present?` for `:any`, so global rows do NOT
    match there.
- **rolify-rust behavior**: the kernel follows the DB path -
  `ResourceFilter::Any` matches by name INCLUDING global rows, in both the
  query path (`kernel::where_`) and the cached path (`kernel::find_cached`).
- **Status**: deliberate divergence from the `new_record?` in-memory path;
  identical to the persisted DB path.
- **Pinned by**: `kernel::ladder_tests::sc1_row::case_04_global_seen_by_any`,
  `case_13_any_matches_held_row` et al.

## Entry 2 - strict corner: `strict(Global)` = exactly-global

- **Topic**: strict predicate applied to the global (nil resource) filter.
- **Gem behavior (internally contradictory)**: unreachable through the gem's
  own gate (strict never engages without a resource), and the two strict
  implementations disagree anyway:
  - `lib/rolify/adapters/active_record/role_adapter.rb:11-26`
    (`where_strict`): `else {}` - degrades to name-only across ANY scope.
  - `lib/rolify/adapters/active_record/role_adapter.rb:41-45`
    (`find_cached_strict`): computes `resource_type = "NilClass"` - matches
    nothing.
- **rolify-rust behavior**: `where_strict`/`find_cached_strict` with
  `ResourceFilter::Global` match exactly-global rows (name AND both scope
  columns unset) - the semantically sane reading of "strict global".
- **Status**: corner choice; unreachable in practice because
  `kernel::strict_engages` returns false for `Global`.
- **Pinned by**: `kernel::strict_tests::corner_strict_global_*`.

## Entry 3 - strict corner: `strict(Any)` = name-only

- **Topic**: strict predicate applied to `:any`.
- **Gem behavior**: `where_strict(:any)` would raise (`:any.id` does not
  exist); the gate (`resource != :any` at `lib/rolify/role.rb:26`,
  `lib/rolify/role.rb:48`, `lib/rolify/finders.rb:4`) makes the call
  unreachable.
- **rolify-rust behavior**: `where_strict`/`find_cached_strict` with
  `ResourceFilter::Any` degrade to name-only matching (mirror of the gem's
  `where_strict` else-branch).
- **Status**: corner choice; unreachable in practice for the same gating
  reason as Entry 2.
- **Pinned by**: `kernel::strict_tests::corner_strict_any_*`.

## Entry 4 - callback veto strengthening

- **Topic**: callback hooks around add/remove.
- **Gem behavior**: hooks are wired through the HABTM options
  (`lib/rolify.rb:28`); the gem's specs
  (`spec/rolify/shared_examples/shared_examples_for_callbacks.rb`) assert
  only that hooks FIRE, never that a `before_` hook can abort the
  operation.
- **rolify-rust behavior**: hooks are typed
  `Arc<dyn Fn(&RoleRecord) -> Result<(), RolifyError> + Send + Sync>`; a
  `before_add`/`before_remove` returning `Err` ABORTS the operation before
  any store mutation and the corresponding `after_*` hook is skipped.
- **Status**: strengthening over the gem's specs.
- **Pinned by**: `crates/rolify-core/src/config/callbacks.rs` (ordering
  probes) + `crates/rolify-test/tests/spi_integration.rs` (end-to-end veto).

## Entry 5 - callback receiver is `&RoleRecord`

- **Topic**: what callback hooks observe.
- **Gem behavior**: Rails HABTM callbacks fire with the JOIN-TARGET record
  (the user/instance being granted), via `lib/rolify.rb:28` association
  options.
- **rolify-rust behavior**: hooks receive `&RoleRecord` (the role row being
  linked/unlinked). The holder's identity is available to the operation but
  is not the hook payload.
- **Status**: deliberate divergence (typed port; the role row is the
  meaningful, scope-carrying value).
- **Pinned by**: `crates/rolify-core/src/config/callbacks.rs` receiver
  assertions.

## Entry 6 - sentinel physical storage `''` instead of `NULL`

- **Topic**: physical representation of global/class scope in the `roles`
  table.
- **Gem behavior**: `resource_type` and `resource_id` are `NULL` for global
  and class scopes (ActiveRecord `polymorphic: true` stores `NULL` when no
  resource is associated; see
  `lib/generators/active_record/templates/migration.rb` with
  `t.references :resource, :polymorphic => true`). `add_index` on
  `[name, resource_type, resource_id]` does not deduplicate `NULL` values
  on any engine (PostgreSQL, MySQL, SQLite all treat `NULL != NULL` for
  unique indexes).
- **rolify-rust behavior**: global/class scopes store empty string `''` in
  both columns (the `SCOPE_SENTINEL` constant in
  `crates/rolify-core/src/role.rs`). The
  `UNIQUE (name, resource_type, resource_id)` constraint deduplicates
  identically across all three engines. The sentinel never crosses the SPI
  boundary: `RoleRecord` keeps `Option<String>` semantics; translation is
  confined to the adapter boundary (`sentinel.rs`).
- **Status**: deliberate divergence from the gem's physical format; required
  for portable uniqueness (concurrency test: 2 parallel `add_role("admin")`
  yields exactly 1 row).
- **Pinned by**: `rolify-diesel` migrations
  (`CONSTRAINT roles_triple_unique`), `concurrency.rs` exact-one-row test,
  `kernel::ladder_tests` global/class disjuncts.

## Entry 7 - join table `UNIQUE(user_id, role_id)`

- **Topic**: uniqueness constraint on the join table.
- **Gem behavior**: `add_index :users_roles, [:user_id, :role_id]`
  (non-unique composite index,
  `lib/generators/active_record/templates/migration.rb:16`). Duplicate
  assignments are prevented at the application level via the `include?`
  check before `<<` (`lib/rolify/adapters/active_record/role_adapter.rb:54-56`).
- **rolify-rust behavior**: `CONSTRAINT users_roles_pair_unique
  UNIQUE (user_id, role_id)` on the join table. `add` becomes a pure
  `INSERT` with catch-and-ignore of unique violation: race-safe without a
  SELECT guard.
- **Status**: deliberate micro-divergence; functionally equivalent for
  consumers (idempotent `add_role`), with the database enforcing it
  directly.
- **Pinned by**: `rolify-diesel` migrations
  (`CONSTRAINT users_roles_pair_unique`), `executor.rs` and
  `concurrency.rs` link-count assertions.

## Entry 8 - foreign key `ON DELETE CASCADE` with app-level resource cleanup

- **Topic**: referential integrity and cascade behavior on resource
  deletion.
- **Gem behavior**: the generator emits zero foreign keys
  (`lib/generators/active_record/templates/migration.rb` has no
  `add_foreign_key`). Resource cleanup is handled by
  `has_many :roles, dependent: :destroy` in the consumer model:
  application-level.
- **rolify-rust behavior**:
  `FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE` on the
  join table. `user_id` has no FK (consumer table not owned by us).
  Polymorphic resource cleanup is app-level:
  `DELETE FROM roles WHERE resource_type = ? AND resource_id = ?`; cascade
  sweeps join rows. Recorded as a comment in the migration files.
- **Status**: deliberate micro-divergence (the gem has no FKs). The cascade
  simplifies orphan sweep for role deletion; resource cleanup remains
  app-level per the gem's `dependent: :destroy` pattern.
- **Pinned by**: `rolify-diesel` migrations (FK definition + decision-record
  comment), `resource_spi_pg.rs` scoped-delete tests, `store.rs`
  `remove_roles_for_scope` implementation.

## Entry 9 - zero DSL extensions

- **Topic**: backend-native query builder conveniences.
- **Gem behavior**: ActiveRecord provides `where`, `joins`, `includes` and
  the full query DSL around every rolify call.
- **rolify-rust behavior**: the Diesel adapter exposes only the sealed SPI
  (`RoleStore`/`ResourceStore`) plus constructors (`DieselStore::new`,
  `with_tables`, `for_holder_table`, `register_resource_table`). No lazy
  builders, no backend-native DSL helpers, no `method_missing` shortcuts.
- **Status**: exclusion (not a deferral). Conveniences only post-v1 with
  real demand.
- **Pinned by**: parity ledger entry only (absence of API surface is the
  assertion) + `COMING-FROM-RUBY.md` migration table.

## Entry 10 - migration runner API

- **Topic**: how consumers apply embedded migrations.
- **Gem behavior**: `rails db:migrate` runs all pending migrations from
  `db/migrate/`.
- **rolify-rust behavior**: public
  `pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!(...)` per
  backend. Consumers call `conn.run_pending_migrations(MIGRATIONS)?`
  explicitly; runtime crates never auto-migrate.
- **Status**: deliberate API naming difference; the underlying mechanism
  (`diesel_migrations::MigrationHarness`) is standard.
- **Pinned by**: `rolify-diesel/src/lib.rs` `MIGRATIONS` const + Usage doc,
  `migrations.rs` apply/revert roundtrip tests.

## Entry 11 - `remove_roles_for_scope` SPI addition

- **Topic**: bulk role deletion by resource scope (resource destruction
  path).
- **Gem behavior**: performed via `has_many :roles, dependent: :destroy` on
  the consumer model (`lib/rolify.rb:46-55` `resourcify`): when the resource
  is destroyed, Rails destroys associated role rows through the join.
- **rolify-rust behavior**: explicit SPI method
  `RoleStore::remove_roles_for_scope(&mut conn, resource_type, resource_id)`
  deletes exactly the role rows matching the given scope pair. Join rows
  vanish via FK `ON DELETE CASCADE` (Entry 8). Class-scoped rows of the
  same type are NOT touched: the WHERE is the exact scope pair, never a
  type-only sweep.
- **Status**: deliberate divergence: the gem hides this in
  `dependent: :destroy`; the port exposes it as an explicit SPI member for
  clarity and testability.
- **Pinned by**: `rolify-core/src/store.rs` SPI definition,
  `rolify-diesel/src/store.rs` implementation, `resource_spi_pg.rs`
  scoped-delete tests.

## Entry 12 - additive engine features in `rolify-sqlx`

- **Topic**: engine feature model across the two SQL adapters.
- **Gem behavior**: n/a (single-ORM gem; the comparison is intra-workspace).
  `rolify-diesel` mirrors its own driver's mutual exclusion:
  `postgres`/`mysql`/`sqlite` are pairwise exclusive (compile_error guards).
- **rolify-rust behavior**: `rolify-sqlx`'s `postgres`/`mysql`/`sqlite`
  features are additive: one build may enable any combination, including
  all three (`SqlxStore<DB>` is generic over the engine), mirroring sqlx's
  own feature model and diverging from the diesel adapter's exclusivity.
- **Status**: deliberate divergence from `rolify-diesel`'s engine model
  (each adapter mirrors its own driver's philosophy).
- **Pinned by**: `rolify-sqlx/Cargo.toml` feature table (no exclusion
  guards), `crates/rolify-sqlx/src/lib.rs` feature-matrix docs, the
  all-engines CI clippy leg.

## Entry 13 - count-by-selection in generic sqlx remove paths

- **Topic**: mutation counts returned by `remove` and
  `remove_roles_for_scope`.
- **Gem behavior**: `lib/rolify/adapters/active_record/role_adapter.rb:58-70`
  uses ActiveRecord destroy/delete return values: the removed-records count
  is inherent to the ORM API.
- **rolify-rust behavior**: generic sqlx code cannot call `rows_affected()`
  (the method is inherent to each concrete `QueryResult`, unreachable on
  generic `DB::QueryResult`), so the sqlx adapter derives removal counts by
  selection: the remove choreography SELECTs the affected rows first and the
  selected length IS the removed-link count (`UNIQUE(user_id, role_id)`,
  Entry 7, makes this exact); `remove_roles_for_scope` SELECTs ids by scope
  then DELETEs by those ids.
- **Status**: deliberate design forced by the generic-store decision; affects
  statement-count expectations of query guards (two statements where a
  driver-specific adapter might use one), not any semantic.
- **Pinned by**: `rolify-sqlx/src/store.rs` remove choreography,
  `rolify-test` suite `query_guards` module (two-statement expectations pass
  on all three sqlx engines).

## Entry 14 - per-crate MSRV floors

- **Topic**: minimum supported Rust versions for the adapters.
- **Gem behavior**: n/a (Ruby gem; no Rust toolchain contract).
- **rolify-rust behavior**: `rolify-diesel` keeps `rust-version = "1.86"` in
  BOTH modes (diesel-async 0.9 declares 1.84 but is effectively 1.86 via
  diesel ~2.3.9); `rolify-sqlx` pins `rust-version = "1.94"`.
- **Status**: deliberate ratification superseding an early roadmap one-floor
  phrasing (corrected on the record; author authority over roadmap text).
- **Pinned by**: `rolify-diesel/Cargo.toml` + `rolify-sqlx/Cargo.toml`
  `rust-version` fields; CI msrv job legs.

## Entry 15 - sentinel `= ''` branches, not `IS NULL`

- **Topic**: SQL shape of the scope branches in the async adapters.
- **Gem behavior**: `lib/rolify/adapters/active_record/role_adapter.rb:106-121`
  builds ActiveRecord conditions that render `resource_type IS NULL` /
  `resource_id IS NULL` for global/class scopes (SQL NULL semantics).
- **rolify-rust behavior**: all SQL adapters inherit the sentinel storage
  verbatim: every scope branch compares `= ''` (the `SCOPE_SENTINEL`), never
  `IS NULL` and never `= NULL`, because NULL values do not deduplicate in
  unique indexes on any engine (Entry 6). Vendored migration trees are
  byte-identical to the canonical diesel trees (drift-guard enforced).
- **Status**: deliberate divergence from the gem's physical NULL storage,
  inherited unchanged wherever SQL is emitted.
- **Pinned by**: `rolify-sqlx/tests/drift_guard.rs` (byte-identity), the
  `rolify-sqlx/src/sentinel.rs` / `rolify-diesel/src/sentinel.rs` boundary
  translation, parity-suite scope-matching cases on all adapter-engine legs.

## Entry 16 - dropped alias surface

- **Topic**: Ruby convenience aliases beyond `grant`/`revoke`.
- **Gem behavior**: `lib/rolify/resource.rb:23-34` defines `with_roles`,
  `find_as`, `find_multiple_as` (aliases of `with_role`) and
  `without_roles`, `except_as`, `except_multiple_as` (aliases of
  `without_role`); `lib/rolify/role.rb:86` keeps deprecated `has_no_role`
  (alias of `remove_role`); `lib/rolify/finders.rb:4-30` is the canonical
  finder surface the user-side aliases mirror.
- **rolify-rust behavior**: one concept, one name. Only `grant` (for
  `add_role`) and `revoke` (for `remove_role`) are kept, per
  `lib/rolify/role.rb:23` (`alias_method :grant, :add_role`) and
  `lib/rolify/role.rb:85` (`alias_method :revoke, :remove_role`).
  Everything else in the paragraph above has no Rust counterpart; the
  migration table maps each dropped alias to its canonical call.
- **Status**: exclusion (Rust minimalism, documented with a migration
  table).
- **Pinned by**: `REQUIREMENTS.md` Out of Scope table (alias list) +
  `COMING-FROM-RUBY.md` alias migration table (absence of API surface is
  the assertion).

## Entry 17 - dynamic shortcuts (`is_admin?` family) excluded

- **Topic**: `method_missing`-driven role predicates.
- **Gem behavior**: `lib/rolify/role.rb:92-112` (`method_missing` plus
  `respond_to?`) answers `user.is_admin?` and `user.is_moderator_of?(forum)`
  by defining the method on first call (`lib/rolify/dynamic.rb`,
  `define_dynamic_method`), consulting the database to answer
  `respond_to?`. The ported-suite file
  `spec/rolify/shared_examples/shared_examples_for_dynamic.rb` (147 lines)
  covers it.
- **rolify-rust behavior**: not ported. Rust has no `method_missing`; the
  idiomatic equivalent is `has_role(&RoleName::from("admin"))`, and in
  tests `assert_has_role(&RoleName::from("admin"), ResourceFilter::Global,
  "...")`. The `shared_examples_for_dynamic.rb` file is the one suite file
  excluded from the port.
- **Status**: exclusion (language-level; documented as the promoted
  alternative, not a gap).
- **Pinned by**: ported-suite file list (`dynamic` absent by decision) +
  `COMING-FROM-RUBY.md` dynamic-shortcuts migration row.

## Entry 18 - STI via `descendant_types()`

- **Topic**: single-table-inheritance matching (a role on a parent class
  covers subclass instances).
- **Gem behavior**: `lib/rolify/adapters/base.rb:27-28`
  (`relation_types_for`) expands a relation to
  `relation.descendants.map(&:to_s).push(relation.to_s)`; every adapter
  query filters `resource_type IN (...)` over that list (e.g.
  `lib/rolify/adapters/active_record/resource_adapter.rb:13-25`).
- **rolify-rust behavior**: the `Resource` trait carries
  `fn descendant_types() -> Vec<&'static str>` (default: just the type
  itself); `find_roles`, `with_role`, and `applied_roles(children = true)`
  expand over it. Class hierarchies are declared explicitly instead of
  discovered through ActiveRecord reflection.
- **Status**: deliberate divergence (explicit associated function instead
  of runtime reflection; same matching semantics).
- **Pinned by**: `crates/rolify-core/src/resource.rs` doctest (Vehicle/Car
  STI example) + ported-suite STI fixtures (Organization/Company pair).

## Entry 19 - global config registry removed

- **Topic**: mutable global state (`Rolify.resource_types`, ORM choice,
  dynamic-shortcut flag).
- **Gem behavior**: `lib/rolify.rb:12` (`@@resource_types = []`) plus
  `lib/rolify.rb:72-74` (`self.resource_types`) accumulate every resourcified
  class in a process-global list; `lib/rolify/configure.rb` holds
  `@@dynamic_shortcuts`, `@@orm`, `@@remove_role_if_empty` as global class
  variables mutated by `Rolify.configure`.
- **rolify-rust behavior**: no global mutable state. Configuration travels
  in an explicit `Send + Sync` `RolifyConfig` builder value owned by the
  `Rolify<S>` handle; resource-type knowledge travels in the `Resource`
  trait's associated items (`type_name`, `descendant_types`). Untestable
  globals become injectable values.
- **Status**: exclusion of the global-mutable pattern (correctness and
  testability requirement, not a feature gap).
- **Pinned by**: `crates/rolify-core/src/config.rs` builder API (no
  `static mut`, no `OnceLock` globals in the public surface).

## Entry 20 - finders return id lists

- **Topic**: what user-class finders hand back.
- **Gem behavior**: `lib/rolify/finders.rb:4-8` (`with_role`) returns a live
  ActiveRecord relation through
  `lib/rolify/adapters/active_record/role_adapter.rb:76-80` (`scope` over
  `relation.joins(:roles)`); callers chain further query methods on it.
- **rolify-rust behavior**: finders (`User::with_role`,
  `User::with_all_roles`, `User::with_any_roles`, `User::without_role`) run
  one store round-trip per query (via `holders_where`) and return
  `Vec<ResourceId>`; the consumer filters its own table with `IN`. The
  vector is treated as a set: no order is guaranteed.
- **Status**: deliberate divergence (no lazy relations in Rust; preserves
  the gem's one-query shape and lets the consumer own its table mapping).
- **Pinned by**: ported-suite finder cases (compared as sets) + `store.rs`
  `holders_where`/`all_holders` SPI docs.

## Entry 21 - resource `without_role` takes a caller-supplied universe

- **Topic**: what "all resources except these" subtracts from.
- **Gem behavior**:
  `lib/rolify/adapters/active_record/resource_adapter.rb:40-43`
  (`all_except`) subtracts from `Forum.all` for free, because ActiveRecord
  owns the consumer's table mapping.
- **rolify-rust behavior**: `Forum::without_role(&mut rolify, name, user,
  candidates: &[ResourceKey])`: the caller supplies the universe of
  resources and core subtracts the with-role set (via `resources_find` /
  `in_list`). The store never owns the consumer's domain tables, so a
  type-to-table binding for every resource would be configuration without
  end.
- **Status**: deliberate divergence (adapter cannot see consumer tables;
  same subtraction semantics over an explicit universe).
- **Pinned by**: `crates/rolify-core/src/resource.rs` `without_role`
  signature + ported-suite resource finder cases.

## Entry 22 - scopes are snapshot-side filters

- **Topic**: where `global` / `class_scoped` / `instance_scoped` filtering
  happens.
- **Gem behavior**: scopes filter server-side (e.g.
  `lib/rolify/adapters/active_record/resource_adapter.rb:32-38`
  `applied_roles` issues `WHERE resource_type ... AND resource_id IS NULL`);
  the scope specs run over the holder's relation (`subject.roles`).
- **rolify-rust behavior**: scopes are pure filters over the snapshot:
  `RoleSet::global()`, `RoleSet::class_scoped(..)`,
  `RoleSet::instance_scoped(..)` narrow `roles_of` results in core, with
  zero new SPI members. Same result sets, fewer round-trips, no per-adapter
  scope SQL to drift.
- **Status**: deliberate divergence in query shape; parity of result.
- **Pinned by**: ported-suite scope cases over `roles_of` snapshots +
  `RoleSet` narrow-accessor unit tests.

## Entry 23 - `find_roles` with user composes over `roles_of`

- **Topic**: how the user-filtered leg of `find_roles` reads.
- **Gem behavior**:
  `lib/rolify/adapters/active_record/resource_adapter.rb:6-11`
  (`find_roles`) branches inside the adapter: with a user it scopes
  `user.roles.where(...)` server-side.
- **rolify-rust behavior**: `find_roles(name?, user)` with a user reads
  `roles_of(holder)` through the already-existing SPI member and filters in
  kernel-pure code. The `:any` runtime sentinels for name and user become
  `Option` (`None` = `:any`) at compile time. No new SPI member was added
  for this leg.
- **Status**: deliberate divergence (composition over existing SPI;
  maximum DRY, one adapter method fewer per backend).
- **Pinned by**: ported-suite `find_roles` matrix (`name: Option`,
  `user: Option`) + `catalog.rs` composition implementation.

## Entry 24 - MongoDB uses explicit null, SQL uses the sentinel

- **Topic**: physical representation of absent scope in MongoDB documents.
- **Gem behavior**: the Mongoid adapter stores `nil` for absent scope
  (`lib/rolify/adapters/mongoid/role_adapter.rb:50-54`
  `find_or_create_by` passes `nil` through; the generator template emits a
  polymorphic `belongs_to :resource` with a nilable reference), and Mongo
  `{ field: null }` queries plus unique indexes treat null consistently.
- **rolify-rust behavior**: the Mongo adapter writes explicit BSON null for
  absent scope (`scope_field`: `None` becomes `Bson::Null`), mirroring the
  Mongoid shape; the SQL adapters store `''` (Entry 6). The sentinel was a
  SQL-only motivation (NULLs do not deduplicate in unique indexes); it is
  not applied where it buys nothing.
- **Status**: deliberate divergence between backends (each physical format
  follows its own engine's consistency rule); the `RoleRecord` `Option`
  semantics are identical above the adapter boundary.
- **Pinned by**: `crates/rolify-mongodb/src/document.rs` (`scope_field`
  plus `scope_option` round-trip, `stringified_integer_and_uuid_...`
  test).
- **Review flag** (open review flag, Phase 05.1): Mongo physical-format lock; re-check against
  the container-backed Mongo suite before any publish tag).

## Entry 25 - SeaORM migrator is native, not vendored SQL

- **Topic**: migration format for the SeaORM adapter.
- **Gem behavior**: n/a (single-ORM gem). The intra-workspace precedent is
  vendored `.sql` copies plus a byte-identity drift guard shared by the
  diesel, sqlx, and CLI trees.
- **rolify-rust behavior**: the SeaORM adapter ships a native
  `sea-orm-migration` migrator instead of vendored `.sql` plus a drift
  guard. The physical schema is identical to the canonical one (VARCHAR
  widths, `''` sentinel, compound unique, composite indexes, FK posture);
  only the runner format is idiomatic to the ecosystem. Consequence: the
  CLI carries its own SeaORM emitter rather than copying `.sql`.
- **Status**: deliberate divergence from the vendored-SQL precedent (user
  decision for the ecosystem-idiomatic migrator).
- **Pinned by**: `crates/rolify-seaorm/src/migration/` (native migrator,
  same physical schema) + CLI SeaORM emitter snapshot tests.
- **Review flag** (open review flag, Phase 05): SeaORM migrator lock; re-check schema identity
  against the canonical trees before any publish tag).

## Entry 26 - MongoDB `sync` feature mirrors the driver

- **Topic**: blocking API for the MongoDB adapter.
- **Gem behavior**: n/a (Ruby is sync throughout; there is no duality to
  mirror).
- **rolify-rust behavior**: `rolify-mongodb` ships an opt-in `sync` feature
  (`sync = ["is_sync", "mongodb/sync"]`) over the same store type via
  `maybe-async`, mirroring the official driver's own `mongodb::sync`
  duality and the diesel-async rider pattern. Default stays async; the full
  suite runs in both modes.
- **Status**: deliberate divergence (no gem counterpart; driver-mirroring
  addition, additive and opt-in).
- **Pinned by**: `crates/rolify-mongodb/Cargo.toml` feature table +
  dual-mode (`cargo test` default plus `--features .../is_sync`) suite runs.
- **Review flag** (open review flag, Phase 05.1): sync-feature lock; re-check both-mode suite
  runs before any publish tag).

## Entry 27 - CLI `generate` verb with four emitters over one canonical schema

- **Topic**: scaffolding command shape and output matrix.
- **Gem behavior**: `lib/generators/rolify/rolify_generator.rb` (entry
  generator: `Role User` args, `hook_for :orm`, initializer plus README
  emission) with the config template at
  `lib/generators/rolify/templates/initializer.rb` and the index contract at
  `lib/generators/active_record/templates/migration.rb`; Mongoid shape at
  `lib/generators/mongoid/rolify_generator.rb`.
- **rolify-rust behavior**: `rolify-cli` exposes `generate` as the primary
  subcommand with `init` kept as a hidden alias; `--backend` takes crate
  names (`diesel | sqlx | seaorm | mongodb`) and is required; positional
  `Role User` args keep rails-g parity. Four emitters (diesel SQL, sqlx
  SQL, `sea-orm-migration` Rust, Mongo docs) render ONE canonical schema:
  diesel and sqlx outputs are byte-identical files (drift-guard enforced),
  up plus down migrations, mirrored trees per engine. `include_str!` plus
  string replacement only; no template engine; the CLI never touches a live
  database.
- **Status**: deliberate divergence (verb renamed with alias; emitter
  matrix replaces per-ORM Rails generators; same schema and index contract
  underneath).
- **Pinned by**: `crates/rolify-cli/src/args.rs` (`Generate` plus
  `alias = "init"`) + emitter snapshot tests + drift-guard dev-test +
  `assert_cmd` end-to-end runs on real containers.
- **Review flag** (open review flag, Phase 06): CLI output-shape lock; re-check emitter
  snapshots plus the container end-to-end gate before any publish tag).

## Entry 28 - holder ids stored in canonical string form on the join side

- **Topic**: physical type of `users_roles.user_id` (SQL) and the elements
  of `user_ids` (MongoDB).
- **Gem behavior**: the ActiveRecord generator emits `t.references :user`,
  a bigint FK column typed after the holder table's PK
  (`lib/generators/active_record/templates/migration.rb:11`), while the
  Mongoid template links string `ObjectId`s via HABTM
  (`lib/generators/rolify/templates/role-mongoid.rb:5`). The gem's own
  specs exercise a non-bigint holder PK: the `Team` fixture declares
  `self.primary_key = "team_code"`
  (`rolify/spec/support/adapters/active_record.rb:81`) and is seeded with
  string ids (`rolify/spec/support/data.rb:24-25`).
- **rolify-rust behavior**: one canonical schema for every backend. SQL
  stores the holder PK in canonical string form in
  `user_id VARCHAR(191)`; MongoDB embeds the same strings in the role
  document's `user_ids` array (`crates/rolify-mongodb/src/document.rs`,
  D-08). Holder identity is string-native in core (`ResourceId`, CORE-01),
  so integer, UUID, and string holder PKs share one schema with no
  per-holder-type migration variants. `role_id` remains a proper integer
  FK to `roles(id)`; the join table carries no surrogate key and no
  `user_type` column.
- **Status**: deliberate divergence (physical format only; integer ids are
  stored as their decimal string form, preserving gem value-level
  behavior).
- **Pinned by**: `crates/rolify-diesel/migrations/*/0000000001_rolify_create_tables/up.sql`
  (`user_id VARCHAR(191)`) byte-locked against the CLI emitters by the
  drift-guard dev-test (`crates/rolify-cli/tests/drift.rs`), the SeaORM
  join entity (`crates/rolify-seaorm/src/entity/join.rs`), and the
  string-id inserts (`'u1'`) in `crates/rolify-diesel/tests/migrations.rs`.

---

## Positioning: complementary, never competing

`rolify-rust` is storage-only role bookkeeping: it answers "does this holder
carry this role at this scope" and nothing else. It performs no
authorization enforcement, ships no policy language, and assigns no
permissions to roles. That boundary is the same one the Ruby gem keeps with
CanCanCan, Pundit, and Authority, and it is what makes this crate compose
with, rather than compete against, enforcement tools: query `has_role`
(resp. `assert_has_role` in tests) at your policy seam and let the
enforcement layer own the decision. SeaORM's built-in `rbac` module is the
same story from the other side: it is enforcement-oriented (one role per
user, table-scoped CRUD permissions, query-time auditing), while rolify is
multi-role, instance-scoped, and enforcement-free. The two solve adjacent
problems; a team can store rolify roles and feed them into an enforcement
engine without either crate reimplementing the other.

### When to choose what

| Tool | Choose it when | rolify-rust's place in that stack |
|------|----------------|-----------------------------------|
| `rolify-rust` | You need scoped role storage and queries (global, class, instance) with gem-compatible semantics | The role layer itself |
| `casbin` / `casbin-rs` | You need policy enforcement (ALLOW/DENY over role hierarchies, ABAC rules, effect combinators) | Query `has_role` at the enforcement seam; rolify stores, casbin decides |
| `cerbos` | You want enforcement as a sidecar service with versioned policies outside the binary | Same seam as casbin: rolify answers membership, the sidecar answers access |
| SeaORM 2.0 `rbac` feature | You want table-scoped CRUD permissions with auditing inside SeaORM | Complementary: keep rolify for multi-role instance-scoped storage; do not rebuild enforcement here (bridging the two is post-v1 work) |

---

*Ledger maintained with the code. Last review pass: Phase 07 (test matchers
and publish hardening). Entries 1-15 carried from the planning parity
matrix with wording tightened for a public audience and identical technical
claims; entries 16-27 mined from phase contexts 01-06 plus shipped code.*
