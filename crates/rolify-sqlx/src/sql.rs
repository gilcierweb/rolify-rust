//! SQL template builders: one function per SPI member, mirroring
//! `role_adapter.rb:106-121` (`build_query`) and the write choreography
//! 1:1 from the diesel reference adapter (`rolify-diesel/src/sql.rs`,
//! Phase 3 D-06). Every value travels as an ordered runtime bind; only
//! validated + per-engine-quoted table names interpolate through
//! `format!`.
//!
//! Conventions:
//! - Table names arrive from `SqlxStore` already validated (D-08
//!   allow-list) and per-engine quoted (`dialect::quote_identifier`);
//!   never interpolated raw.
//! - Placeholders emit through `dialect::placeholder::<DB>` and switch on
//!   `DB::NAME` at runtime (engines are additive, D-13): `$N` for
//!   PostgreSQL, `?` for MySQL/SQLite.
//! - ONE BIND PER OCCURRENCE. Postgres can reuse `$N` across occurrences
//!   but MySQL/SQLite `?` marks cannot share a bind, so every placeholder
//!   occurrence in a template consumes exactly one entry of the ordered
//!   bind list on every engine. This diverges from the diesel reference,
//!   whose reused name placeholder under-binds the MySQL/SQLite `?` forms
//!   of the class/instance ladders (a latent bug this port does not
//!   carry); the per-occurrence table is pinned by the unit tests below.
//! - Projection lists are explicit (`role_row.name AS name, ...`), never
//!   `SELECT *` (joined name clashes, the diesel Pitfall 9).
//! - Sentinel `''` for global/class scope (Phase 3 D-01/D-02): every
//!   scope branch compares against `= ''`, never `= NULL` and never a
//!   bound `Option::None`.
//! - Each function cites the gem `file:line` range it mirrors.
//!
//! Inventory deltas vs the diesel reference (dead templates are not
//! ported; unused `pub(crate)` items fail the clippy gate):
//! - `select_role_exists_by_triple` (diesel carries it unused): the
//!   `exists` SPI member reads a scope column over the join instead
//!   (`select_scoped_exists`).
//! - `delete_roles_by_scope` (rows_affected-derived count): replaced by
//!   `select_role_ids_by_scope` + `delete_roles_by_ids` so the count
//!   derives from selection (Pitfall 5) and equals the deleted set inside
//!   one transaction.
//! - `select_instance_resource_keys` + `select_resources_find_class_expansion`
//!   (diesel l.1416-1425 inline block and l.465-501): the `resources_find`
//!   arms - a direct roles-table scan for instance-scoped rows and the
//!   registry-driven class expansion joining the resource table.
//! - `select_holder_scope_rows` (diesel's `in_list` inline query): the
//!   holder's rows narrowed by name; coverage is decided caller-side.
//! - `select_roles_of` and `select_scoped_exists` are hoisted from the
//!   diesel store's inline `format!` blocks into named templates so this
//!   module owns the full statement surface (the 04-06 query counter
//!   depends on every statement being built here).

use rolify_core::catalog::CatalogScope;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::store::ScopeColumn;
use sqlx::database::Database;

use crate::dialect::{cast_to_text, placeholder};

/// The explicit role-row projection shared by every read template.
const ROLE_PROJECTION: &str = "role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id";

/// Build the non-strict ladder WHERE fragment for a single `RoleQuery`,
/// starting placeholders at `start_index`.
///
/// Mirrors `build_query` (`role_adapter.rb:106-121`) and the disjunct
/// table in `rolify-core/src/kernel.rs`: global rows override class
/// queries, class rows cover instance queries, and `Any` short-circuits
/// to name-only (the gem's database path, `build_query:107`).
///
/// Bind counts per filter (per-occurrence discipline):
/// - Global: 3 (name, sentinel type, sentinel id)
/// - Class: 6 (name, `''`, `''`, name, type, `''`)
/// - Instance: 9 (name, `''`, `''`, name, type, `''`, name, type, id)
/// - Any: 1 (name)
///
/// Returns the fragment plus the next free placeholder index.
#[must_use]
pub(crate) fn build_ladder_where<DB: Database>(
    query: &RoleQuery<'_>,
    start_index: usize,
) -> (String, usize) {
    // gem: `:any` short-circuits to name-only (build_query:107, D-02).
    if matches!(query.filter, ResourceFilter::Any) {
        return (
            format!("(role_row.name = {})", placeholder::<DB>(start_index)),
            start_index + 1,
        );
    }

    // Global disjunct (always present for non-Any): name + both sentinels
    // (gem: `resource == nil`, build_query:108-109).
    let global_disjunct = format!(
        "(role_row.name = {name_ph} AND role_row.resource_type = {type_ph} AND role_row.resource_id = {id_ph})",
        name_ph = placeholder::<DB>(start_index),
        type_ph = placeholder::<DB>(start_index + 1),
        id_ph = placeholder::<DB>(start_index + 2),
    );

    match &query.filter {
        ResourceFilter::Global => (format!("({global_disjunct})"), start_index + 3),
        // gem: global OR (type AND sentinel id) (build_query:112-113)
        ResourceFilter::Class(_type_name) => {
            let class_disjunct = ladder_scope_disjunct::<DB>(start_index + 3);
            (
                format!("({global_disjunct} OR {class_disjunct})"),
                start_index + 6,
            )
        }
        // gem: global OR class OR (type AND id) (build_query:114-117)
        ResourceFilter::Instance(_type_name, _resource_id) => {
            let class_disjunct = ladder_scope_disjunct::<DB>(start_index + 3);
            let instance_disjunct = ladder_scope_disjunct::<DB>(start_index + 6);
            (
                format!("({global_disjunct} OR {class_disjunct} OR {instance_disjunct})"),
                start_index + 9,
            )
        }
        ResourceFilter::Any => unreachable!("Any short-circuits above; kept for exhaustiveness"),
    }
}

/// One scope disjunct at `start_index`: name again (per-occurrence
/// bind), the type, and the concrete or sentinel id. The VALUES never
/// appear here: they travel as the store's ordered binds.
fn ladder_scope_disjunct<DB: Database>(start_index: usize) -> String {
    format!(
        "(role_row.name = {name_ph} AND role_row.resource_type = {type_ph} AND role_row.resource_id = {id_ph})",
        name_ph = placeholder::<DB>(start_index),
        type_ph = placeholder::<DB>(start_index + 1),
        id_ph = placeholder::<DB>(start_index + 2),
    )
}

/// Build the strict WHERE fragment for a single `RoleQuery` (exact scope,
/// no override ladder), mirroring `where_strict`
/// (`role_adapter.rb:11-26`).
///
/// Bind counts: 3 for Global/Class/Instance (name, type, id with the
/// sentinel substituted for absent scopes), 1 for `Any` (the ratified
/// kernel corner: name-only).
#[must_use]
pub(crate) fn build_strict_where<DB: Database>(
    query: &RoleQuery<'_>,
    start_index: usize,
) -> (String, usize) {
    // Any short-circuits to name-only (kernel ratified corner).
    if matches!(query.filter, ResourceFilter::Any) {
        return (
            format!("(role_row.name = {})", placeholder::<DB>(start_index)),
            start_index + 1,
        );
    }

    // The scope values themselves travel as binds (the sentinel for absent
    // scope columns); the template only fixes the placeholder count.
    (
        format!(
            "(role_row.name = {name_ph} AND role_row.resource_type = {type_ph} AND role_row.resource_id = {id_ph})",
            name_ph = placeholder::<DB>(start_index),
            type_ph = placeholder::<DB>(start_index + 1),
            id_ph = placeholder::<DB>(start_index + 2),
        ),
        start_index + 3,
    )
}

/// Build OR-joined ladders for `where_any`: one ladder per query, joined
/// with ` OR `, mirroring `build_conditions` (`role_adapter.rb:88-104`,
/// the gem's `join(' OR ')`). Each sub-ladder continues the shared
/// placeholder sequence.
#[must_use]
pub(crate) fn build_any_where<DB: Database>(
    queries: &[RoleQuery<'_>],
    start_index: usize,
) -> (String, usize) {
    let mut fragments: Vec<String> = Vec::with_capacity(queries.len());
    let mut next_index = start_index;
    for query in queries {
        let (fragment, advanced) = build_ladder_where::<DB>(query, next_index);
        fragments.push(fragment);
        next_index = advanced;
    }
    (fragments.join(" OR "), next_index)
}

/// SELECT role rows joined to one holder, filtered by `where_clause`
/// (a ladder or strict fragment; `select_roles_for_holder` in the diesel
/// inventory).
///
/// Bind order: 1=holder id, then the fragment's binds in order.
#[must_use]
pub(crate) fn select_roles_for_holder<DB: Database>(
    role_table: &str,
    join_table: &str,
    where_clause: &str,
) -> String {
    format!(
        "SELECT {ROLE_PROJECTION} \
         FROM {role_table} AS role_row \
         INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
         WHERE link.user_id = {holder_ph} AND {where_clause}",
        holder_ph = placeholder::<DB>(1),
    )
}

/// SELECT role rows for the holder's whole association (the `user.roles`
/// read; inline in the diesel store, hoisted here as a named template).
///
/// Bind order: 1=holder id.
#[must_use]
pub(crate) fn select_roles_of<DB: Database>(role_table: &str, join_table: &str) -> String {
    format!(
        "SELECT {ROLE_PROJECTION} \
         FROM {role_table} AS role_row \
         INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
         WHERE link.user_id = {holder_ph}",
        holder_ph = placeholder::<DB>(1),
    )
}

/// SELECT a role row by the exact `(name, resource_type, resource_id)`
/// triple: the `find_or_create_by` SELECT-first leg and the re-SELECT
/// after INSERT. Returns at most one row (the UNIQUE triple).
///
/// Bind order: 1=name, 2=resource_type, 3=resource_id.
#[must_use]
pub(crate) fn select_role_by_triple<DB: Database>(role_table: &str) -> String {
    format!(
        "SELECT name, resource_type, resource_id \
         FROM {role_table} \
         WHERE name = {name_ph} AND resource_type = {type_ph} AND resource_id = {id_ph}",
        name_ph = placeholder::<DB>(1),
        type_ph = placeholder::<DB>(2),
        id_ph = placeholder::<DB>(3),
    )
}

/// INSERT a new role row (name, resource_type, resource_id). The caller
/// re-SELECTs by triple afterwards to load the generated id (portable:
/// MySQL has no RETURNING).
///
/// Bind order: 1=name, 2=resource_type, 3=resource_id.
#[must_use]
pub(crate) fn insert_role<DB: Database>(role_table: &str) -> String {
    format!(
        "INSERT INTO {role_table} (name, resource_type, resource_id) \
         VALUES ({name_ph}, {type_ph}, {id_ph})",
        name_ph = placeholder::<DB>(1),
        type_ph = placeholder::<DB>(2),
        id_ph = placeholder::<DB>(3),
    )
}

/// SELECT a role row id by triple (link building and the orphan sweep).
///
/// Bind order: 1=name, 2=resource_type, 3=resource_id.
#[must_use]
pub(crate) fn select_role_id_by_triple<DB: Database>(role_table: &str) -> String {
    format!(
        "SELECT id FROM {role_table} \
         WHERE name = {name_ph} AND resource_type = {type_ph} AND resource_id = {id_ph}",
        name_ph = placeholder::<DB>(1),
        type_ph = placeholder::<DB>(2),
        id_ph = placeholder::<DB>(3),
    )
}

/// INSERT a link (holder -> role) into the join table
/// (`relation.roles << role`, `role_adapter.rb:52-54`).
///
/// Bind order: 1=user_id (text), 2=role_id (i64).
#[must_use]
pub(crate) fn insert_link<DB: Database>(join_table: &str) -> String {
    format!(
        "INSERT INTO {join_table} (user_id, role_id) VALUES ({user_ph}, {role_ph})",
        user_ph = placeholder::<DB>(1),
        role_ph = placeholder::<DB>(2),
    )
}

/// DELETE the holder's links whose role rows fall under the removal
/// `target`, mirroring `remove` (`role_adapter.rb:58-70`) and
/// `kernel::removal_match`:
/// - `NameOnly`: every scope with this name
/// - `TypeSweep(type)`: this name AND `resource_type = type` (class and
///   instance rows of the type alike, the subtle sweep)
/// - `Exact(type, id)`: the exact triple only
///
/// Bind order: 1=holder id, 2=name, (3=type), (4=instance id).
#[must_use]
pub(crate) fn delete_links_for_target<DB: Database>(
    join_table: &str,
    role_table: &str,
    target: &RemovalTarget<'_>,
) -> String {
    match target {
        RemovalTarget::NameOnly => format!(
            "DELETE FROM {join_table} \
             WHERE user_id = {holder_ph} \
               AND role_id IN (SELECT id FROM {role_table} WHERE name = {name_ph})",
            holder_ph = placeholder::<DB>(1),
            name_ph = placeholder::<DB>(2),
        ),
        RemovalTarget::TypeSweep(_type_name) => format!(
            "DELETE FROM {join_table} \
             WHERE user_id = {holder_ph} \
               AND role_id IN (SELECT id FROM {role_table} \
                               WHERE name = {name_ph} AND resource_type = {type_ph})",
            holder_ph = placeholder::<DB>(1),
            name_ph = placeholder::<DB>(2),
            type_ph = placeholder::<DB>(3),
        ),
        RemovalTarget::Exact(_type_name, _resource_id) => format!(
            "DELETE FROM {join_table} \
             WHERE user_id = {holder_ph} \
               AND role_id IN (SELECT id FROM {role_table} \
                               WHERE name = {name_ph} AND resource_type = {type_ph} \
                                 AND resource_id = {id_ph})",
            holder_ph = placeholder::<DB>(1),
            name_ph = placeholder::<DB>(2),
            type_ph = placeholder::<DB>(3),
            id_ph = placeholder::<DB>(4),
        ),
    }
}

/// SELECT the affected role rows for a removal target (the rows whose
/// links `delete_links_for_target` will delete), so the orphan sweep has
/// the records before they detach and the removed-link count derives
/// from selection (Pitfall 5).
///
/// Bind order: 1=holder id, 2=name, (3=type), (4=instance id).
#[must_use]
pub(crate) fn select_affected_roles<DB: Database>(
    role_table: &str,
    join_table: &str,
    target: &RemovalTarget<'_>,
) -> String {
    match target {
        RemovalTarget::NameOnly => format!(
            "SELECT {ROLE_PROJECTION} \
             FROM {role_table} AS role_row \
             INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
             WHERE link.user_id = {holder_ph} AND role_row.name = {name_ph}",
            holder_ph = placeholder::<DB>(1),
            name_ph = placeholder::<DB>(2),
        ),
        RemovalTarget::TypeSweep(_type_name) => format!(
            "SELECT {ROLE_PROJECTION} \
             FROM {role_table} AS role_row \
             INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
             WHERE link.user_id = {holder_ph} AND role_row.name = {name_ph} \
               AND role_row.resource_type = {type_ph}",
            holder_ph = placeholder::<DB>(1),
            name_ph = placeholder::<DB>(2),
            type_ph = placeholder::<DB>(3),
        ),
        RemovalTarget::Exact(_type_name, _resource_id) => format!(
            "SELECT {ROLE_PROJECTION} \
             FROM {role_table} AS role_row \
             INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
             WHERE link.user_id = {holder_ph} AND role_row.name = {name_ph} \
               AND role_row.resource_type = {type_ph} AND role_row.resource_id = {id_ph}",
            holder_ph = placeholder::<DB>(1),
            name_ph = placeholder::<DB>(2),
            type_ph = placeholder::<DB>(3),
            id_ph = placeholder::<DB>(4),
        ),
    }
}

/// SELECT whether any link still references a role row: the orphan-sweep
/// decision. sqlx replacement for the diesel store's rows_affected read
/// on the orphan DELETE (unreachable on generic `DB::QueryResult`,
/// Pitfall 5): the sweep decides BEFORE deleting.
///
/// Bind order: 1=role id (i64).
#[must_use]
pub(crate) fn select_link_exists_for_role<DB: Database>(join_table: &str) -> String {
    format!(
        "SELECT 1 FROM {join_table} WHERE role_id = {role_ph} LIMIT 1",
        role_ph = placeholder::<DB>(1),
    )
}

/// DELETE a role row when it has no remaining links: the
/// `remove_role_if_empty` sweep (`role.destroy if ... limit(1).empty?`,
/// `role_adapter.rb:58-70`). The `NOT EXISTS` guard re-checks inside the
/// same transaction as a belt-and-suspenders defense.
///
/// Bind order: 1=role id, 2=role id (one bind per placeholder
/// occurrence; Postgres reuses `$N`, MySQL/SQLite need one `?` each).
#[must_use]
pub(crate) fn delete_orphan_role<DB: Database>(role_table: &str, join_table: &str) -> String {
    format!(
        "DELETE FROM {role_table} \
         WHERE id = {id_ph} \
           AND NOT EXISTS (SELECT 1 FROM {join_table} WHERE role_id = {guard_ph})",
        id_ph = placeholder::<DB>(1),
        guard_ph = placeholder::<DB>(2),
    )
}

/// SELECT the role row ids for an exact resource scope
/// (`resource_type`, `resource_id`): the count-by-selection leg of
/// `remove_roles_for_scope` (Pitfall 5).
///
/// Bind order: 1=resource_type, 2=resource_id.
#[must_use]
pub(crate) fn select_role_ids_by_scope<DB: Database>(role_table: &str) -> String {
    format!(
        "SELECT id FROM {role_table} \
         WHERE resource_type = {type_ph} AND resource_id = {id_ph}",
        type_ph = placeholder::<DB>(1),
        id_ph = placeholder::<DB>(2),
    )
}

/// DELETE role rows by their ids (the exact set selected by
/// `select_role_ids_by_scope` inside the same transaction, so the count
/// and the deleted set cannot drift apart). Join rows vanish via the FK
/// `ON DELETE CASCADE` (D-10).
///
/// Bind order: 1..=`role_count` = role ids (i64), in selection order.
#[must_use]
pub(crate) fn delete_roles_by_ids<DB: Database>(role_table: &str, role_count: usize) -> String {
    debug_assert!(role_count > 0, "empty id lists skip the DELETE entirely");
    let placeholders: Vec<String> = (1..=role_count)
        .map(|index| placeholder::<DB>(index))
        .collect();
    format!(
        "DELETE FROM {role_table} WHERE id IN ({})",
        placeholders.join(", ")
    )
}

/// SELECT the instance-scoped keys for `resources_find`: the gem join's
/// equality arm (`roles.resource_id = relation.pk`,
/// `resource_adapter.rb:21-23`) reads as a direct roles-table scan
/// carrying the concrete (non-sentinel) ids of the family types with the
/// role name. The class arm lives in
/// [`select_resources_find_class_expansion`]; both decode through the
/// same `name, resource_type, resource_id` projection.
///
/// Bind order: 1..=`type_count` = the family types, then the role name.
#[must_use]
pub(crate) fn select_instance_resource_keys<DB: Database>(
    role_table: &str,
    type_count: usize,
) -> String {
    debug_assert!(
        type_count > 0,
        "empty type families return before SQL assembles"
    );
    let type_placeholders: Vec<String> = (1..=type_count)
        .map(|index| placeholder::<DB>(index))
        .collect();
    format!(
        "SELECT DISTINCT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
         FROM {role_table} AS role_row \
         WHERE role_row.resource_type IN ({}) \
           AND role_row.name = {} \
           AND role_row.resource_id != ''",
        type_placeholders.join(", "),
        placeholder::<DB>(type_count + 1),
    )
}

/// SELECT the class-scope expansion for `resources_find`: the gem join's
/// NULL arm (`roles.resource_id IS NULL`, `resource_adapter.rb:21-23`)
/// reads as a join with the resource's OWN table (resolved through the
/// store's registry), so one class-scoped role row for `resource_type`
/// surfaces every persisted resource of that table as a key.
///
/// AUDIT (T-04-07): `resource_type` interpolates as a LITERAL both in
/// the projection and in the join condition. That is safe because the
/// interpolated value is a resource-table REGISTRY entry that passed
/// `RolifyConfigBuilder::validate_identifier` at
/// `SqlxStore::register_resource_table` time (allow-list grammar
/// `^[A-Za-z_][A-Za-z0-9_]*$`), never consumer input - the diesel
/// reference carries the same literal interpolation under the same
/// justification (`rolify-diesel/src/sql.rs:465-501`,
/// `select_resources_find_class_expansion`). `resource_table` and
/// `pk_column` arrive from the same validated registry (the table is
/// already per-engine quoted). The role name travels as a bind, and
/// resource ids leave the engine through the integer-to-text cast.
///
/// Bind order: 1=name.
#[must_use]
pub(crate) fn select_resources_find_class_expansion<DB: Database>(
    role_table: &str,
    resource_table: &str,
    pk_column: &str,
    resource_type: &str,
) -> String {
    format!(
        "SELECT DISTINCT role_row.name AS name, '{resource_type}' AS resource_type, {pk_projection} AS resource_id \
         FROM {role_table} AS role_row \
         INNER JOIN {resource_table} AS res \
           ON role_row.resource_type = '{resource_type}' \
          AND role_row.resource_id = '' \
          AND role_row.name = {name_ph}",
        pk_projection = cast_to_text::<DB>(&format!("res.{pk_column}")),
        name_ph = placeholder::<DB>(1),
    )
}

/// SELECT the holder's rows for the `in_list` name list: the gem's
/// `user.roles.where(name: role_names)` leg (`resource_adapter.rb:28`).
/// Coverage is decided caller-side in Rust (mirroring
/// `InMemoryStore::in_list` exactly: a candidate is covered by a same-id
/// row or by a scopeless row, with no resource-type check), so this query
/// only narrows the join by holder and names.
///
/// Bind order: 1=holder id, 2..=`name_count` + 1 = the role names.
#[must_use]
pub(crate) fn select_holder_scope_rows<DB: Database>(
    role_table: &str,
    join_table: &str,
    name_count: usize,
) -> String {
    debug_assert!(
        name_count > 0,
        "empty name lists return before SQL assembles"
    );
    let name_placeholders: Vec<String> = (2..2 + name_count)
        .map(|index| placeholder::<DB>(index))
        .collect();
    format!(
        "SELECT DISTINCT {ROLE_PROJECTION} \
         FROM {role_table} AS role_row \
         INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
         WHERE link.user_id = {holder_ph} \
           AND role_row.name IN ({})",
        name_placeholders.join(", "),
        holder_ph = placeholder::<DB>(1),
    )
}

/// SELECT whether the holder holds any row with a non-sentinel scope
/// column: the gem's `exists?`
/// (`relation.where("<column> IS NOT NULL")`, `role_adapter.rb:72-74`),
/// with the sentinel substitution of Phase 3 D-01/D-02 (the branch is
/// `!= ''`, never `IS NOT NULL`). Inline in the diesel store; hoisted
/// here as a named template.
///
/// Bind order: 1=holder id.
#[must_use]
pub(crate) fn select_scoped_exists<DB: Database>(
    role_table: &str,
    join_table: &str,
    column: ScopeColumn,
) -> String {
    let scope_condition = match column {
        ScopeColumn::ResourceType => "role_row.resource_type != ''",
        ScopeColumn::ResourceId => "role_row.resource_id != ''",
    };
    format!(
        "SELECT 1 FROM {role_table} AS role_row \
         INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
         WHERE link.user_id = {holder_ph} AND {scope_condition} LIMIT 1",
        holder_ph = placeholder::<DB>(1),
    )
}

/// SELECT holder ids whose LINKED role rows match the ladder or strict
/// fragment, restricted to the `rolify_type` registry filter
/// (the D-03 `types` slice; `finders.rb:5-9` behind `adapter.scope`).
///
/// `type_filter` and `where_clause` arrive pre-built; the placeholder
/// sequence is types first, then the fragment (the caller threads the
/// index accordingly).
#[must_use]
pub(crate) fn select_holders_where(
    holder_table: &str,
    join_table: &str,
    role_table: &str,
    type_filter: &str,
    where_clause: &str,
) -> String {
    format!(
        "SELECT DISTINCT holder.id AS user_id \
         FROM {holder_table} AS holder \
         INNER JOIN {join_table} AS link ON link.user_id = holder.id \
         INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
         WHERE {type_filter} AND {where_clause}"
    )
}

/// SELECT every holder id of the given types: the FULL holder universe
/// including never-rolificated holders (`User.all` behind `all_except`,
/// `finders.rb:13`). `type_filter` arrives pre-built.
#[must_use]
pub(crate) fn select_all_holders(holder_table: &str, type_filter: &str) -> String {
    format!("SELECT id AS user_id FROM {holder_table} WHERE {type_filter}")
}

/// SELECT the catalog read base for `roles_matching`
/// (`find_roles`, `resource_adapter.rb:6-11`): globals are structurally
/// excluded (`resource_type != ''`, the `resource_adapter.rb:8`
/// asymmetry); the four `{{...}}` fragment slots are filled by the
/// caller through `String` replacement.
///
/// The holder join is appended only when `has_holder` (the gem's
/// `user.roles` branch); `holder_table` must be the quoted name then.
/// The holder table join key is cast to text (`cast_to_text`): the
/// fixture holder tables carry integer primary keys while the link
/// column stores text, and Postgres rejects `integer = text`.
#[must_use]
pub(crate) fn select_roles_matching<DB: Database>(
    role_table: &str,
    join_table: &str,
    holder_table: Option<&str>,
    has_holder: bool,
) -> String {
    let holder_join = if has_holder {
        format!(
            "INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
             INNER JOIN {holder_table} AS holder ON {} = link.user_id",
            cast_to_text::<DB>("holder.id"),
            holder_table = holder_table.unwrap_or_default(),
        )
    } else {
        String::new()
    };
    format!(
        "SELECT {ROLE_PROJECTION} \
         FROM {role_table} AS role_row \
         {holder_join} \
         WHERE role_row.resource_type != '' \
           {{type_filter}} \
           {{name_filter}} \
           {{scope_filter}} \
           {{holder_filter}}"
    )
}

/// Type filter fragment for `roles_matching`: `resource_type IN (...)`
/// with one placeholder per type (empty type slices match nothing by the
/// SPI contract and never reach the SQL).
#[must_use]
pub(crate) fn roles_matching_type_filter<DB: Database>(
    types: &[&str],
    start_index: usize,
) -> (String, usize) {
    let placeholders: Vec<String> = (start_index..start_index + types.len())
        .map(|index| placeholder::<DB>(index))
        .collect();
    (
        format!(
            "AND role_row.resource_type IN ({})",
            placeholders.join(", ")
        ),
        start_index + types.len(),
    )
}

/// Name filter fragment for `roles_matching` (the gem's
/// `role_name != :any` branch).
#[must_use]
pub(crate) fn roles_matching_name_filter<DB: Database>(start_index: usize) -> (String, usize) {
    (
        format!("AND role_row.name = {}", placeholder::<DB>(start_index)),
        start_index + 1,
    )
}

/// Scope filter fragment for `roles_matching`:
/// - `ClassAndInstance`: no extra condition (globals already excluded)
/// - `ClassOnly`: `resource_id` equals the sentinel (class rows only)
/// - `InstanceOnly(Some)`: `resource_id` equals the instance id
/// - `InstanceOnly(None)`: every instance row within `types`
#[must_use]
pub(crate) fn roles_matching_scope_filter<DB: Database>(
    scope: &CatalogScope<'_>,
    start_index: usize,
) -> (String, usize) {
    match scope {
        CatalogScope::ClassAndInstance => (String::new(), start_index),
        CatalogScope::ClassOnly => (
            format!(
                "AND role_row.resource_id = {}",
                placeholder::<DB>(start_index)
            ),
            start_index + 1,
        ),
        CatalogScope::InstanceOnly { resource_id } => match resource_id {
            Some(_) => (
                format!(
                    "AND role_row.resource_id = {}",
                    placeholder::<DB>(start_index)
                ),
                start_index + 1,
            ),
            None => (String::new(), start_index),
        },
    }
}

/// Holder filter fragment for `roles_matching` (the `user.roles` join
/// branch: only rows linked to the holder). The holder primary key is an
/// integer on the fixture tables while the bind carries the stringified
/// id, so the comparison binds against the text cast (same rationale as
/// the holder join in [`select_roles_matching`]).
#[must_use]
pub(crate) fn roles_matching_holder_filter<DB: Database>(start_index: usize) -> (String, usize) {
    (
        format!(
            "AND {} = {}",
            cast_to_text::<DB>("holder.id"),
            placeholder::<DB>(start_index)
        ),
        start_index + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::quote_identifier;
    use rolify_core::role::{ResourceId, RoleName};

    fn assert_ladder_bind_counts<DB: Database>() {
        let admin = RoleName::from("admin");
        let forum_id = ResourceId::from("42");

        // Global: 3 binds (name + both sentinels)
        let (_, next) = build_ladder_where::<DB>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Global,
            },
            2,
        );
        assert_eq!(next - 2, 3, "Global ladder carries 3 binds");

        // Class: 6 binds (per-occurrence: the name binds once per disjunct)
        let (_, next) = build_ladder_where::<DB>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Class("Forum"),
            },
            2,
        );
        assert_eq!(next - 2, 6, "Class ladder carries 6 binds");

        // Instance: 9 binds (three disjuncts, three binds each)
        let (sql, next) = build_ladder_where::<DB>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Instance("Forum", &forum_id),
            },
            2,
        );
        assert_eq!(next - 2, 9, "Instance ladder carries 9 binds");
        assert_eq!(sql.matches(" OR ").count(), 2, "three disjuncts, two ORs");

        // Any: 1 bind (name only, the gem's build_query:107 path)
        let (sql, next) = build_ladder_where::<DB>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Any,
            },
            2,
        );
        assert_eq!(next - 2, 1, "Any ladder carries 1 bind");
        assert_eq!(
            sql,
            format!("(role_row.name = {})", placeholder::<DB>(2)),
            "Any short-circuits to a name-only fragment"
        );
    }

    fn assert_strict_bind_counts<DB: Database>() {
        let admin = RoleName::from("admin");
        let forum_id = ResourceId::from("42");

        for filter in [
            ResourceFilter::Global,
            ResourceFilter::Class("Forum"),
            ResourceFilter::Instance("Forum", &forum_id),
        ] {
            let (_, next) = build_strict_where::<DB>(
                &RoleQuery {
                    name: &admin,
                    filter,
                },
                1,
            );
            assert_eq!(next - 1, 3, "strict single-disjunct carries 3 binds");
        }

        let (_, next) = build_strict_where::<DB>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Any,
            },
            1,
        );
        assert_eq!(next - 1, 1, "strict Any carries 1 bind");
    }

    fn assert_any_where_joins_ladders<DB: Database>() {
        let admin = RoleName::from("admin");
        let ghost = RoleName::from("ghost");
        let queries = [
            RoleQuery {
                name: &ghost,
                filter: ResourceFilter::Global,
            },
            RoleQuery {
                name: &admin,
                filter: ResourceFilter::Any,
            },
        ];
        let (sql, next) = build_any_where::<DB>(&queries, 2);
        assert_eq!(next - 2, 3 + 1, "each sub-ladder threads the shared index");
        assert_eq!(sql.matches(" OR ").count(), 1, "two ladders joined once");
    }

    fn assert_templates<DB: Database>() {
        let role_table = quote_identifier::<DB>("roles");
        let join_table = quote_identifier::<DB>("users_roles");

        let by_triple = select_role_by_triple::<DB>(&role_table);
        assert!(by_triple.contains("name = $1") || by_triple.contains("name = ?"));
        assert!(by_triple.contains(&role_table));

        let insert = insert_role::<DB>(&role_table);
        assert!(insert.starts_with(&format!(
            "INSERT INTO {role_table} (name, resource_type, resource_id)"
        )));

        let link = insert_link::<DB>(&join_table);
        assert!(link.contains("(user_id, role_id)"));
        assert!(link.contains(&join_table));

        // Orphan sweep: two placeholder occurrences for the same id
        // (Postgres emits $1 and $2, MySQL/SQLite emit ? twice); the
        // store binds the id once per occurrence on every engine.
        let orphan = delete_orphan_role::<DB>(&role_table, &join_table);
        assert!(orphan.contains("NOT EXISTS"));
        let orphan_occurrences = if DB::NAME == "PostgreSQL" {
            orphan.matches('$').count()
        } else {
            orphan.matches('?').count()
        };
        assert_eq!(orphan_occurrences, 2, "one bind per placeholder occurrence");

        // Scope delete: one placeholder per selected id.
        let by_ids = delete_roles_by_ids::<DB>(&role_table, 3);
        assert_eq!(
            by_ids.matches('$').count() + by_ids.matches('?').count(),
            3,
            "one placeholder per id"
        );

        let scoped_exists =
            select_scoped_exists::<DB>(&role_table, &join_table, ScopeColumn::ResourceType);
        assert!(scoped_exists.contains("role_row.resource_type != ''"));
        let scoped_exists_id =
            select_scoped_exists::<DB>(&role_table, &join_table, ScopeColumn::ResourceId);
        assert!(scoped_exists_id.contains("role_row.resource_id != ''"));

        let link_exists = select_link_exists_for_role::<DB>(&join_table);
        assert!(link_exists.contains("LIMIT 1"));
    }

    fn assert_resource_templates<DB: Database>() {
        let role_table = quote_identifier::<DB>("roles");
        let join_table = quote_identifier::<DB>("users_roles");
        let placeholder_count = |sql_text: &str| {
            if DB::NAME == "PostgreSQL" {
                sql_text.matches('$').count()
            } else {
                sql_text.matches('?').count()
            }
        };

        // Instance keys arm: types 1..=2, then the name bind.
        let instance_keys = select_instance_resource_keys::<DB>(&role_table, 2);
        assert!(instance_keys.contains("resource_type IN ("));
        assert!(instance_keys.contains("resource_id != ''"));
        assert_eq!(placeholder_count(&instance_keys), 3, "types plus name");
        if DB::NAME == "PostgreSQL" {
            assert!(instance_keys.contains("resource_type IN ($1, $2)"));
            assert!(instance_keys.contains("role_row.name = $3"));
        }

        // Class expansion: the registry type interpolates as a literal
        // twice (projection and join), the name binds once, and the pk
        // projection casts to text.
        let expansion = select_resources_find_class_expansion::<DB>(
            &role_table,
            &quote_identifier::<DB>("forums"),
            "id",
            "Forum",
        );
        assert!(expansion.contains("'Forum' AS resource_type"));
        assert!(expansion.contains("role_row.resource_type = 'Forum'"));
        assert!(expansion.contains("role_row.resource_id = ''"));
        assert_eq!(placeholder_count(&expansion), 1, "only the name binds");
        let expected_cast = if DB::NAME == "MySQL" {
            "CAST(res.id AS CHAR)"
        } else {
            "CAST(res.id AS TEXT)"
        };
        assert!(expansion.contains(expected_cast));

        // in_list holder scan: one holder bind plus one per name.
        let holder_rows = select_holder_scope_rows::<DB>(&role_table, &join_table, 2);
        assert!(holder_rows.contains("role_row.name IN ("));
        assert_eq!(placeholder_count(&holder_rows), 3, "holder plus names");
        if DB::NAME == "PostgreSQL" {
            assert!(holder_rows.contains("link.user_id = $1"));
            assert!(holder_rows.contains("role_row.name IN ($2, $3)"));
        }

        // The holder join and filter cast the integer holder key to text
        // (fixture holder tables are integer-keyed, the link column text).
        let holder_cast = if DB::NAME == "MySQL" {
            "CAST(holder.id AS CHAR)"
        } else {
            "CAST(holder.id AS TEXT)"
        };
        let catalog = select_roles_matching::<DB>(&role_table, &join_table, Some("users"), true);
        assert!(catalog.contains(holder_cast));
        let (holder_filter, _) = roles_matching_holder_filter::<DB>(4);
        assert!(holder_filter.contains(holder_cast));
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_placeholder_and_bind_table() {
        let admin = RoleName::from("admin");
        let (sql, _) = build_ladder_where::<sqlx::Postgres>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Global,
            },
            2,
        );
        assert!(sql.contains("$2"));
        assert!(sql.contains("$3"));
        assert!(sql.contains("$4"));
        assert_ladder_bind_counts::<sqlx::Postgres>();
        assert_strict_bind_counts::<sqlx::Postgres>();
        assert_any_where_joins_ladders::<sqlx::Postgres>();
        assert_templates::<sqlx::Postgres>();
        assert_resource_templates::<sqlx::Postgres>();
    }

    #[cfg(feature = "mysql")]
    #[test]
    fn mysql_placeholder_and_bind_table() {
        let admin = RoleName::from("admin");
        let (sql, _) = build_ladder_where::<sqlx::MySql>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Global,
            },
            2,
        );
        assert_eq!(sql.matches('?').count(), 3, "three anonymous binds");
        assert_ladder_bind_counts::<sqlx::MySql>();
        assert_strict_bind_counts::<sqlx::MySql>();
        assert_any_where_joins_ladders::<sqlx::MySql>();
        assert_templates::<sqlx::MySql>();
        assert_resource_templates::<sqlx::MySql>();
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_placeholder_and_bind_table() {
        let admin = RoleName::from("admin");
        let (sql, _) = build_ladder_where::<sqlx::Sqlite>(
            &RoleQuery {
                name: &admin,
                filter: ResourceFilter::Global,
            },
            2,
        );
        assert_eq!(sql.matches('?').count(), 3, "three anonymous binds");
        assert_ladder_bind_counts::<sqlx::Sqlite>();
        assert_strict_bind_counts::<sqlx::Sqlite>();
        assert_any_where_joins_ladders::<sqlx::Sqlite>();
        assert_templates::<sqlx::Sqlite>();
        assert_resource_templates::<sqlx::Sqlite>();
    }
}
