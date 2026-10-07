//! Ladder-only raw SQL builders (D-03): ONLY the three gem ladder branches
//! live here as raw text; every other query in the adapter uses
//! `Entity`/`QueryFilter`. The text mirrors `role_adapter.rb:106-121`
//! (`build_query`) and the disjunct table in `rolify-core/src/kernel.rs`
//! 1:1, with sentinel `= ''` scope branches (inherited Phase 3 D-01/D-02):
//! scope branches compare with equals against the empty-string sentinel
//! bind, NEVER `IS NULL`.
//!
//! Conventions:
//! - Every value travels as an ordered bind; only allow-list-validated and
//!   quoted table/column names may interpolate via `format!`, and they do
//!   so at the call sites that compose the full statement.
//! - Placeholders follow the backend compiled into the statement
//!   (`$1`-style for Postgres, `?` for `MySQL`) via the private
//!   [`placeholder`] helper; the store lands them in
//!   `Statement::from_sql_and_values`.
//! - The roles table is referenced by the fixed alias `role_row`, matching
//!   the composed `SELECT` the store builds.

use rolify_core::query::{ResourceFilter, RoleQuery};
use sea_orm::DbBackend;

/// Native placeholder for `index` on the target backend.
///
/// Postgres uses `$N`; `MySQL` (and any other backend) uses the portable
/// `?` positional form (`DbBackend` is non-exhaustive; unknown engines
/// fall back to `?`, like the diesel adapter's `dialect::placeholder`).
pub(crate) fn placeholder(backend: DbBackend, index: usize) -> String {
    match backend {
        DbBackend::Postgres => format!("${index}"),
        _ => "?".to_owned(),
    }
}

/// Build the non-strict ladder WHERE clause for a single `RoleQuery`.
///
/// Mirrors `build_query` (role_adapter.rb:106-121) and `kernel.rs`'s
/// disjunct table. Disjunct counts per filter:
/// - Global: 1 disjunct (name + both sentinel)
/// - Class: 2 disjuncts (global OR class-scope)
/// - Instance: 3 disjuncts (global OR class OR instance)
/// - Any: name-only short-circuit (D-02 DB path)
///
/// Returns (`sql_fragment`, `next_bind_index_after_this_fragment`).
/// Bind order: name first, then the sentinel/type/id values in disjunct
/// order; the store supplies the matching value vector.
#[must_use]
pub fn build_ladder_where(
    backend: DbBackend,
    query: &RoleQuery<'_>,
    start_index: usize,
) -> (String, usize) {
    // The role name is factored ONCE per ladder: every value occurrence
    // needs its own positional placeholder on `?` backends (a reused `$N`
    // works on Postgres only). The factored shape is equivalent by
    // distributivity: name AND (scope1 OR scope2).
    let name_ph = placeholder(backend, start_index);
    let mut index = start_index + 1;

    // Short-circuit for Any: name-only (gem: build_query:107)
    if matches!(query.filter, ResourceFilter::Any) {
        let sql = format!("(role_row.name = {name_ph})");
        return (sql, index);
    }

    // Global scope pair (always present for non-Any): both sentinel
    // gem: resource == nil -> global disjunct only (build_query:108-109)
    let sentinel_type_ph = placeholder(backend, index);
    let sentinel_id_ph = placeholder(backend, index + 1);
    index += 2;

    let global_pair = format!(
        "(role_row.resource_type = {sentinel_type_ph} AND role_row.resource_id = {sentinel_id_ph})"
    );

    match &query.filter {
        ResourceFilter::Global => (
            format!("(role_row.name = {name_ph} AND {global_pair})"),
            index,
        ),
        ResourceFilter::Class(_type_name) => {
            // Global OR (type + sentinel id) — gem: build_query:112-113
            let class_type_ph = placeholder(backend, index);
            index += 1;
            let class_id_ph = placeholder(backend, index);
            index += 1;

            let class_pair = format!(
                "(role_row.resource_type = {class_type_ph} AND role_row.resource_id = {class_id_ph})"
            );
            let sql = format!("(role_row.name = {name_ph} AND ({global_pair} OR {class_pair}))");
            (sql, index)
        }
        ResourceFilter::Instance(_type_name, _resource_id) => {
            // Global OR Class(type) OR Instance(type, id) — gem: build_query:114-117
            let class_type_ph = placeholder(backend, index);
            index += 1;
            let class_id_ph = placeholder(backend, index);
            index += 1;
            let class_pair = format!(
                "(role_row.resource_type = {class_type_ph} AND role_row.resource_id = {class_id_ph})"
            );

            let inst_type_ph = placeholder(backend, index);
            index += 1;
            let inst_id_ph = placeholder(backend, index);
            index += 1;
            let inst_pair = format!(
                "(role_row.resource_type = {inst_type_ph} AND role_row.resource_id = {inst_id_ph})"
            );

            let sql = format!(
                "(role_row.name = {name_ph} AND ({global_pair} OR {class_pair} OR {inst_pair}))"
            );
            (sql, index)
        }
        ResourceFilter::Any => {
            // Short-circuited above; unreachable arm retained for
            // exhaustiveness (kernel ratified corner).
            (
                format!("(role_row.name = {name_ph} AND {global_pair})"),
                index,
            )
        }
    }
}

/// Build the strict WHERE clause for a single `RoleQuery` (exact scope).
///
/// Mirrors `kernel::where_strict` (role_adapter.rb:11-26): always one
/// disjunct per filter; `Any` short-circuits to name-only (kernel
/// ratified corner).
#[must_use]
pub fn build_strict_where(
    backend: DbBackend,
    query: &RoleQuery<'_>,
    start_index: usize,
) -> (String, usize) {
    let name_ph = placeholder(backend, start_index);
    let mut index = start_index + 1;

    if matches!(query.filter, ResourceFilter::Any) {
        let sql = format!("(role_row.name = {name_ph})");
        return (sql, index);
    }

    let type_ph = placeholder(backend, index);
    let id_ph = placeholder(backend, index + 1);
    index += 2;

    let sql = format!(
        "(role_row.name = {name_ph} AND role_row.resource_type = {type_ph} AND role_row.resource_id = {id_ph})"
    );
    (format!("({sql})"), index)
}

/// Build OR-joined ladders for `where_any`: one ladder per query, joined
/// by ` OR ` (mirrors `build_conditions`, role_adapter.rb:88-104: each
/// disjunct's `join(' OR ')`). Each sub-ladder gets its own bind sequence.
///
/// The joined group is wrapped in one enclosing pair of parentheses: the
/// store embeds the fragment as `WHERE link.user_id = ? AND {fragment}`,
/// and SQL binds `AND` tighter than `OR`, so an unparenthesized group
/// would parse as `(holder AND ladder1) OR ladder2` and leak every
/// ladder after the first out of the holder gate (the gem's
/// `ActiveRecord` composes each where fragment parenthesized,
/// role_adapter.rb:88-104).
#[must_use]
pub fn build_any_where(
    backend: DbBackend,
    queries: &[RoleQuery<'_>],
    start_index: usize,
) -> (String, usize) {
    let mut parts = Vec::new();
    let mut index = start_index;

    for query in queries {
        let (part, next_index) = build_ladder_where(backend, query, index);
        parts.push(part);
        index = next_index;
    }

    (format!("({})", parts.join(" OR ")), index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolify_core::role::{ResourceId, RoleName};

    #[test]
    fn ladder_global_one_disjunct_postgres_placeholders() {
        let (sql, next) = build_ladder_where(
            DbBackend::Postgres,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            },
            2,
        );
        // Global: 1 disjunct with 3 binds (name, rt = '', rid = '')
        assert!(sql.contains("resource_type = $3"));
        assert!(sql.contains("resource_id = $4"));
        assert!(!sql.contains("IS NULL"));
        assert_eq!(next, 5);
    }

    #[test]
    fn ladder_global_uses_mysql_placeholders() {
        let (sql, next) = build_ladder_where(
            DbBackend::MySql,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            },
            2,
        );
        assert!(sql.contains("resource_type = ?"));
        assert!(!sql.contains('$'));
        assert_eq!(next, 5);
    }

    #[test]
    fn ladder_class_two_disjuncts() {
        let (sql, next) = build_ladder_where(
            DbBackend::Postgres,
            &RoleQuery {
                name: &RoleName::from("manager"),
                filter: ResourceFilter::Class("Forum"),
            },
            2,
        );
        // Class: global disjunct (3 binds) + class disjunct (2 more)
        assert_eq!(sql.matches(" OR ").count(), 1);
        assert_eq!(next, 7);
    }

    #[test]
    fn ladder_instance_three_disjuncts() {
        let (sql, next) = build_ladder_where(
            DbBackend::Postgres,
            &RoleQuery {
                name: &RoleName::from("moderator"),
                filter: ResourceFilter::Instance("Forum", &ResourceId::from("42")),
            },
            2,
        );
        // Instance: 3 disjuncts = 3 + 2 + 2 = 7 binds after name
        assert_eq!(sql.matches(" OR ").count(), 2);
        assert_eq!(next, 9);
    }

    #[test]
    fn ladder_any_name_only_short_circuit() {
        let (sql, next) = build_ladder_where(
            DbBackend::Postgres,
            &RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Any,
            },
            2,
        );
        assert_eq!(sql, "(role_row.name = $2)");
        assert_eq!(next, 3);
    }

    #[test]
    fn strict_where_exact_triple() {
        let (sql, next) = build_strict_where(
            DbBackend::Postgres,
            &RoleQuery {
                name: &RoleName::from("moderator"),
                filter: ResourceFilter::Class("Forum"),
            },
            2,
        );
        assert!(sql.contains("resource_type = $3"));
        assert!(sql.contains("resource_id = $4"));
        assert_eq!(next, 5);
    }

    #[test]
    fn any_where_joins_ladders_with_or() {
        let name_a = RoleName::from("admin");
        let id = ResourceId::from("7");
        let queries = [
            RoleQuery {
                name: &name_a,
                filter: ResourceFilter::Global,
            },
            RoleQuery {
                name: &name_a,
                filter: ResourceFilter::Instance("Forum", &id),
            },
        ];
        let (sql, next) = build_any_where(DbBackend::Postgres, &queries, 1);
        // Two ladders: global (3 binds) + instance (7 binds) = 10 binds
        assert_eq!(sql.matches(" OR ").count(), 3);
        assert_eq!(next, 11);
    }

    #[test]
    fn any_where_wraps_the_or_group_in_parens() {
        let name = RoleName::from("solo");
        let queries = [
            RoleQuery {
                name: &name,
                filter: ResourceFilter::Global,
            },
            RoleQuery {
                name: &name,
                filter: ResourceFilter::Class("Forum"),
            },
        ];
        let (sql, _) = build_any_where(DbBackend::Postgres, &queries, 2);
        assert!(
            sql.starts_with('(') && sql.ends_with(')'),
            "the OR group must stay inside the holder gate: {sql}"
        );
    }
}
