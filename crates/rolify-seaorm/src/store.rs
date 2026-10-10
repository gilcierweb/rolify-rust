//! `SeaormStore`: the `RoleStore` + `ResourceStore` implementation for
//! `SeaORM` 2.0 (async-only, D-05).
//!
//! The store is generic over `C: ConnectionTrait`, so a
//! `DatabaseConnection` (pool), a loose connection, or a caller-owned
//! `DatabaseTransaction` all satisfy the same code path (SC-5); the trait
//! is not dyn-compatible, so never `Box<dyn ConnectionTrait>` (RESEARCH
//! Pitfall 2).
//!
//! Query strategy (D-01 hybrid, D-03 cut):
//! - Ladder reads (`where_`/`where_strict`/`where_any`) run as raw
//!   `Statement`s built by `crate::ladder` (1:1 with
//!   `role_adapter.rb:106-121`) via `query_all_raw` with bound values.
//! - Role/link writes and simple role-row reads use the static entities.
//! - Finder/catalog reads that must join consumer tables (`holders_where`,
//!   `all_holders`, `roles_matching`, `resources_find`) are raw
//!   statements by construction: the holder/resource table names come from
//!   the consumer registry at runtime, which static entities cannot
//!   express (the diesel/sqlx adapters follow the same raw-SQL line for
//!   these reads).
//!
//! Every value travels as a bind; only allow-list-validated identifiers
//! interpolate (quoted per backend). Role names and ids compare
//! byte-exact; scope columns use the `''` sentinel (never `IS NULL`).

use std::fmt::Write as _;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use maybe_async::maybe_async;
use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
use rolify_core::config::{HolderIdKind, RolifyConfig, RolifyConfigBuilder};
use rolify_core::error::RolifyError;
use rolify_core::holder::{parse_holder_id, ParsedHolderId};
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord, SCOPE_SENTINEL};
use rolify_core::store::{
    RemovalOutcome, ResourceKey, ResourceStore, RoleStore, ScopeColumn, Sealed,
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, QueryFilter, QueryResult, Set,
    SqlErr, Statement, Value,
};


use crate::entity::role;
use crate::error::Error;
use crate::ladder::{build_any_where, build_ladder_where, build_strict_where, placeholder};

/// `SeaORM` store: config-derived table names plus holder/resource
/// registries, generic over the executor type (D-05/D-09).
///
/// One store per holder/role-table pair. Construct with
/// [`SeaormStore::new`] (config table names) and chain
/// [`SeaormStore::for_holder_table`] / [`SeaormStore::register_resource_table`].
/// The static entities map the canonical `roles`/`users_roles` tables, so
/// custom role/join names in `RolifyConfig` must match the migrated table
/// names (the Migrator emits the canonical names; consumers renaming
/// tables adapt the emitted migration accordingly, same contract as the
/// diesel adapter's CLI-emitted schema).
#[derive(Debug)]
pub struct SeaormStore<C: ConnectionTrait> {
    role_table: String,
    join_table: String,
    holder_table: Option<String>,
    resource_tables: Vec<(String, String, String)>, // (type_name, table_name, pk_column)
    query_counter: Arc<AtomicUsize>,
    holder_id_kind: HolderIdKind,
    _executor: PhantomData<fn() -> C>,
}

impl<C: ConnectionTrait + Send + 'static> Clone for SeaormStore<C> {
    fn clone(&self) -> Self {
        Self {
            role_table: self.role_table.clone(),
            join_table: self.join_table.clone(),
            holder_table: self.holder_table.clone(),
            resource_tables: self.resource_tables.clone(),
            query_counter: Arc::clone(&self.query_counter),
            holder_id_kind: self.holder_id_kind,
            _executor: PhantomData,
        }
    }
}

impl<C: ConnectionTrait> SeaormStore<C> {
    /// Create a store from a validated `RolifyConfig`.
    #[must_use]
    pub fn new(config: &RolifyConfig) -> Self {
        Self {
            role_table: config.role_table().to_owned(),
            join_table: config.join_table().to_owned(),
            holder_table: None,
            resource_tables: Vec::new(),
            query_counter: Arc::new(AtomicUsize::new(0)),
            holder_id_kind: config.holder_id_kind(),
            _executor: PhantomData,
        }
    }

    /// Set the holder table (e.g., "users", "customers") for
    /// `holders_where` / `all_holders`; validated by the config allow-list.
    ///
    /// # Panics
    ///
    /// Panics when `holder_table` fails the identifier allow-list
    /// (`^[A-Za-z_][A-Za-z0-9_]*$`).
    #[must_use]
    pub fn for_holder_table(mut self, holder_table: &str) -> Self {
        RolifyConfigBuilder::validate_identifier(holder_table)
            .expect("holder table name must pass validation");
        self.holder_table = Some(holder_table.to_owned());
        self
    }

    /// Register a resource type for class-scope expansion in
    /// `resources_find` (STI family expansion per `relation_types_for`,
    /// base.rb:27-28).
    ///
    /// # Panics
    ///
    /// Panics when any identifier fails the allow-list.
    #[must_use]
    pub fn register_resource_table(
        mut self,
        type_name: &str,
        table_name: &str,
        pk_column: &str,
    ) -> Self {
        RolifyConfigBuilder::validate_identifier(table_name)
            .expect("resource table name must pass validation");
        RolifyConfigBuilder::validate_identifier(pk_column)
            .expect("pk column name must pass validation");
        RolifyConfigBuilder::validate_identifier(type_name)
            .expect("type name must pass validation");
        self.resource_tables.push((
            type_name.to_owned(),
            table_name.to_owned(),
            pk_column.to_owned(),
        ));
        self
    }

    /// Query counter for the TEST-05 guard tests (`rolify-test` backend
    /// seam; mirrors the sqlx adapter's explicit counter: `SeaORM` exposes
    /// no instrumentation hook).
    #[must_use]
    pub fn query_count(&self) -> usize {
        self.query_counter.load(Ordering::Relaxed)
    }

    /// Reset the query counter (test seam).
    pub fn reset_query_count(&self) {
        self.query_counter.store(0, Ordering::Relaxed);
    }

    /// Shared handle to the query counter (test seam: the suite backend
    /// reads the count without borrowing the store mutably).
    #[doc(hidden)]
    #[must_use]
    pub fn query_counter_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.query_counter)
    }

    fn bump(&self, by: usize) {
        self.query_counter.fetch_add(by, Ordering::Relaxed);
    }

    /// `(name, resource_type, resource_id)` physical triple for a write
    /// scope (D-06 sentinel edge; mirrors diesel's `scope_to_triple`).
    fn scope_to_triple(name: &RoleName, scope: ResourceRef<'_>) -> (String, String, String) {
        let (resource_type, resource_id) = match scope {
            ResourceRef::Global => (SCOPE_SENTINEL.to_owned(), SCOPE_SENTINEL.to_owned()),
            ResourceRef::Class(type_name) => (type_name.to_owned(), SCOPE_SENTINEL.to_owned()),
            ResourceRef::Instance(type_name, id) => (type_name.to_owned(), id.as_str().to_owned()),
        };
        (name.as_str().to_owned(), resource_type, resource_id)
    }
}

impl<C: ConnectionTrait + Send + 'static> Sealed for SeaormStore<C> {}

/// Quote a validated identifier for the backend (`"name"` on Postgres,
/// backticks elsewhere). Only config/registry-validated names reach this
/// helper; values always travel as binds.
fn quote_identifier(backend: DbBackend, name: &str) -> String {
    match backend {
        DbBackend::Postgres => format!("\"{name}\""),
        _ => format!("`{name}`"),
    }
}

/// Text-cast expression per backend (integer holder PK joins meet the
/// string `user_id` column, the gem's stringified-id contract).
fn cast_to_text(backend: DbBackend, expr: &str) -> String {
    match backend {
        DbBackend::Postgres => format!("CAST({expr} AS TEXT)"),
        _ => format!("CAST({expr} AS CHAR)"),
    }
}

fn backend_of<C: ConnectionTrait>(conn: &C) -> DbBackend {
    conn.get_database_backend()
}

/// Construct a runtime-typed `sea_orm::Value` for a holder id according to the configured kind.
/// Parses and validates the holder id before any SQL executes (D-08-05).
fn holder_value(
    kind: HolderIdKind,
    holder: &ResourceId,
) -> Result<Value, Error> {
    let parsed = parse_holder_id(kind, holder.as_str()).map_err(Error::Core)?;
    Ok(match parsed {
        ParsedHolderId::Integer(n) => Value::BigInt(Some(n)),
        ParsedHolderId::Uuid(u) => {
            // Value::Uuid requires sea-orm/with-uuid feature (enabled in Cargo.toml)
            Value::Uuid(Some(u))
        }
        ParsedHolderId::Text(s) => Value::String(Some(s)),
    })
}

/// Decode a holder id from a query result row according to the configured kind.
/// The projection should be cast to text for integer/uuid kinds (see `cast_to_text`),
/// so String decode works uniformly and we canonicalize via `holder_id_to_string`.
fn decode_holder_id(kind: HolderIdKind, row_user_id: &str) -> ResourceId {
    match kind {
        HolderIdKind::Integer | HolderIdKind::Uuid => {
            // Projection was CAST(... AS TEXT/CHAR), so decode as string and canonicalize
            ResourceId::new(row_user_id)
        }
        HolderIdKind::String => ResourceId::new(row_user_id),
    }
}

/// Compose the `roles_of`/`where_*` SELECT: role rows for one holder
/// under the alias contract the ladder fragments assume (`role_row`
/// alias, join alias `link`). Projection is explicit (never `SELECT *`).
fn select_roles_for_holder(
    backend: DbBackend,
    role_table: &str,
    join_table: &str,
    where_clause: &str,
) -> String {
    let holder_ph = placeholder(backend, 1);
    format!(
        "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
         FROM {} AS role_row \
         INNER JOIN {} AS link ON link.role_id = role_row.id \
         WHERE link.user_id = {holder_ph} AND {where_clause}",
        quote_identifier(backend, role_table),
        quote_identifier(backend, join_table),
    )
}

/// Ladder bind values in placeholder order (name is factored once per
/// ladder; every `?` occurrence needs its own slot on `MySQL`).
fn push_ladder_binds(values: &mut Vec<Value>, query: &RoleQuery<'_>) {
    let name = query.name.as_str();
    match &query.filter {
        ResourceFilter::Any => values.push(Value::from(name.to_owned())),
        ResourceFilter::Global => {
            values.push(Value::from(name.to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
        }
        ResourceFilter::Class(type_name) => {
            values.push(Value::from(name.to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
            values.push(Value::from((*type_name).to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
        }
        ResourceFilter::Instance(type_name, resource_id) => {
            values.push(Value::from(name.to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
            values.push(Value::from((*type_name).to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
            values.push(Value::from((*type_name).to_owned()));
            values.push(Value::from(resource_id.as_str().to_owned()));
        }
    }
}

/// Strict bind values in placeholder order (name, type, id; `Any` keeps
/// the name only).
fn push_strict_binds(values: &mut Vec<Value>, query: &RoleQuery<'_>) {
    match &query.filter {
        ResourceFilter::Any => values.push(Value::from(query.name.as_str().to_owned())),
        ResourceFilter::Global => {
            values.push(Value::from(query.name.as_str().to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
        }
        ResourceFilter::Class(type_name) => {
            values.push(Value::from(query.name.as_str().to_owned()));
            values.push(Value::from((*type_name).to_owned()));
            values.push(Value::from(SCOPE_SENTINEL.to_owned()));
        }
        ResourceFilter::Instance(type_name, resource_id) => {
            values.push(Value::from(query.name.as_str().to_owned()));
            values.push(Value::from((*type_name).to_owned()));
            values.push(Value::from(resource_id.as_str().to_owned()));
        }
    }
}

fn is_unique_violation(err: &DbErr) -> bool {
    // Portable classifier (RESEARCH Pattern 4): sql_err() detects
    // unique-key violations across Postgres and MySQL; SqlErr is
    // non-exhaustive, matches! sidesteps exhaustiveness concerns.
    matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

fn model_from_row(row: &QueryResult) -> Result<RoleRecord, Error> {
    let name: String = row.try_get("", "name").map_err(Error::Db)?;
    let resource_type: String = row.try_get("", "resource_type").map_err(Error::Db)?;
    let resource_id: String = row.try_get("", "resource_id").map_err(Error::Db)?;
    Ok(role::Model {
        id: 0,
        name,
        resource_type,
        resource_id,
    }
    .to_record())
}

#[maybe_async(AFIT)]
impl<C: ConnectionTrait + Send + 'static> RoleStore for SeaormStore<C> {
    type Conn = C;
    type Error = Error;

    /// Gem `where` ladder over the holder's rows (`role_adapter.rb:106-121`).
    async fn where_(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> Result<Vec<RoleRecord>, Self::Error> {
        let backend = backend_of(conn);
        let (where_clause, _) = build_ladder_where(backend, query, 2);
        let sql =
            select_roles_for_holder(backend, &self.role_table, &self.join_table, &where_clause);
        let mut values = vec![holder_value(self.holder_id_kind, holder)?];
        push_ladder_binds(&mut values, query);
        let stmt = Statement::from_sql_and_values(backend, sql, values);
        self.bump(1);
        let rows = conn.query_all_raw(stmt).await.map_err(Error::Db)?;
        rows.iter().map(model_from_row).collect()
    }

    /// Gem `where_strict`: exact scope, no override (role_adapter.rb:11-26).
    async fn where_strict(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        query: &RoleQuery<'_>,
    ) -> Result<Vec<RoleRecord>, Self::Error> {
        let backend = backend_of(conn);
        let (where_clause, _) = build_strict_where(backend, query, 2);
        let sql =
            select_roles_for_holder(backend, &self.role_table, &self.join_table, &where_clause);
        let mut values = vec![holder_value(self.holder_id_kind, holder)?];
        push_strict_binds(&mut values, query);
        let stmt = Statement::from_sql_and_values(backend, sql, values);
        self.bump(1);
        let rows = conn.query_all_raw(stmt).await.map_err(Error::Db)?;
        rows.iter().map(model_from_row).collect()
    }

    /// Gem `where` OR-fold: ONE round-trip for "any of these queries"
    /// (`build_conditions`, role_adapter.rb:88-104).
    async fn where_any(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        queries: &[RoleQuery<'_>],
    ) -> Result<Vec<RoleRecord>, Self::Error> {
        let backend = backend_of(conn);
        let (where_clause, _) = build_any_where(backend, queries, 2);
        let sql =
            select_roles_for_holder(backend, &self.role_table, &self.join_table, &where_clause);
        let mut values = vec![holder_value(self.holder_id_kind, holder)?];
        for query in queries {
            push_ladder_binds(&mut values, query);
        }
        let stmt = Statement::from_sql_and_values(backend, sql, values);
        self.bump(1);
        let rows = conn.query_all_raw(stmt).await.map_err(Error::Db)?;
        rows.iter().map(model_from_row).collect()
    }

    /// Gem `find_or_create_by` (`role_adapter.rb)`: select first, insert,
    /// catch the unique violation via the portable classifier, re-read.
    /// The unique triple is the race arbiter (never pre-check-only).
    async fn find_or_create_by(
        &mut self,
        conn: &mut Self::Conn,
        name: &RoleName,
        scope: ResourceRef<'_>,
    ) -> Result<RoleRecord, Self::Error> {
        let (name_value, resource_type, resource_id) = Self::scope_to_triple(name, scope);
        self.bump(1);
        if let Some(model) = role::Entity::find()
            .filter(role::Column::Name.eq(&name_value))
            .filter(role::Column::ResourceType.eq(&resource_type))
            .filter(role::Column::ResourceId.eq(&resource_id))
            .one(&*conn)
            .await
            .map_err(Error::Db)?
        {
            return Ok(model.to_record());
        }
        // Not found: insert (the unique triple arbitrates races).
        let active = role::ActiveModel {
            name: Set(name_value.clone()),
            resource_type: Set(resource_type.clone()),
            resource_id: Set(resource_id.clone()),
            ..Default::default()
        };
        self.bump(1);
        match role::Entity::insert(active).exec(&*conn).await {
            Ok(_) => {}
            Err(db_err) if is_unique_violation(&db_err) => {}
            Err(other) => return Err(Error::Db(other)),
        }
        // Re-read so a race loser observes the row the winner wrote.
        self.bump(1);
        let model = role::Entity::find()
            .filter(role::Column::Name.eq(&name_value))
            .filter(role::Column::ResourceType.eq(&resource_type))
            .filter(role::Column::ResourceId.eq(&resource_id))
            .one(&*conn)
            .await
            .map_err(Error::Db)?
            .ok_or_else(|| Error::Core(RolifyError::RoleNotFound { name: name_value }))?;
        Ok(model.to_record())
    }

    /// Gem `add` (role_adapter.rb:52-54): idempotent link creation -
    /// returns `false` when the pair already exists (level-2 dedupe).
    async fn add(
        &mut self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        role: &RoleRecord,
    ) -> Result<bool, Self::Error> {
        let resource_type = crate::entity::role::to_storage(role.resource_type.as_deref());
        let resource_id = crate::entity::role::resource_id_to_storage(role.resource_id.as_ref());
        self.bump(1);
        let model = role::Entity::find()
            .filter(role::Column::Name.eq(role.name.as_str()))
            .filter(role::Column::ResourceType.eq(resource_type))
            .filter(role::Column::ResourceId.eq(&resource_id))
            .one(&*conn)
            .await
            .map_err(Error::Db)?
            .ok_or_else(|| {
                Error::Core(RolifyError::RoleNotFound {
                    name: role.name.as_str().to_owned(),
                })
            })?;
        let backend = backend_of(conn);
        let sql = format!(
            "INSERT INTO {} (user_id, role_id) VALUES ({}, {})",
            quote_identifier(backend, &self.join_table),
            placeholder(backend, 1),
            placeholder(backend, 2),
        );
        let holder_val = holder_value(self.holder_id_kind, holder)?;
        let values = vec![holder_val, Value::BigInt(Some(model.id))];
        let stmt = Statement::from_sql_and_values(backend, sql, values);
        self.bump(1);
        match conn.execute_raw(stmt).await {
            Ok(_) => Ok(true),
            Err(db_err) if is_unique_violation(&db_err) => Ok(false),
            Err(other) => Err(Error::Db(other)),
        }
    }

    /// Gem `remove` (role_adapter.rb:58-70): collect affected roles first,
    /// delete the holder's links, then sweep role rows whose last link
    /// vanished when `remove_role_if_empty` is set.
    ///
    /// Single end-to-end choreography mirrors the diesel store (the
    /// line-count posture follows the same documented trade-off).
    #[allow(clippy::too_many_lines)]
    async fn remove(
        &mut self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        name: &RoleName,
        target: RemovalTarget<'_>,
        remove_role_if_empty: bool,
    ) -> Result<RemovalOutcome, Self::Error> {
        let backend = backend_of(conn);
        let role_table = quote_identifier(backend, &self.role_table);
        let join_table = quote_identifier(backend, &self.join_table);

        // Scope filter per RemovalTarget (kernel::removal_match).
        let mut values = vec![
            holder_value(self.holder_id_kind, holder)?,
            Value::from(name.as_str().to_owned()),
        ];
        let scope_filter = match target {
            RemovalTarget::NameOnly => String::new(),
            RemovalTarget::TypeSweep(type_name) => {
                // Class rows AND instance rows of that type (id free).
                values.push(Value::from(type_name.to_owned()));
                format!(" AND role_row.resource_type = {}", placeholder(backend, 3))
            }
            RemovalTarget::Exact(type_name, resource_id) => {
                values.push(Value::from(type_name.to_owned()));
                values.push(Value::from(resource_id.as_str().to_owned()));
                format!(
                    " AND role_row.resource_type = {} AND role_row.resource_id = {}",
                    placeholder(backend, 3),
                    placeholder(backend, 4)
                )
            }
        };

        // Affected role rows (collected BEFORE any deletion).
        let affected_sql = format!(
            "SELECT role_row.id AS id, role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
             FROM {role_table} AS role_row \
             INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
             WHERE link.user_id = {} AND role_row.name = {}{}",
            placeholder(backend, 1),
            placeholder(backend, 2),
            scope_filter
        );
        self.bump(1);
        let affected_rows = conn
            .query_all_raw(Statement::from_sql_and_values(
                backend,
                affected_sql,
                values,
            ))
            .await
            .map_err(Error::Db)?;
        if affected_rows.is_empty() {
            return Ok(RemovalOutcome {
                removed_links: 0,
                removed_roles: Vec::new(),
            });
        }
        let affected: Vec<(i64, RoleRecord)> = affected_rows
            .iter()
            .map(|row| {
                let id: i64 = row.try_get("", "id").map_err(Error::Db)?;
                Ok((id, model_from_row(row)?))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let affected_ids: Vec<i64> = affected.iter().map(|(id, _)| *id).collect();

        // Delete this holder's links to the affected roles.
        let id_list = affected_ids
            .iter()
            .enumerate()
            .map(|(offset, _)| placeholder(backend, offset + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let delete_links_sql = format!(
            "DELETE FROM {join_table} WHERE user_id = {} AND role_id IN ({id_list})",
            placeholder(backend, 1)
        );
        let mut link_values = vec![holder_value(self.holder_id_kind, holder)?];
        for id in &affected_ids {
            link_values.push(Value::from(*id));
        }
        self.bump(1);
        let removed = conn
            .execute_raw(Statement::from_sql_and_values(
                backend,
                delete_links_sql,
                link_values,
            ))
            .await
            .map_err(Error::Db)?;
        let removed_links = usize::try_from(removed.rows_affected()).unwrap_or(usize::MAX);

        // Orphan sweep: drop role rows whose last link just vanished
        // (remove_role_if_empty, role_adapter.rb:66-68).
        let mut removed_roles = Vec::new();
        if remove_role_if_empty {
            let sweep_sql = format!(
                "DELETE FROM {role_table} WHERE id = {} AND NOT EXISTS (SELECT 1 FROM {join_table} WHERE role_id = {})",
                placeholder(backend, 1),
                placeholder(backend, 2)
            );
            for (id, record) in &affected {
                self.bump(1);
                let result = conn
                    .execute_raw(Statement::from_sql_and_values(
                        backend,
                        sweep_sql.clone(),
                        vec![Value::from(*id), Value::from(*id)],
                    ))
                    .await
                    .map_err(Error::Db)?;
                if result.rows_affected() > 0 {
                    removed_roles.push(record.clone());
                }
            }
        }

        Ok(RemovalOutcome {
            removed_links,
            removed_roles,
        })
    }

    /// Gem `exists?` (role_adapter.rb:72-74): does the holder have any
    /// role row with the given scope column set (sentinel `= ''` inversion:
    /// set means `<> ''`).
    async fn exists(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
        column: ScopeColumn,
    ) -> Result<bool, Self::Error> {
        let backend = backend_of(conn);
        let column_name = match column {
            ScopeColumn::ResourceType => "resource_type",
            ScopeColumn::ResourceId => "resource_id",
        };
        let sql = format!(
            "SELECT COUNT(*) AS count FROM {} AS link \
             INNER JOIN {} AS role_row ON role_row.id = link.role_id \
             WHERE link.user_id = {} AND role_row.{column_name} <> ''",
            quote_identifier(backend, &self.join_table),
            quote_identifier(backend, &self.role_table),
            placeholder(backend, 1),
        );
        self.bump(1);
        let row = conn
            .query_one_raw(Statement::from_sql_and_values(
                backend,
                sql,
                vec![holder_value(self.holder_id_kind, holder)?],
            ))
            .await
            .map_err(Error::Db)?
            .expect("COUNT(*) always returns one row");
        let count: i64 = row.try_get("", "count").map_err(Error::Db)?;
        Ok(count > 0)
    }

    /// All role rows linked to `holder` (`user.roles`,
    /// role.rb:77-90 feed).
    async fn roles_of(
        &self,
        conn: &mut Self::Conn,
        holder: &ResourceId,
    ) -> Result<Vec<RoleRecord>, Self::Error> {
        let backend = backend_of(conn);
        let sql = format!(
            "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
             FROM {} AS role_row \
             INNER JOIN {} AS link ON link.role_id = role_row.id \
             WHERE link.user_id = {}",
            quote_identifier(backend, &self.role_table),
            quote_identifier(backend, &self.join_table),
            placeholder(backend, 1),
        );
        self.bump(1);
        let rows = conn
            .query_all_raw(Statement::from_sql_and_values(
                backend,
                sql,
                vec![holder_value(self.holder_id_kind, holder)?],
            ))
            .await
            .map_err(Error::Db)?;
        rows.iter().map(model_from_row).collect()
    }

    /// Holder ids whose linked roles match `query` (finder read): one
    /// round trip; empty `holder_types` matches nothing; `strict` selects
    /// ladder vs strict semantics (the caller resolves the flag).
    async fn holders_where(
        &self,
        conn: &mut Self::Conn,
        holder_types: &[&str],
        query: &RoleQuery<'_>,
        strict: bool,
    ) -> Result<Vec<ResourceId>, Self::Error> {
        if holder_types.is_empty() {
            return Ok(Vec::new());
        }
        let holder_table = self.holder_table.as_deref().ok_or_else(|| {
            Error::Core(RolifyError::InvalidConfig {
                reason: "holders_where requires for_holder_table(..) on the store".to_owned(),
            })
        })?;
        let backend = backend_of(conn);
        let holder_table_quoted = quote_identifier(backend, holder_table);
        let role_table = quote_identifier(backend, &self.role_table);
        let join_table = quote_identifier(backend, &self.join_table);

        // Type binds occupy placeholders 1..=len(types); the ladder starts after.
        let type_placeholders = (1..=holder_types.len())
            .map(|offset| placeholder(backend, offset))
            .collect::<Vec<_>>()
            .join(", ");
        let ladder_start = holder_types.len() + 1;
        let (where_clause, _) = if strict {
            build_strict_where(backend, query, ladder_start)
        } else {
            build_ladder_where(backend, query, ladder_start)
        };

        let sql = format!(
            "SELECT DISTINCT {} AS user_id \
             FROM {holder_table_quoted} AS holder \
             INNER JOIN {join_table} AS link ON link.user_id = {} \
             INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
             WHERE holder.rolify_type IN ({type_placeholders}) AND {where_clause}",
            cast_to_text(backend, "holder.id"),
            cast_to_text(backend, "holder.id"),
        );
        let mut values: Vec<Value> = holder_types
            .iter()
            .map(|type_name| Value::from((*type_name).to_owned()))
            .collect();
        if strict {
            push_strict_binds(&mut values, query);
        } else {
            push_ladder_binds(&mut values, query);
        }
        self.bump(1);
        let rows = conn
            .query_all_raw(Statement::from_sql_and_values(backend, sql, values))
            .await
            .map_err(Error::Db)?;
        let mut ids = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.try_get("", "user_id").map_err(Error::Db)?;
            ids.push(decode_holder_id(self.holder_id_kind, &id));
        }
        Ok(ids)
    }

    /// The FULL holder universe for the given types (including
    /// never-rolified holders; D-02, `finders.rb:13` `all_except` feed).
    async fn all_holders(
        &self,
        conn: &mut Self::Conn,
        holder_types: &[&str],
    ) -> Result<Vec<ResourceId>, Self::Error> {
        if holder_types.is_empty() {
            return Ok(Vec::new());
        }
        let holder_table = self.holder_table.as_deref().ok_or_else(|| {
            Error::Core(RolifyError::InvalidConfig {
                reason: "all_holders requires for_holder_table(..) on the store".to_owned(),
            })
        })?;
        let backend = backend_of(conn);
        let type_placeholders = (1..=holder_types.len())
            .map(|offset| placeholder(backend, offset))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {} AS user_id FROM {} AS holder WHERE holder.rolify_type IN ({type_placeholders})",
            cast_to_text(backend, "holder.id"),
            quote_identifier(backend, holder_table),
        );
        let values: Vec<Value> = holder_types
            .iter()
            .map(|type_name| Value::from((*type_name).to_owned()))
            .collect();
        self.bump(1);
        let rows = conn
            .query_all_raw(Statement::from_sql_and_values(backend, sql, values))
            .await
            .map_err(Error::Db)?;
        let mut ids = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.try_get("", "user_id").map_err(Error::Db)?;
            ids.push(decode_holder_id(self.holder_id_kind, &id));
        }
        Ok(ids)
    }

    /// Gem `find_roles` catalog branch (`resource_adapter.rb:6-11`):
    /// resource rows never global (structural `resource_type != ''`),
    /// `types`/`name`/`scope`/`holder` filters per the SPI contract.
    async fn roles_matching(
        &self,
        conn: &mut Self::Conn,
        query: &RoleCatalogQuery<'_>,
    ) -> Result<Vec<RoleRecord>, Self::Error> {
        if query.types.is_empty() {
            return Ok(Vec::new());
        }
        // A holder filter without a registered holder table would
        // silently read every holder's rows; fail loudly instead (the
        // same InvalidConfig posture as holders_where / all_holders).
        if query.holder.is_some() && self.holder_table.is_none() {
            return Err(Error::Core(RolifyError::InvalidConfig {
                reason:
                    "roles_matching with a holder filter requires for_holder_table(..) on the store"
                        .to_owned(),
            }));
        }
        let backend = backend_of(conn);
        let role_table = quote_identifier(backend, &self.role_table);
        let join_table = quote_identifier(backend, &self.join_table);
        let holder_join = match (&query.holder, self.holder_table.as_deref()) {
            (Some(_), Some(holder_table)) => format!(
                "INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 INNER JOIN {} AS holder ON {} = link.user_id",
                quote_identifier(backend, holder_table),
                cast_to_text(backend, "holder.id"),
            ),
            _ => String::new(),
        };

        let mut values: Vec<Value> = Vec::new();
        let mut index = 1;
        let type_placeholders = (index..=query.types.len())
            .map(|offset| placeholder(backend, offset))
            .collect::<Vec<_>>()
            .join(", ");
        index += query.types.len();
        for type_name in query.types {
            values.push(Value::from((*type_name).to_owned()));
        }
        let mut sql = format!(
            "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
             FROM {role_table} AS role_row {holder_join} \
             WHERE role_row.resource_type != '' AND role_row.resource_type IN ({type_placeholders})"
        );
        if let Some(name) = query.name {
            let _ = write!(sql, " AND role_row.name = {}", placeholder(backend, index));
            values.push(Value::from(name.as_str().to_owned()));
            index += 1;
        }
        match &query.scope {
            CatalogScope::ClassAndInstance => {}
            CatalogScope::ClassOnly => {
                let _ = write!(
                    sql,
                    " AND role_row.resource_id = {}",
                    placeholder(backend, index)
                );
                values.push(Value::from(SCOPE_SENTINEL.to_owned()));
                index += 1;
            }
            CatalogScope::InstanceOnly { resource_id } => match resource_id {
                Some(id) => {
                    let _ = write!(
                        sql,
                        " AND role_row.resource_id = {}",
                        placeholder(backend, index)
                    );
                    values.push(Value::from(id.as_str().to_owned()));
                    index += 1;
                }
                // `None` is the documented "every instance row in
                // types" read (catalog.rs): class rows stay out, the
                // same `resource_id != ''` filter the diesel and sqlx
                // adapters emit.
                None => {
                    let _ = write!(sql, " AND role_row.resource_id != ''");
                }
            },
        }
        if let Some(holder_id) = &query.holder {
            // Holder primary keys are integers while the bind carries
            // the stringified id, so the comparison needs the same
            // text cast the join uses (Postgres rejects integer =
            // text without it).
            let _ = write!(
                sql,
                " AND {} = {}",
                cast_to_text(backend, "holder.id"),
                placeholder(backend, index)
            );
            values.push(holder_value(self.holder_id_kind, holder_id)?);
        }

        self.bump(1);
        let rows = conn
            .query_all_raw(Statement::from_sql_and_values(backend, sql, values))
            .await
            .map_err(Error::Db)?;
        rows.iter().map(model_from_row).collect()
    }

    /// Resource-scoped role deletion (SC-3/D-10): delete exactly the
    /// `(resource_type, resource_id)` pair; links sweep via FK cascade;
    /// class rows (sentinel id) are untouched.
    async fn remove_roles_for_scope(
        &mut self,
        conn: &mut Self::Conn,
        resource_type: &str,
        resource_id: &ResourceId,
    ) -> Result<usize, Self::Error> {
        let backend = backend_of(conn);
        let sql = format!(
            "DELETE FROM {} WHERE resource_type = {} AND resource_id = {}",
            quote_identifier(backend, &self.role_table),
            placeholder(backend, 1),
            placeholder(backend, 2),
        );
        self.bump(1);
        let result = conn
            .execute_raw(Statement::from_sql_and_values(
                backend,
                sql,
                vec![
                    Value::from(resource_type.to_owned()),
                    Value::from(resource_id.as_str().to_owned()),
                ],
            ))
            .await
            .map_err(Error::Db)?;
        Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
    }
}

#[maybe_async(AFIT)]
impl<C: ConnectionTrait + Send + 'static> ResourceStore for SeaormStore<C> {
    type Conn = C;
    type Error = Error;

    /// Gem `resources_find` (`resource_adapter.rb`): resources of the
    /// given STI type family holding `name` at class scope (expanded
    /// through the registry, one joined SELECT per registered type) or at
    /// their own instance scope (one SELECT over stored ids).
    async fn resources_find(
        &self,
        conn: &mut Self::Conn,
        types: &[&str],
        name: &RoleName,
    ) -> Result<Vec<ResourceKey>, Self::Error> {
        if types.is_empty() {
            return Ok(Vec::new());
        }
        let backend = backend_of(conn);
        let role_table = quote_identifier(backend, &self.role_table);
        let mut keys: Vec<ResourceKey> = Vec::new();

        // Instance-scope rows: direct reads of stored (type, id) pairs.
        let type_placeholders = (1..=types.len())
            .map(|offset| placeholder(backend, offset))
            .collect::<Vec<_>>()
            .join(", ");
        let name_ph = placeholder(backend, types.len() + 1);
        let instance_sql = format!(
            "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
             FROM {role_table} AS role_row \
             WHERE role_row.resource_type IN ({type_placeholders}) AND role_row.resource_id != '' AND role_row.name = {name_ph}"
        );
        let mut values: Vec<Value> = types
            .iter()
            .map(|type_name| Value::from((*type_name).to_owned()))
            .collect();
        values.push(Value::from(name.as_str().to_owned()));
        self.bump(1);
        let rows = conn
            .query_all_raw(Statement::from_sql_and_values(
                backend,
                instance_sql,
                values,
            ))
            .await
            .map_err(Error::Db)?;
        for row in &rows {
            let resource_type: String = row.try_get("", "resource_type").map_err(Error::Db)?;
            let resource_id: String = row.try_get("", "resource_id").map_err(Error::Db)?;
            keys.push(ResourceKey::new(
                resource_type,
                ResourceId::new(resource_id),
            ));
        }

        // Class-scope rows: for each registered type, join the resource
        // table so the class row expands to every persisted id of that
        // type (gem class-covers-instance asymmetry).
        for (type_name, table_name, pk_column) in &self.resource_tables {
            if !types.contains(&type_name.as_str()) {
                continue;
            }
            let class_sql = format!(
                "SELECT DISTINCT {} AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {} AS res ON role_row.resource_type = {} AND role_row.resource_id = '' AND role_row.name = {}",
                cast_to_text(backend, &format!("res.{pk_column}")),
                quote_identifier(backend, table_name),
                placeholder(backend, 1),
                placeholder(backend, 2),
            );
            let class_values = vec![
                Value::from(type_name.clone()),
                Value::from(name.as_str().to_owned()),
            ];
            self.bump(1);
            let rows = conn
                .query_all_raw(Statement::from_sql_and_values(
                    backend,
                    class_sql,
                    class_values,
                ))
                .await
                .map_err(Error::Db)?;
            for row in &rows {
                let resource_id: String = row.try_get("", "resource_id").map_err(Error::Db)?;
                keys.push(ResourceKey::new(
                    type_name.clone(),
                    ResourceId::new(resource_id),
                ));
            }
        }

        // Unordered set semantics (D-04): each key appears at most once.
        keys.sort_by(|a, b| {
            a.resource_type
                .cmp(&b.resource_type)
                .then_with(|| a.resource_id.as_str().cmp(b.resource_id.as_str()))
        });
        keys.dedup();
        Ok(keys)
    }

    /// Gem `in` (`resource_adapter.rb:27-30`): among `candidates`, the
    /// resources where `holder` holds any of `names` at class or
    /// instance scope. Coverage is decided caller-side with NO
    /// resource-type check, mirroring the gem's SQL
    /// (`resource_id = pk OR resource_id IS NULL`) and the `InMemory`
    /// reference: a same-id instance row, a class row, or a global row
    /// covers the candidate.
    async fn in_list(
        &self,
        conn: &mut Self::Conn,
        candidates: &[ResourceKey],
        holder: &ResourceId,
        names: &[RoleName],
    ) -> Result<Vec<ResourceKey>, Self::Error> {
        if candidates.is_empty() || names.is_empty() {
            return Ok(Vec::new());
        }
        let backend = backend_of(conn);
        let role_table = quote_identifier(backend, &self.role_table);
        let join_table = quote_identifier(backend, &self.join_table);
        let name_placeholders = (2..=names.len() + 1)
            .map(|offset| placeholder(backend, offset))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
             FROM {role_table} AS role_row \
             INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
             WHERE link.user_id = {} AND role_row.name IN ({name_placeholders})",
            placeholder(backend, 1),
        );
        let mut values = vec![holder_value(self.holder_id_kind, holder)?];
        for name in names {
            values.push(Value::from(name.as_str().to_owned()));
        }
        self.bump(1);
        let rows = conn
            .query_all_raw(Statement::from_sql_and_values(backend, sql, values))
            .await
            .map_err(Error::Db)?;
        let held: Vec<(String, String)> = rows
            .iter()
            .map(|row| {
                Ok((
                    row.try_get("", "resource_type").map_err(Error::Db)?,
                    row.try_get("", "resource_id").map_err(Error::Db)?,
                ))
            })
            .collect::<Result<Vec<(String, String)>, Error>>()?;
        Ok(candidates
            .iter()
            .filter(|candidate| {
                held.iter().any(|(_resource_type, resource_id)| {
                    resource_id == candidate.resource_id.as_str() || resource_id == SCOPE_SENTINEL
                })
            })
            .cloned()
            .collect())
    }
}
