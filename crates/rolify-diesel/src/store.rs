//! DieselStore — the sync `RoleStore` + `ResourceStore` implementation
//! for Diesel 2.3. Uses concrete connection types per backend feature;
//! pool checkouts and caller-owned transactions work via `DerefMut`.

use diesel::Connection;
use diesel::RunQueryDsl;
use diesel::sql_types::BigInt;
use diesel::sql_types::Text;
use maybe_async::maybe_async;
use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
use rolify_core::config::RolifyConfig;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::RoleQuery;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord, SCOPE_SENTINEL};
use rolify_core::store::{RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn, Sealed};

use crate::dialect::{placeholder, quote_identifier};
use crate::error::Error;
use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
use crate::sentinel::{resource_id_to_storage, to_storage};

/// Diesel sync store — holds only the configured table names.
///
/// One store per holder/role-table pair (D-07). Construct via
/// `DieselStore::new(&RolifyConfig)` (uses config's default table names) or
/// `DieselStore::with_tables(role_table, join_table)` for custom names.
///
/// The store is NOT generic — the connection type is fixed per backend feature:
/// - `postgres` feature: `Conn = PgConnection` (also works with r2d2 pool checkouts via `DerefMut`)
/// - `mysql` feature: `Conn = MysqlConnection`
/// - `sqlite` feature: `Conn = SqliteConnection`
#[derive(Debug, Clone)]
pub struct DieselStore {
    role_table: String,
    join_table: String,
}

impl DieselStore {
    /// Create a store from a validated `RolifyConfig`.
    #[must_use]
    pub fn new(config: &RolifyConfig) -> Self {
        Self {
            role_table: quote_identifier(&config.role_table()),
            join_table: quote_identifier(&config.join_table()),
        }
    }

    /// Create a store with explicit table names (for multi-pair setups).
    #[must_use]
    pub fn with_tables(role_table: &str, join_table: &str) -> Self {
        Self {
            role_table: quote_identifier(role_table),
            join_table: quote_identifier(join_table),
        }
    }

    #[must_use]
    pub fn role_table(&self) -> &str {
        &self.role_table
    }

    #[must_use]
    pub fn join_table(&self) -> &str {
        &self.join_table
    }

    fn scope_to_triple(&self, name: &RoleName, scope: ResourceRef<'_>) -> (String, String, String) {
        let (rt, rid) = match scope {
            ResourceRef::Global => (SCOPE_SENTINEL.to_owned(), SCOPE_SENTINEL.to_owned()),
            ResourceRef::Class(type_name) => (type_name.to_owned(), SCOPE_SENTINEL.to_owned()),
            ResourceRef::Instance(type_name, id) => (type_name.to_owned(), id.as_str().to_owned()),
        };
        (name.as_str().to_owned(), rt, rid)
    }
}

impl Sealed for DieselStore {}

// === Backend-specific impls ===

#[cfg(feature = "postgres")]
mod pg_impl {
    use super::*;
    use diesel::pg::PgConnection;

    // Helper functions for PgConnection
    fn find_or_create_by_triple(
        conn: &mut PgConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<RoleRecord, Error> {
        let q = diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        if let Ok(row) = q.get_result::<RoleRow>(conn) {
            return Ok(row.to_record());
        }

        let insert_sql = crate::sql::insert_role(role_table);
        let insert_q = diesel::sql_query(insert_sql)
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        match insert_q.execute(conn) {
            Ok(_) => {}
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => {}
            Err(other) => return Err(Error::Diesel(other)),
        }

        let q = diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        q.get_result::<RoleRow>(conn)
            .map(|row| row.to_record())
            .map_err(Error::Diesel)
    }

    fn get_role_id(
        conn: &mut PgConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<i64, Error> {
        let q = diesel::sql_query(crate::sql::select_role_id_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        q.get_result::<IdRow>(conn)
            .map(|row| row.id)
            .map_err(Error::Diesel)
    }

    // --- RoleStore ---

    #[maybe_async(AFIT)]
    impl RoleStore for DieselStore {
        type Conn = PgConnection;
        type Error = Error;

        fn where_(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            query: &RoleQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let (where_clause, _) = crate::sql::build_ladder_where(
                &self.role_table,
                &self.join_table,
                &placeholder(1),
                query,
                2,
            );
            let sql = crate::sql::select_roles_for_holder(&self.role_table, &self.join_table, &where_clause);
            let q = diesel::sql_query(sql).bind::<Text, _>(holder.as_str());
            let rows: Vec<RoleRow> = q.load(conn).map_err(Error::Diesel)?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        fn where_strict(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            query: &RoleQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let (where_clause, _) = crate::sql::build_strict_where(
                &self.role_table,
                &self.join_table,
                &placeholder(1),
                query,
                2,
            );
            let sql = crate::sql::select_roles_for_holder(&self.role_table, &self.join_table, &where_clause);
            let q = diesel::sql_query(sql).bind::<Text, _>(holder.as_str());
            let rows: Vec<RoleRow> = q.load(conn).map_err(Error::Diesel)?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        fn where_any(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            queries: &[RoleQuery<'_>],
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            if queries.is_empty() {
                return async { Ok(Vec::new()) };
            }
            let (where_clause, _) = crate::sql::build_any_where(
                &self.role_table,
                &self.join_table,
                &placeholder(1),
                queries,
                2,
            );
            let sql = crate::sql::select_roles_for_holder(&self.role_table, &self.join_table, &where_clause);
            let q = diesel::sql_query(sql).bind::<Text, _>(holder.as_str());
            let rows: Vec<RoleRow> = q.load(conn).map_err(Error::Diesel)?;
            async move {
                let mut seen = Vec::new();
                for row in rows {
                    let record = row.to_record();
                    if !seen.contains(&record) {
                        seen.push(record);
                    }
                }
                Ok(seen)
            }
        }

        fn find_or_create_by(
            &mut self,
            conn: &mut Self::Conn,
            name: &RoleName,
            scope: ResourceRef<'_>,
        ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send {
            let (name_owned, rt, rid) = self.scope_to_triple(name, scope);
            let name_ref = RoleName::new(name_owned);
            let result = find_or_create_by_triple(conn, &self.role_table, &name_ref, &rt, &rid);
            async move { result }
        }

        fn add(
            &mut self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            role: &RoleRecord,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str();
            let rt = to_storage(role.resource_type.as_deref());
            let rid = resource_id_to_storage(role.resource_id.as_ref());

            let role_record = match find_or_create_by_triple(conn, &self.role_table, &role.name, &rt, &rid) {
                Ok(r) => r,
                Err(e) => return async move { Err(e) },
            };

            let role_id = match get_role_id(
                conn,
                &self.role_table,
                &role_record.name,
                role_record.resource_type.as_ref().map(|s| s.as_str()).unwrap_or_default(),
                role_record.resource_id.as_ref().map(|r| r.as_str()).unwrap_or_default(),
            ) {
                Ok(id) => id,
                Err(e) => return async move { Err(e) },
            };

            let insert_sql = crate::sql::insert_link(&self.join_table);
            let q = diesel::sql_query(insert_sql)
                .bind::<Text, _>(holder_id)
                .bind::<BigInt, _>(role_id);
            match q.execute(conn) {
                Ok(_) => async move { Ok(true) },
                Err(diesel::result::Error::DatabaseError(
                    diesel::result::DatabaseErrorKind::UniqueViolation,
                    _,
                )) => async move { Ok(false) },
                Err(e) => async move { Err(Error::Diesel(e)) },
            }
        }

        fn remove(
            &mut self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            name: &RoleName,
            target: RemovalTarget<'_>,
            remove_role_if_empty: bool,
        ) -> impl Future<Output = Result<RemovalOutcome, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            let name_owned = name.as_str().to_owned();
            let target_owned = match target {
                RemovalTarget::NameOnly => RemovalTarget::NameOnly,
                RemovalTarget::TypeSweep(t) => RemovalTarget::TypeSweep(t),
                RemovalTarget::Exact(t, id) => RemovalTarget::Exact(t, &id.clone()),
            };
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();

            async move {
                conn.transaction::<_, Error, _>(|conn| {
                    let affected_sql = crate::sql::select_affected_roles(&role_table, &join_table, &target_owned);
                    let affected_rows: Vec<RoleRow> = match &target_owned {
                        RemovalTarget::NameOnly => {
                            let q = diesel::sql_query(affected_sql)
                                .bind::<Text, _>(&holder_id)
                                .bind::<Text, _>(&name_owned);
                            q.load(conn).map_err(Error::Diesel)?
                        }
                        RemovalTarget::TypeSweep(t) => {
                            let q = diesel::sql_query(affected_sql)
                                .bind::<Text, _>(&holder_id)
                                .bind::<Text, _>(&name_owned)
                                .bind::<Text, _>(t);
                            q.load(conn).map_err(Error::Diesel)?
                        }
                        RemovalTarget::Exact(t, id) => {
                            let q = diesel::sql_query(affected_sql)
                                .bind::<Text, _>(&holder_id)
                                .bind::<Text, _>(&name_owned)
                                .bind::<Text, _>(t)
                                .bind::<Text, _>(id.as_str());
                            q.load(conn).map_err(Error::Diesel)?
                        }
                    };
                    let affected_records: Vec<RoleRecord> = affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(&join_table, &role_table, &target_owned);
                    let removed_links = match &target_owned {
                        RemovalTarget::NameOnly => {
                            let q = diesel::sql_query(delete_sql)
                                .bind::<Text, _>(&holder_id)
                                .bind::<Text, _>(&name_owned);
                            q.execute(conn).map_err(Error::Diesel)?
                        }
                        RemovalTarget::TypeSweep(t) => {
                            let q = diesel::sql_query(delete_sql)
                                .bind::<Text, _>(&holder_id)
                                .bind::<Text, _>(&name_owned)
                                .bind::<Text, _>(t);
                            q.execute(conn).map_err(Error::Diesel)?
                        }
                        RemovalTarget::Exact(t, id) => {
                            let q = diesel::sql_query(delete_sql)
                                .bind::<Text, _>(&holder_id)
                                .bind::<Text, _>(&name_owned)
                                .bind::<Text, _>(t)
                                .bind::<Text, _>(id.as_str());
                            q.execute(conn).map_err(Error::Diesel)?
                        }
                    };

                    let mut removed_roles = Vec::new();
                    if remove_role_if_empty {
                        for role in &affected_records {
                            let role_id = get_role_id(
                                conn,
                                &role_table,
                                &role.name,
                                role.resource_type.as_ref().map(|s| s.as_str()).unwrap_or_default(),
                                role.resource_id.as_ref().map(|r| r.as_str()).unwrap_or_default(),
                            )?;
                            let sweep_sql = crate::sql::delete_orphan_role(&role_table, &join_table);
                            let deleted = diesel::sql_query(sweep_sql)
                                .bind::<BigInt, _>(role_id)
                                .execute(conn)
                                .map_err(Error::Diesel)?;
                            if deleted > 0 {
                                removed_roles.push(role.clone());
                            }
                        }
                    }

                    Ok(RemovalOutcome {
                        removed_links,
                        removed_roles,
                    })
                })
            }
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str();
            let (rt_cond, rid_cond) = match column {
                ScopeColumn::ResourceType => ("role_row.resource_type != ''", ""),
                ScopeColumn::ResourceId => ("", "AND role_row.resource_id != ''"),
            };
            let sql = format!(
                "SELECT 1 AS dummy FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = $1 AND {rt_cond} {rid_cond} LIMIT 1",
                role_table = self.role_table,
                join_table = self.join_table,
                rt_cond = rt_cond,
                rid_cond = rid_cond,
            );
            #[derive(diesel::deserialize::QueryableByName)]
            struct ExistsRow {
                #[diesel(sql_type = diesel::sql_types::Integer)]
                dummy: i32,
            }
            let q = diesel::sql_query(sql).bind::<Text, _>(holder_id);
            let found = q.get_result::<ExistsRow>(conn).is_ok();
            async move { Ok(found) }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let holder_id = holder.as_str();
            let sql = format!(
                "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = $1",
                role_table = self.role_table,
                join_table = self.join_table,
            );
            let q = diesel::sql_query(sql).bind::<Text, _>(holder_id);
            let rows: Vec<RoleRow> = q.load(conn).map_err(Error::Diesel)?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        fn holders_where(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
            query: &RoleQuery<'_>,
            strict: bool,
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            if holder_types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let (where_clause, _) = if strict {
                crate::sql::build_strict_where(
                    &self.role_table,
                    &self.join_table,
                    &placeholder(1),
                    query,
                    1 + holder_types.len(),
                )
            } else {
                crate::sql::build_ladder_where(
                    &self.role_table,
                    &self.join_table,
                    &placeholder(1),
                    query,
                    1 + holder_types.len(),
                )
            };

            let holder_table = quote_identifier("users");
            let type_placeholders: Vec<String> = (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT holder.id AS user_id \
                 FROM {holder_table} AS holder \
                 INNER JOIN {join_table} AS link ON link.user_id = holder.id \
                 INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                 WHERE {type_filter} AND {where_clause}",
                holder_table = holder_table,
                join_table = self.join_table,
                role_table = self.role_table,
                type_filter = type_filter,
                where_clause = where_clause,
            );

            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match holder_types.len() {
                0 => unreachable!(),
                1 => q.bind::<Text, _>(holder_types[0]).load(conn).map_err(Error::Diesel)?,
                2 => q.bind::<Text, _>(holder_types[0]).bind::<Text, _>(holder_types[1]).load(conn).map_err(Error::Diesel)?,
                3 => q.bind::<Text, _>(holder_types[0]).bind::<Text, _>(holder_types[1]).bind::<Text, _>(holder_types[2]).load(conn).map_err(Error::Diesel)?,
                4 => q.bind::<Text, _>(holder_types[0]).bind::<Text, _>(holder_types[1]).bind::<Text, _>(holder_types[2]).bind::<Text, _>(holder_types[3]).load(conn).map_err(Error::Diesel)?,
                _ => panic!("holders_where: too many holder types (max 4)"),
            };
            async move { Ok(rows.into_iter().map(|r| ResourceId::new(r.user_id)).collect()) }
        }

        fn all_holders(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            if holder_types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let holder_table = quote_identifier("users");
            let type_placeholders: Vec<String> = (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT id AS user_id FROM {holder_table} WHERE {type_filter}",
                holder_table = holder_table,
                type_filter = type_filter,
            );

            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match holder_types.len() {
                0 => unreachable!(),
                1 => q.bind::<Text, _>(holder_types[0]).load(conn).map_err(Error::Diesel)?,
                2 => q.bind::<Text, _>(holder_types[0]).bind::<Text, _>(holder_types[1]).load(conn).map_err(Error::Diesel)?,
                3 => q.bind::<Text, _>(holder_types[0]).bind::<Text, _>(holder_types[1]).bind::<Text, _>(holder_types[2]).load(conn).map_err(Error::Diesel)?,
                4 => q.bind::<Text, _>(holder_types[0]).bind::<Text, _>(holder_types[1]).bind::<Text, _>(holder_types[2]).bind::<Text, _>(holder_types[3]).load(conn).map_err(Error::Diesel)?,
                _ => panic!("all_holders: too many holder types (max 4)"),
            };
            async move { Ok(rows.into_iter().map(|r| ResourceId::new(r.user_id)).collect()) }
        }

        fn roles_matching(
            &self,
            conn: &mut Self::Conn,
            query: &RoleCatalogQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            if query.types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let has_holder = query.holder.is_some();
            let holder_table = query.holder.map(|_| quote_identifier("users"));

            let base_sql = crate::sql::select_roles_matching(&self.role_table, &self.join_table, holder_table.as_deref(), has_holder);

            let (type_filter, idx) = crate::sql::roles_matching_type_filter(query.types, 1);
            let (name_filter, idx) = if let Some(name) = query.name {
                crate::sql::roles_matching_name_filter(name, idx)
            } else {
                (String::new(), idx)
            };
            let (scope_filter, idx) = crate::sql::roles_matching_scope_filter(&query.scope, idx);
            let (holder_filter, _) = if let Some(holder) = query.holder {
                crate::sql::roles_matching_holder_filter(holder, idx)
            } else {
                (String::new(), idx)
            };

            let sql = base_sql
                .replace("{{type_filter}}", &type_filter)
                .replace("{{name_filter}}", &name_filter)
                .replace("{{scope_filter}}", &scope_filter)
                .replace("{{holder_filter}}", &holder_filter);

            let mut type_vals: Vec<&str> = query.types.iter().map(|s| *s).collect();
            if let Some(name) = query.name {
                type_vals.push(name.as_str());
            }
            match &query.scope {
                CatalogScope::ClassOnly => {
                    type_vals.push(SCOPE_SENTINEL);
                }
                CatalogScope::InstanceOnly { resource_id } => {
                    if let Some(rid) = resource_id {
                        type_vals.push(rid.as_str());
                    }
                }
                CatalogScope::ClassAndInstance => {}
            }
            if let Some(holder) = query.holder {
                type_vals.push(holder.as_str());
            }

            let q = diesel::sql_query(sql);
            let rows: Vec<ResourceKeyRow> = match type_vals.len() {
                0 => unreachable!(),
                1 => q.bind::<Text, _>(type_vals[0]).load(conn).map_err(Error::Diesel)?,
                2 => q.bind::<Text, _>(type_vals[0]).bind::<Text, _>(type_vals[1]).load(conn).map_err(Error::Diesel)?,
                3 => q.bind::<Text, _>(type_vals[0]).bind::<Text, _>(type_vals[1]).bind::<Text, _>(type_vals[2]).load(conn).map_err(Error::Diesel)?,
                4 => q.bind::<Text, _>(type_vals[0]).bind::<Text, _>(type_vals[1]).bind::<Text, _>(type_vals[2]).bind::<Text, _>(type_vals[3]).load(conn).map_err(Error::Diesel)?,
                5 => q.bind::<Text, _>(type_vals[0]).bind::<Text, _>(type_vals[1]).bind::<Text, _>(type_vals[2]).bind::<Text, _>(type_vals[3]).bind::<Text, _>(type_vals[4]).load(conn).map_err(Error::Diesel)?,
                6 => q.bind::<Text, _>(type_vals[0]).bind::<Text, _>(type_vals[1]).bind::<Text, _>(type_vals[2]).bind::<Text, _>(type_vals[3]).bind::<Text, _>(type_vals[4]).bind::<Text, _>(type_vals[5]).load(conn).map_err(Error::Diesel)?,
                7 => q.bind::<Text, _>(type_vals[0]).bind::<Text, _>(type_vals[1]).bind::<Text, _>(type_vals[2]).bind::<Text, _>(type_vals[3]).bind::<Text, _>(type_vals[4]).bind::<Text, _>(type_vals[5]).bind::<Text, _>(type_vals[6]).load(conn).map_err(Error::Diesel)?,
                8 => q.bind::<Text, _>(type_vals[0]).bind::<Text, _>(type_vals[1]).bind::<Text, _>(type_vals[2]).bind::<Text, _>(type_vals[3]).bind::<Text, _>(type_vals[4]).bind::<Text, _>(type_vals[5]).bind::<Text, _>(type_vals[6]).bind::<Text, _>(type_vals[7]).load(conn).map_err(Error::Diesel)?,
                _ => panic!("roles_matching: too many bind values (max 8)"),
            };
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }
    }

    // --- ResourceStore ---

    #[maybe_async(AFIT)]
    impl ResourceStore for DieselStore {
        type Conn = PgConnection;
        type Error = Error;

        fn resources_find(
            &self,
            conn: &mut Self::Conn,
            types: &[&str],
            name: &RoleName,
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            if types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let type_placeholders: Vec<String> = (1..1 + types.len()).map(placeholder).collect();
            let type_filter = format!("resource_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT resource_type, resource_id \
                 FROM {role_table} \
                 WHERE {type_filter} \
                   AND name = {} \
                   AND resource_type != ''",
                placeholder(type_placeholders.len() + 1),
                role_table = self.role_table,
                type_filter = type_filter,
            );

            let q = diesel::sql_query(sql);
            let rows: Vec<ResourceKeyRow> = match types.len() {
                0 => unreachable!(),
                1 => q.bind::<Text, _>(types[0]).bind::<Text, _>(name.as_str()).load(conn).map_err(Error::Diesel)?,
                2 => q.bind::<Text, _>(types[0]).bind::<Text, _>(types[1]).bind::<Text, _>(name.as_str()).load(conn).map_err(Error::Diesel)?,
                3 => q.bind::<Text, _>(types[0]).bind::<Text, _>(types[1]).bind::<Text, _>(types[2]).bind::<Text, _>(name.as_str()).load(conn).map_err(Error::Diesel)?,
                4 => q.bind::<Text, _>(types[0]).bind::<Text, _>(types[1]).bind::<Text, _>(types[2]).bind::<Text, _>(types[3]).bind::<Text, _>(name.as_str()).load(conn).map_err(Error::Diesel)?,
                _ => panic!("resources_find: too many types (max 4)"),
            };
            async move { Ok(rows.into_iter().map(|r| r.to_key()).collect()) }
        }

        fn in_list(
            &self,
            conn: &mut Self::Conn,
            candidates: &[ResourceKey],
            holder: &ResourceId,
            names: &[RoleName],
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            if candidates.is_empty() || names.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let candidate_pairs: Vec<String> = candidates
                .iter()
                .map(|c| format!("('{}', '{}')", c.resource_type, c.resource_id.as_str()))
                .collect();
            let candidate_filter = format!("(role_row.resource_type, role_row.resource_id) IN ({})", candidate_pairs.join(", "));

            let name_placeholders: Vec<String> = (2..2 + names.len()).map(placeholder).collect();
            let name_filter = format!("role_row.name IN ({})", name_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT role_row.resource_type, role_row.resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = $1 \
                   AND {name_filter} \
                   AND {candidate_filter}",
                role_table = self.role_table,
                join_table = self.join_table,
                name_filter = name_filter,
                candidate_filter = candidate_filter,
            );

            let q = diesel::sql_query(sql).bind::<Text, _>(holder.as_str());
            let rows: Vec<ResourceKeyRow> = match names.len() {
                0 => unreachable!(),
                1 => q.bind::<Text, _>(names[0].as_str()).load(conn).map_err(Error::Diesel)?,
                2 => q.bind::<Text, _>(names[0].as_str()).bind::<Text, _>(names[1].as_str()).load(conn).map_err(Error::Diesel)?,
                3 => q.bind::<Text, _>(names[0].as_str()).bind::<Text, _>(names[1].as_str()).bind::<Text, _>(names[2].as_str()).load(conn).map_err(Error::Diesel)?,
                4 => q.bind::<Text, _>(names[0].as_str()).bind::<Text, _>(names[1].as_str()).bind::<Text, _>(names[2].as_str()).bind::<Text, _>(names[3].as_str()).load(conn).map_err(Error::Diesel)?,
                _ => panic!("in_list: too many names (max 4)"),
            };
            async move { Ok(rows.into_iter().map(|r| r.to_key()).collect()) }
        }
    }
}

#[cfg(feature = "mysql")]
mod mysql_impl {
    use super::*;
    // TODO: implement for MySQL
}

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use super::*;
    // TODO: implement for SQLite
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{placeholder, quote_identifier};
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::role::{ResourceId, RoleName};

    #[cfg(feature = "postgres")]
    #[test]
    fn store_new_quotes_tables() {
        let config = RolifyConfig::builder().build().unwrap();
        let store = DieselStore::new(&config);
        assert_eq!(store.role_table(), "\"roles\"");
        assert_eq!(store.join_table(), "\"users_roles\"");
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn store_with_tables_quotes_custom() {
        let store = DieselStore::with_tables("custom_roles", "custom_join");
        assert_eq!(store.role_table(), "\"custom_roles\"");
        assert_eq!(store.join_table(), "\"custom_join\"");
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn scope_to_triple_global() {
        let config = RolifyConfig::builder().build().unwrap();
        let store = DieselStore::new(&config);
        let (name, rt, rid) = store.scope_to_triple(&RoleName::from("admin"), ResourceRef::Global);
        assert_eq!(name, "admin");
        assert_eq!(rt, "");
        assert_eq!(rid, "");
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn scope_to_triple_class() {
        let config = RolifyConfig::builder().build().unwrap();
        let store = DieselStore::new(&config);
        let (name, rt, rid) = store.scope_to_triple(&RoleName::from("manager"), ResourceRef::Class("Forum"));
        assert_eq!(name, "manager");
        assert_eq!(rt, "Forum");
        assert_eq!(rid, "");
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn scope_to_triple_instance() {
        let config = RolifyConfig::builder().build().unwrap();
        let store = DieselStore::new(&config);
        let (name, rt, rid) = store.scope_to_triple(&RoleName::from("moderator"), ResourceRef::Instance("Forum", &ResourceId::from("42")));
        assert_eq!(name, "moderator");
        assert_eq!(rt, "Forum");
        assert_eq!(rid, "42");
    }
}