//! [`SqlxStore`] - the generic async `RoleStore` implementation for any
//! `sqlx` backend (D-13): one store type serves `sqlx::Postgres`,
//! `sqlx::MySql`, and `sqlx::Sqlite` alike, because the engine is a
//! runtime property of `DB` (`Database::NAME`), never a `cfg` switch.
//!
//! Architecture (D-12/D-13, RESEARCH Pattern 1):
//!
//! - **Runtime binds.** Every statement is assembled by the `sql`
//!   module and executed through `sqlx::query` with ordered `.bind`
//!   calls. No compile-time checked macros (`query!`/`query_as!`): the
//!   crate carries no `DATABASE_URL` build dependency and no `.sqlx`
//!   offline cache; `sqlx::migrate!` (the vendored static trees) is the
//!   sole compile-time macro.
//! - **The bound stack.** sqlx 0.9 does not imply `IntoArguments`, the
//!   connection-reference `Executor` HRTB, or the `Encode`/`Decode`/
//!   `ColumnIndex` impls from `Database` alone (sqlx-core implements
//!   them per concrete backend, "required due to lack of lazy
//!   normalization"), so the probe-verified stack (RESEARCH Pattern 1,
//!   plus the `i64` pair for generated role ids) rides on the two
//!   execution impls below. Instantiating `SqlxStore<Postgres>`
//!   satisfies every bound implicitly: consumers never spell one.
//! - **Two choke points.** Every statement passes through
//!   [`SqlxStore::fetch_rows`] or [`SqlxStore::execute_statement`] -
//!   the single seam the later query counter plugs into (04-06).
//! - **`AssertSqlSafe`.** sqlx 0.9's `SqlSafeStr` gate demands an
//!   explicit audit of runtime-built SQL; the audit is real and lives on
//!   the choke points (T-04-04): table names passed the D-08 allow-list
//!   and are per-engine quoted; every value travels as a bind.
//! - **Count by selection.** `rows_affected` is unreachable on generic
//!   `DB::QueryResult` (Pitfall 5), so removed-link and scope-delete
//!   counts derive from SELECTs inside the same transaction.
//! - **One transaction per choreography** (SC-5 / prohibition 3): the
//!   remove sweep and the scope delete begin, run, and commit on the
//!   single SPI connection; pool handles never execute multi-statement
//!   choreographies because each statement could land on a different
//!   connection.
//!
//! `ResourceStore` is deliberately absent here: it lands with the
//! resource-side expansion in the next plan of this phase.

use core::future::Future;
use std::marker::PhantomData;

use rolify_core::catalog::RoleCatalogQuery;
use rolify_core::config::{RolifyConfig, RolifyConfigBuilder};
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord, SCOPE_SENTINEL};
use rolify_core::store::{RemovalOutcome, RoleStore, ScopeColumn, Sealed};
use sqlx::database::Database;
use sqlx::query::Query;
use sqlx::{AssertSqlSafe, Connection as _, Executor, IntoArguments};

use crate::dialect::{placeholder, quote_identifier};
use crate::error::Error;
use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
use crate::sentinel::{resource_id_to_storage, to_storage};
use crate::sql;

/// The bind count ahead of the ladder/strict fragments in holder reads
/// (the `link.user_id` bind is always first).
const HOLDER_BIND_COUNT: usize = 1;

/// The generic async store over any `sqlx` backend: holds the configured,
/// per-engine-quoted table names and the resource-table registry.
///
/// Construct via [`SqlxStore::new`] (config-derived names), then chain
/// [`SqlxStore::for_holder_table`] for the user-class finder reads and
/// [`SqlxStore::register_resource_table`] per resource type for the
/// class-scope expansion that lands with `ResourceStore`.
///
/// The engine is the type parameter: `SqlxStore<sqlx::Postgres>`,
/// `SqlxStore<sqlx::MySql>`, or `SqlxStore<sqlx::Sqlite>` - or all three
/// in one build, since the engine features are additive (D-13).
#[derive(Debug, Clone)]
pub struct SqlxStore<DB: Database> {
    role_table: String,
    join_table: String,
    holder_table: Option<String>,
    resource_tables: Vec<(String, String, String)>,
    engine: PhantomData<fn() -> DB>,
}

impl<DB: Database> SqlxStore<DB> {
    /// Create a store from a validated [`RolifyConfig`], quoting the
    /// config's table names for this store's engine.
    #[must_use]
    pub fn new(config: &RolifyConfig) -> Self {
        Self {
            role_table: quote_identifier::<DB>(config.role_table()),
            join_table: quote_identifier::<DB>(config.join_table()),
            holder_table: None,
            resource_tables: Vec::new(),
            engine: PhantomData,
        }
    }

    /// Set the holder table (e.g. `"users"`, `"customers"`) for
    /// `holders_where` / `all_holders` (the user-class finder reads).
    ///
    /// The name passes the D-08 allow-list before quoting, mirroring the
    /// diesel reference boundary.
    ///
    /// # Panics
    ///
    /// Panics when `holder_table` violates the identifier allow-list
    /// (`^[A-Za-z_][A-Za-z0-9_]*$`); configuration errors are
    /// programmer errors and the diesel reference panics identically.
    #[must_use]
    pub fn for_holder_table(mut self, holder_table: &str) -> Self {
        RolifyConfigBuilder::validate_identifier(holder_table)
            .expect("holder table name must pass validation");
        self.holder_table = Some(quote_identifier::<DB>(holder_table));
        self
    }

    /// Register a resource type for the class-scope expansion of the
    /// resource-side finders: `type_name` is the STI type (e.g.
    /// `"Forum"`), `table_name` the storage table (e.g. `"forums"`), and
    /// `pk_column` its primary key column (`"id"` or the string-PK case
    /// like `"team_code"`).
    ///
    /// # Panics
    ///
    /// Panics when any of the three names violates the identifier
    /// allow-list (same D-08 boundary as [`SqlxStore::for_holder_table`]).
    #[must_use]
    pub fn register_resource_table(
        mut self,
        type_name: &str,
        table_name: &str,
        pk_column: &str,
    ) -> Self {
        RolifyConfigBuilder::validate_identifier(type_name)
            .expect("resource type name must pass validation");
        RolifyConfigBuilder::validate_identifier(table_name)
            .expect("resource table name must pass validation");
        RolifyConfigBuilder::validate_identifier(pk_column)
            .expect("primary key column name must pass validation");
        self.resource_tables.push((
            type_name.to_owned(),
            quote_identifier::<DB>(table_name),
            pk_column.to_owned(),
        ));
        self
    }

    /// The per-engine-quoted role table name.
    #[must_use]
    pub fn role_table(&self) -> &str {
        &self.role_table
    }

    /// The per-engine-quoted join table name.
    #[must_use]
    pub fn join_table(&self) -> &str {
        &self.join_table
    }

    /// The per-engine-quoted holder table name, when configured.
    #[must_use]
    pub fn holder_table(&self) -> Option<&str> {
        self.holder_table.as_deref()
    }

    /// The registered resource tables: `(type_name, quoted table, pk
    /// column)` triples.
    #[must_use]
    pub fn resource_tables(&self) -> &[(String, String, String)] {
        &self.resource_tables
    }

    /// The holder table reference for SQL, defaulting to `"users"` when
    /// never configured (the gem's default join counterpart).
    fn holder_table_sql(&self) -> String {
        self.holder_table
            .clone()
            .unwrap_or_else(|| quote_identifier::<DB>("users"))
    }
}

impl<DB: Database> Sealed for SqlxStore<DB> {}

/// One runtime bind value: the typed, ordered payload of a statement.
///
/// Every consumer value (role name, scope columns, holder id, resource
/// type, resource id) is text; the generated role ids the join table
/// and the orphan sweep reference are the only integer binds.
#[derive(Debug, Clone)]
enum BindValue {
    /// A text bind (names, scope columns, stringified ids).
    Text(String),
    /// A generated role-row id bind.
    Integer(i64),
}

impl<DB> SqlxStore<DB>
where
    DB: Database,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
    for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    /// Attach one bind to a statement, in order.
    fn bind_value<'q>(
        statement: Query<'q, DB, <DB as Database>::Arguments>,
        value: &BindValue,
    ) -> Query<'q, DB, <DB as Database>::Arguments> {
        match value {
            BindValue::Text(text) => statement.bind(text.clone()),
            BindValue::Integer(number) => statement.bind(*number),
        }
    }

    /// Fetch choke point: EVERY select flows through here, so a later
    /// query-count seam has one instrumentation point (04-06).
    ///
    /// AUDIT (T-04-04, sqlx 0.9 `SqlSafeStr` gate): the SQL text is
    /// runtime-built; the ONLY interpolated fragments are table and
    /// column names that passed the D-08 allow-list
    /// (`RolifyConfigBuilder::validate_identifier`) and were quoted per
    /// engine by `dialect::quote_identifier`. Every VALUE travels as an
    /// ordered runtime bind and never interpolates.
    async fn fetch_rows(
        conn: &mut <DB as Database>::Connection,
        sql_text: String,
        binds: &[BindValue],
    ) -> Result<Vec<<DB as Database>::Row>, Error> {
        let mut statement = sqlx::query::<DB>(AssertSqlSafe(sql_text.as_str()));
        for value in binds {
            statement = Self::bind_value(statement, value);
        }
        Ok(statement.fetch_all(&mut *conn).await?)
    }

    /// Execute choke point: EVERY mutation flows through here (same
    /// audit trail and seam contract as [`Self::fetch_rows`]).
    async fn execute_statement(
        conn: &mut <DB as Database>::Connection,
        sql_text: String,
        binds: &[BindValue],
    ) -> Result<(), Error> {
        let mut statement = sqlx::query::<DB>(AssertSqlSafe(sql_text.as_str()));
        for value in binds {
            statement = Self::bind_value(statement, value);
        }
        statement.execute(&mut *conn).await?;
        Ok(())
    }

    /// Decode `name, resource_type, resource_id` projection rows into
    /// semantic records (the sentinel translation happens in `rows.rs`).
    fn decode_role_rows(rows: &[<DB as Database>::Row]) -> Result<Vec<RoleRecord>, Error> {
        rows.iter()
            .map(|row| RoleRow::from_row::<DB>(row).map(|role_row| role_row.to_record()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Decode `id` projection rows into generated role ids.
    fn decode_id_rows(rows: &[<DB as Database>::Row]) -> Result<Vec<i64>, Error> {
        rows.iter()
            .map(|row| IdRow::from_row::<DB>(row).map(|id_row| id_row.id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Decode `user_id` projection rows into holder ids (the
    /// string-first, integer-fallback decode covers both fixture PK
    /// shapes without engine-specific casts).
    fn decode_holder_rows(rows: &[<DB as Database>::Row]) -> Result<Vec<ResourceId>, Error> {
        rows.iter()
            .map(|row| {
                HolderIdRow::from_row::<DB>(row)
                    .map(|holder_row| ResourceId::new(holder_row.user_id))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Decode the catalog projection rows into semantic records through
    /// the `ResourceKeyRow` struct (mirrors the diesel reference's
    /// catalog decode path).
    fn decode_catalog_rows(rows: &[<DB as Database>::Row]) -> Result<Vec<RoleRecord>, Error> {
        rows.iter()
            .map(|row| ResourceKeyRow::from_row::<DB>(row).map(|key_row| key_row.to_record()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Gem `find_or_create_by` over the raw triple: SELECT-first, then
    /// INSERT catching the concurrent winner's unique violation, then
    /// re-SELECT (the portable catch, T-04-05). The single INSERT is
    /// atomic on its own, so no transaction is opened here; the method
    /// also runs unchanged inside a caller-owned transaction.
    async fn find_or_create_by_triple(
        conn: &mut <DB as Database>::Connection,
        role_table: &str,
        name: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<RoleRecord, Error> {
        let select_sql = sql::select_role_by_triple::<DB>(role_table);
        let insert_sql = sql::insert_role::<DB>(role_table);
        let triple_binds = [
            BindValue::Text(name.to_owned()),
            BindValue::Text(resource_type.to_owned()),
            BindValue::Text(resource_id.to_owned()),
        ];

        // SELECT first: the common case is an existing row.
        let rows = Self::fetch_rows(conn, select_sql.clone(), &triple_binds).await?;
        if let Some(row) = rows.into_iter().next() {
            return Ok(RoleRow::from_row::<DB>(&row)?.to_record());
        }

        // INSERT; a unique violation means another task won the race
        // (the DB-level UNIQUE triple is the arbiter).
        match Self::execute_statement(conn, insert_sql, &triple_binds).await {
            Ok(()) => {}
            Err(Error::Sqlx(ref error)) if is_unique_violation(error) => {}
            Err(error) => return Err(error),
        }

        // Re-SELECT: either we inserted, or the concurrent winner did.
        let rows = Self::fetch_rows(conn, select_sql, &triple_binds).await?;
        match rows.into_iter().next() {
            Some(row) => Ok(RoleRow::from_row::<DB>(&row)?.to_record()),
            None => Err(Error::Sqlx(sqlx::Error::RowNotFound)),
        }
    }

    /// Resolve the generated id of a role row by its exact triple.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error::RowNotFound` (wrapped in [`Error::Sqlx`])
    /// when no row carries the triple.
    async fn get_role_id_by_triple(
        conn: &mut <DB as Database>::Connection,
        role_table: &str,
        name: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<i64, Error> {
        let select_sql = sql::select_role_id_by_triple::<DB>(role_table);
        let triple_binds = [
            BindValue::Text(name.to_owned()),
            BindValue::Text(resource_type.to_owned()),
            BindValue::Text(resource_id.to_owned()),
        ];
        let rows = Self::fetch_rows(conn, select_sql, &triple_binds).await?;
        match rows.into_iter().next() {
            Some(row) => Ok(IdRow::from_row::<DB>(&row)?.id),
            None => Err(Error::Sqlx(sqlx::Error::RowNotFound)),
        }
    }
}

/// Portable unique-violation detection (T-04-05):
/// `DatabaseError::code()` is the engine's SQLSTATE-like code. Recorded
/// from the 04-01 migration probes on real engines: Postgres reports
/// `23505`, MySQL reports `23000` (the SQLSTATE through `code()`; the
/// native `1062` exists only on `MySqlDatabaseError::number()` and is
/// caught here as well for drivers that surface it through `code()`),
/// and SQLite reports the extended result code `2067`
/// (`SQLITE_CONSTRAINT_UNIQUE`).
fn is_unique_violation(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(database_error) => matches!(
            database_error.code().as_deref(),
            Some("23505" | "23000" | "1062" | "2067")
        ),
        _ => false,
    }
}

/// Append the non-strict ladder binds for `query`, in template order
/// (one bind per placeholder occurrence: the name re-binds in every
/// disjunct).
fn push_ladder_binds(binds: &mut Vec<BindValue>, query: &RoleQuery<'_>) {
    let name = query.name.as_str();
    let sentinel = SCOPE_SENTINEL;
    match &query.filter {
        ResourceFilter::Any => binds.push(BindValue::Text(name.to_owned())),
        ResourceFilter::Global => {
            binds.push(BindValue::Text(name.to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
        }
        ResourceFilter::Class(type_name) => {
            binds.push(BindValue::Text(name.to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
            binds.push(BindValue::Text(name.to_owned()));
            binds.push(BindValue::Text((*type_name).to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
        }
        ResourceFilter::Instance(type_name, resource_id) => {
            binds.push(BindValue::Text(name.to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
            binds.push(BindValue::Text(name.to_owned()));
            binds.push(BindValue::Text((*type_name).to_owned()));
            binds.push(BindValue::Text(sentinel.to_owned()));
            binds.push(BindValue::Text(name.to_owned()));
            binds.push(BindValue::Text((*type_name).to_owned()));
            binds.push(BindValue::Text(resource_id.as_str().to_owned()));
        }
    }
}

/// Append the strict binds for `query`: name plus the exact scope pair
/// (the sentinel standing in for absent scope columns).
fn push_strict_binds(binds: &mut Vec<BindValue>, query: &RoleQuery<'_>) {
    match &query.filter {
        ResourceFilter::Any => binds.push(BindValue::Text(query.name.as_str().to_owned())),
        ResourceFilter::Global => {
            binds.push(BindValue::Text(query.name.as_str().to_owned()));
            binds.push(BindValue::Text(SCOPE_SENTINEL.to_owned()));
            binds.push(BindValue::Text(SCOPE_SENTINEL.to_owned()));
        }
        ResourceFilter::Class(type_name) => {
            binds.push(BindValue::Text(query.name.as_str().to_owned()));
            binds.push(BindValue::Text((*type_name).to_owned()));
            binds.push(BindValue::Text(SCOPE_SENTINEL.to_owned()));
        }
        ResourceFilter::Instance(type_name, resource_id) => {
            binds.push(BindValue::Text(query.name.as_str().to_owned()));
            binds.push(BindValue::Text((*type_name).to_owned()));
            binds.push(BindValue::Text(resource_id.as_str().to_owned()));
        }
    }
}

/// The removal binds: holder, name, then the target's conjunctive
/// conditions, in `delete_links_for_target` / `select_affected_roles`
/// order.
fn removal_binds(holder: &str, name: &str, target: &RemovalTarget<'_>) -> Vec<BindValue> {
    let mut binds = vec![
        BindValue::Text(holder.to_owned()),
        BindValue::Text(name.to_owned()),
    ];
    match target {
        RemovalTarget::NameOnly => {}
        RemovalTarget::TypeSweep(type_name) => {
            binds.push(BindValue::Text((*type_name).to_owned()));
        }
        RemovalTarget::Exact(type_name, resource_id) => {
            binds.push(BindValue::Text((*type_name).to_owned()));
            binds.push(BindValue::Text(resource_id.as_str().to_owned()));
        }
    }
    binds
}

/// The holder-side `rolify_type` filter fragment with one placeholder
/// per type (bind order: types first, then the ladder/strict fragment).
fn holder_type_filter<DB: Database>(holder_alias: Option<&str>, type_count: usize) -> String {
    let placeholders: Vec<String> = (1..=type_count)
        .map(|index| placeholder::<DB>(index))
        .collect();
    match holder_alias {
        Some(alias) => format!("{alias}.rolify_type IN ({})", placeholders.join(", ")),
        None => format!("rolify_type IN ({})", placeholders.join(", ")),
    }
}

impl<DB> RoleStore for SqlxStore<DB>
where
    DB: Database,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    for<'q> &'q str: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'r> i64: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
    for<'r> usize: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    type Conn = <DB as Database>::Connection;
    type Error = Error;

    /// Gem `where` (`role_adapter.rb:106-121` through `build_query`):
    /// the non-strict three-disjunct ladder scoped to the holder.
    fn where_(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let join_table = self.join_table.clone();
        async move {
            let (where_clause, _) = sql::build_ladder_where::<DB>(query, HOLDER_BIND_COUNT + 1);
            let sql_text =
                sql::select_roles_for_holder::<DB>(&role_table, &join_table, &where_clause);
            let mut binds = vec![BindValue::Text(holder.as_str().to_owned())];
            push_ladder_binds(&mut binds, query);
            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            Self::decode_role_rows(&rows)
        }
    }

    /// Gem `where_strict` (`role_adapter.rb:11-26`): exact scope, no
    /// override ladder.
    fn where_strict(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let join_table = self.join_table.clone();
        async move {
            let (where_clause, _) = sql::build_strict_where::<DB>(query, HOLDER_BIND_COUNT + 1);
            let sql_text =
                sql::select_roles_for_holder::<DB>(&role_table, &join_table, &where_clause);
            let mut binds = vec![BindValue::Text(holder.as_str().to_owned())];
            push_strict_binds(&mut binds, query);
            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            Self::decode_role_rows(&rows)
        }
    }

    /// Gem `where` with several conditions OR-joined
    /// (`build_conditions`, `role_adapter.rb:88-104`): ONE round-trip,
    /// never N sequential checks.
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
            let (where_clause, _) = sql::build_any_where::<DB>(queries, HOLDER_BIND_COUNT + 1);
            let sql_text =
                sql::select_roles_for_holder::<DB>(&role_table, &join_table, &where_clause);
            let mut binds = vec![BindValue::Text(holder.as_str().to_owned())];
            for query in queries {
                push_ladder_binds(&mut binds, query);
            }
            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            let records = Self::decode_role_rows(&rows)?;
            // Set semantics: one row can satisfy several OR-joined
            // ladders; the result carries it once.
            let mut seen: Vec<RoleRecord> = Vec::with_capacity(records.len());
            for record in records {
                if !seen.contains(&record) {
                    seen.push(record);
                }
            }
            Ok(seen)
        }
    }

    /// Gem `find_or_create_by`: role-row dedupe on the exact triple
    /// (idempotence level 1; the link guard lives in [`Self::add`]).
    fn find_or_create_by(
        &mut self,
        conn: &mut Self::Conn,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> impl Future<Output = Result<RoleRecord, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let (name_text, resource_type, resource_id) = scope_to_triple(name, scope);
        async move {
            Self::find_or_create_by_triple(
                conn,
                &role_table,
                &name_text,
                &resource_type,
                &resource_id,
            )
            .await
        }
    }

    /// Gem `add` (`role_adapter.rb:52-54`):
    /// `relation.roles << role unless relation.roles.include?(role)` -
    /// line-idempotent link creation (level-2 dedupe through the join
    /// UNIQUE pair plus the portable catch).
    fn add(
        &mut self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        role: &RoleRecord,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let join_table = self.join_table.clone();
        let holder_text = holder.as_str().to_owned();
        let name_text = role.name.as_str().to_owned();
        let resource_type = to_storage(role.resource_type.as_deref()).to_owned();
        let resource_id = resource_id_to_storage(role.resource_id.as_ref()).to_owned();
        async move {
            // Level-1: the role row must exist even for a hand-built
            // record (the diesel reference ensures the same).
            Self::find_or_create_by_triple(
                conn,
                &role_table,
                &name_text,
                &resource_type,
                &resource_id,
            )
            .await?;
            let role_id = Self::get_role_id_by_triple(
                conn,
                &role_table,
                &name_text,
                &resource_type,
                &resource_id,
            )
            .await?;

            let sql_text = sql::insert_link::<DB>(&join_table);
            let binds = [BindValue::Text(holder_text), BindValue::Integer(role_id)];
            match Self::execute_statement(conn, sql_text, &binds).await {
                Ok(()) => Ok(true),
                // Already linked: the join UNIQUE pair is the arbiter
                // (catch-and-ignore, T-04-05).
                Err(Error::Sqlx(ref error)) if is_unique_violation(error) => Ok(false),
                Err(error) => Err(error),
            }
        }
    }

    /// Gem `remove` (`role_adapter.rb:58-70`): delete the holder's links
    /// under the conjunctive `target` sweep, then the `remove_role_if_empty`
    /// orphan cleanup - the whole choreography inside ONE transaction on
    /// the ONE SPI connection (SC-5 / prohibition 3).
    fn remove(
        &mut self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        name: &RoleName,
        target: RemovalTarget<'_>,
        remove_role_if_empty: bool,
    ) -> impl Future<Output = Result<RemovalOutcome, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let join_table = self.join_table.clone();
        async move {
            // ONE transaction on ONE connection: the affected-roles
            // SELECT, the link DELETE, and the orphan sweep commit or
            // roll back together.
            let mut transaction = conn.begin().await?;

            let holder_text = holder.as_str().to_owned();
            let name_text = name.as_str().to_owned();

            let affected_sql = sql::select_affected_roles::<DB>(&role_table, &join_table, &target);
            let affected_binds = removal_binds(&holder_text, &name_text, &target);
            let rows = Self::fetch_rows(&mut *transaction, affected_sql, &affected_binds).await?;
            let affected_records = Self::decode_role_rows(&rows)?;

            // Count by selection (Pitfall 5): with UNIQUE(user_id,
            // role_id), each affected role row has exactly one link for
            // this holder, so the selection length IS the deleted-link
            // count.
            let removed_links = affected_records.len();

            let delete_sql = sql::delete_links_for_target::<DB>(&join_table, &role_table, &target);
            Self::execute_statement(&mut *transaction, delete_sql, &affected_binds).await?;

            let mut removed_roles = Vec::new();
            if remove_role_if_empty {
                for record in &affected_records {
                    let resource_type = to_storage(record.resource_type.as_deref()).to_owned();
                    let resource_id =
                        resource_id_to_storage(record.resource_id.as_ref()).to_owned();
                    // Resolve the row id; a concurrent remove may have
                    // swept the row already (nothing left to clean).
                    let role_id = match Self::get_role_id_by_triple(
                        &mut *transaction,
                        &role_table,
                        record.name.as_str(),
                        &resource_type,
                        &resource_id,
                    )
                    .await
                    {
                        Ok(role_id) => role_id,
                        Err(Error::Sqlx(sqlx::Error::RowNotFound)) => continue,
                        Err(error) => return Err(error),
                    };

                    // Orphan decision BEFORE deleting: rows_affected is
                    // unreachable on generic DB (Pitfall 5).
                    let link_exists_sql = sql::select_link_exists_for_role::<DB>(&join_table);
                    let link_binds = [BindValue::Integer(role_id)];
                    let remaining =
                        Self::fetch_rows(&mut *transaction, link_exists_sql, &link_binds).await?;
                    if !remaining.is_empty() {
                        continue;
                    }

                    let orphan_sql = sql::delete_orphan_role::<DB>(&role_table, &join_table);
                    let orphan_binds = [BindValue::Integer(role_id), BindValue::Integer(role_id)];
                    Self::execute_statement(&mut *transaction, orphan_sql, &orphan_binds).await?;
                    removed_roles.push(record.clone());
                }
            }

            transaction.commit().await?;
            Ok(RemovalOutcome {
                removed_links,
                removed_roles,
            })
        }
    }

    /// Gem `exists?` (`role_adapter.rb:72-74`): whether the holder holds
    /// any row whose scope column is set (sentinel `!= ''`, never
    /// `IS NOT NULL`).
    fn exists(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        column: ScopeColumn,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let join_table = self.join_table.clone();
        async move {
            let sql_text = sql::select_scoped_exists::<DB>(&role_table, &join_table, column);
            let binds = [BindValue::Text(holder.as_str().to_owned())];
            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            Ok(!rows.is_empty())
        }
    }

    /// All role rows linked to the holder (the `user.roles` association
    /// read).
    fn roles_of(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let join_table = self.join_table.clone();
        async move {
            let sql_text = sql::select_roles_of::<DB>(&role_table, &join_table);
            let binds = [BindValue::Text(holder.as_str().to_owned())];
            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            Self::decode_role_rows(&rows)
        }
    }

    /// User-class finder read (D-01, `finders.rb:5-9`): the holder ids
    /// registered under one of `holder_types` whose LINKED rows satisfy
    /// the ladder (or the strict predicates when `strict`), in ONE
    /// round-trip.
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
            let type_filter = holder_type_filter::<DB>(Some("holder"), holder_types.len());
            let fragment_start = holder_types.len() + 1;
            let (where_clause, _) = if strict {
                sql::build_strict_where::<DB>(query, fragment_start)
            } else {
                sql::build_ladder_where::<DB>(query, fragment_start)
            };
            let sql_text = sql::select_holders_where(
                &holder_table,
                &join_table,
                &role_table,
                &type_filter,
                &where_clause,
            );
            let mut binds: Vec<BindValue> = holder_types
                .iter()
                .map(|type_name| BindValue::Text((*type_name).to_owned()))
                .collect();
            if strict {
                push_strict_binds(&mut binds, query);
            } else {
                push_ladder_binds(&mut binds, query);
            }
            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            Self::decode_holder_rows(&rows)
        }
    }

    /// The FULL holder universe for the given types (D-02): including
    /// never-rolificated holders, the base `without_role` subtracts from.
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
            let type_filter = holder_type_filter::<DB>(None, holder_types.len());
            let sql_text = sql::select_all_holders(&holder_table, &type_filter);
            let binds: Vec<BindValue> = holder_types
                .iter()
                .map(|type_name| BindValue::Text((*type_name).to_owned()))
                .collect();
            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            Self::decode_holder_rows(&rows)
        }
    }

    /// Gem `find_roles` catalog branch (`resource_adapter.rb:6-11`): the
    /// ONE catalog read per adapter (D-15), with the filter semantics
    /// documented on the SPI member.
    fn roles_matching(
        &self,
        conn: &mut Self::Conn,
        query: &RoleCatalogQuery<'_>,
    ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let join_table = self.join_table.clone();
        let holder_table = self.holder_table_sql();
        async move {
            if query.types.is_empty() {
                return Ok(Vec::new());
            }
            let has_holder = query.holder.is_some();
            let base_sql = sql::select_roles_matching(
                &role_table,
                &join_table,
                has_holder.then_some(holder_table.as_str()),
                has_holder,
            );
            let (type_filter, next_index) = sql::roles_matching_type_filter::<DB>(query.types, 1);
            let (name_filter, next_index) = if query.name.is_some() {
                sql::roles_matching_name_filter::<DB>(next_index)
            } else {
                (String::new(), next_index)
            };
            let (scope_filter, next_index) =
                sql::roles_matching_scope_filter::<DB>(&query.scope, next_index);
            let (holder_filter, _) = if has_holder {
                sql::roles_matching_holder_filter::<DB>(next_index)
            } else {
                (String::new(), next_index)
            };
            let sql_text = base_sql
                .replace("{{type_filter}}", &type_filter)
                .replace("{{name_filter}}", &name_filter)
                .replace("{{scope_filter}}", &scope_filter)
                .replace("{{holder_filter}}", &holder_filter);

            let mut binds: Vec<BindValue> = query
                .types
                .iter()
                .map(|type_name| BindValue::Text((*type_name).to_owned()))
                .collect();
            if let Some(name) = query.name {
                binds.push(BindValue::Text(name.as_str().to_owned()));
            }
            match &query.scope {
                rolify_core::catalog::CatalogScope::ClassOnly => {
                    binds.push(BindValue::Text(SCOPE_SENTINEL.to_owned()));
                }
                rolify_core::catalog::CatalogScope::InstanceOnly {
                    resource_id: Some(resource_id),
                } => binds.push(BindValue::Text(resource_id.as_str().to_owned())),
                rolify_core::catalog::CatalogScope::InstanceOnly { resource_id: None }
                | rolify_core::catalog::CatalogScope::ClassAndInstance => {}
            }
            if let Some(holder) = query.holder {
                binds.push(BindValue::Text(holder.as_str().to_owned()));
            }

            let rows = Self::fetch_rows(conn, sql_text, &binds).await?;
            Self::decode_catalog_rows(&rows)
        }
    }

    /// Resource-scoped role deletion (OQ1 / SC-3 / D-10): delete exactly
    /// the role rows matching `(resource_type, resource_id)`, count by
    /// selection inside one transaction (Pitfall 5). Join rows vanish
    /// via the FK `ON DELETE CASCADE`.
    fn remove_roles_for_scope(
        &mut self,
        conn: &mut Self::Conn,
        resource_type: &str,
        resource_id: &ResourceId,
    ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
        let role_table = self.role_table.clone();
        let type_text = resource_type.to_owned();
        let id_text = resource_id.as_str().to_owned();
        async move {
            // ONE transaction: the ids SELECT and the DELETE see the
            // same rows, so the count and the deleted set cannot drift.
            let mut transaction = conn.begin().await?;

            let ids_sql = sql::select_role_ids_by_scope::<DB>(&role_table);
            let scope_binds = [BindValue::Text(type_text), BindValue::Text(id_text)];
            let rows = Self::fetch_rows(&mut *transaction, ids_sql, &scope_binds).await?;
            let role_ids = Self::decode_id_rows(&rows)?;
            let removed_count = role_ids.len();

            if removed_count > 0 {
                let delete_sql = sql::delete_roles_by_ids::<DB>(&role_table, role_ids.len());
                let delete_binds: Vec<BindValue> = role_ids
                    .iter()
                    .map(|&role_id| BindValue::Integer(role_id))
                    .collect();
                Self::execute_statement(&mut *transaction, delete_sql, &delete_binds).await?;
            }

            transaction.commit().await?;
            Ok(removed_count)
        }
    }
}

/// Decompose a write scope into its physical triple binds (the sentinel
/// for absent scope columns, D-02).
fn scope_to_triple(name: &RoleName, scope: ResourceRef<'_>) -> (String, String, String) {
    let (resource_type, resource_id) = match scope {
        ResourceRef::Global => (SCOPE_SENTINEL.to_owned(), SCOPE_SENTINEL.to_owned()),
        ResourceRef::Class(type_name) => (type_name.to_owned(), SCOPE_SENTINEL.to_owned()),
        ResourceRef::Instance(type_name, resource_id) => {
            (type_name.to_owned(), resource_id.as_str().to_owned())
        }
    };
    (name.as_str().to_owned(), resource_type, resource_id)
}
