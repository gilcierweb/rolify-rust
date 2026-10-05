//! `DieselStore`: the `RoleStore` + `ResourceStore` implementation for
//! Diesel 2.3 in both modes (D-08). Sync builds use the concrete sync
//! connection type per backend feature; the `async` feature reuses the
//! SAME store type and the SAME sql/rows/sentinel templates over
//! diesel-async connections (mirrored execution, Phase 3 D-06). Pool
//! checkouts and caller-owned transactions work via `DerefMut`.

use rolify_core::config::{RolifyConfig, RolifyConfigBuilder};
// The SPI value types below reach SQL only through the engine-gated impl
// blocks; the inert stub (no engine feature) never names them.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use rolify_core::resource::ResourceRef;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use rolify_core::role::{ResourceId, RoleName, SCOPE_SENTINEL};
use rolify_core::store::Sealed;

use crate::dialect::quote_identifier;

/// Diesel sync store — holds the configured table names and a resource registry.
///
/// One store per holder/role-table pair (D-07). Construct via
/// `DieselStore::new(&RolifyConfig)` (uses config's default table names) or
/// `DieselStore::with_tables(role_table, join_table)` for custom names.
/// Then chain `.for_holder_table("users")` to set the holder table (validated).
/// Register resource types with `.register_resource_table("Forum", "forums", "id")`
/// for class-scope expansion in `resources_find`.
///
/// The store is NOT generic — the connection type is fixed per backend feature:
/// - `postgres` feature: `Conn = PgConnection` (also works with r2d2 pool checkouts via `DerefMut`)
/// - `mysql` feature: `Conn = MysqlConnection`
/// - `sqlite` feature: `Conn = SqliteConnection`
#[derive(Debug, Clone)]
pub struct DieselStore {
    role_table: String,
    join_table: String,
    holder_table: Option<String>,
    resource_tables: Vec<(String, String, String)>, // (type_name, table_name, pk_column)
}

impl DieselStore {
    /// Create a store from a validated `RolifyConfig`.
    #[must_use]
    pub fn new(config: &RolifyConfig) -> Self {
        Self {
            role_table: quote_identifier(config.role_table()),
            join_table: quote_identifier(config.join_table()),
            holder_table: None,
            resource_tables: Vec::new(),
        }
    }

    /// Create a store with explicit table names (for multi-pair setups).
    #[must_use]
    pub fn with_tables(role_table: &str, join_table: &str) -> Self {
        Self {
            role_table: quote_identifier(role_table),
            join_table: quote_identifier(join_table),
            holder_table: None,
            resource_tables: Vec::new(),
        }
    }

    /// Set the holder table (e.g., "users", "customers") for `holders_where` / `all_holders`.
    /// Validates the identifier via `RolifyConfig`'s allow-list and quotes per engine.
    ///
    /// # Panics
    ///
    /// Panics when `holder_table` fails the identifier allow-list
    /// (`^[A-Za-z_][A-Za-z0-9_]*$`).
    #[must_use]
    pub fn for_holder_table(mut self, holder_table: &str) -> Self {
        // Validate via config's identifier validation (allow-list ^[A-Za-z_][A-Za-z0-9_]*$)
        RolifyConfigBuilder::validate_identifier(holder_table)
            .expect("holder table name must pass validation");
        self.holder_table = Some(quote_identifier(holder_table));
        self
    }

    /// Register a resource type for class-scope expansion in `resources_find`.
    /// `type_name` is the STI type (e.g., "Forum"), `table_name` is the DB table (e.g., "forums"),
    /// `pk_column` is the primary key column (e.g., "id" or "`team_code`").
    ///
    /// # Panics
    ///
    /// Panics when any of `type_name`, `table_name`, or `pk_column`
    /// fails the identifier allow-list (`^[A-Za-z_][A-Za-z0-9_]*$`).
    #[must_use]
    pub fn register_resource_table(
        mut self,
        type_name: &str,
        table_name: &str,
        pk_column: &str,
    ) -> Self {
        // Validate identifiers using the same allow-list as config
        RolifyConfigBuilder::validate_identifier(table_name)
            .expect("resource table name must pass validation");
        RolifyConfigBuilder::validate_identifier(pk_column)
            .expect("pk column name must pass validation");
        RolifyConfigBuilder::validate_identifier(type_name)
            .expect("type name must pass validation");

        self.resource_tables.push((
            type_name.to_string(),
            quote_identifier(table_name),
            pk_column.to_owned(),
        ));
        self
    }

    #[must_use]
    pub fn role_table(&self) -> &str {
        &self.role_table
    }

    #[must_use]
    pub fn join_table(&self) -> &str {
        &self.join_table
    }

    #[must_use]
    pub fn holder_table(&self) -> Option<&str> {
        self.holder_table.as_deref()
    }

    #[must_use]
    pub fn resource_tables(&self) -> &[(String, String, String)] {
        &self.resource_tables
    }

    // Both helpers below serve the engine-gated impl blocks only (sync and
    // async alike); the inert stub compiles them out.
    #[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
    #[allow(clippy::unused_self)]
    fn scope_to_triple(&self, name: &RoleName, scope: ResourceRef<'_>) -> (String, String, String) {
        let (rt, rid) = match scope {
            ResourceRef::Global => (SCOPE_SENTINEL.to_owned(), SCOPE_SENTINEL.to_owned()),
            ResourceRef::Class(type_name) => (type_name.to_string(), SCOPE_SENTINEL.to_owned()),
            ResourceRef::Instance(type_name, id) => (type_name.to_string(), id.as_str().to_owned()),
        };
        (name.as_str().to_owned(), rt, rid)
    }

    /// Build the holder table reference for SQL, defaulting to "users" if not set.
    #[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
    fn holder_table_sql(&self) -> String {
        self.holder_table
            .clone()
            .unwrap_or_else(|| quote_identifier("users"))
    }
}

impl Sealed for DieselStore {}

// === Backend-specific impls ===
//
// The sync impls compile only when the `sync` mode feature is on (D-08
// engines are mode-agnostic); the async rider lives in cfg-gated
// sibling modules that reuse the same sql/rows/sentinel templates.

#[cfg(all(feature = "postgres", feature = "sync"))]
mod pg_impl {
    use super::*;
    use diesel::Connection;
    use diesel::RunQueryDsl;
    use diesel::pg::PgConnection;
    use diesel::sql_types::{BigInt, Text};
    use maybe_async::maybe_async;
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::RoleQuery;
    use rolify_core::role::RoleRecord;
    use rolify_core::store::{RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn};

    use crate::dialect::placeholder;
    use crate::error::Error;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            let rows: Vec<RoleRow> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .bind::<Text, _>(type_name)
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>(resource_id.as_str())
                        .load(conn)
                        .map_err(Error::Diesel)?
                }
                rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .load(conn)
                    .map_err(Error::Diesel)?,
            };
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            let rows: Vec<RoleRow> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>(type_name)
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>(resource_id.as_str())
                        .load(conn)
                        .map_err(Error::Diesel)?
                }
                rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .load(conn)
                    .map_err(Error::Diesel)?,
            };
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            // Collect all bind values in order, then match on query count for typed binds
            let mut all_values: Vec<String> = Vec::new();
            all_values.push(holder.as_str().to_owned());
            for query in queries {
                let name = query.name.as_str();
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            }
            let q = diesel::sql_query(sql);
            let rows: Vec<RoleRow> = match all_values.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(&all_values[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                10 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                11 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                12 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                13 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                14 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                15 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                16 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                17 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                18 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                19 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                20 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                // The ported suite batches up to four queries (l.68:
                // Instance + Global + Instance + Class = 23 binds).
                21 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                22 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .bind::<Text, _>(&all_values[21])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                23 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .bind::<Text, _>(&all_values[21])
                    .bind::<Text, _>(&all_values[22])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("where_any: too many bind values (max 23)"),
            };
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

            let role_record =
                match find_or_create_by_triple(conn, &self.role_table, &role.name, &rt, &rid) {
                    Ok(r) => r,
                    Err(e) => return async move { Err(e) },
                };

            let role_id = match get_role_id(
                conn,
                &self.role_table,
                &role_record.name,
                role_record
                    .resource_type
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or_default(),
                role_record
                    .resource_id
                    .as_ref()
                    .map(|r| r.as_str())
                    .unwrap_or_default(),
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
                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &target_owned);
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
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &target_owned,
                    );
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
                                role.resource_type
                                    .as_ref()
                                    .map(|s| s.as_str())
                                    .unwrap_or_default(),
                                role.resource_id
                                    .as_ref()
                                    .map(|r| r.as_str())
                                    .unwrap_or_default(),
                            )?;
                            let sweep_sql =
                                crate::sql::delete_orphan_role(&role_table, &join_table);
                            let deleted = diesel::sql_query(sweep_sql)
                                .bind::<BigInt, _>(role_id)
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

        fn remove_roles_for_scope(
            &mut self,
            conn: &mut Self::Conn,
            resource_type: &str,
            resource_id: &ResourceId,
        ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
            let rt = resource_type.to_owned();
            let rid = resource_id.as_str().to_owned();
            let role_table = self.role_table.clone();

            async move {
                let sql = crate::sql::delete_roles_by_scope(&role_table);
                let deleted = diesel::sql_query(sql)
                    .bind::<Text, _>(rt)
                    .bind::<Text, _>(rid)
                    .execute(conn)
                    .map_err(Error::Diesel)?;
                Ok(deleted)
            }
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str();
            // One complete condition per column: interpolating an empty
            // half would emit `AND  AND` (a syntax error the `is_ok`
            // below would swallow into a wrong `false`).
            let scope_cond = match column {
                ScopeColumn::ResourceType => "role_row.resource_type != ''",
                ScopeColumn::ResourceId => "role_row.resource_id != ''",
            };
            let sql = format!(
                "SELECT 1 AS dummy FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {holder_ph} AND {scope_cond} LIMIT 1",
                role_table = self.role_table,
                join_table = self.join_table,
                scope_cond = scope_cond,
                holder_ph = placeholder(1),
            );
            #[derive(diesel::deserialize::QueryableByName)]
            #[allow(dead_code)]
            struct ExistsRow {
                // Named exactly like the `AS dummy` projection: by-name
                // decoding matches on it (an underscore prefix broke the
                // match and made `exists` always false).
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
                 WHERE link.user_id = {holder_ph}",
                role_table = self.role_table,
                join_table = self.join_table,
                holder_ph = placeholder(1),
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

            let holder_table = self.holder_table_sql();
            let type_placeholders: Vec<String> =
                (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT CAST(holder.id AS TEXT) AS user_id \
                 FROM {holder_table} AS holder \
                 INNER JOIN {join_table} AS link ON link.user_id = CAST(holder.id AS TEXT) \
                 INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                 WHERE {type_filter} AND {where_clause}",
                holder_table = holder_table,
                join_table = self.join_table,
                role_table = self.role_table,
                type_filter = type_filter,
                where_clause = where_clause,
            );

            // Binds are positional: the holder types first (placeholders
            // $1..$N), then the ladder values continuing the same
            // sequence. Ladder shapes mirror `where_` / `where_strict`.
            // Values collect owned (the `where_any` precedent): the bind
            // chain is type-level, so the count match below issues the
            // exact static chain per total.
            let mut all_values: Vec<String> = Vec::new();
            for holder_type in holder_types {
                all_values.push((*holder_type).to_owned());
            }
            let name = query.name.as_str();
            if strict {
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            } else {
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            }
            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match all_values.len() {
                2 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                10 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                11 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("holders_where: too many holder types (max 4)"),
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        fn all_holders(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            if holder_types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let holder_table = self.holder_table_sql();
            let type_placeholders: Vec<String> =
                (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT CAST(id AS TEXT) AS user_id FROM {holder_table} WHERE {type_filter}",
                holder_table = holder_table,
                type_filter = type_filter,
            );

            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match holder_types.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(holder_types[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .bind::<Text, _>(holder_types[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .bind::<Text, _>(holder_types[2])
                    .bind::<Text, _>(holder_types[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("all_holders: too many holder types (max 4)"),
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
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
            let holder_table = query.holder.map(|_| self.holder_table_sql());

            let base_sql = crate::sql::select_roles_matching(
                &self.role_table,
                &self.join_table,
                holder_table.as_deref(),
                has_holder,
            );

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
                .replace("{type_filter}", &type_filter)
                .replace("{name_filter}", &name_filter)
                .replace("{scope_filter}", &scope_filter)
                .replace("{holder_filter}", &holder_filter);

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
                1 => q
                    .bind::<Text, _>(type_vals[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .bind::<Text, _>(type_vals[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .bind::<Text, _>(type_vals[6])
                    .bind::<Text, _>(type_vals[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
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

            let mut all_keys: Vec<ResourceKey> = Vec::new();

            // 1. Instance-scoped roles: direct query on roles table
            let type_placeholders: Vec<String> = (1..1 + types.len()).map(placeholder).collect();
            let type_filter = format!("resource_type IN ({})", type_placeholders.join(", "));
            let name_ph = placeholder(type_placeholders.len() + 1);

            let sql = format!(
                "SELECT DISTINCT name AS name, resource_type, resource_id \
                 FROM {role_table} \
                 WHERE {type_filter} \
                   AND name = {name_ph} \
                   AND resource_id != ''",
                role_table = self.role_table,
                type_filter = type_filter,
                name_ph = name_ph,
            );

            let instance_rows: Vec<ResourceKeyRow> = match types.len() {
                1 => diesel::sql_query(&sql)
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(&sql)
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(&sql)
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(types[2])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(&sql)
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(types[2])
                    .bind::<Text, _>(types[3])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("resources_find: too many types (max 4)"),
            };
            for row in instance_rows {
                all_keys.push(row.to_key());
            }

            // 2. Class-scoped roles: expand via resource registry
            for (type_name, resource_table, pk_column) in &self.resource_tables {
                if types.contains(&type_name.as_str()) {
                    let name_ph = placeholder(1);
                    let expansion_sql = crate::sql::select_resources_find_class_expansion(
                        &self.role_table,
                        resource_table,
                        pk_column,
                        type_name,
                        &name_ph,
                    );
                    let q = diesel::sql_query(expansion_sql).bind::<Text, _>(name.as_str());
                    let class_rows: Vec<ResourceKeyRow> = q.load(conn).map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
            }

            async move { Ok(all_keys) }
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

            // The holder's rows for the requested names; coverage is
            // decided below in Rust, mirroring `InMemoryStore::in_list`
            // exactly: a candidate is covered by a same-id row or by a
            // scopeless (global/class) row, with no resource-type check.
            // Candidate keys (not row keys) are returned, so scopeless
            // covering rows never reach `to_key`.
            let name_placeholders: Vec<String> = (2..2 + names.len()).map(placeholder).collect();
            let name_filter = format!("role_row.name IN ({})", name_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT role_row.name AS name, role_row.resource_type, role_row.resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {} \
                   AND {name_filter}",
                placeholder(1),
                role_table = self.role_table,
                join_table = self.join_table,
                name_filter = name_filter,
            );

            // Collect bind values in order: holder, then names.
            let mut all_values: Vec<&str> = Vec::new();
            all_values.push(holder.as_str());
            for name in names {
                all_values.push(name.as_str());
            }

            let rows: Vec<RoleRow> = match all_values.len() {
                2 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .bind::<Text, _>(all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => diesel::sql_query(&sql)
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .bind::<Text, _>(all_values[7])
                    .bind::<Text, _>(all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("in_list: too many names (max 8)"),
            };
            let records: Vec<RoleRecord> = rows.iter().map(RoleRow::to_record).collect();
            let mut covered: Vec<ResourceKey> = Vec::new();
            for key in candidates {
                let matches = records.iter().any(|row| {
                    names.contains(&row.name)
                        && (row.resource_id.is_none()
                            || row.resource_id.as_ref() == Some(&key.resource_id))
                });
                if matches && !covered.contains(key) {
                    covered.push(key.clone());
                }
            }
            async move { Ok(covered) }
        }
    }
}

#[cfg(all(feature = "mysql", feature = "sync"))]
mod mysql_impl {
    use super::*;
    use diesel::Connection;
    use diesel::RunQueryDsl;
    use diesel::mysql::MysqlConnection;
    use diesel::sql_types::{BigInt, Text};
    use maybe_async::maybe_async;
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::RoleQuery;
    use rolify_core::role::RoleRecord;
    use rolify_core::store::{RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn};

    use crate::dialect::placeholder;
    use crate::error::Error;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // MySQL implementation
    fn find_or_create_by_triple(
        conn: &mut MysqlConnection,
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
        conn: &mut MysqlConnection,
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

    #[maybe_async(AFIT)]
    impl RoleStore for DieselStore {
        type Conn = MysqlConnection;
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            let rows: Vec<RoleRow> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .bind::<Text, _>(type_name)
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>(resource_id.as_str())
                        .load(conn)
                        .map_err(Error::Diesel)?
                }
                rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .load(conn)
                    .map_err(Error::Diesel)?,
            };
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            let rows: Vec<RoleRow> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>(type_name)
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>(resource_id.as_str())
                        .load(conn)
                        .map_err(Error::Diesel)?
                }
                rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .load(conn)
                    .map_err(Error::Diesel)?,
            };
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            // Collect all bind values in order, then match on query count for typed binds
            let mut all_values: Vec<String> = Vec::new();
            all_values.push(holder.as_str().to_owned());
            for query in queries {
                let name = query.name.as_str();
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            }
            let q = diesel::sql_query(sql);
            let rows: Vec<RoleRow> = match all_values.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(&all_values[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                10 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                11 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                12 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                13 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                14 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                15 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                16 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                17 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                18 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                19 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                20 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                // The ported suite batches up to four queries (l.68:
                // Instance + Global + Instance + Class = 23 binds).
                21 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                22 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .bind::<Text, _>(&all_values[21])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                23 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .bind::<Text, _>(&all_values[21])
                    .bind::<Text, _>(&all_values[22])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("where_any: too many bind values (max 23)"),
            };
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

            let role_record =
                match find_or_create_by_triple(conn, &self.role_table, &role.name, &rt, &rid) {
                    Ok(r) => r,
                    Err(e) => return async move { Err(e) },
                };

            let role_id = match get_role_id(
                conn,
                &self.role_table,
                &role_record.name,
                role_record
                    .resource_type
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or_default(),
                role_record
                    .resource_id
                    .as_ref()
                    .map(|r| r.as_str())
                    .unwrap_or_default(),
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
                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &target_owned);
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
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &target_owned,
                    );
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
                                role.resource_type
                                    .as_ref()
                                    .map(|s| s.as_str())
                                    .unwrap_or_default(),
                                role.resource_id
                                    .as_ref()
                                    .map(|r| r.as_str())
                                    .unwrap_or_default(),
                            )?;
                            let sweep_sql =
                                crate::sql::delete_orphan_role(&role_table, &join_table);
                            let deleted = diesel::sql_query(sweep_sql)
                                .bind::<BigInt, _>(role_id)
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

        fn remove_roles_for_scope(
            &mut self,
            conn: &mut Self::Conn,
            resource_type: &str,
            resource_id: &ResourceId,
        ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
            let rt = resource_type.to_owned();
            let rid = resource_id.as_str().to_owned();
            let role_table = self.role_table.clone();

            async move {
                let sql = crate::sql::delete_roles_by_scope(&role_table);
                let deleted = diesel::sql_query(sql)
                    .bind::<Text, _>(rt)
                    .bind::<Text, _>(rid)
                    .execute(conn)
                    .map_err(Error::Diesel)?;
                Ok(deleted)
            }
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str();
            // One complete condition per column: interpolating an empty
            // half would emit `AND  AND` (a syntax error the `is_ok`
            // below would swallow into a wrong `false`).
            let scope_cond = match column {
                ScopeColumn::ResourceType => "role_row.resource_type != ''",
                ScopeColumn::ResourceId => "role_row.resource_id != ''",
            };
            let sql = format!(
                "SELECT 1 AS dummy FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ? AND {scope_cond} LIMIT 1",
                role_table = self.role_table,
                join_table = self.join_table,
                scope_cond = scope_cond,
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
                 WHERE link.user_id = ?",
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

            let holder_table = self.holder_table_sql();
            let type_placeholders: Vec<String> =
                (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT CAST(holder.id AS CHAR) AS user_id \
                 FROM {holder_table} AS holder \
                 INNER JOIN {join_table} AS link ON link.user_id = CAST(holder.id AS CHAR) \
                 INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                 WHERE {type_filter} AND {where_clause}",
                holder_table = holder_table,
                join_table = self.join_table,
                role_table = self.role_table,
                type_filter = type_filter,
                where_clause = where_clause,
            );

            // Binds are positional: the holder types first (placeholders
            // $1..$N), then the ladder values continuing the same
            // sequence. Ladder shapes mirror `where_` / `where_strict`.
            // Values collect owned (the `where_any` precedent): the bind
            // chain is type-level, so the count match below issues the
            // exact static chain per total.
            let mut all_values: Vec<String> = Vec::new();
            for holder_type in holder_types {
                all_values.push((*holder_type).to_owned());
            }
            let name = query.name.as_str();
            if strict {
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            } else {
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            }
            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match all_values.len() {
                2 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                10 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                11 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("holders_where: too many holder types (max 4)"),
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        fn all_holders(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            if holder_types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let holder_table = self.holder_table_sql();
            let type_placeholders: Vec<String> =
                (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT CAST(id AS CHAR) AS user_id FROM {holder_table} WHERE {type_filter}",
                holder_table = holder_table,
                type_filter = type_filter,
            );

            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match holder_types.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(holder_types[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .bind::<Text, _>(holder_types[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .bind::<Text, _>(holder_types[2])
                    .bind::<Text, _>(holder_types[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("all_holders: too many holder types (max 4)"),
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
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
            let holder_table = query.holder.map(|_| self.holder_table_sql());

            let base_sql = crate::sql::select_roles_matching(
                &self.role_table,
                &self.join_table,
                holder_table.as_deref(),
                has_holder,
            );

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
                .replace("{type_filter}", &type_filter)
                .replace("{name_filter}", &name_filter)
                .replace("{scope_filter}", &scope_filter)
                .replace("{holder_filter}", &holder_filter);

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
                1 => q
                    .bind::<Text, _>(type_vals[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .bind::<Text, _>(type_vals[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .bind::<Text, _>(type_vals[6])
                    .bind::<Text, _>(type_vals[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("roles_matching: too many bind values (max 8)"),
            };
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }
    }

    #[maybe_async(AFIT)]
    impl ResourceStore for DieselStore {
        type Conn = MysqlConnection;
        type Error = Error;

        // MySQL-specific implementation
        fn resources_find(
            &self,
            conn: &mut Self::Conn,
            types: &[&str],
            name: &RoleName,
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            if types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let mut all_keys: Vec<ResourceKey> = Vec::new();

            // 1. Instance-scoped roles: direct query on roles table
            let type_placeholders: Vec<String> = (1..1 + types.len()).map(placeholder).collect();
            let type_filter = format!("resource_type IN ({})", type_placeholders.join(", "));
            let name_ph = placeholder(type_placeholders.len() + 1);

            let sql = format!(
                "SELECT DISTINCT name AS name, resource_type, resource_id \
                 FROM {role_table} \
                 WHERE {type_filter} \
                   AND name = {name_ph} \
                   AND resource_id != ''",
                role_table = self.role_table,
                type_filter = type_filter,
                name_ph = name_ph,
            );

            let q = diesel::sql_query(sql);
            let instance_rows: Vec<ResourceKeyRow> = match types.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(types[2])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(types[2])
                    .bind::<Text, _>(types[3])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("resources_find: too many types (max 4)"),
            };
            for row in instance_rows {
                all_keys.push(row.to_key());
            }

            // 2. Class-scoped roles: expand via resource registry
            for (type_name, resource_table, pk_column) in &self.resource_tables {
                if types.contains(&type_name.as_str()) {
                    let name_ph = placeholder(1);
                    let expansion_sql = crate::sql::select_resources_find_class_expansion(
                        &self.role_table,
                        resource_table,
                        pk_column,
                        type_name,
                        &name_ph,
                    );
                    let q = diesel::sql_query(expansion_sql).bind::<Text, _>(name.as_str());
                    let class_rows: Vec<ResourceKeyRow> = q.load(conn).map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
            }

            async move { Ok(all_keys) }
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

            // The holder's rows for the requested names; coverage is
            // decided below in Rust, mirroring `InMemoryStore::in_list`
            // exactly: a candidate is covered by a same-id row or by a
            // scopeless (global/class) row, with no resource-type check.
            // Candidate keys (not row keys) are returned, so scopeless
            // covering rows never reach `to_key`.
            let name_placeholders: Vec<String> = (2..2 + names.len()).map(placeholder).collect();
            let name_filter = format!("role_row.name IN ({})", name_placeholders.join(", "));
            let sql = format!(
                "SELECT DISTINCT role_row.name AS name, role_row.resource_type, role_row.resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ? \
                   AND {name_filter}",
                role_table = self.role_table,
                join_table = self.join_table,
                name_filter = name_filter,
            );

            let mut all_values: Vec<&str> = Vec::new();
            all_values.push(holder.as_str());
            for name in names {
                all_values.push(name.as_str());
            }

            let q = diesel::sql_query(sql);
            let rows: Vec<RoleRow> = match all_values.len() {
                2 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .bind::<Text, _>(all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .bind::<Text, _>(all_values[7])
                    .bind::<Text, _>(all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("in_list: too many names (max 8)"),
            };
            let records: Vec<RoleRecord> = rows.iter().map(RoleRow::to_record).collect();
            let mut covered: Vec<ResourceKey> = Vec::new();
            for key in candidates {
                let matches = records.iter().any(|row| {
                    names.contains(&row.name)
                        && (row.resource_id.is_none()
                            || row.resource_id.as_ref() == Some(&key.resource_id))
                });
                if matches && !covered.contains(key) {
                    covered.push(key.clone());
                }
            }
            async move { Ok(covered) }
        }
    }
}

#[cfg(all(feature = "sqlite", feature = "sync"))]
mod sqlite_impl {
    use super::*;
    use diesel::Connection;
    use diesel::RunQueryDsl;
    use diesel::sql_types::{BigInt, Text};
    use diesel::sqlite::SqliteConnection;
    use maybe_async::maybe_async;
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::RoleQuery;
    use rolify_core::role::RoleRecord;
    use rolify_core::store::{RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn};

    use crate::dialect::placeholder;
    use crate::error::Error;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // SQLite implementation
    fn find_or_create_by_triple(
        conn: &mut SqliteConnection,
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
        conn: &mut SqliteConnection,
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

    #[maybe_async(AFIT)]
    impl RoleStore for DieselStore {
        type Conn = SqliteConnection;
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            let rows: Vec<RoleRow> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .bind::<Text, _>(type_name)
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>(resource_id.as_str())
                        .load(conn)
                        .map_err(Error::Diesel)?
                }
                rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .load(conn)
                    .map_err(Error::Diesel)?,
            };
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            let rows: Vec<RoleRow> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>("")
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .bind::<Text, _>(type_name)
                    .bind::<Text, _>("")
                    .load(conn)
                    .map_err(Error::Diesel)?,
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>(resource_id.as_str())
                        .load(conn)
                        .map_err(Error::Diesel)?
                }
                rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                    .bind::<Text, _>(holder.as_str())
                    .bind::<Text, _>(name)
                    .load(conn)
                    .map_err(Error::Diesel)?,
            };
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            // Collect all bind values in order, then match on query count for typed binds
            let mut all_values: Vec<String> = Vec::new();
            all_values.push(holder.as_str().to_owned());
            for query in queries {
                let name = query.name.as_str();
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push("".to_owned());
                        all_values.push(type_name.to_string());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            }
            let q = diesel::sql_query(sql);
            let rows: Vec<RoleRow> = match all_values.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(&all_values[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                10 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                11 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                12 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                13 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                14 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                15 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                16 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                17 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                18 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                19 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                20 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                // The ported suite batches up to four queries (l.68:
                // Instance + Global + Instance + Class = 23 binds).
                21 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                22 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .bind::<Text, _>(&all_values[21])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                23 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .bind::<Text, _>(&all_values[11])
                    .bind::<Text, _>(&all_values[12])
                    .bind::<Text, _>(&all_values[13])
                    .bind::<Text, _>(&all_values[14])
                    .bind::<Text, _>(&all_values[15])
                    .bind::<Text, _>(&all_values[16])
                    .bind::<Text, _>(&all_values[17])
                    .bind::<Text, _>(&all_values[18])
                    .bind::<Text, _>(&all_values[19])
                    .bind::<Text, _>(&all_values[20])
                    .bind::<Text, _>(&all_values[21])
                    .bind::<Text, _>(&all_values[22])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("where_any: too many bind values (max 23)"),
            };
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

            let role_record =
                match find_or_create_by_triple(conn, &self.role_table, &role.name, &rt, &rid) {
                    Ok(r) => r,
                    Err(e) => return async move { Err(e) },
                };

            let role_id = match get_role_id(
                conn,
                &self.role_table,
                &role_record.name,
                role_record
                    .resource_type
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or_default(),
                role_record
                    .resource_id
                    .as_ref()
                    .map(|r| r.as_str())
                    .unwrap_or_default(),
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
                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &target_owned);
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
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &target_owned,
                    );
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
                                role.resource_type
                                    .as_ref()
                                    .map(|s| s.as_str())
                                    .unwrap_or_default(),
                                role.resource_id
                                    .as_ref()
                                    .map(|r| r.as_str())
                                    .unwrap_or_default(),
                            )?;
                            let sweep_sql =
                                crate::sql::delete_orphan_role(&role_table, &join_table);
                            let deleted = diesel::sql_query(sweep_sql)
                                .bind::<BigInt, _>(role_id)
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

        fn remove_roles_for_scope(
            &mut self,
            conn: &mut Self::Conn,
            resource_type: &str,
            resource_id: &ResourceId,
        ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
            let rt = resource_type.to_owned();
            let rid = resource_id.as_str().to_owned();
            let role_table = self.role_table.clone();

            async move {
                let sql = crate::sql::delete_roles_by_scope(&role_table);
                let deleted = diesel::sql_query(sql)
                    .bind::<Text, _>(rt)
                    .bind::<Text, _>(rid)
                    .execute(conn)
                    .map_err(Error::Diesel)?;
                Ok(deleted)
            }
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str();
            // One complete condition per column: interpolating an empty
            // half would emit `AND  AND` (a syntax error the `is_ok`
            // below would swallow into a wrong `false`).
            let scope_cond = match column {
                ScopeColumn::ResourceType => "role_row.resource_type != ''",
                ScopeColumn::ResourceId => "role_row.resource_id != ''",
            };
            let sql = format!(
                "SELECT 1 AS dummy FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ? AND {scope_cond} LIMIT 1",
                role_table = self.role_table,
                join_table = self.join_table,
                scope_cond = scope_cond,
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
                 WHERE link.user_id = ?",
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

            let holder_table = self.holder_table_sql();
            let type_placeholders: Vec<String> =
                (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT CAST(holder.id AS TEXT) AS user_id \
                 FROM {holder_table} AS holder \
                 INNER JOIN {join_table} AS link ON link.user_id = CAST(holder.id AS TEXT) \
                 INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                 WHERE {type_filter} AND {where_clause}",
                holder_table = holder_table,
                join_table = self.join_table,
                role_table = self.role_table,
                type_filter = type_filter,
                where_clause = where_clause,
            );

            // Binds are positional: the holder types first (placeholders
            // $1..$N), then the ladder values continuing the same
            // sequence. Ladder shapes mirror `where_` / `where_strict`.
            // Values collect owned (the `where_any` precedent): the bind
            // chain is type-level, so the count match below issues the
            // exact static chain per total.
            let mut all_values: Vec<String> = Vec::new();
            for holder_type in holder_types {
                all_values.push((*holder_type).to_owned());
            }
            let name = query.name.as_str();
            if strict {
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            } else {
                match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        all_values.push(name.to_owned());
                        all_values.push("".to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push("".to_owned());
                        all_values.push((*type_name).to_owned());
                        all_values.push(resource_id.as_str().to_owned());
                    }
                    rolify_core::query::ResourceFilter::Any => {
                        all_values.push(name.to_owned());
                    }
                }
            }
            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match all_values.len() {
                2 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                10 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                11 => q
                    .bind::<Text, _>(&all_values[0])
                    .bind::<Text, _>(&all_values[1])
                    .bind::<Text, _>(&all_values[2])
                    .bind::<Text, _>(&all_values[3])
                    .bind::<Text, _>(&all_values[4])
                    .bind::<Text, _>(&all_values[5])
                    .bind::<Text, _>(&all_values[6])
                    .bind::<Text, _>(&all_values[7])
                    .bind::<Text, _>(&all_values[8])
                    .bind::<Text, _>(&all_values[9])
                    .bind::<Text, _>(&all_values[10])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("holders_where: too many holder types (max 4)"),
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        fn all_holders(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            if holder_types.is_empty() {
                return async { Ok(Vec::new()) };
            }

            let holder_table = self.holder_table_sql();
            let type_placeholders: Vec<String> =
                (1..1 + holder_types.len()).map(placeholder).collect();
            let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT CAST(id AS TEXT) AS user_id FROM {holder_table} WHERE {type_filter}",
                holder_table = holder_table,
                type_filter = type_filter,
            );

            let q = diesel::sql_query(sql);
            let rows: Vec<HolderIdRow> = match holder_types.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(holder_types[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .bind::<Text, _>(holder_types[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(holder_types[0])
                    .bind::<Text, _>(holder_types[1])
                    .bind::<Text, _>(holder_types[2])
                    .bind::<Text, _>(holder_types[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("all_holders: too many holder types (max 4)"),
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
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
            let holder_table = query.holder.map(|_| self.holder_table_sql());

            let base_sql = crate::sql::select_roles_matching(
                &self.role_table,
                &self.join_table,
                holder_table.as_deref(),
                has_holder,
            );

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
                .replace("{type_filter}", &type_filter)
                .replace("{name_filter}", &name_filter)
                .replace("{scope_filter}", &scope_filter)
                .replace("{holder_filter}", &holder_filter);

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
                1 => q
                    .bind::<Text, _>(type_vals[0])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .bind::<Text, _>(type_vals[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(type_vals[0])
                    .bind::<Text, _>(type_vals[1])
                    .bind::<Text, _>(type_vals[2])
                    .bind::<Text, _>(type_vals[3])
                    .bind::<Text, _>(type_vals[4])
                    .bind::<Text, _>(type_vals[5])
                    .bind::<Text, _>(type_vals[6])
                    .bind::<Text, _>(type_vals[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("roles_matching: too many bind values (max 8)"),
            };
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }
    }

    #[maybe_async(AFIT)]
    impl ResourceStore for DieselStore {
        type Conn = SqliteConnection;
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

            let mut all_keys: Vec<ResourceKey> = Vec::new();

            // 1. Instance-scoped roles: direct query on roles table
            let type_placeholders: Vec<String> = (1..1 + types.len()).map(placeholder).collect();
            let type_filter = format!("resource_type IN ({})", type_placeholders.join(", "));
            let name_ph = placeholder(type_placeholders.len() + 1);

            let sql = format!(
                "SELECT DISTINCT name AS name, resource_type, resource_id \
                 FROM {role_table} \
                 WHERE {type_filter} \
                   AND name = {name_ph} \
                   AND resource_id != ''",
                role_table = self.role_table,
                type_filter = type_filter,
                name_ph = name_ph,
            );

            let q = diesel::sql_query(sql);
            let instance_rows: Vec<ResourceKeyRow> = match types.len() {
                0 => unreachable!(),
                1 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                2 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(types[2])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(types[0])
                    .bind::<Text, _>(types[1])
                    .bind::<Text, _>(types[2])
                    .bind::<Text, _>(types[3])
                    .bind::<Text, _>(name.as_str())
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("resources_find: too many types (max 4)"),
            };
            for row in instance_rows {
                all_keys.push(row.to_key());
            }

            // 2. Class-scoped roles: expand via resource registry
            for (type_name, resource_table, pk_column) in &self.resource_tables {
                if types.contains(&type_name.as_str()) {
                    let name_ph = placeholder(1);
                    let expansion_sql = crate::sql::select_resources_find_class_expansion(
                        &self.role_table,
                        resource_table,
                        pk_column,
                        type_name,
                        &name_ph,
                    );
                    let q = diesel::sql_query(expansion_sql).bind::<Text, _>(name.as_str());
                    let class_rows: Vec<ResourceKeyRow> = q.load(conn).map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
            }

            async move { Ok(all_keys) }
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

            // The holder's rows for the requested names; coverage is
            // decided below in Rust, mirroring `InMemoryStore::in_list`
            // exactly: a candidate is covered by a same-id row or by a
            // scopeless (global/class) row, with no resource-type check.
            // Candidate keys (not row keys) are returned, so scopeless
            // covering rows never reach `to_key`.
            let name_placeholders: Vec<String> = (2..2 + names.len()).map(placeholder).collect();
            let name_filter = format!("role_row.name IN ({})", name_placeholders.join(", "));
            let sql = format!(
                "SELECT DISTINCT role_row.name AS name, role_row.resource_type, role_row.resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ? \
                   AND {name_filter}",
                role_table = self.role_table,
                join_table = self.join_table,
                name_filter = name_filter,
            );

            let mut all_values: Vec<&str> = Vec::new();
            all_values.push(holder.as_str());
            for name in names {
                all_values.push(name.as_str());
            }

            let q = diesel::sql_query(sql);
            let rows: Vec<RoleRow> = match all_values.len() {
                2 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                3 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                4 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                5 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                6 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                7 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                8 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .bind::<Text, _>(all_values[7])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                9 => q
                    .bind::<Text, _>(all_values[0])
                    .bind::<Text, _>(all_values[1])
                    .bind::<Text, _>(all_values[2])
                    .bind::<Text, _>(all_values[3])
                    .bind::<Text, _>(all_values[4])
                    .bind::<Text, _>(all_values[5])
                    .bind::<Text, _>(all_values[6])
                    .bind::<Text, _>(all_values[7])
                    .bind::<Text, _>(all_values[8])
                    .load(conn)
                    .map_err(Error::Diesel)?,
                _ => panic!("in_list: too many names (max 8)"),
            };
            let records: Vec<RoleRecord> = rows.iter().map(RoleRow::to_record).collect();
            let mut covered: Vec<ResourceKey> = Vec::new();
            for key in candidates {
                let matches = records.iter().any(|row| {
                    names.contains(&row.name)
                        && (row.resource_id.is_none()
                            || row.resource_id.as_ref() == Some(&key.resource_id))
                });
                if matches && !covered.contains(key) {
                    covered.push(key.clone());
                }
            }
            async move { Ok(covered) }
        }
    }
}

// === Async impls (D-08 rider, mirrored execution) ===
//
// The SAME DieselStore type, gated under cfg(all(async, postgres)):
// every body reuses the SAME sql.rs / rows.rs / sentinel.rs templates
// as the sync impls (Phase 3 D-06; prohibition: zero SQL re-written).
// Only the execution layer swaps: `diesel_async::RunQueryDsl` (never
// the sync prelude's; Pitfall 7) plus `.await`, and multi-statement
// choreographies run inside `AsyncConnection::transaction` with native
// Rust-2024 async closures (`async move |conn| ...`; the old-style
// closure-returning-async-block form fails the AsyncFnOnce HRTB,
// Pitfall 6).

#[cfg(all(feature = "postgres", feature = "async"))]
mod pg_async_impl {
    use super::{DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL};
    use core::future::Future;
    use diesel::sql_types::{BigInt, Text};
    use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::RoleQuery;
    use rolify_core::role::RoleRecord;
    use rolify_core::store::{RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn};

    use crate::dialect::placeholder;
    use crate::error::Error;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // Helper functions for AsyncPgConnection (async mirrors of the sync
    // helpers above; same template calls, only the execution point
    // gains `.await`).
    async fn find_or_create_by_triple(
        conn: &mut AsyncPgConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<RoleRecord, Error> {
        let existing = diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<RoleRow>(conn)
            .await;
        if let Ok(row) = existing {
            return Ok(row.to_record());
        }

        let insert_sql = crate::sql::insert_role(role_table);
        let insert_q = diesel::sql_query(insert_sql)
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        match insert_q.execute(conn).await {
            // A unique violation means a concurrent holder won the
            // insert race; the re-SELECT below reads the winner's row.
            Ok(_)
            | Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => {}
            Err(other) => return Err(Error::Diesel(other)),
        }

        diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<RoleRow>(conn)
            .await
            .map(|row| row.to_record())
            .map_err(Error::Diesel)
    }

    async fn get_role_id(
        conn: &mut AsyncPgConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<i64, Error> {
        diesel::sql_query(crate::sql::select_role_id_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<IdRow>(conn)
            .await
            .map(|row| row.id)
            .map_err(Error::Diesel)
    }

    // --- RoleStore ---

    impl RoleStore for DieselStore {
        type Conn = AsyncPgConnection;
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            async move {
                let rows: Vec<RoleRow> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        diesel::sql_query(sql)
                            .bind::<Text, _>(holder.as_str())
                            .bind::<Text, _>(name)
                            .bind::<Text, _>("")
                            .bind::<Text, _>("")
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>("")
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>(resource_id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?
                    }
                    rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            async move {
                let rows: Vec<RoleRow> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        diesel::sql_query(sql)
                            .bind::<Text, _>(holder.as_str())
                            .bind::<Text, _>(name)
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>(resource_id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?
                    }
                    rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }

        // Clippy: the typed `.bind` chain changes type per call, so the
        // arity ladder (1..=23, mirroring the sync impl) cannot be a loop.
        #[allow(clippy::too_many_lines)]
        fn where_any(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            queries: &[RoleQuery<'_>],
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            async move {
                if queries.is_empty() {
                    return Ok(Vec::new());
                }
                let (where_clause, _) = crate::sql::build_any_where(
                    &role_table,
                    &join_table,
                    &placeholder(1),
                    queries,
                    2,
                );
                let sql = crate::sql::select_roles_for_holder(
                    &role_table,
                    &join_table,
                    &where_clause,
                    &placeholder(1),
                );
                // Collect all bind values in order, then match on query count
                // for typed binds (mirrors the sync path's all_values idiom).
                let mut all_values: Vec<String> = Vec::new();
                all_values.push(holder.as_str().to_owned());
                for query in queries {
                    let name = query.name.as_str();
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                }
                let q = diesel::sql_query(sql);
                let rows: Vec<RoleRow> = match all_values.len() {
                    0 => unreachable!(),
                    1 => q
                        .bind::<Text, _>(&all_values[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    10 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    11 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    12 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    13 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    14 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    15 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    16 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    17 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    18 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    19 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    20 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    // The ported suite batches up to four queries (l.68:
                    // Instance + Global + Instance + Class = 23 binds).
                    21 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    22 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .bind::<Text, _>(&all_values[21])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    23 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .bind::<Text, _>(&all_values[21])
                        .bind::<Text, _>(&all_values[22])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("where_any: too many bind values (max 23)"),
                };
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
            let role_table = self.role_table.clone();
            async move {
                let name_ref = RoleName::new(name_owned);
                find_or_create_by_triple(conn, &role_table, &name_ref, &rt, &rid).await
            }
        }

        fn add(
            &mut self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            role: &RoleRecord,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            let rt = to_storage(role.resource_type.as_deref()).to_owned();
            let rid = resource_id_to_storage(role.resource_id.as_ref()).to_owned();
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let role_name = role.name.clone();

            async move {
                let role_record =
                    find_or_create_by_triple(conn, &role_table, &role_name, &rt, &rid).await?;

                let role_id = get_role_id(
                    conn,
                    &role_table,
                    &role_record.name,
                    role_record.resource_type.as_deref().unwrap_or_default(),
                    role_record
                        .resource_id
                        .as_ref()
                        .map(rolify_core::role::ResourceId::as_str)
                        .unwrap_or_default(),
                )
                .await?;

                let insert_sql = crate::sql::insert_link(&join_table);
                let q = diesel::sql_query(insert_sql)
                    .bind::<Text, _>(&holder_id)
                    .bind::<BigInt, _>(role_id);
                match q.execute(conn).await {
                    Ok(_) => Ok(true),
                    Err(diesel::result::Error::DatabaseError(
                        diesel::result::DatabaseErrorKind::UniqueViolation,
                        _,
                    )) => Ok(false),
                    Err(e) => Err(Error::Diesel(e)),
                }
            }
        }

        // Clippy: one mirrored choreography (affected SELECT, link DELETE,
        // per-role orphan sweep) inside a single native async closure.
        #[allow(clippy::too_many_lines)]
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
            // Own the target pieces: the borrowed RemovalTarget cannot
            // live inside the returned future (the sync path's
            // reference-clone idiom only works because the body is
            // eager there).
            let target_type: Option<String> = match target {
                RemovalTarget::NameOnly => None,
                RemovalTarget::TypeSweep(t) | RemovalTarget::Exact(t, _) => Some(t.to_owned()),
            };
            let target_id: Option<String> = match target {
                RemovalTarget::Exact(_, id) => Some(id.as_str().to_owned()),
                _ => None,
            };
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();

            async move {
                conn.transaction::<_, Error, _>(async move |conn| {
                    // Rebuild the borrowed target from the owned pieces
                    // (sql templates take &RemovalTarget; nothing
                    // borrows past the statement boundary).
                    let exact_id = target_id.as_ref().map(|id| ResourceId::new(id.clone()));
                    let scope_target = match (&target_type, &exact_id) {
                        (None, None) => RemovalTarget::NameOnly,
                        (Some(type_name), None) => RemovalTarget::TypeSweep(type_name),
                        (Some(type_name), Some(id)) => RemovalTarget::Exact(type_name, id),
                        (None, Some(_)) => {
                            unreachable!("an id with no type is not a removal target")
                        }
                    };

                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &scope_target);
                    let affected_rows: Vec<RoleRow> = match &scope_target {
                        RemovalTarget::NameOnly => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::TypeSweep(t) => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::Exact(t, id) => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .bind::<Text, _>(id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                    };
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &scope_target,
                    );
                    let removed_links = match &scope_target {
                        RemovalTarget::NameOnly => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::TypeSweep(t) => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::Exact(t, id) => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .bind::<Text, _>(id.as_str())
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                    };

                    let mut removed_roles = Vec::new();
                    if remove_role_if_empty {
                        for role in &affected_records {
                            let role_id = get_role_id(
                                conn,
                                &role_table,
                                &role.name,
                                role.resource_type.as_deref().unwrap_or_default(),
                                role.resource_id
                                    .as_ref()
                                    .map(rolify_core::role::ResourceId::as_str)
                                    .unwrap_or_default(),
                            )
                            .await?;
                            let sweep_sql =
                                crate::sql::delete_orphan_role(&role_table, &join_table);
                            let deleted = diesel::sql_query(sweep_sql)
                                .bind::<BigInt, _>(role_id)
                                .bind::<BigInt, _>(role_id)
                                .execute(conn)
                                .await
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
                .await
            }
        }

        fn remove_roles_for_scope(
            &mut self,
            conn: &mut Self::Conn,
            resource_type: &str,
            resource_id: &ResourceId,
        ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
            let rt = resource_type.to_owned();
            let rid = resource_id.as_str().to_owned();
            let role_table = self.role_table.clone();

            async move {
                let sql = crate::sql::delete_roles_by_scope(&role_table);
                let deleted = diesel::sql_query(sql)
                    .bind::<Text, _>(rt)
                    .bind::<Text, _>(rid)
                    .execute(conn)
                    .await
                    .map_err(Error::Diesel)?;
                Ok(deleted)
            }
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            // One complete condition per column: interpolating an empty
            // half would emit `AND  AND` (a syntax error the `is_ok`
            // below would swallow into a wrong `false`).
            let scope_cond = match column {
                ScopeColumn::ResourceType => "role_row.resource_type != ''",
                ScopeColumn::ResourceId => "role_row.resource_id != ''",
            };
            let sql = format!(
                "SELECT 1 AS dummy FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {holder_ph} AND {scope_cond} LIMIT 1",
                role_table = self.role_table,
                join_table = self.join_table,
                scope_cond = scope_cond,
                holder_ph = placeholder(1),
            );
            async move {
                #[derive(diesel::deserialize::QueryableByName)]
                #[allow(dead_code)]
                struct ExistsRow {
                    // Named exactly like the `AS dummy` projection: by-name
                    // decoding matches on it (an underscore prefix broke the
                    // match and made `exists` always false).
                    #[diesel(sql_type = diesel::sql_types::Integer)]
                    dummy: i32,
                }
                let found = diesel::sql_query(sql)
                    .bind::<Text, _>(&holder_id)
                    .get_result::<ExistsRow>(conn)
                    .await
                    .is_ok();
                Ok(found)
            }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            let sql = format!(
                "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {holder_ph}",
                role_table = self.role_table,
                join_table = self.join_table,
                holder_ph = placeholder(1),
            );
            async move {
                let rows: Vec<RoleRow> = diesel::sql_query(sql)
                    .bind::<Text, _>(&holder_id)
                    .load(conn)
                    .await
                    .map_err(Error::Diesel)?;
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }

        // Clippy: the strict/ladder value pushes plus the arity ladder
        // mirror the sync impl; the typed bind chain cannot be a loop.
        #[allow(clippy::too_many_lines)]
        fn holders_where(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
            query: &RoleQuery<'_>,
            strict: bool,
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let holder_table = self.holder_table_sql();
            async move {
                if holder_types.is_empty() {
                    return Ok(Vec::new());
                }

                let (where_clause, _) = if strict {
                    crate::sql::build_strict_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        query,
                        1 + holder_types.len(),
                    )
                } else {
                    crate::sql::build_ladder_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        query,
                        1 + holder_types.len(),
                    )
                };

                let type_placeholders: Vec<String> =
                    (1..=holder_types.len()).map(placeholder).collect();
                let type_filter =
                    format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

                let sql = format!(
                    "SELECT DISTINCT CAST(holder.id AS TEXT) AS user_id \
                     FROM {holder_table} AS holder \
                     INNER JOIN {join_table} AS link ON link.user_id = CAST(holder.id AS TEXT) \
                     INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                     WHERE {type_filter} AND {where_clause}",
                );

                // Binds are positional: the holder types first (placeholders
                // $1..$N), then the ladder values continuing the same
                // sequence. Ladder shapes mirror `where_` / `where_strict`.
                let mut all_values: Vec<String> = Vec::new();
                for holder_type in holder_types {
                    all_values.push((*holder_type).to_owned());
                }
                let name = query.name.as_str();
                if strict {
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push((*type_name).to_owned());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                } else {
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                }
                let q = diesel::sql_query(sql);
                let rows: Vec<HolderIdRow> = match all_values.len() {
                    2 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    10 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    11 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("holders_where: too many holder types (max 4)"),
                };
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        fn all_holders(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            let holder_table = self.holder_table_sql();
            async move {
                if holder_types.is_empty() {
                    return Ok(Vec::new());
                }

                let type_placeholders: Vec<String> =
                    (1..=holder_types.len()).map(placeholder).collect();
                let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

                let sql = format!(
                    "SELECT CAST(id AS TEXT) AS user_id FROM {holder_table} WHERE {type_filter}",
                );

                let q = diesel::sql_query(sql);
                let rows: Vec<HolderIdRow> = match holder_types.len() {
                    0 => unreachable!(),
                    1 => q
                        .bind::<Text, _>(holder_types[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .bind::<Text, _>(holder_types[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .bind::<Text, _>(holder_types[2])
                        .bind::<Text, _>(holder_types[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("all_holders: too many holder types (max 4)"),
                };
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        // Clippy: the catalog filter assembly plus the arity ladder
        // mirror the sync impl; the typed bind chain cannot be a loop.
        #[allow(clippy::too_many_lines)]
        fn roles_matching(
            &self,
            conn: &mut Self::Conn,
            query: &RoleCatalogQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let holder_table = query.holder.map(|_| self.holder_table_sql());
            async move {
                if query.types.is_empty() {
                    return Ok(Vec::new());
                }

                let has_holder = query.holder.is_some();
                let base_sql = crate::sql::select_roles_matching(
                    &role_table,
                    &join_table,
                    holder_table.as_deref(),
                    has_holder,
                );

                let (type_filter, idx) = crate::sql::roles_matching_type_filter(query.types, 1);
                let (name_filter, idx) = if let Some(name) = query.name {
                    crate::sql::roles_matching_name_filter(name, idx)
                } else {
                    (String::new(), idx)
                };
                let (scope_filter, idx) =
                    crate::sql::roles_matching_scope_filter(&query.scope, idx);
                let (holder_filter, _) = if let Some(holder) = query.holder {
                    crate::sql::roles_matching_holder_filter(holder, idx)
                } else {
                    (String::new(), idx)
                };

                let sql = base_sql
                    .replace("{type_filter}", &type_filter)
                    .replace("{name_filter}", &name_filter)
                    .replace("{scope_filter}", &scope_filter)
                    .replace("{holder_filter}", &holder_filter);

                let mut type_vals: Vec<&str> = query.types.to_vec();
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
                    1 => q
                        .bind::<Text, _>(type_vals[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .bind::<Text, _>(type_vals[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .bind::<Text, _>(type_vals[6])
                        .bind::<Text, _>(type_vals[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("roles_matching: too many bind values (max 8)"),
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }
    }

    // --- ResourceStore ---

    impl ResourceStore for DieselStore {
        type Conn = AsyncPgConnection;
        type Error = Error;

        fn resources_find(
            &self,
            conn: &mut Self::Conn,
            types: &[&str],
            name: &RoleName,
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let resource_tables = self.resource_tables.clone();
            let name_owned = name.as_str().to_owned();
            async move {
                if types.is_empty() {
                    return Ok(Vec::new());
                }

                let mut all_keys: Vec<ResourceKey> = Vec::new();

                // 1. Instance-scoped roles: direct query on roles table
                let type_placeholders: Vec<String> = (1..=types.len()).map(placeholder).collect();
                let type_filter = format!("resource_type IN ({})", type_placeholders.join(", "));
                let name_ph = placeholder(type_placeholders.len() + 1);

                let sql = format!(
                    "SELECT DISTINCT name AS name, resource_type, resource_id \
                     FROM {role_table} \
                     WHERE {type_filter} \
                       AND name = {name_ph} \
                       AND resource_id != ''",
                );

                // Class expansion mirrors the sync path: one expansion query
                // per registered resource table of the requested types.
                let expansions: Vec<String> = resource_tables
                    .iter()
                    .filter(|(type_name, _, _)| types.contains(&type_name.as_str()))
                    .map(|(type_name, resource_table, pk_column)| {
                        let name_ph = placeholder(1);
                        crate::sql::select_resources_find_class_expansion(
                            &role_table,
                            resource_table,
                            pk_column,
                            type_name,
                            &name_ph,
                        )
                    })
                    .collect();

                let instance_rows: Vec<ResourceKeyRow> = match types.len() {
                    1 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(types[2])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(types[2])
                        .bind::<Text, _>(types[3])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("resources_find: too many types (max 4)"),
                };
                for row in instance_rows {
                    all_keys.push(row.to_key());
                }

                // 2. Class-scoped roles: expand via resource registry
                for expansion_sql in expansions {
                    let class_rows: Vec<ResourceKeyRow> = diesel::sql_query(expansion_sql)
                        .bind::<Text, _>(&name_owned)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
                Ok(all_keys)
            }
        }

        // Clippy: the name arity ladder mirrors the sync impl; the typed
        // bind chain cannot be a loop (coverage folds in Rust after).
        #[allow(clippy::too_many_lines)]
        fn in_list(
            &self,
            conn: &mut Self::Conn,
            candidates: &[ResourceKey],
            holder: &ResourceId,
            names: &[RoleName],
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            async move {
                if candidates.is_empty() || names.is_empty() {
                    return Ok(Vec::new());
                }

                // The holder's rows for the requested names; coverage is
                // decided below in Rust, mirroring `InMemoryStore::in_list`
                // exactly: a candidate is covered by a same-id row or by a
                // scopeless (global/class) row, with no resource-type check.
                // Candidate keys (not row keys) are returned, so scopeless
                // covering rows never reach `to_key`.
                let name_placeholders: Vec<String> =
                    (2..2 + names.len()).map(placeholder).collect();
                let name_filter = format!("role_row.name IN ({})", name_placeholders.join(", "));

                let sql = format!(
                    "SELECT DISTINCT role_row.name AS name, role_row.resource_type, role_row.resource_id \
                     FROM {role_table} AS role_row \
                     INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                     WHERE link.user_id = {} \
                       AND {name_filter}",
                    placeholder(1),
                    role_table = role_table,
                    join_table = join_table,
                    name_filter = name_filter,
                );

                // Collect bind values in order: holder, then names.
                let mut all_values: Vec<&str> = Vec::new();
                all_values.push(holder.as_str());
                for name in names {
                    all_values.push(name.as_str());
                }

                let rows: Vec<RoleRow> = match all_values.len() {
                    2 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .bind::<Text, _>(all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .bind::<Text, _>(all_values[7])
                        .bind::<Text, _>(all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("in_list: too many names (max 8)"),
                };
                let records: Vec<RoleRecord> = rows.iter().map(RoleRow::to_record).collect();
                let mut covered: Vec<ResourceKey> = Vec::new();
                for key in candidates {
                    let matches = records.iter().any(|row| {
                        names.contains(&row.name)
                            && (row.resource_id.is_none()
                                || row.resource_id.as_ref() == Some(&key.resource_id))
                    });
                    if matches && !covered.contains(key) {
                        covered.push(key.clone());
                    }
                }
                Ok(covered)
            }
        }
    }
} // mod pg_async_impl

// === MySQL async impl ===
//
// Mirrors the sync MySQL impl and the pg_async_impl, using
// diesel_async::AsyncMysqlConnection and the same sql/rows/sentinel templates.
#[cfg(all(feature = "mysql", feature = "async"))]
mod mysql_async_impl {
    use super::{DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL};
    use core::future::Future;
    use diesel::sql_types::{BigInt, Text};
    use diesel_async::{AsyncConnection, AsyncMysqlConnection, RunQueryDsl};
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::RoleQuery;
    use rolify_core::role::RoleRecord;
    use rolify_core::store::{RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn};

    use crate::dialect::placeholder;
    use crate::error::Error;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // Helper functions for AsyncMysqlConnection (async mirrors of the sync
    // helpers above; same template calls, only the execution point gains `.await`).
    async fn find_or_create_by_triple(
        conn: &mut AsyncMysqlConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<RoleRecord, Error> {
        let existing = diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<RoleRow>(conn)
            .await;
        if let Ok(row) = existing {
            return Ok(row.to_record());
        }

        let insert_sql = crate::sql::insert_role(role_table);
        let insert_q = diesel::sql_query(insert_sql)
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        match insert_q.execute(conn).await {
            // A unique violation means a concurrent holder won the
            // insert race; the re-SELECT below reads the winner's row.
            Ok(_)
            | Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => {}
            Err(other) => return Err(Error::Diesel(other)),
        }

        diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<RoleRow>(conn)
            .await
            .map(|row| row.to_record())
            .map_err(Error::Diesel)
    }

    async fn get_role_id(
        conn: &mut AsyncMysqlConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<i64, Error> {
        diesel::sql_query(crate::sql::select_role_id_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<IdRow>(conn)
            .await
            .map(|row| row.id)
            .map_err(Error::Diesel)
    }

    // --- RoleStore ---

    impl RoleStore for DieselStore {
        type Conn = AsyncMysqlConnection;
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            async move {
                let rows: Vec<RoleRow> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        diesel::sql_query(sql)
                            .bind::<Text, _>(holder.as_str())
                            .bind::<Text, _>(name)
                            .bind::<Text, _>("")
                            .bind::<Text, _>("")
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>("")
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>(resource_id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?
                    }
                    rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            async move {
                let rows: Vec<RoleRow> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        diesel::sql_query(sql)
                            .bind::<Text, _>(holder.as_str())
                            .bind::<Text, _>(name)
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>(resource_id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?
                    }
                    rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }

        #[allow(clippy::too_many_lines)]
        fn where_any(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            queries: &[RoleQuery<'_>],
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            async move {
                if queries.is_empty() {
                    return Ok(Vec::new());
                }
                let (where_clause, _) = crate::sql::build_any_where(
                    &role_table,
                    &join_table,
                    &placeholder(1),
                    queries,
                    2,
                );
                let sql = crate::sql::select_roles_for_holder(
                    &role_table,
                    &join_table,
                    &where_clause,
                    &placeholder(1),
                );
                // Collect all bind values in order, then match on query count
                // for typed binds (mirrors the sync path's all_values idiom).
                let mut all_values: Vec<String> = Vec::new();
                all_values.push(holder.as_str().to_owned());
                for query in queries {
                    let name = query.name.as_str();
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                }
                let q = diesel::sql_query(sql);
                let rows: Vec<RoleRow> = match all_values.len() {
                    0 => unreachable!(),
                    1 => q
                        .bind::<Text, _>(&all_values[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    10 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    11 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    12 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    13 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    14 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    15 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    16 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    17 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    18 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    19 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    20 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    // The ported suite batches up to four queries (l.68:
                    // Instance + Global + Instance + Class = 23 binds).
                    21 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    22 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .bind::<Text, _>(&all_values[21])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    23 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .bind::<Text, _>(&all_values[21])
                        .bind::<Text, _>(&all_values[22])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("where_any: too many bind values (max 23)"),
                };
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
            let role_table = self.role_table.clone();
            async move {
                let name_ref = RoleName::new(name_owned);
                find_or_create_by_triple(conn, &role_table, &name_ref, &rt, &rid).await
            }
        }

        fn add(
            &mut self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            role: &RoleRecord,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            let rt = to_storage(role.resource_type.as_deref()).to_owned();
            let rid = resource_id_to_storage(role.resource_id.as_ref()).to_owned();
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let role_name = role.name.clone();

            async move {
                let role_record =
                    find_or_create_by_triple(conn, &role_table, &role_name, &rt, &rid).await?;

                let role_id = get_role_id(
                    conn,
                    &role_table,
                    &role_record.name,
                    role_record.resource_type.as_deref().unwrap_or_default(),
                    role_record
                        .resource_id
                        .as_ref()
                        .map(rolify_core::role::ResourceId::as_str)
                        .unwrap_or_default(),
                )
                .await?;

                let insert_sql = crate::sql::insert_link(&join_table);
                let q = diesel::sql_query(insert_sql)
                    .bind::<Text, _>(&holder_id)
                    .bind::<BigInt, _>(role_id);
                match q.execute(conn).await {
                    Ok(_) => Ok(true),
                    Err(diesel::result::Error::DatabaseError(
                        diesel::result::DatabaseErrorKind::UniqueViolation,
                        _,
                    )) => Ok(false),
                    Err(e) => Err(Error::Diesel(e)),
                }
            }
        }

        #[allow(clippy::too_many_lines)]
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
            // Own the target pieces: the borrowed RemovalTarget cannot
            // live inside the returned future (the sync path's
            // reference-clone idiom only works because the body is
            // eager there).
            let target_type: Option<String> = match target {
                RemovalTarget::NameOnly => None,
                RemovalTarget::TypeSweep(t) | RemovalTarget::Exact(t, _) => Some(t.to_owned()),
            };
            let target_id: Option<String> = match target {
                RemovalTarget::Exact(_, id) => Some(id.as_str().to_owned()),
                _ => None,
            };
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();

            async move {
                conn.transaction::<_, Error, _>(async move |conn| {
                    // Rebuild the borrowed target from the owned pieces
                    // (sql templates take &RemovalTarget; nothing
                    // borrows past the statement boundary).
                    let exact_id = target_id.as_ref().map(|id| ResourceId::new(id.clone()));
                    let scope_target = match (&target_type, &exact_id) {
                        (None, None) => RemovalTarget::NameOnly,
                        (Some(type_name), None) => RemovalTarget::TypeSweep(type_name),
                        (Some(type_name), Some(id)) => RemovalTarget::Exact(type_name, id),
                        (None, Some(_)) => {
                            unreachable!("an id with no type is not a removal target")
                        }
                    };

                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &scope_target);
                    let affected_rows: Vec<RoleRow> = match &scope_target {
                        RemovalTarget::NameOnly => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::TypeSweep(t) => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::Exact(t, id) => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .bind::<Text, _>(id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                    };
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &scope_target,
                    );
                    let removed_links = match &scope_target {
                        RemovalTarget::NameOnly => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::TypeSweep(t) => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::Exact(t, id) => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .bind::<Text, _>(id.as_str())
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                    };

                    let mut removed_roles = Vec::new();
                    if remove_role_if_empty {
                        for role in &affected_records {
                            let role_id = get_role_id(
                                conn,
                                &role_table,
                                &role.name,
                                role.resource_type.as_deref().unwrap_or_default(),
                                role.resource_id
                                    .as_ref()
                                    .map(rolify_core::role::ResourceId::as_str)
                                    .unwrap_or_default(),
                            )
                            .await?;
                            let sweep_sql =
                                crate::sql::delete_orphan_role(&role_table, &join_table);
                            let deleted = diesel::sql_query(sweep_sql)
                                .bind::<BigInt, _>(role_id)
                                .bind::<BigInt, _>(role_id)
                                .execute(conn)
                                .await
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
                .await
            }
        }

        fn remove_roles_for_scope(
            &mut self,
            conn: &mut Self::Conn,
            resource_type: &str,
            resource_id: &ResourceId,
        ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
            let rt = resource_type.to_owned();
            let rid = resource_id.as_str().to_owned();
            let role_table = self.role_table.clone();

            async move {
                let sql = crate::sql::delete_roles_by_scope(&role_table);
                let deleted = diesel::sql_query(sql)
                    .bind::<Text, _>(rt)
                    .bind::<Text, _>(rid)
                    .execute(conn)
                    .await
                    .map_err(Error::Diesel)?;
                Ok(deleted)
            }
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            // One complete condition per column: interpolating an empty
            // half would emit `AND  AND` (a syntax error the `is_ok`
            // below would swallow into a wrong `false`).
            let scope_cond = match column {
                ScopeColumn::ResourceType => "role_row.resource_type != ''",
                ScopeColumn::ResourceId => "role_row.resource_id != ''",
            };
            let sql = format!(
                "SELECT 1 AS dummy FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {holder_ph} AND {scope_cond} LIMIT 1",
                role_table = self.role_table,
                join_table = self.join_table,
                scope_cond = scope_cond,
                holder_ph = placeholder(1),
            );
            async move {
                #[derive(diesel::deserialize::QueryableByName)]
                #[allow(dead_code)]
                struct ExistsRow {
                    // Named exactly like the `AS dummy` projection: by-name
                    // decoding matches on it (an underscore prefix broke the
                    // match and made `exists` always false).
                    #[diesel(sql_type = diesel::sql_types::Integer)]
                    dummy: i32,
                }
                let found = diesel::sql_query(sql)
                    .bind::<Text, _>(&holder_id)
                    .get_result::<ExistsRow>(conn)
                    .await
                    .is_ok();
                Ok(found)
            }
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
                 WHERE link.user_id = ?",
                role_table = self.role_table,
                join_table = self.join_table,
            );
            async move {
                let q = diesel::sql_query(sql).bind::<Text, _>(holder_id);
                let rows: Vec<RoleRow> = q.load(conn).await.map_err(Error::Diesel)?;
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }

        #[allow(clippy::too_many_lines)]
        fn holders_where(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
            query: &RoleQuery<'_>,
            strict: bool,
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let holder_table = self.holder_table_sql();
            async move {
                if holder_types.is_empty() {
                    return Ok(Vec::new());
                }

                let (where_clause, _) = if strict {
                    crate::sql::build_strict_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        query,
                        1 + holder_types.len(),
                    )
                } else {
                    crate::sql::build_ladder_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        query,
                        1 + holder_types.len(),
                    )
                };

                let type_placeholders: Vec<String> =
                    (1..=holder_types.len()).map(placeholder).collect();
                let type_filter =
                    format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

                let sql = format!(
                    "SELECT DISTINCT CAST(holder.id AS CHAR) AS user_id \
                     FROM {holder_table} AS holder \
                     INNER JOIN {join_table} AS link ON link.user_id = CAST(holder.id AS CHAR) \
                     INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                     WHERE {type_filter} AND {where_clause}",
                );

                // Binds are positional: the holder types first (placeholders
                // $1..$N), then the ladder values continuing the same
                // sequence. Ladder shapes mirror `where_` / `where_strict`.
                let mut all_values: Vec<String> = Vec::new();
                for holder_type in holder_types {
                    all_values.push((*holder_type).to_owned());
                }
                let name = query.name.as_str();
                if strict {
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push((*type_name).to_owned());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                } else {
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                }
                let q = diesel::sql_query(sql);
                let rows: Vec<HolderIdRow> = match all_values.len() {
                    2 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    10 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    11 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("holders_where: too many holder types (max 4)"),
                };
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        fn all_holders(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            let holder_table = self.holder_table_sql();
            async move {
                if holder_types.is_empty() {
                    return Ok(Vec::new());
                }

                let type_placeholders: Vec<String> =
                    (1..=holder_types.len()).map(placeholder).collect();
                let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

                let sql = format!(
                    "SELECT CAST(id AS CHAR) AS user_id FROM {holder_table} WHERE {type_filter}",
                );

                let q = diesel::sql_query(sql);
                let rows: Vec<HolderIdRow> = match holder_types.len() {
                    0 => unreachable!(),
                    1 => q
                        .bind::<Text, _>(holder_types[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .bind::<Text, _>(holder_types[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .bind::<Text, _>(holder_types[2])
                        .bind::<Text, _>(holder_types[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("all_holders: too many holder types (max 4)"),
                };
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        // Clippy: the catalog filter assembly plus the arity ladder
        // mirror the sync impl; the typed bind chain cannot be a loop.
        #[allow(clippy::too_many_lines)]
        fn roles_matching(
            &self,
            conn: &mut Self::Conn,
            query: &RoleCatalogQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let holder_table = query.holder.map(|_| self.holder_table_sql());
            async move {
                if query.types.is_empty() {
                    return Ok(Vec::new());
                }

                let has_holder = query.holder.is_some();
                let base_sql = crate::sql::select_roles_matching(
                    &role_table,
                    &join_table,
                    holder_table.as_deref(),
                    has_holder,
                );

                let (type_filter, idx) = crate::sql::roles_matching_type_filter(query.types, 1);
                let (name_filter, idx) = if let Some(name) = query.name {
                    crate::sql::roles_matching_name_filter(name, idx)
                } else {
                    (String::new(), idx)
                };
                let (scope_filter, idx) =
                    crate::sql::roles_matching_scope_filter(&query.scope, idx);
                let (holder_filter, _) = if let Some(holder) = query.holder {
                    crate::sql::roles_matching_holder_filter(holder, idx)
                } else {
                    (String::new(), idx)
                };

                let sql = base_sql
                    .replace("{type_filter}", &type_filter)
                    .replace("{name_filter}", &name_filter)
                    .replace("{scope_filter}", &scope_filter)
                    .replace("{holder_filter}", &holder_filter);

                let mut type_vals: Vec<&str> = query.types.to_vec();
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
                    1 => q
                        .bind::<Text, _>(type_vals[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .bind::<Text, _>(type_vals[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .bind::<Text, _>(type_vals[6])
                        .bind::<Text, _>(type_vals[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("roles_matching: too many bind values (max 8)"),
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }
    }

    // --- ResourceStore ---

    impl ResourceStore for DieselStore {
        type Conn = AsyncMysqlConnection;
        type Error = Error;

        // Clippy: the type arity ladder mirrors the sync impl; the typed
        // bind chain cannot be a loop (coverage folds in Rust after).
        #[allow(clippy::too_many_lines)]
        fn resources_find(
            &self,
            conn: &mut Self::Conn,
            types: &[&str],
            name: &RoleName,
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let resource_tables = self.resource_tables.clone();
            let name_owned = name.as_str().to_owned();
            async move {
                if types.is_empty() {
                    return Ok(Vec::new());
                }

                let mut all_keys: Vec<ResourceKey> = Vec::new();

                // 1. Instance-scoped roles: direct query on roles table
                let type_placeholders: Vec<String> = (1..=types.len()).map(placeholder).collect();
                let type_filter = format!("resource_type IN ({})", type_placeholders.join(", "));
                let name_ph = placeholder(type_placeholders.len() + 1);

                let sql = format!(
                    "SELECT DISTINCT name AS name, resource_type, resource_id \
                     FROM {role_table} \
                     WHERE {type_filter} \
                       AND name = {name_ph} \
                       AND resource_id != ''",
                );

                // Class expansion mirrors the sync path: one expansion query
                // per registered resource table of the requested types.
                let expansions: Vec<String> = resource_tables
                    .iter()
                    .filter(|(type_name, _, _)| types.contains(&type_name.as_str()))
                    .map(|(type_name, resource_table, pk_column)| {
                        let name_ph = placeholder(1);
                        crate::sql::select_resources_find_class_expansion(
                            &role_table,
                            resource_table,
                            pk_column,
                            type_name,
                            &name_ph,
                        )
                    })
                    .collect();

                let instance_rows: Vec<ResourceKeyRow> = match types.len() {
                    1 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(types[2])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(types[2])
                        .bind::<Text, _>(types[3])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("resources_find: too many types (max 4)"),
                };
                for row in instance_rows {
                    all_keys.push(row.to_key());
                }

                // 2. Class-scoped roles: expand via resource registry
                for expansion_sql in expansions {
                    let class_rows: Vec<ResourceKeyRow> = diesel::sql_query(expansion_sql)
                        .bind::<Text, _>(&name_owned)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
                Ok(all_keys)
            }
        }

        // Clippy: the name arity ladder mirrors the sync impl; the typed
        // bind chain cannot be a loop (coverage folds in Rust after).
        #[allow(clippy::too_many_lines)]
        fn in_list(
            &self,
            conn: &mut Self::Conn,
            candidates: &[ResourceKey],
            holder: &ResourceId,
            names: &[RoleName],
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            async move {
                if candidates.is_empty() || names.is_empty() {
                    return Ok(Vec::new());
                }

                // The holder's rows for the requested names; coverage is
                // decided below in Rust, mirroring `InMemoryStore::in_list`
                // exactly: a candidate is covered by a same-id row or by a
                // scopeless (global/class) row, with no resource-type check.
                // Candidate keys (not row keys) are returned, so scopeless
                // covering rows never reach `to_key`.
                let name_placeholders: Vec<String> =
                    (2..2 + names.len()).map(placeholder).collect();
                let name_filter = format!("role_row.name IN ({})", name_placeholders.join(", "));

                let sql = format!(
                    "SELECT DISTINCT role_row.name AS name, role_row.resource_type, role_row.resource_id \
                     FROM {role_table} AS role_row \
                     INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                     WHERE link.user_id = {} \
                       AND {name_filter}",
                    placeholder(1),
                    role_table = role_table,
                    join_table = join_table,
                    name_filter = name_filter,
                );

                // Collect bind values in order: holder, then names.
                let mut all_values: Vec<&str> = Vec::new();
                all_values.push(holder.as_str());
                for name in names {
                    all_values.push(name.as_str());
                }

                let rows: Vec<RoleRow> = match all_values.len() {
                    2 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .bind::<Text, _>(all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .bind::<Text, _>(all_values[7])
                        .bind::<Text, _>(all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("in_list: too many names (max 8)"),
                };
                let records: Vec<RoleRecord> = rows.iter().map(RoleRow::to_record).collect();
                let mut covered: Vec<ResourceKey> = Vec::new();
                for key in candidates {
                    let matches = records.iter().any(|row| {
                        names.contains(&row.name)
                            && (row.resource_id.is_none()
                                || row.resource_id.as_ref() == Some(&key.resource_id))
                    });
                    if matches && !covered.contains(key) {
                        covered.push(key.clone());
                    }
                }
                Ok(covered)
            }
        }
    }
} // mod mysql_async_impl

// === SQLite async impl ===
//
// Uses diesel_async::SyncConnectionWrapper<SqliteConnection> to execute
// sync SQLite on a thread pool (diesel-async's connection-wrapper
// offloading). Mirrors the sync SQLite impl and the pg_async_impl.
#[cfg(all(feature = "sqlite", feature = "async"))]
mod sqlite_async_impl {
    use super::{DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL};
    use core::future::Future;
    use diesel::sql_types::{BigInt, Text};
    use diesel::sqlite::SqliteConnection;
    use diesel_async::{
        AsyncConnection, RunQueryDsl, sync_connection_wrapper::SyncConnectionWrapper,
    };
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::RoleQuery;
    use rolify_core::role::RoleRecord;
    use rolify_core::store::{RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn};

    use crate::dialect::placeholder;
    use crate::error::Error;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    type AsyncSqliteConnection = SyncConnectionWrapper<SqliteConnection>;

    // Helper functions for AsyncSqliteConnection (async mirrors of the sync
    // helpers above; same template calls, only the execution point gains `.await`).
    async fn find_or_create_by_triple(
        conn: &mut AsyncSqliteConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<RoleRecord, Error> {
        let existing = diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<RoleRow>(conn)
            .await;
        if let Ok(row) = existing {
            return Ok(row.to_record());
        }

        let insert_sql = crate::sql::insert_role(role_table);
        let insert_q = diesel::sql_query(insert_sql)
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        match insert_q.execute(conn).await {
            // A unique violation means a concurrent holder won the
            // insert race; the re-SELECT below reads the winner's row.
            Ok(_)
            | Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => {}
            Err(other) => return Err(Error::Diesel(other)),
        }

        diesel::sql_query(crate::sql::select_role_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<RoleRow>(conn)
            .await
            .map(|row| row.to_record())
            .map_err(Error::Diesel)
    }

    async fn get_role_id(
        conn: &mut AsyncSqliteConnection,
        role_table: &str,
        name: &RoleName,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<i64, Error> {
        diesel::sql_query(crate::sql::select_role_id_by_triple(role_table))
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id)
            .get_result::<IdRow>(conn)
            .await
            .map(|row| row.id)
            .map_err(Error::Diesel)
    }

    // --- RoleStore ---

    impl RoleStore for DieselStore {
        type Conn = AsyncSqliteConnection;
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            async move {
                let rows: Vec<RoleRow> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        diesel::sql_query(sql)
                            .bind::<Text, _>(holder.as_str())
                            .bind::<Text, _>(name)
                            .bind::<Text, _>("")
                            .bind::<Text, _>("")
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>("")
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>(resource_id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?
                    }
                    rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
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
            let sql = crate::sql::select_roles_for_holder(
                &self.role_table,
                &self.join_table,
                &where_clause,
                &placeholder(1),
            );
            let name = query.name.as_str();
            async move {
                let rows: Vec<RoleRow> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>("")
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Class(type_name) => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .bind::<Text, _>(type_name)
                        .bind::<Text, _>("")
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        diesel::sql_query(sql)
                            .bind::<Text, _>(holder.as_str())
                            .bind::<Text, _>(name)
                            .bind::<Text, _>(type_name)
                            .bind::<Text, _>(resource_id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?
                    }
                    rolify_core::query::ResourceFilter::Any => diesel::sql_query(sql)
                        .bind::<Text, _>(holder.as_str())
                        .bind::<Text, _>(name)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }

        #[allow(clippy::too_many_lines)]
        fn where_any(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            queries: &[RoleQuery<'_>],
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            async move {
                if queries.is_empty() {
                    return Ok(Vec::new());
                }
                let (where_clause, _) = crate::sql::build_any_where(
                    &role_table,
                    &join_table,
                    &placeholder(1),
                    queries,
                    2,
                );
                let sql = crate::sql::select_roles_for_holder(
                    &role_table,
                    &join_table,
                    &where_clause,
                    &placeholder(1),
                );
                // Collect all bind values in order, then match on query count
                // for typed binds (mirrors the sync path's all_values idiom).
                let mut all_values: Vec<String> = Vec::new();
                all_values.push(holder.as_str().to_owned());
                for query in queries {
                    let name = query.name.as_str();
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(String::new());
                            all_values.push(type_name.to_string());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                }
                let q = diesel::sql_query(sql);
                let rows: Vec<RoleRow> = match all_values.len() {
                    0 => unreachable!(),
                    1 => q
                        .bind::<Text, _>(&all_values[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    10 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    11 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    12 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    13 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    14 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    15 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    16 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    17 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    18 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    19 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    20 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    // The ported suite batches up to four queries (l.68:
                    // Instance + Global + Instance + Class = 23 binds).
                    21 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    22 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .bind::<Text, _>(&all_values[21])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    23 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .bind::<Text, _>(&all_values[11])
                        .bind::<Text, _>(&all_values[12])
                        .bind::<Text, _>(&all_values[13])
                        .bind::<Text, _>(&all_values[14])
                        .bind::<Text, _>(&all_values[15])
                        .bind::<Text, _>(&all_values[16])
                        .bind::<Text, _>(&all_values[17])
                        .bind::<Text, _>(&all_values[18])
                        .bind::<Text, _>(&all_values[19])
                        .bind::<Text, _>(&all_values[20])
                        .bind::<Text, _>(&all_values[21])
                        .bind::<Text, _>(&all_values[22])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("where_any: too many bind values (max 23)"),
                };
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
            let role_table = self.role_table.clone();
            async move {
                let name_ref = RoleName::new(name_owned);
                find_or_create_by_triple(conn, &role_table, &name_ref, &rt, &rid).await
            }
        }

        fn add(
            &mut self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            role: &RoleRecord,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            let rt = to_storage(role.resource_type.as_deref()).to_owned();
            let rid = resource_id_to_storage(role.resource_id.as_ref()).to_owned();
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let role_name = role.name.clone();

            async move {
                let role_record =
                    find_or_create_by_triple(conn, &role_table, &role_name, &rt, &rid).await?;

                let role_id = get_role_id(
                    conn,
                    &role_table,
                    &role_record.name,
                    role_record.resource_type.as_deref().unwrap_or_default(),
                    role_record
                        .resource_id
                        .as_ref()
                        .map(rolify_core::role::ResourceId::as_str)
                        .unwrap_or_default(),
                )
                .await?;

                let insert_sql = crate::sql::insert_link(&join_table);
                let q = diesel::sql_query(insert_sql)
                    .bind::<Text, _>(&holder_id)
                    .bind::<BigInt, _>(role_id);
                match q.execute(conn).await {
                    Ok(_) => Ok(true),
                    Err(diesel::result::Error::DatabaseError(
                        diesel::result::DatabaseErrorKind::UniqueViolation,
                        _,
                    )) => Ok(false),
                    Err(e) => Err(Error::Diesel(e)),
                }
            }
        }

        #[allow(clippy::too_many_lines)]
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
            // Own the target pieces: the borrowed RemovalTarget cannot
            // live inside the returned future (the sync path's
            // reference-clone idiom only works because the body is
            // eager there).
            let target_type: Option<String> = match target {
                RemovalTarget::NameOnly => None,
                RemovalTarget::TypeSweep(t) | RemovalTarget::Exact(t, _) => Some(t.to_owned()),
            };
            let target_id: Option<String> = match target {
                RemovalTarget::Exact(_, id) => Some(id.as_str().to_owned()),
                _ => None,
            };
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();

            async move {
                conn.transaction::<_, Error, _>(async move |conn| {
                    // Rebuild the borrowed target from the owned pieces
                    // (sql templates take &RemovalTarget; nothing
                    // borrows past the statement boundary).
                    let exact_id = target_id.as_ref().map(|id| ResourceId::new(id.clone()));
                    let scope_target = match (&target_type, &exact_id) {
                        (None, None) => RemovalTarget::NameOnly,
                        (Some(type_name), None) => RemovalTarget::TypeSweep(type_name),
                        (Some(type_name), Some(id)) => RemovalTarget::Exact(type_name, id),
                        (None, Some(_)) => {
                            unreachable!("an id with no type is not a removal target")
                        }
                    };

                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &scope_target);
                    let affected_rows: Vec<RoleRow> = match &scope_target {
                        RemovalTarget::NameOnly => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::TypeSweep(t) => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::Exact(t, id) => diesel::sql_query(affected_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .bind::<Text, _>(id.as_str())
                            .load(conn)
                            .await
                            .map_err(Error::Diesel)?,
                    };
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &scope_target,
                    );
                    let removed_links = match &scope_target {
                        RemovalTarget::NameOnly => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::TypeSweep(t) => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                        RemovalTarget::Exact(t, id) => diesel::sql_query(delete_sql)
                            .bind::<Text, _>(&holder_id)
                            .bind::<Text, _>(&name_owned)
                            .bind::<Text, _>(*t)
                            .bind::<Text, _>(id.as_str())
                            .execute(conn)
                            .await
                            .map_err(Error::Diesel)?,
                    };

                    let mut removed_roles = Vec::new();
                    if remove_role_if_empty {
                        for role in &affected_records {
                            let role_id = get_role_id(
                                conn,
                                &role_table,
                                &role.name,
                                role.resource_type.as_deref().unwrap_or_default(),
                                role.resource_id
                                    .as_ref()
                                    .map(rolify_core::role::ResourceId::as_str)
                                    .unwrap_or_default(),
                            )
                            .await?;
                            let sweep_sql =
                                crate::sql::delete_orphan_role(&role_table, &join_table);
                            let deleted = diesel::sql_query(sweep_sql)
                                .bind::<BigInt, _>(role_id)
                                .bind::<BigInt, _>(role_id)
                                .execute(conn)
                                .await
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
                .await
            }
        }

        fn remove_roles_for_scope(
            &mut self,
            conn: &mut Self::Conn,
            resource_type: &str,
            resource_id: &ResourceId,
        ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
            let rt = resource_type.to_owned();
            let rid = resource_id.as_str().to_owned();
            let role_table = self.role_table.clone();

            async move {
                let sql = crate::sql::delete_roles_by_scope(&role_table);
                let deleted = diesel::sql_query(sql)
                    .bind::<Text, _>(rt)
                    .bind::<Text, _>(rid)
                    .execute(conn)
                    .await
                    .map_err(Error::Diesel)?;
                Ok(deleted)
            }
        }

        fn exists(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            column: ScopeColumn,
        ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
            let holder_id = holder.as_str().to_owned();
            // One complete condition per column: interpolating an empty
            // half would emit `AND  AND` (a syntax error the `is_ok`
            // below would swallow into a wrong `false`).
            let scope_cond = match column {
                ScopeColumn::ResourceType => "role_row.resource_type != ''",
                ScopeColumn::ResourceId => "role_row.resource_id != ''",
            };
            let sql = format!(
                "SELECT 1 AS dummy FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {holder_ph} AND {scope_cond} LIMIT 1",
                role_table = self.role_table,
                join_table = self.join_table,
                scope_cond = scope_cond,
                holder_ph = placeholder(1),
            );
            async move {
                #[derive(diesel::deserialize::QueryableByName)]
                #[allow(dead_code)]
                struct ExistsRow {
                    // Named exactly like the `AS dummy` projection: by-name
                    // decoding matches on it (an underscore prefix broke the
                    // match and made `exists` always false).
                    #[diesel(sql_type = diesel::sql_types::Integer)]
                    dummy: i32,
                }
                let found = diesel::sql_query(sql)
                    .bind::<Text, _>(&holder_id)
                    .get_result::<ExistsRow>(conn)
                    .await
                    .is_ok();
                Ok(found)
            }
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
                 WHERE link.user_id = ?",
                role_table = self.role_table,
                join_table = self.join_table,
            );
            async move {
                let q = diesel::sql_query(sql).bind::<Text, _>(holder_id);
                let rows: Vec<RoleRow> = q.load(conn).await.map_err(Error::Diesel)?;
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }

        #[allow(clippy::too_many_lines)]
        fn holders_where(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
            query: &RoleQuery<'_>,
            strict: bool,
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let holder_table = self.holder_table_sql();
            async move {
                if holder_types.is_empty() {
                    return Ok(Vec::new());
                }

                let (where_clause, _) = if strict {
                    crate::sql::build_strict_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        query,
                        1 + holder_types.len(),
                    )
                } else {
                    crate::sql::build_ladder_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        query,
                        1 + holder_types.len(),
                    )
                };

                let type_placeholders: Vec<String> =
                    (1..=holder_types.len()).map(placeholder).collect();
                let type_filter =
                    format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

                let sql = format!(
                    "SELECT DISTINCT CAST(holder.id AS TEXT) AS user_id \
                     FROM {holder_table} AS holder \
                     INNER JOIN {join_table} AS link ON link.user_id = CAST(holder.id AS TEXT) \
                     INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                     WHERE {type_filter} AND {where_clause}",
                );

                // Binds are positional: the holder types first (placeholders
                // $1..$N), then the ladder values continuing the same
                // sequence. Ladder shapes mirror `where_` / `where_strict`.
                let mut all_values: Vec<String> = Vec::new();
                for holder_type in holder_types {
                    all_values.push((*holder_type).to_owned());
                }
                let name = query.name.as_str();
                if strict {
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push((*type_name).to_owned());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                } else {
                    match &query.filter {
                        rolify_core::query::ResourceFilter::Global => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Class(type_name) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                        }
                        rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                            all_values.push(name.to_owned());
                            all_values.push(String::new());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(String::new());
                            all_values.push((*type_name).to_owned());
                            all_values.push(resource_id.as_str().to_owned());
                        }
                        rolify_core::query::ResourceFilter::Any => {
                            all_values.push(name.to_owned());
                        }
                    }
                }
                let q = diesel::sql_query(sql);
                let rows: Vec<HolderIdRow> = match all_values.len() {
                    2 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    10 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    11 => q
                        .bind::<Text, _>(&all_values[0])
                        .bind::<Text, _>(&all_values[1])
                        .bind::<Text, _>(&all_values[2])
                        .bind::<Text, _>(&all_values[3])
                        .bind::<Text, _>(&all_values[4])
                        .bind::<Text, _>(&all_values[5])
                        .bind::<Text, _>(&all_values[6])
                        .bind::<Text, _>(&all_values[7])
                        .bind::<Text, _>(&all_values[8])
                        .bind::<Text, _>(&all_values[9])
                        .bind::<Text, _>(&all_values[10])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("holders_where: too many holder types (max 4)"),
                };
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        fn all_holders(
            &self,
            conn: &mut Self::Conn,
            holder_types: &[&str],
        ) -> impl Future<Output = Result<Vec<ResourceId>, Self::Error>> + Send {
            let holder_table = self.holder_table_sql();
            async move {
                if holder_types.is_empty() {
                    return Ok(Vec::new());
                }

                let type_placeholders: Vec<String> =
                    (1..=holder_types.len()).map(placeholder).collect();
                let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

                let sql = format!(
                    "SELECT CAST(id AS TEXT) AS user_id FROM {holder_table} WHERE {type_filter}",
                );

                let q = diesel::sql_query(sql);
                let rows: Vec<HolderIdRow> = match holder_types.len() {
                    0 => unreachable!(),
                    1 => q
                        .bind::<Text, _>(holder_types[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .bind::<Text, _>(holder_types[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(holder_types[0])
                        .bind::<Text, _>(holder_types[1])
                        .bind::<Text, _>(holder_types[2])
                        .bind::<Text, _>(holder_types[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("all_holders: too many holder types (max 4)"),
                };
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        // Clippy: the catalog filter assembly plus the arity ladder
        // mirror the sync impl; the typed bind chain cannot be a loop.
        #[allow(clippy::too_many_lines)]
        fn roles_matching(
            &self,
            conn: &mut Self::Conn,
            query: &RoleCatalogQuery<'_>,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let holder_table = query.holder.map(|_| self.holder_table_sql());
            async move {
                if query.types.is_empty() {
                    return Ok(Vec::new());
                }

                let has_holder = query.holder.is_some();
                let base_sql = crate::sql::select_roles_matching(
                    &role_table,
                    &join_table,
                    holder_table.as_deref(),
                    has_holder,
                );

                let (type_filter, idx) = crate::sql::roles_matching_type_filter(query.types, 1);
                let (name_filter, idx) = if let Some(name) = query.name {
                    crate::sql::roles_matching_name_filter(name, idx)
                } else {
                    (String::new(), idx)
                };
                let (scope_filter, idx) =
                    crate::sql::roles_matching_scope_filter(&query.scope, idx);
                let (holder_filter, _) = if let Some(holder) = query.holder {
                    crate::sql::roles_matching_holder_filter(holder, idx)
                } else {
                    (String::new(), idx)
                };

                let sql = base_sql
                    .replace("{type_filter}", &type_filter)
                    .replace("{name_filter}", &name_filter)
                    .replace("{scope_filter}", &scope_filter)
                    .replace("{holder_filter}", &holder_filter);

                let mut type_vals: Vec<&str> = query.types.to_vec();
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
                    1 => q
                        .bind::<Text, _>(type_vals[0])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .bind::<Text, _>(type_vals[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => q
                        .bind::<Text, _>(type_vals[0])
                        .bind::<Text, _>(type_vals[1])
                        .bind::<Text, _>(type_vals[2])
                        .bind::<Text, _>(type_vals[3])
                        .bind::<Text, _>(type_vals[4])
                        .bind::<Text, _>(type_vals[5])
                        .bind::<Text, _>(type_vals[6])
                        .bind::<Text, _>(type_vals[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("roles_matching: too many bind values (max 8)"),
                };
                Ok(rows.into_iter().map(|r| r.to_record()).collect())
            }
        }
    }

    // --- ResourceStore ---

    impl ResourceStore for DieselStore {
        type Conn = AsyncSqliteConnection;
        type Error = Error;

        // Clippy: the type arity ladder mirrors the sync impl; the typed
        // bind chain cannot be a loop (coverage folds in Rust after).
        #[allow(clippy::too_many_lines)]
        fn resources_find(
            &self,
            conn: &mut Self::Conn,
            types: &[&str],
            name: &RoleName,
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let resource_tables = self.resource_tables.clone();
            let name_owned = name.as_str().to_owned();
            async move {
                if types.is_empty() {
                    return Ok(Vec::new());
                }

                let mut all_keys: Vec<ResourceKey> = Vec::new();

                // 1. Instance-scoped roles: direct query on roles table
                let type_placeholders: Vec<String> = (1..=types.len()).map(placeholder).collect();
                let type_filter = format!("resource_type IN ({})", type_placeholders.join(", "));
                let name_ph = placeholder(type_placeholders.len() + 1);

                let sql = format!(
                    "SELECT DISTINCT name AS name, resource_type, resource_id \
                     FROM {role_table} \
                     WHERE {type_filter} \
                       AND name = {name_ph} \
                       AND resource_id != ''",
                );

                // Class expansion mirrors the sync path: one expansion query
                // per registered resource table of the requested types.
                let expansions: Vec<String> = resource_tables
                    .iter()
                    .filter(|(type_name, _, _)| types.contains(&type_name.as_str()))
                    .map(|(type_name, resource_table, pk_column)| {
                        let name_ph = placeholder(1);
                        crate::sql::select_resources_find_class_expansion(
                            &role_table,
                            resource_table,
                            pk_column,
                            type_name,
                            &name_ph,
                        )
                    })
                    .collect();

                let instance_rows: Vec<ResourceKeyRow> = match types.len() {
                    1 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    2 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(types[2])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => diesel::sql_query(&sql)
                        .bind::<Text, _>(types[0])
                        .bind::<Text, _>(types[1])
                        .bind::<Text, _>(types[2])
                        .bind::<Text, _>(types[3])
                        .bind::<Text, _>(name.as_str())
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("resources_find: too many types (max 4)"),
                };
                for row in instance_rows {
                    all_keys.push(row.to_key());
                }

                // 2. Class-scoped roles: expand via resource registry
                for expansion_sql in expansions {
                    let class_rows: Vec<ResourceKeyRow> = diesel::sql_query(expansion_sql)
                        .bind::<Text, _>(&name_owned)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
                Ok(all_keys)
            }
        }

        // Clippy: the name arity ladder mirrors the sync impl; the typed
        // bind chain cannot be a loop (coverage folds in Rust after).
        #[allow(clippy::too_many_lines)]
        fn in_list(
            &self,
            conn: &mut Self::Conn,
            candidates: &[ResourceKey],
            holder: &ResourceId,
            names: &[RoleName],
        ) -> impl Future<Output = Result<Vec<ResourceKey>, Self::Error>> + Send {
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            async move {
                if candidates.is_empty() || names.is_empty() {
                    return Ok(Vec::new());
                }

                // The holder's rows for the requested names; coverage is
                // decided below in Rust, mirroring `InMemoryStore::in_list`
                // exactly: a candidate is covered by a same-id row or by a
                // scopeless (global/class) row, with no resource-type check.
                // Candidate keys (not row keys) are returned, so scopeless
                // covering rows never reach `to_key`.
                let name_placeholders: Vec<String> =
                    (2..2 + names.len()).map(placeholder).collect();
                let name_filter = format!("role_row.name IN ({})", name_placeholders.join(", "));

                let sql = format!(
                    "SELECT DISTINCT role_row.name AS name, role_row.resource_type, role_row.resource_id \
                     FROM {role_table} AS role_row \
                     INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                     WHERE link.user_id = {} \
                       AND {name_filter}",
                    placeholder(1),
                    role_table = role_table,
                    join_table = join_table,
                    name_filter = name_filter,
                );

                // Collect bind values in order: holder, then names.
                let mut all_values: Vec<&str> = Vec::new();
                all_values.push(holder.as_str());
                for name in names {
                    all_values.push(name.as_str());
                }

                let rows: Vec<RoleRow> = match all_values.len() {
                    2 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    3 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    4 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    5 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    6 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    7 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    8 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .bind::<Text, _>(all_values[7])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    9 => diesel::sql_query(&sql)
                        .bind::<Text, _>(all_values[0])
                        .bind::<Text, _>(all_values[1])
                        .bind::<Text, _>(all_values[2])
                        .bind::<Text, _>(all_values[3])
                        .bind::<Text, _>(all_values[4])
                        .bind::<Text, _>(all_values[5])
                        .bind::<Text, _>(all_values[6])
                        .bind::<Text, _>(all_values[7])
                        .bind::<Text, _>(all_values[8])
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?,
                    _ => panic!("in_list: too many names (max 8)"),
                };
                let records: Vec<RoleRecord> = rows.iter().map(RoleRow::to_record).collect();
                let mut covered: Vec<ResourceKey> = Vec::new();
                for key in candidates {
                    let matches = records.iter().any(|row| {
                        names.contains(&row.name)
                            && (row.resource_id.is_none()
                                || row.resource_id.as_ref() == Some(&key.resource_id))
                    });
                    if matches && !covered.contains(key) {
                        covered.push(key.clone());
                    }
                }
                Ok(covered)
            }
        }
    }
} // mod sqlite_async_impl

// Every test below asserts postgres-flavored identifier quoting, so the
// module rides the postgres feature; the imports then never dangle in the
// inert test build.
#[cfg(all(test, feature = "postgres"))]
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
        let (name, rt, rid) =
            store.scope_to_triple(&RoleName::from("manager"), ResourceRef::Class("Forum"));
        assert_eq!(name, "manager");
        assert_eq!(rt, "Forum");
        assert_eq!(rid, "");
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn scope_to_triple_instance() {
        let config = RolifyConfig::builder().build().unwrap();
        let store = DieselStore::new(&config);
        let (name, rt, rid) = store.scope_to_triple(
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &ResourceId::from("42")),
        );
        assert_eq!(name, "moderator");
        assert_eq!(rt, "Forum");
        assert_eq!(rid, "42");
    }
}
