//! SQL template builders — one function per SPI member, mirroring
//! `role_adapter.rb:106-121` (`build_query`) and the write choreography
//! 1:1. Every value travels as an ordered bind; only validated+quoted
//! table names interpolate via `format!`.
//!
//! Conventions:
//! - Table names come from `DieselStore` config and are routed through
//!   `dialect::quote_identifier` at call sites (never interpolated raw).
//! - Placeholders use `dialect::placeholder(n)` (1-based for Postgres,
//!   `?` for MySQL/SQLite).
//! - Projection lists are explicit (`role_row.name AS name, ...`) — never
//!   `SELECT *` (Pitfall 9: joined `QueryableByName` name clashes).
//! - Sentinel `''` for global/class scope (D-01/D-02): scope branches
//!   compare against `= ''`, never `= NULL` or binding `Option::None`.
//! - Each function cites the gem <file:line> range it mirrors.
//!
//! This module is only compiled when a backend feature is enabled
//! (`postgres`, `mysql`, or `sqlite`). When no backend feature is enabled,
//! the crate compiles as an inert stub.

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod backend_sql {
    use crate::dialect::{cast_to_text, placeholder};
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::role::{ResourceId, RoleName};

    /// Build the non-strict ladder WHERE clause for a single `RoleQuery`.
    ///
    /// Mirrors `build_query` (role_adapter.rb:106-121) and the disjunct table
    /// in `kernel.rs:34-41`. Disjunct counts per filter:
    /// - Global: 1 disjunct (name + both sentinel)
    /// - Class: 2 disjuncts (global OR class-scope)
    /// - Instance: 3 disjuncts (global OR class OR instance)
    /// - Any: name-only short-circuit (D-02 DB path)
    ///
    /// The `role_table` and `join_table` parameters are **already quoted**
    /// identifiers (from `quote_identifier`).
    ///
    /// The `holder_id_placeholder` is the bind index for the holder's `user_id`
    /// in the join condition (typically `$1` / `?`).
    ///
    /// Returns (`sql_fragment`, `next_placeholder_index_after_this_fragment`).
    #[must_use]
    pub fn build_ladder_where(
        _role_table: &str,
        _join_table: &str,
        _holder_id_placeholder: &str,
        query: &RoleQuery<'_>,
        start_index: usize,
    ) -> (String, usize) {
        // The role name is factored ONCE per ladder: every value
        // occurrence needs its own positional placeholder on `?`
        // backends (a reused `$N` works on Postgres only, since `?`
        // binds fill occurrences in order). The factored shape is
        // equivalent by distributivity: name AND (scope1 OR scope2).
        let name_ph = placeholder(start_index);
        let mut idx = start_index + 1;

        // Short-circuit for Any: name-only (gem: build_query:107)
        if matches!(query.filter, ResourceFilter::Any) {
            let sql = format!("(role_row.name = {name_ph})");
            return (sql, idx);
        }

        // Global scope pair (always present for non-Any): both sentinel
        // gem: resource == nil -> global disjunct only (build_query:108-109)
        let rt_ph = placeholder(idx);
        let rid_ph = placeholder(idx + 1);
        idx += 2;

        let global_pair =
            format!("(role_row.resource_type = {rt_ph} AND role_row.resource_id = {rid_ph})");

        match &query.filter {
            ResourceFilter::Global => {
                // Only the global pair
                (
                    format!("(role_row.name = {name_ph} AND {global_pair})"),
                    idx,
                )
            }
            ResourceFilter::Class(_type_name) => {
                // Global OR (type + sentinel id) — gem: build_query:112-113
                let class_rt_ph = placeholder(idx);
                idx += 1;
                let class_rid_ph = placeholder(idx);
                idx += 1;

                let class_pair = format!(
                    "(role_row.resource_type = {class_rt_ph} AND role_row.resource_id = {class_rid_ph})"
                );
                let sql =
                    format!("(role_row.name = {name_ph} AND ({global_pair} OR {class_pair}))");
                (sql, idx)
            }
            ResourceFilter::Instance(_type_name, _resource_id) => {
                // Global OR Class(type) OR Instance(type, id) — gem: build_query:114-117
                let class_rt_ph = placeholder(idx);
                idx += 1;
                let class_rid_ph = placeholder(idx);
                idx += 1;

                let class_pair = format!(
                    "(role_row.resource_type = {class_rt_ph} AND role_row.resource_id = {class_rid_ph})"
                );

                let inst_rt_ph = placeholder(idx);
                idx += 1;
                let inst_rid_ph = placeholder(idx);
                idx += 1;

                let inst_pair = format!(
                    "(role_row.resource_type = {inst_rt_ph} AND role_row.resource_id = {inst_rid_ph})"
                );
                let sql = format!(
                    "(role_row.name = {name_ph} AND ({global_pair} OR {class_pair} OR {inst_pair}))"
                );
                (sql, idx)
            }
            ResourceFilter::Any => {
                // Handled above — unreachable but kept for exhaustiveness
                (
                    format!("(role_row.name = {name_ph} AND {global_pair})"),
                    idx,
                )
            }
        }
    }

    /// Build the strict WHERE clause for a single `RoleQuery` (exact scope, no ladder).
    ///
    /// Mirrors `kernel::where_strict` (role_adapter.rb:11-26). Disjunct count
    /// is always 1 per filter:
    /// - Global: name + both sentinel
    /// - Class(type): name + type + sentinel id
    /// - Instance(type, id): name + type + id
    /// - Any: name-only (kernel ratified corner)
    ///
    /// `holder_id_placeholder` is the bind for the join's `user_id`.
    ///
    /// The WHERE clause references the `role_row` alias used in `select_roles_for_holder`.
    #[must_use]
    pub fn build_strict_where(
        _role_table: &str,
        _join_table: &str,
        _holder_id_placeholder: &str,
        query: &RoleQuery<'_>,
        start_index: usize,
    ) -> (String, usize) {
        let name_ph = placeholder(start_index);
        let mut idx = start_index + 1;

        // Any short-circuits to name-only (kernel ratified)
        if matches!(query.filter, ResourceFilter::Any) {
            let sql = format!("(role_row.name = {name_ph})");
            return (sql, idx);
        }

        let rt_ph = placeholder(idx);
        let rid_ph = placeholder(idx + 1);
        idx += 2;

        // Strict matching is the exact triple for every scoped filter
        // shape; only the `Any` short-circuit differs (name only).
        let sql = match &query.filter {
            ResourceFilter::Global | ResourceFilter::Class(_) | ResourceFilter::Instance(_, _) => {
                format!(
                    "(role_row.name = {name_ph} AND role_row.resource_type = {rt_ph} AND role_row.resource_id = {rid_ph})"
                )
            }
            ResourceFilter::Any => format!("(role_row.name = {name_ph})"),
        };
        (format!("({sql})"), idx)
    }

    /// Build OR-joined ladders for `where_any` — one ladder per query, joined by OR.
    ///
    /// Mirrors `build_conditions` (role_adapter.rb:88-104): `join(' OR ')`.
    /// Each sub-ladder gets its own placeholder sequence.
    #[must_use]
    pub fn build_any_where(
        role_table: &str,
        join_table: &str,
        holder_id_placeholder: &str,
        queries: &[RoleQuery<'_>],
        start_index: usize,
    ) -> (String, usize) {
        let mut parts = Vec::new();
        let mut idx = start_index;

        for query in queries {
            let (part, next_idx) =
                build_ladder_where(role_table, join_table, holder_id_placeholder, query, idx);
            parts.push(part);
            idx = next_idx;
        }

        (parts.join(" OR "), idx)
    }

    /// SELECT role rows for `where_` / `where_strict` / `where_any`.
    ///
    /// Projection: `role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id`
    /// Joins `role_table` (aliased `role_row`) with `join_table` on `role_id = roles.id`
    /// and filters by `holder_id` (bind index 1).
    #[must_use]
    pub fn select_roles_for_holder(
        role_table: &str,
        join_table: &str,
        where_clause: &str,
        holder_id_placeholder: &str,
    ) -> String {
        format!(
            "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
         FROM {role_table} AS role_row \
         INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
         WHERE link.user_id = {holder_id_placeholder} AND {where_clause}"
        )
    }

    /// SELECT role row by exact triple (name, `resource_type`, `resource_id`).
    ///
    /// Used by `find_or_create_by` (SELECT-first) and the re-SELECT after
    /// INSERT or caught `UniqueViolation`. Returns at most one row.
    ///
    /// Bind order: 1=name, `2=resource_type`, `3=resource_id`.
    #[must_use]
    pub fn select_role_by_triple(role_table: &str) -> String {
        format!(
            "SELECT name, resource_type, resource_id \
         FROM {role_table} \
         WHERE name = {ph1} AND resource_type = {ph2} AND resource_id = {ph3}",
            role_table = role_table,
            ph1 = placeholder(1),
            ph2 = placeholder(2),
            ph3 = placeholder(3),
        )
    }

    /// INSERT a new role row (name, `resource_type`, `resource_id`).
    ///
    /// Returns the number of rows inserted (0 or 1). The caller must re-SELECT
    /// by triple to get the generated `id` (portable: `MySQL` lacks RETURNING).
    ///
    /// Bind order: 1=name, `2=resource_type`, `3=resource_id`.
    #[must_use]
    pub fn insert_role(role_table: &str) -> String {
        format!(
            "INSERT INTO {role_table} (name, resource_type, resource_id) VALUES ({ph1}, {ph2}, {ph3})",
            role_table = role_table,
            ph1 = placeholder(1),
            ph2 = placeholder(2),
            ph3 = placeholder(3),
        )
    }

    /// SELECT role `id` by triple (for link insertion after `find_or_create_by`).
    ///
    /// Bind order: 1=name, `2=resource_type`, `3=resource_id`.
    #[must_use]
    pub fn select_role_id_by_triple(role_table: &str) -> String {
        format!(
            "SELECT id FROM {role_table} WHERE name = {ph1} AND resource_type = {ph2} AND resource_id = {ph3}",
            role_table = role_table,
            ph1 = placeholder(1),
            ph2 = placeholder(2),
            ph3 = placeholder(3),
        )
    }

    /// INSERT a link (holder -> role).
    ///
    /// Bind order: `1=user_id`, `2=role_id`.
    #[must_use]
    pub fn insert_link(join_table: &str) -> String {
        format!(
            "INSERT INTO {join_table} (user_id, role_id) VALUES ({ph1}, {ph2})",
            join_table = join_table,
            ph1 = placeholder(1),
            ph2 = placeholder(2),
        )
    }

    /// DELETE links for a holder matching a removal target.
    ///
    /// Mirrors `kernel::removal_match` (role_adapter.rb:58-70). The `target`
    /// determines which role rows are swept:
    /// - `NameOnly`: all roles with this name (any scope)
    /// - `TypeSweep(type)`: roles with this name AND `resource_type` = type (class + instance)
    /// - `Exact(type, id)`: exact triple only
    ///
    /// The join uses a subquery to find matching `role_ids`, then deletes from
    /// the join table where `user_id = holder` AND `role_id IN (...)`.
    ///
    /// Bind order: `1=holder_id`, 2=name, (3=type for TypeSweep/Exact), (4=id for Exact).
    #[must_use]
    pub fn delete_links_for_target(
        join_table: &str,
        role_table: &str,
        target: &rolify_core::kernel::RemovalTarget<'_>,
    ) -> String {
        match target {
            rolify_core::kernel::RemovalTarget::NameOnly => {
                format!(
                    "DELETE FROM {join_table} \
                 WHERE user_id = {ph1} \
                   AND role_id IN (SELECT id FROM {role_table} WHERE name = {ph2})",
                    join_table = join_table,
                    role_table = role_table,
                    ph1 = placeholder(1),
                    ph2 = placeholder(2),
                )
            }
            rolify_core::kernel::RemovalTarget::TypeSweep(_type_name) => {
                format!(
                    "DELETE FROM {join_table} \
                 WHERE user_id = {ph1} \
                   AND role_id IN (SELECT id FROM {role_table} WHERE name = {ph2} AND resource_type = {ph3})",
                    join_table = join_table,
                    role_table = role_table,
                    ph1 = placeholder(1),
                    ph2 = placeholder(2),
                    ph3 = placeholder(3),
                )
            }
            rolify_core::kernel::RemovalTarget::Exact(_type_name, _resource_id) => {
                format!(
                    "DELETE FROM {join_table} \
                 WHERE user_id = {ph1} \
                   AND role_id IN (SELECT id FROM {role_table} WHERE name = {ph2} AND resource_type = {ph3} AND resource_id = {ph4})",
                    join_table = join_table,
                    role_table = role_table,
                    ph1 = placeholder(1),
                    ph2 = placeholder(2),
                    ph3 = placeholder(3),
                    ph4 = placeholder(4),
                )
            }
        }
    }

    /// SELECT affected role rows for a holder matching a removal target.
    ///
    /// Used by `remove` to collect rows before deleting links (for the orphan
    /// sweep). Same WHERE logic as `delete_links_for_target` but projects
    /// the role columns.
    ///
    /// Bind order matches the corresponding `delete_links_for_target`.
    #[must_use]
    pub fn select_affected_roles(
        role_table: &str,
        join_table: &str,
        target: &rolify_core::kernel::RemovalTarget<'_>,
    ) -> String {
        match target {
            rolify_core::kernel::RemovalTarget::NameOnly => {
                format!(
                    "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {ph1} AND role_row.name = {ph2}",
                    role_table = role_table,
                    join_table = join_table,
                    ph1 = placeholder(1),
                    ph2 = placeholder(2),
                )
            }
            rolify_core::kernel::RemovalTarget::TypeSweep(_) => {
                format!(
                    "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {ph1} AND role_row.name = {ph2} AND role_row.resource_type = {ph3}",
                    role_table = role_table,
                    join_table = join_table,
                    ph1 = placeholder(1),
                    ph2 = placeholder(2),
                    ph3 = placeholder(3),
                )
            }
            rolify_core::kernel::RemovalTarget::Exact(_, _) => {
                format!(
                    "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {ph1} AND role_row.name = {ph2} AND role_row.resource_type = {ph3} AND role_row.resource_id = {ph4}",
                    role_table = role_table,
                    join_table = join_table,
                    ph1 = placeholder(1),
                    ph2 = placeholder(2),
                    ph3 = placeholder(3),
                    ph4 = placeholder(4),
                )
            }
        }
    }

    /// DELETE a role row if it has NO remaining links in the join table.
    ///
    /// Orphan sweep: `DELETE FROM roles WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM join_table WHERE role_id = $1)`.
    /// Returns 1 if deleted, 0 if links still exist.
    ///
    /// Bind order: `1=role_id`.
    #[must_use]
    pub fn delete_orphan_role(role_table: &str, join_table: &str) -> String {
        // Two placeholders (not one reused): `?` backends fill
        // occurrences positionally, so a shared `$1` would leave the
        // second occurrence unbound and the sweep would always fire.
        format!(
            "DELETE FROM {role_table} \
         WHERE id = {ph1} \
           AND NOT EXISTS (SELECT 1 FROM {join_table} WHERE role_id = {ph2})",
            role_table = role_table,
            join_table = join_table,
            ph1 = placeholder(1),
            ph2 = placeholder(2),
        )
    }

    /// DELETE roles by exact resource scope (`resource_type`, `resource_id`).
    ///
    /// Implements `RoleStore::remove_roles_for_scope` (OQ1 / SC-3 / D-10).
    /// Deletes exactly the role rows matching the given `resource_type` and `resource_id`.
    /// Join rows vanish via FK ON DELETE CASCADE (D-10). Class-scoped rows of the
    /// same type (where `resource_id` = '') are NOT touched: the WHERE is the exact
    /// scope pair, never a type-only sweep.
    ///
    /// Mirrors the gem's `dependent: :destroy` on resource destruction
    /// (`rolify/spec/rolify/resource_spec.rb:507-510`).
    ///
    /// Bind order: `1=resource_type`, `2=resource_id`.
    #[must_use]
    pub fn delete_roles_by_scope(role_table: &str) -> String {
        format!(
            "DELETE FROM {role_table} WHERE resource_type = {ph1} AND resource_id = {ph2}",
            role_table = role_table,
            ph1 = placeholder(1),
            ph2 = placeholder(2),
        )
    }

    /// SELECT resource keys for class-scope role expansion in `resources_find`.
    ///
    /// For a given resource type with a class-scoped role (`resource_id` = ''),
    /// expand to all instances by joining with the resource's table.
    ///
    /// Mirrors `resource_adapter.rb:13-25`:
    /// ```ruby
    /// resources = relation.joins("INNER JOIN roles ON roles.resource_type IN (...) AND
    ///                               (roles.resource_id IS NULL OR roles.resource_id = relation.id)")
    /// ```
    ///
    /// Bind order: 1=name.
    /// The `resource_type` is interpolated as a literal (validated via registry).
    /// The `resource_table` and `pk_column` are interpolated (validated identifiers).
    #[must_use]
    pub fn select_resources_find_class_expansion(
        role_table: &str,
        resource_table: &str,
        pk_column: &str,
        resource_type: &str,
        name_placeholder: &str,
    ) -> String {
        format!(
            "SELECT DISTINCT role_row.name AS name, '{}' AS resource_type, {} AS resource_id \
         FROM {} AS role_row \
         INNER JOIN {} AS res \
           ON role_row.resource_type = '{}' \
          AND role_row.resource_id = '' \
          AND role_row.name = {}",
            resource_type,
            cast_to_text(&format!("res.{pk_column}")),
            role_table,
            resource_table,
            resource_type,
            name_placeholder,
        )
    }

    /// SELECT 1 if a role with the exact triple exists (for `exists` SPI).
    ///
    /// Bind order: 1=name, `2=resource_type`, `3=resource_id`.
    #[must_use]
    pub fn select_role_exists_by_triple(role_table: &str) -> String {
        format!(
            "SELECT 1 FROM {role_table} WHERE name = {ph1} AND resource_type = {ph2} AND resource_id = {ph3} LIMIT 1",
            role_table = role_table,
            ph1 = placeholder(1),
            ph2 = placeholder(2),
            ph3 = placeholder(3),
        )
    }

    /// SELECT holder ids for `holders_where` / `all_holders`.
    ///
    /// Joins the holder's table (consumer-provided) with the join table and
    /// roles, applying the same ladder/strict logic. The holder table name is
    /// a runtime value from the fixture registry — validated at config build
    /// and quoted per engine.
    ///
    /// This is a template; the caller provides the holder table name and the
    /// WHERE clause (built by `build_ladder_where` or `build_strict_where`).
    #[must_use]
    pub fn select_holders_where(
        holder_table: &str,
        join_table: &str,
        role_table: &str,
        where_clause: &str,
    ) -> String {
        format!(
            "SELECT DISTINCT holder.id AS user_id \
         FROM {holder_table} AS holder \
         INNER JOIN {join_table} AS link ON link.user_id = holder.id \
         INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
         WHERE {where_clause}"
        )
    }

    /// SELECT all holder ids for given types (for `all_holders`).
    ///
    /// No role join — just the holder table filtered by the type registry.
    #[must_use]
    pub fn select_all_holders(holder_table: &str) -> String {
        format!("SELECT id AS user_id FROM {holder_table}")
    }

    /// SELECT resource keys for `roles_matching` catalog read.
    ///
    /// Mirrors the filter semantics documented on `RoleStore::roles_matching`:
    /// - `types`: `resource_type` IN (types) - globals (sentinel) NEVER match
    /// - `name`: byte-exact when Some
    /// - `scope`: `ClassAndInstance` / `ClassOnly` / `InstanceOnly`
    /// - `holder`: when Some, only rows linked to that holder
    ///
    /// The holder table and join are only included when `holder` is Some.
    #[must_use]
    pub fn select_roles_matching(
        role_table: &str,
        join_table: &str,
        holder_table: Option<&str>,
        has_holder: bool,
    ) -> String {
        let holder_join = if has_holder {
            // The join key is stringified in the link table while the
            // holder primary key is an integer: cast for the comparison
            // (and the `Text` decode) on every backend.
            format!(
                "INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
             INNER JOIN {holder_table} AS holder ON {holder_id} = link.user_id",
                join_table = join_table,
                holder_table = holder_table.unwrap_or(""),
                holder_id = cast_to_text("holder.id"),
            )
        } else {
            String::new()
        };

        format!(
            "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
         FROM {role_table} AS role_row \
         {holder_join} \
         WHERE role_row.resource_type != '' \
           {{type_filter}} \
           {{name_filter}} \
           {{scope_filter}} \
           {{holder_filter}}"
        )
    }

    /// Build the type filter fragment for `roles_matching`.
    ///
    /// `types` is non-empty (empty slice matches nothing per SPI contract).
    #[must_use]
    pub fn roles_matching_type_filter(types: &[&str], start_index: usize) -> (String, usize) {
        let placeholders: Vec<String> = (start_index..start_index + types.len())
            .map(placeholder)
            .collect();
        let sql = format!(
            "AND role_row.resource_type IN ({})",
            placeholders.join(", ")
        );
        (sql, start_index + types.len())
    }

    /// Build the name filter fragment for `roles_matching`.
    #[must_use]
    pub fn roles_matching_name_filter(_name: &RoleName, index: usize) -> (String, usize) {
        let ph = placeholder(index);
        (format!("AND role_row.name = {ph}"), index + 1)
    }

    /// Build the scope filter fragment for `roles_matching`.
    #[must_use]
    pub fn roles_matching_scope_filter(
        scope: &rolify_core::catalog::CatalogScope<'_>,
        start_index: usize,
    ) -> (String, usize) {
        match scope {
            rolify_core::catalog::CatalogScope::ClassAndInstance => {
                // No additional filter — already excluded globals by resource_type != ''
                (String::new(), start_index)
            }
            rolify_core::catalog::CatalogScope::ClassOnly => {
                // resource_id = sentinel (class-scoped only)
                let ph = placeholder(start_index);
                (format!("AND role_row.resource_id = {ph}"), start_index + 1)
            }
            rolify_core::catalog::CatalogScope::InstanceOnly { resource_id } => {
                match resource_id {
                    Some(_) => {
                        // resource_id = specific id
                        let ph = placeholder(start_index);
                        (format!("AND role_row.resource_id = {ph}"), start_index + 1)
                    }
                    None => {
                        // Every instance row in types — no additional filter
                        (String::new(), start_index)
                    }
                }
            }
        }
    }

    /// Build the holder filter fragment for `roles_matching` (when holder is Some).
    #[must_use]
    pub fn roles_matching_holder_filter(_holder_id: &ResourceId, index: usize) -> (String, usize) {
        let ph = placeholder(index);
        // Holder primary keys are integers; the bind carries the
        // stringified id (see `cast_to_text`).
        (
            format!("AND {} = {}", cast_to_text("holder.id"), ph),
            index + 1,
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::dialect::{placeholder, quote_identifier};
        use rolify_core::query::{ResourceFilter, RoleQuery};
        use rolify_core::role::{ResourceId, RoleName};

        #[test]
        fn build_ladder_global_one_disjunct() {
            let role_table = quote_identifier("roles");
            let join_table = quote_identifier("users_roles");
            let (sql, next) = build_ladder_where(
                &role_table,
                &join_table,
                &placeholder(1),
                &RoleQuery {
                    name: &RoleName::from("admin"),
                    filter: ResourceFilter::Global,
                },
                2,
            );
            // Global: 1 disjunct with 3 binds (name, rt='', rid='')
            // The roles table is aliased `role_row` by the SELECT caller.
            // Placeholders render per compiled backend ($N / ?).
            assert!(sql.contains(&format!("resource_type = {}", placeholder(3))));
            assert!(sql.contains(&format!("resource_id = {}", placeholder(4))));
            assert_eq!(next, 5);
        }

        #[test]
        fn build_ladder_class_two_disjuncts() {
            let role_table = quote_identifier("roles");
            let join_table = quote_identifier("users_roles");
            let (sql, next) = build_ladder_where(
                &role_table,
                &join_table,
                &placeholder(1),
                &RoleQuery {
                    name: &RoleName::from("manager"),
                    filter: ResourceFilter::Class("Forum"),
                },
                2,
            );
            // Class: global disjunct (3 binds) + class disjunct (2 more: type, rid='')
            assert!(sql.contains("OR"));
            assert_eq!(next, 7);
        }

        #[test]
        fn build_ladder_instance_three_disjuncts() {
            let role_table = quote_identifier("roles");
            let join_table = quote_identifier("users_roles");
            let (sql, next) = build_ladder_where(
                &role_table,
                &join_table,
                &placeholder(1),
                &RoleQuery {
                    name: &RoleName::from("moderator"),
                    filter: ResourceFilter::Instance("Forum", &ResourceId::from("42")),
                },
                2,
            );
            // Instance: 3 disjuncts = 3 + 2 + 2 = 7 binds after holder
            assert!(sql.matches("OR").count() == 2);
            assert_eq!(next, 9);
        }

        #[test]
        fn build_ladder_any_name_only() {
            let role_table = quote_identifier("roles");
            let join_table = quote_identifier("users_roles");
            let (sql, next) = build_ladder_where(
                &role_table,
                &join_table,
                &placeholder(1),
                &RoleQuery {
                    name: &RoleName::from("admin"),
                    filter: ResourceFilter::Any,
                },
                2,
            );
            // Any: name-only, 1 bind (the roles table is aliased `role_row`)
            assert_eq!(sql, format!("(role_row.name = {})", placeholder(2)));
            assert_eq!(next, 3);
        }

        #[test]
        fn select_role_by_triple_template() {
            let role_table = quote_identifier("roles");
            let sql = select_role_by_triple(&role_table);
            assert!(sql.contains(&format!("name = {}", placeholder(1))));
            assert!(sql.contains(&format!("resource_type = {}", placeholder(2))));
            assert!(sql.contains(&format!("resource_id = {}", placeholder(3))));
        }

        #[test]
        fn insert_link_template() {
            let join_table = quote_identifier("users_roles");
            let sql = insert_link(&join_table);
            assert_eq!(
                sql,
                format!(
                    "INSERT INTO {} (user_id, role_id) VALUES ({}, {})",
                    join_table,
                    placeholder(1),
                    placeholder(2)
                )
            );
        }

        #[test]
        fn delete_orphan_role_template() {
            let role_table = quote_identifier("roles");
            let join_table = quote_identifier("users_roles");
            let sql = delete_orphan_role(&role_table, &join_table);
            assert!(sql.contains("NOT EXISTS"));
            assert!(sql.contains(&format!("id = {}", placeholder(1))));
        }
    }
} // Close backend_sql module

// Re-export all SQL template functions when a backend feature is enabled.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub use backend_sql::*;
