//! `DieselStore`: the `RoleStore` + `ResourceStore` implementation for
//! Diesel 2.3 in both modes (D-08). Sync builds use the concrete sync
//! connection type per backend feature; the `async` feature reuses the
//! SAME store type and the SAME sql/rows/sentinel templates over
//! diesel-async connections (mirrored execution, Phase 3 D-06). Pool
//! checkouts and caller-owned transactions work via `DerefMut`.

use rolify_core::config::{HolderIdKind, RolifyConfig, RolifyConfigBuilder};
// The SPI value types below reach SQL only through the engine-gated impl
// blocks; the inert stub (no engine feature) never names them.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use rolify_core::resource::ResourceRef;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use rolify_core::role::{ResourceId, RoleName, SCOPE_SENTINEL};
use rolify_core::store::Sealed;

use crate::dialect::quote_identifier;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use rolify_core::query::{ResourceFilter, RoleQuery};

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
    holder_id_kind: HolderIdKind,
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
            holder_id_kind: config.holder_id_kind(),
        }
    }

    /// Create a store with explicit table names (for multi-pair setups).
    ///
    /// # Panics
    ///
    /// Panics when `role_table` or `join_table` fails the identifier
    /// allow-list (`^[A-Za-z_][A-Za-z0-9_]*$`). The config boundary
    /// documents that adapter constructors must re-validate names that
    /// bypass config, the same rule the sibling constructors apply:
    /// a quote character in a table name breaks out of the quoted
    /// identifier and injects into every generated statement.
    #[must_use]
    pub fn with_tables(role_table: &str, join_table: &str) -> Self {
        RolifyConfigBuilder::validate_identifier(role_table)
            .expect("role table name must pass validation");
        RolifyConfigBuilder::validate_identifier(join_table)
            .expect("join table name must pass validation");
        Self {
            role_table: quote_identifier(role_table),
            join_table: quote_identifier(join_table),
            holder_table: None,
            resource_tables: Vec::new(),
            holder_id_kind: HolderIdKind::default(),
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

    /// Override the holder-id kind (used when constructing via `with_tables` directly).
    ///
    /// Defaults come from the consuming `RolifyConfig` (D-08-03 coherence);
    /// this setter is the explicit multi-pair bypass.
    #[must_use]
    pub fn with_holder_id_kind(mut self, kind: HolderIdKind) -> Self {
        self.holder_id_kind = kind;
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

    /// The holder-id kind this store binds with (D-08-05).
    #[must_use]
    pub fn holder_id_kind(&self) -> HolderIdKind {
        self.holder_id_kind
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

/// Bind cap of the typed `where_any` ladder: the engine impls enumerate
/// bind chains by arity, so one statement cannot carry more binds than
/// this. `has_any_roles` forwards arbitrary consumer query lists (four
/// Instance queries alone cost the holder plus 28 binds), so the impls
/// chunk the list into ladder-sized batches and union-dedup the rows:
/// OR across batches is OR within.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
const WHERE_ANY_LADDER_BIND_CAP: usize = 23;

/// Bind cost of one `RoleQuery` in the `where_any` ladder: Global binds
/// the name plus two sentinel columns, Class adds the type and a
/// sentinel slot, Instance binds the name, sentinels, type and id, and
/// Any binds the name alone.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn where_any_bind_cost(query: &RoleQuery<'_>) -> usize {
    match &query.filter {
        ResourceFilter::Global => 3,
        ResourceFilter::Class(_) => 5,
        ResourceFilter::Instance(_, _) => 7,
        ResourceFilter::Any => 1,
    }
}

/// Split `queries` into consecutive slices whose bind total (the holder
/// bind included) stays within [`WHERE_ANY_LADDER_BIND_CAP`]. A single
/// query can never exceed the cap by itself (Instance costs 8 with the
/// holder), so the splitter always makes progress.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn split_where_any_batches<'batches, 'data>(
    queries: &'batches [RoleQuery<'data>],
) -> Vec<&'batches [RoleQuery<'data>]> {
    let mut batches = Vec::new();
    let mut batch_start = 0;
    let mut batch_binds = 1; // the holder bind
    for (index, query) in queries.iter().enumerate() {
        let query_cost = where_any_bind_cost(query);
        if batch_binds + query_cost > WHERE_ANY_LADDER_BIND_CAP && index > batch_start {
            batches.push(&queries[batch_start..index]);
            batch_start = index;
            batch_binds = 1;
        }
        batch_binds += query_cost;
    }
    batches.push(&queries[batch_start..]);
    batches
}

// ============================================================
// Typed holder-bind ladders (gate fix, D-08-05 wiring)
// ============================================================
//
// Diesel's raw `sql_query` chain gains a distinct type per `.bind()`
// call, so a runtime `match` on the holder kind cannot hand the query
// back from its arms (E0308: no two arms produce the same
// `UncheckedBind` type). The ladders below enumerate one static bind
// chain per trailing arity and execute the statement INSIDE each arm,
// returning the loaded rows or the affected count, which unify fine.
// Each ladder is generic over the holder's SQL type so a single
// enumeration serves every `DieselHolderBind` arm (BigInt / Uuid /
// UuidBytes / Text, per the compiled engine).

/// Generates the three typed holder-bind ladder helpers for one SYNC
/// engine module: `$conn` is the concrete connection type and
/// `$backend` the diesel backend marker type.
// The mode twins are used per build flavor only, so each
// definition carries the flavor-dependent unused allow.
#[allow(unused_macros)]
macro_rules! sync_bind_ladders {
    ($conn:ty, $backend:ty) => {
        /// Load rows binding the typed holder first, then `rest` as
        /// trailing text binds in order (placeholder 1 is the holder).
        ///
        /// # Panics
        ///
        /// Panics past 22 trailing binds: the `where_any` splitter
        /// caps a batch at 23 total binds, so the arm is unreachable.
        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates chains by arity
        fn load_holder_then_text<HolderSqlType, Holder, Row>(
            conn: &mut $conn,
            sql: &str,
            holder: Holder,
            rest: &[String],
        ) -> Result<Vec<Row>, Error>
        where
            HolderSqlType: diesel::sql_types::SqlType
                + diesel::sql_types::SingleValue
                + diesel::query_builder::QueryId,
            $backend: diesel::sql_types::HasSqlType<HolderSqlType>,
            Holder: diesel::serialize::ToSql<HolderSqlType, $backend>,
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            let rows = match rest.len() {
                0 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                1 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                5 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                6 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                7 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                8 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                9 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                10 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                11 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                12 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                13 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                14 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                15 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                16 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                17 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                18 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                19 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                20 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .bind::<Text, _>(&rest[19])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                21 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .bind::<Text, _>(&rest[19])
                    .bind::<Text, _>(&rest[20])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                22 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .bind::<Text, _>(&rest[19])
                    .bind::<Text, _>(&rest[20])
                    .bind::<Text, _>(&rest[21])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed bind ladder exceeded the 23-bind where_any cap".into(),
                        },
                    ))
                }
            };
            Ok(rows)
        }

        /// Execute a statement binding the typed holder first, then
        /// `rest` as trailing text binds; returns the affected row count.
        fn execute_holder_then_text<HolderSqlType, Holder>(
            conn: &mut $conn,
            sql: &str,
            holder: Holder,
            rest: &[String],
        ) -> Result<usize, Error>
        where
            HolderSqlType: diesel::sql_types::SqlType
                + diesel::sql_types::SingleValue
                + diesel::query_builder::QueryId,
            $backend: diesel::sql_types::HasSqlType<HolderSqlType>,
            Holder: diesel::serialize::ToSql<HolderSqlType, $backend>,
        {
            let affected = match rest.len() {
                0 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .execute(conn)
                    .map_err(Error::Diesel)?,
                1 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .execute(conn)
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .execute(conn)
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .execute(conn)
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed delete ladder exceeded the 3-bind removal cap".into(),
                        },
                    ))
                }
            };
            Ok(affected)
        }

        /// Load rows binding `prefix` as leading text binds, then the
        /// typed holder LAST (the `roles_matching` holder filter closes
        /// the placeholder sequence).
        ///
        /// # Panics
        ///
        /// Panics past 7 leading binds: the catalog read caps the bind
        /// count at 8 including the holder, so the arm is unreachable.
        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates chains by arity
        fn load_text_then_holder<HolderSqlType, Holder, Row>(
            conn: &mut $conn,
            sql: &str,
            prefix: &[String],
            holder: Holder,
        ) -> Result<Vec<Row>, Error>
        where
            HolderSqlType: diesel::sql_types::SqlType
                + diesel::sql_types::SingleValue
                + diesel::query_builder::QueryId,
            $backend: diesel::sql_types::HasSqlType<HolderSqlType>,
            Holder: diesel::serialize::ToSql<HolderSqlType, $backend>,
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            let rows = match prefix.len() {
                0 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                1 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                5 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<Text, _>(&prefix[4])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                6 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<Text, _>(&prefix[4])
                    .bind::<Text, _>(&prefix[5])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                7 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<Text, _>(&prefix[4])
                    .bind::<Text, _>(&prefix[5])
                    .bind::<Text, _>(&prefix[6])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed bind ladder exceeded the 8-bind catalog cap".into(),
                        },
                    ))
                }
            };
            Ok(rows)
        }

        /// Load rows binding `binds` as an all-text chain (the
        /// `roles_matching` catalog read without a holder filter).
        ///
        /// # Panics
        ///
        /// Panics outside the 1..=8 catalog window: types are non-empty
        /// and the read caps at 8 total binds.
        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates chains by arity
        fn load_text_only<Row>(
            conn: &mut $conn,
            sql: &str,
            binds: &[String],
        ) -> Result<Vec<Row>, Error>
        where
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            let rows = match binds.len() {
                1 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                5 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                6 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .bind::<Text, _>(&binds[5])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                7 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .bind::<Text, _>(&binds[5])
                    .bind::<Text, _>(&binds[6])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                8 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .bind::<Text, _>(&binds[5])
                    .bind::<Text, _>(&binds[6])
                    .bind::<Text, _>(&binds[7])
                    .load::<Row>(conn)
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed bind ladder exceeded the 8-bind catalog cap".into(),
                        },
                    ))
                }
            };
            Ok(rows)
        }
    };
}

/// Generates the three typed holder-bind ladder helpers for one ASYNC
/// engine module: same ladders as the sync twin, with the execution
/// point gaining `.await` (diesel-async's `RunQueryDsl`).
// The mode twins are used per build flavor only, so each
// definition carries the flavor-dependent unused allow.
#[allow(unused_macros)]
macro_rules! async_bind_ladders {
    ($conn:ty, $backend:ty) => {
        /// Load rows binding the typed holder first, then `rest` as
        /// trailing text binds in order (placeholder 1 is the holder).
        ///
        /// # Panics
        ///
        /// Panics past 22 trailing binds: the `where_any` splitter
        /// caps a batch at 23 total binds, so the arm is unreachable.
        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates chains by arity
        async fn load_holder_then_text<HolderSqlType, Holder, Row>(
            conn: &mut $conn,
            sql: &str,
            holder: Holder,
            rest: &[String],
        ) -> Result<Vec<Row>, Error>
        where
            HolderSqlType: diesel::sql_types::SqlType
                + diesel::sql_types::SingleValue
                + diesel::query_builder::QueryId
                + Send
                + Sync
                + 'static,
            $backend: diesel::sql_types::HasSqlType<HolderSqlType>,
            Holder: diesel::serialize::ToSql<HolderSqlType, $backend> + Send + Sync,
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            let rows = match rest.len() {
                0 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                1 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                5 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                6 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                7 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                8 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                9 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                10 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                11 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                12 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                13 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                14 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                15 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                16 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                17 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                18 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                19 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                20 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .bind::<Text, _>(&rest[19])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                21 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .bind::<Text, _>(&rest[19])
                    .bind::<Text, _>(&rest[20])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                22 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .bind::<Text, _>(&rest[3])
                    .bind::<Text, _>(&rest[4])
                    .bind::<Text, _>(&rest[5])
                    .bind::<Text, _>(&rest[6])
                    .bind::<Text, _>(&rest[7])
                    .bind::<Text, _>(&rest[8])
                    .bind::<Text, _>(&rest[9])
                    .bind::<Text, _>(&rest[10])
                    .bind::<Text, _>(&rest[11])
                    .bind::<Text, _>(&rest[12])
                    .bind::<Text, _>(&rest[13])
                    .bind::<Text, _>(&rest[14])
                    .bind::<Text, _>(&rest[15])
                    .bind::<Text, _>(&rest[16])
                    .bind::<Text, _>(&rest[17])
                    .bind::<Text, _>(&rest[18])
                    .bind::<Text, _>(&rest[19])
                    .bind::<Text, _>(&rest[20])
                    .bind::<Text, _>(&rest[21])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed bind ladder exceeded the 23-bind where_any cap".into(),
                        },
                    ))
                }
            };
            Ok(rows)
        }

        /// Execute a statement binding the typed holder first, then
        /// `rest` as trailing text binds; returns the affected row count.
        async fn execute_holder_then_text<HolderSqlType, Holder>(
            conn: &mut $conn,
            sql: &str,
            holder: Holder,
            rest: &[String],
        ) -> Result<usize, Error>
        where
            HolderSqlType: diesel::sql_types::SqlType
                + diesel::sql_types::SingleValue
                + diesel::query_builder::QueryId
                + Send
                + Sync
                + 'static,
            $backend: diesel::sql_types::HasSqlType<HolderSqlType>,
            Holder: diesel::serialize::ToSql<HolderSqlType, $backend> + Send + Sync,
        {
            let affected = match rest.len() {
                0 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .execute(conn)
                    .await
                    .map_err(Error::Diesel)?,
                1 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .execute(conn)
                    .await
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .execute(conn)
                    .await
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .bind::<Text, _>(&rest[0])
                    .bind::<Text, _>(&rest[1])
                    .bind::<Text, _>(&rest[2])
                    .execute(conn)
                    .await
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed delete ladder exceeded the 3-bind removal cap".into(),
                        },
                    ))
                }
            };
            Ok(affected)
        }

        /// Load rows binding `prefix` as leading text binds, then the
        /// typed holder LAST (the `roles_matching` holder filter closes
        /// the placeholder sequence).
        ///
        /// # Panics
        ///
        /// Panics past 7 leading binds: the catalog read caps the bind
        /// count at 8 including the holder, so the arm is unreachable.
        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates chains by arity
        async fn load_text_then_holder<HolderSqlType, Holder, Row>(
            conn: &mut $conn,
            sql: &str,
            prefix: &[String],
            holder: Holder,
        ) -> Result<Vec<Row>, Error>
        where
            HolderSqlType: diesel::sql_types::SqlType
                + diesel::sql_types::SingleValue
                + diesel::query_builder::QueryId
                + Send
                + Sync
                + 'static,
            $backend: diesel::sql_types::HasSqlType<HolderSqlType>,
            Holder: diesel::serialize::ToSql<HolderSqlType, $backend> + Send + Sync,
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            let rows = match prefix.len() {
                0 => diesel::sql_query(sql)
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                1 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                5 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<Text, _>(&prefix[4])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                6 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<Text, _>(&prefix[4])
                    .bind::<Text, _>(&prefix[5])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                7 => diesel::sql_query(sql)
                    .bind::<Text, _>(&prefix[0])
                    .bind::<Text, _>(&prefix[1])
                    .bind::<Text, _>(&prefix[2])
                    .bind::<Text, _>(&prefix[3])
                    .bind::<Text, _>(&prefix[4])
                    .bind::<Text, _>(&prefix[5])
                    .bind::<Text, _>(&prefix[6])
                    .bind::<HolderSqlType, _>(holder)
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed bind ladder exceeded the 8-bind catalog cap".into(),
                        },
                    ))
                }
            };
            Ok(rows)
        }

        /// Load rows binding `binds` as an all-text chain (the
        /// `roles_matching` catalog read without a holder filter).
        ///
        /// # Panics
        ///
        /// Panics outside the 1..=8 catalog window: types are non-empty
        /// and the read caps at 8 total binds.
        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates chains by arity
        async fn load_text_only<Row>(
            conn: &mut $conn,
            sql: &str,
            binds: &[String],
        ) -> Result<Vec<Row>, Error>
        where
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            let rows = match binds.len() {
                1 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                2 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                3 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                4 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                5 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                6 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .bind::<Text, _>(&binds[5])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                7 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .bind::<Text, _>(&binds[5])
                    .bind::<Text, _>(&binds[6])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                8 => diesel::sql_query(sql)
                    .bind::<Text, _>(&binds[0])
                    .bind::<Text, _>(&binds[1])
                    .bind::<Text, _>(&binds[2])
                    .bind::<Text, _>(&binds[3])
                    .bind::<Text, _>(&binds[4])
                    .bind::<Text, _>(&binds[5])
                    .bind::<Text, _>(&binds[6])
                    .bind::<Text, _>(&binds[7])
                    .load::<Row>(conn)
                    .await
                    .map_err(Error::Diesel)?,
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "typed bind ladder exceeded the 8-bind catalog cap".into(),
                        },
                    ))
                }
            };
            Ok(rows)
        }
    };
}

/// Generates the `DieselHolderBind` dispatch wrappers for one SYNC
/// engine module: the variant-to-SQL-type arms are the ONLY
/// engine-specific knowledge in the bind lane, so every method body
/// stays uniform across engines. `$pairs` lists the variants the
/// compiled engine can produce with their diesel SQL types.
// The mode twins are used per build flavor only, so each
// definition carries the flavor-dependent unused allow.
#[allow(unused_macros)]
macro_rules! sync_bind_dispatch {
    ($conn:ty, $backend:ty, $( $variant:path => $holder_sql_type:path ),+ $(,)?) => {
        /// Dispatch a load through the typed holder ladder: binds the
        /// holder variant first, then `rest` as trailing text binds.
        fn load_typed_rows<Row>(
            conn: &mut $conn,
            sql: &str,
            holder_bind: &DieselHolderBind,
            rest: &[String],
        ) -> Result<Vec<Row>, Error>
        where
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            match holder_bind {
                $( $variant(value) => {
                    load_holder_then_text::<$holder_sql_type, _, Row>(conn, sql, value, rest)
                } )+
            }
        }

        /// Dispatch an execute through the typed holder ladder; the
        /// return value is the affected row count.
        fn execute_typed(
            conn: &mut $conn,
            sql: &str,
            holder_bind: &DieselHolderBind,
            rest: &[String],
        ) -> Result<usize, Error> {
            match holder_bind {
                $( $variant(value) => {
                    execute_holder_then_text::<$holder_sql_type, _>(conn, sql, value, rest)
                } )+
            }
        }

        /// Dispatch a load whose placeholder sequence puts the typed
        /// holder LAST (the `roles_matching` holder filter).
        fn load_text_then_typed<Row>(
            conn: &mut $conn,
            sql: &str,
            prefix: &[String],
            holder_bind: &DieselHolderBind,
        ) -> Result<Vec<Row>, Error>
        where
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            match holder_bind {
                $( $variant(value) => {
                    load_text_then_holder::<$holder_sql_type, _, Row>(conn, sql, prefix, value)
                } )+
            }
        }

        /// Execute the link INSERT (holder bind, then the generated
        /// role-row id); the raw diesel result flows back so the
        /// caller keeps the unique-violation catch-and-ignore contract.
        fn execute_link_insert(
            conn: &mut $conn,
            sql: &str,
            holder_bind: &DieselHolderBind,
            role_id: i64,
        ) -> Result<usize, diesel::result::Error> {
            match holder_bind {
                $( $variant(value) => diesel::sql_query(sql)
                    .bind::<$holder_sql_type, _>(value)
                    .bind::<BigInt, _>(role_id)
                    .execute(conn), )+
            }
        }
    };
}

/// Generates the `DieselHolderBind` dispatch wrappers for one ASYNC
/// engine module (the `.await` twin of [`sync_bind_dispatch`]).
// The mode twins are used per build flavor only, so each
// definition carries the flavor-dependent unused allow.
#[allow(unused_macros)]
macro_rules! async_bind_dispatch {
    ($conn:ty, $backend:ty, $( $variant:path => $holder_sql_type:path ),+ $(,)?) => {
        /// Dispatch a load through the typed holder ladder: binds the
        /// holder variant first, then `rest` as trailing text binds.
        async fn load_typed_rows<Row>(
            conn: &mut $conn,
            sql: &str,
            holder_bind: &DieselHolderBind,
            rest: &[String],
        ) -> Result<Vec<Row>, Error>
        where
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            match holder_bind {
                $( $variant(value) => {
                    load_holder_then_text::<$holder_sql_type, _, Row>(conn, sql, value, rest)
                        .await
                } )+
            }
        }

        /// Dispatch an execute through the typed holder ladder; the
        /// return value is the affected row count.
        async fn execute_typed(
            conn: &mut $conn,
            sql: &str,
            holder_bind: &DieselHolderBind,
            rest: &[String],
        ) -> Result<usize, Error> {
            match holder_bind {
                $( $variant(value) => {
                    execute_holder_then_text::<$holder_sql_type, _>(conn, sql, value, rest).await
                } )+
            }
        }

        /// Dispatch a load whose placeholder sequence puts the typed
        /// holder LAST (the `roles_matching` holder filter).
        async fn load_text_then_typed<Row>(
            conn: &mut $conn,
            sql: &str,
            prefix: &[String],
            holder_bind: &DieselHolderBind,
        ) -> Result<Vec<Row>, Error>
        where
            Row: diesel::deserialize::QueryableByName<$backend> + Send + 'static,
        {
            match holder_bind {
                $( $variant(value) => {
                    load_text_then_holder::<$holder_sql_type, _, Row>(conn, sql, prefix, value)
                        .await
                } )+
            }
        }

        /// Execute the link INSERT (holder bind, then the generated
        /// role-row id); the raw diesel result flows back so the
        /// caller keeps the unique-violation catch-and-ignore contract.
        async fn execute_link_insert(
            conn: &mut $conn,
            sql: &str,
            holder_bind: &DieselHolderBind,
            role_id: i64,
        ) -> Result<usize, diesel::result::Error> {
            match holder_bind {
                $( $variant(value) => diesel::sql_query(sql)
                    .bind::<$holder_sql_type, _>(value)
                    .bind::<BigInt, _>(role_id)
                    .execute(conn)
                    .await, )+
            }
        }
    };
}

#[cfg(all(feature = "postgres", feature = "sync"))]
mod pg_impl {
    use super::{
        DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL, split_where_any_batches,
    };
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
    use crate::holder::DieselHolderBind;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // Typed holder-bind ladders for this engine (see the macro docs).
    sync_bind_ladders!(PgConnection, diesel::pg::Pg);
    sync_bind_dispatch!(
        PgConnection,
        diesel::pg::Pg,
        DieselHolderBind::BigInt => BigInt,
        DieselHolderBind::Uuid => diesel::sql_types::Uuid,
        DieselHolderBind::Text => Text,
    );

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
        match q.get_result::<RoleRow>(conn) {
            Ok(row) => return Ok(row.to_record()),
            Err(diesel::result::Error::NotFound) => {}
            Err(e) => return Err(Error::Diesel(e)),
        }

        let insert_sql = crate::sql::insert_role(role_table);
        let insert_q = diesel::sql_query(insert_sql)
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        match insert_q.execute(conn) {
            Ok(_)
            | Err(diesel::result::Error::DatabaseError(
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
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type
            // (typed bind lane: BIGINT for Integer, native UUID for Uuid).
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rest: Vec<String> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => {
                    vec![name.to_owned(), String::new(), String::new()]
                }
                rolify_core::query::ResourceFilter::Class(type_name) => {
                    vec![
                        name.to_owned(),
                        String::new(),
                        String::new(),
                        type_name.to_string(),
                        String::new(),
                    ]
                }
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    vec![
                        name.to_owned(),
                        String::new(),
                        String::new(),
                        type_name.to_string(),
                        String::new(),
                        type_name.to_string(),
                        resource_id.as_str().to_owned(),
                    ]
                }
                rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
            };
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest)?;
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
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rest: Vec<String> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => {
                    vec![name.to_owned(), String::new(), String::new()]
                }
                rolify_core::query::ResourceFilter::Class(type_name) => {
                    vec![name.to_owned(), type_name.to_string(), String::new()]
                }
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    vec![
                        name.to_owned(),
                        type_name.to_string(),
                        resource_id.as_str().to_owned(),
                    ]
                }
                rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
            };
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest)?;
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
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            // The typed ladder helper enumerates bind chains by arity,
            // up to 23 binds per statement, and `has_any_roles` forwards
            // any consumer query list: chunk into ladder-sized batches,
            // then union-dedup the rows (OR across batches is OR
            // within).
            let mut seen: Vec<RoleRecord> = Vec::new();
            for batch in split_where_any_batches(queries) {
                let (where_clause, _) = crate::sql::build_any_where(
                    &self.role_table,
                    &self.join_table,
                    &placeholder(1),
                    batch,
                    2,
                );
                let sql = crate::sql::select_roles_for_holder(
                    &self.role_table,
                    &self.join_table,
                    &where_clause,
                    &placeholder(1),
                );
                // Collect the batch's trailing text binds in order: the
                // holder is placeholder 1 and binds typed via the
                // dispatch below (where_any precedents, owned values).
                let mut all_values: Vec<String> = Vec::new();
                for query in batch {
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
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &all_values)?;
                for row in rows {
                    let record = row.to_record();
                    if !seen.contains(&record) {
                        seen.push(record);
                    }
                }
            }
            async move { Ok(seen) }
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
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rt = to_storage(role.resource_type.as_deref());
            let rid = resource_id_to_storage(role.resource_id.as_ref());

            // `?` cannot replace this match: the fn returns a future,
            // so the error arm must wrap the error in one.
            #[allow(clippy::question_mark)]
            let role_record =
                match find_or_create_by_triple(conn, &self.role_table, &role.name, rt, rid) {
                    Ok(r) => r,
                    Err(e) => return async move { Err(e) },
                };

            // Same future-returning-fn shape as `add`: `?` cannot
            // produce the early-returned future.
            #[allow(clippy::question_mark)]
            let role_id = match get_role_id(
                conn,
                &self.role_table,
                &role_record.name,
                role_record.resource_type.as_deref().unwrap_or_default(),
                role_record
                    .resource_id
                    .as_ref()
                    .map(ResourceId::as_str)
                    .unwrap_or_default(),
            ) {
                Ok(id) => id,
                Err(e) => return async move { Err(e) },
            };

            let insert_sql = crate::sql::insert_link(&self.join_table);
            match execute_link_insert(conn, &insert_sql, &holder_bind, role_id) {
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
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let name_owned = name.as_str().to_owned();
            let target_owned = target;
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();

            async move {
                conn.transaction::<_, Error, _>(|conn| {
                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &target_owned);
                    let affected_rest: Vec<String> = match &target_owned {
                        RemovalTarget::NameOnly => vec![name_owned.clone()],
                        RemovalTarget::TypeSweep(type_name) => {
                            vec![name_owned.clone(), (*type_name).to_owned()]
                        }
                        RemovalTarget::Exact(type_name, resource_id) => vec![
                            name_owned.clone(),
                            (*type_name).to_owned(),
                            resource_id.as_str().to_owned(),
                        ],
                    };
                    let affected_rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(
                        conn,
                        &affected_sql,
                        &holder_bind,
                        &affected_rest,
                    )?;
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &target_owned,
                    );
                    let removed_links =
                        execute_typed(conn, &delete_sql, &holder_bind, &affected_rest)?;

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
                                    .map(ResourceId::as_str)
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
            #[derive(diesel::deserialize::QueryableByName)]
            #[allow(dead_code)]
            struct ExistsRow {
                // Named exactly like the `AS dummy` projection: by-name
                // decoding matches on it (an underscore prefix broke the
                // match and made `exists` always false).
                #[diesel(sql_type = diesel::sql_types::Integer)]
                dummy: i32,
            }
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
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
            // Same swallow-as-false contract as the old get_result
            // probe: any execution error reports "no role exists".
            #[allow(clippy::question_mark)]
            let rows = match load_typed_rows::<ExistsRow>(conn, &sql, &holder_bind, &[]) {
                Ok(rows) => rows,
                Err(e) => return async move { Err(e) },
            };
            let found = !rows.is_empty();
            async move { Ok(found) }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let sql = format!(
                "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = {holder_ph}",
                role_table = self.role_table,
                join_table = self.join_table,
                holder_ph = placeholder(1),
            );
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &[])?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        #[allow(clippy::too_many_lines)] // per-arity typed bind dispatch for the holder finder
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
                (1..=holder_types.len()).map(placeholder).collect();
            let type_filter = format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT {holder_projection} AS user_id \
                 FROM {holder_table} AS holder \
                 INNER JOIN {join_table} AS link ON {holder_join} \
                 INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                 WHERE {type_filter} AND {where_clause}",
                holder_table = holder_table,
                join_table = self.join_table,
                role_table = self.role_table,
                type_filter = type_filter,
                where_clause = where_clause,
                // Kind-aware join (D-08-04): typed kinds compare the
                // primary key natively; the string kind keeps the cast.
                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine.
                holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "holder.id",),
                holder_join = crate::holder::holder_join_condition(
                    self.holder_id_kind(),
                    "holder.id",
                    "link.user_id",
                ),
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "holders_where: too many holder types (max 4)".into(),
                        },
                    ));
                }
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
                (1..=holder_types.len()).map(placeholder).collect();
            let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

            // Kind-aware projection (WR-01): canonical text for the
            // Text-decoding HolderIdRow on every engine.
            let holder_projection =
                crate::holder::holder_text_projection(self.holder_id_kind(), "id");
            let sql = format!(
                "SELECT {holder_projection} AS user_id FROM {holder_table} WHERE {type_filter}",
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "all_holders: too many holder types (max 4)".into(),
                        },
                    ));
                }
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        #[allow(clippy::too_many_lines)] // catalog filter composition with per-arity typed binds
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
                self.holder_id_kind(),
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
            let (holder_filter, _) = if has_holder {
                crate::sql::roles_matching_holder_filter(self.holder_id_kind(), idx)
            } else {
                (String::new(), idx)
            };

            let sql = base_sql
                .replace("{type_filter}", &type_filter)
                .replace("{name_filter}", &name_filter)
                .replace("{scope_filter}", &scope_filter)
                .replace("{holder_filter}", &holder_filter);

            // Leading text binds (types, optional name, optional scope
            // id); the typed holder closes the placeholder sequence
            // when present (D-08-05: parsed at the boundary, bound per
            // the kind's physical type).
            let mut text_binds: Vec<String> = query
                .types
                .iter()
                .map(|resource_type| (*resource_type).to_owned())
                .collect();
            if let Some(name) = query.name {
                text_binds.push(name.as_str().to_owned());
            }
            match &query.scope {
                CatalogScope::ClassOnly => {
                    text_binds.push(SCOPE_SENTINEL.to_owned());
                }
                CatalogScope::InstanceOnly { resource_id } => {
                    if let Some(resource_id) = resource_id {
                        text_binds.push(resource_id.as_str().to_owned());
                    }
                }
                CatalogScope::ClassAndInstance => {}
            }

            let rows: Vec<ResourceKeyRow> = if let Some(holder) = query.holder {
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return async move { Err(Error::Core(e)) },
                    };
                load_text_then_typed::<ResourceKeyRow>(conn, &sql, &text_binds, &holder_bind)?
            } else {
                load_text_only::<ResourceKeyRow>(conn, &sql, &text_binds)?
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
            let type_placeholders: Vec<String> = (1..=types.len()).map(placeholder).collect();
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "resources_find: too many types (max 4)".into(),
                        },
                    ));
                }
            };
            for row in instance_rows {
                all_keys.push(row.to_key());
            }

            // 2. Class-scoped roles: expand via resource registry
            for (type_name, resource_table, pk_column) in &self.resource_tables {
                if types.contains(&type_name.as_str()) {
                    let expansion_sql = crate::sql::select_resources_find_class_expansion(
                        &self.role_table,
                        resource_table,
                        pk_column,
                        type_name,
                    );
                    let q = diesel::sql_query(expansion_sql).bind::<Text, _>(name.as_str());
                    let class_rows: Vec<ResourceKeyRow> = q.load(conn).map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
            }

            let mut seen: std::collections::HashSet<ResourceKey> = std::collections::HashSet::new();
            all_keys.retain(|k| seen.insert(k.clone()));
            async move { Ok(all_keys) }
        }

        #[allow(clippy::too_many_lines)] // typed bind ladder for the candidate coverage read
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
            let name_placeholders: Vec<String> = (2..=names.len() + 1).map(placeholder).collect();
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

            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            // Collect the trailing name binds; the holder is
            // placeholder 1 and binds typed through the dispatch.
            let name_binds: Vec<String> =
                names.iter().map(|name| name.as_str().to_owned()).collect();
            let rows: Vec<RoleRow> =
                load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &name_binds)?;
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
    use super::{
        DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL, split_where_any_batches,
    };
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
    use crate::holder::DieselHolderBind;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // Typed holder-bind ladders for this engine (see the macro docs).
    sync_bind_ladders!(MysqlConnection, diesel::mysql::Mysql);

    sync_bind_dispatch!(
        MysqlConnection,
        diesel::mysql::Mysql,
        DieselHolderBind::BigInt => BigInt,
        DieselHolderBind::UuidBytes => diesel::sql_types::Binary,
        DieselHolderBind::Text => Text,
    );

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
        match q.get_result::<RoleRow>(conn) {
            Ok(row) => return Ok(row.to_record()),
            Err(diesel::result::Error::NotFound) => {}
            Err(e) => return Err(Error::Diesel(e)),
        }

        let insert_sql = crate::sql::insert_role(role_table);
        let insert_q = diesel::sql_query(insert_sql)
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        match insert_q.execute(conn) {
            Ok(_)
            | Err(diesel::result::Error::DatabaseError(
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
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type
            // (typed bind lane: BIGINT for Integer, native UUID for Uuid).
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rest: Vec<String> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => {
                    vec![name.to_owned(), String::new(), String::new()]
                }
                rolify_core::query::ResourceFilter::Class(type_name) => {
                    vec![
                        name.to_owned(),
                        String::new(),
                        String::new(),
                        type_name.to_string(),
                        String::new(),
                    ]
                }
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    vec![
                        name.to_owned(),
                        String::new(),
                        String::new(),
                        type_name.to_string(),
                        String::new(),
                        type_name.to_string(),
                        resource_id.as_str().to_owned(),
                    ]
                }
                rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
            };
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest)?;
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
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rest: Vec<String> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => {
                    vec![name.to_owned(), String::new(), String::new()]
                }
                rolify_core::query::ResourceFilter::Class(type_name) => {
                    vec![name.to_owned(), type_name.to_string(), String::new()]
                }
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    vec![
                        name.to_owned(),
                        type_name.to_string(),
                        resource_id.as_str().to_owned(),
                    ]
                }
                rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
            };
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest)?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates bind chains by arity
        fn where_any(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            queries: &[RoleQuery<'_>],
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            if queries.is_empty() {
                return async { Ok(Vec::new()) };
            }
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            // The typed ladder helper enumerates bind chains by arity,
            // up to 23 binds per statement, and `has_any_roles` forwards
            // any consumer query list: chunk into ladder-sized batches,
            // then union-dedup the rows (OR across batches is OR
            // within).
            let mut seen: Vec<RoleRecord> = Vec::new();
            for batch in split_where_any_batches(queries) {
                let (where_clause, _) = crate::sql::build_any_where(
                    &self.role_table,
                    &self.join_table,
                    &placeholder(1),
                    batch,
                    2,
                );
                let sql = crate::sql::select_roles_for_holder(
                    &self.role_table,
                    &self.join_table,
                    &where_clause,
                    &placeholder(1),
                );
                // Collect the batch's trailing text binds in order:
                // the holder is placeholder 1 and binds typed via the
                // dispatch below (where_any precedents, owned values).
                let mut all_values: Vec<String> = Vec::new();
                for query in batch {
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
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &all_values)?;
                for row in rows {
                    let record = row.to_record();
                    if !seen.contains(&record) {
                        seen.push(record);
                    }
                }
            }
            async move { Ok(seen) }
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
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rt = to_storage(role.resource_type.as_deref());
            let rid = resource_id_to_storage(role.resource_id.as_ref());

            // `?` cannot replace this match: the fn returns a future,
            // so the error arm must wrap the error in one.
            #[allow(clippy::question_mark)]
            let role_record =
                match find_or_create_by_triple(conn, &self.role_table, &role.name, rt, rid) {
                    Ok(r) => r,
                    Err(e) => return async move { Err(e) },
                };

            // Same future-returning-fn shape as `add`: `?` cannot
            // produce the early-returned future.
            #[allow(clippy::question_mark)]
            let role_id = match get_role_id(
                conn,
                &self.role_table,
                &role_record.name,
                role_record.resource_type.as_deref().unwrap_or_default(),
                role_record
                    .resource_id
                    .as_ref()
                    .map(ResourceId::as_str)
                    .unwrap_or_default(),
            ) {
                Ok(id) => id,
                Err(e) => return async move { Err(e) },
            };

            let insert_sql = crate::sql::insert_link(&self.join_table);
            match execute_link_insert(conn, &insert_sql, &holder_bind, role_id) {
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
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let name_owned = name.as_str().to_owned();
            let target_owned = target;
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();

            async move {
                conn.transaction::<_, Error, _>(|conn| {
                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &target_owned);
                    let affected_rest: Vec<String> = match &target_owned {
                        RemovalTarget::NameOnly => vec![name_owned.clone()],
                        RemovalTarget::TypeSweep(type_name) => {
                            vec![name_owned.clone(), (*type_name).to_owned()]
                        }
                        RemovalTarget::Exact(type_name, resource_id) => vec![
                            name_owned.clone(),
                            (*type_name).to_owned(),
                            resource_id.as_str().to_owned(),
                        ],
                    };
                    let affected_rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(
                        conn,
                        &affected_sql,
                        &holder_bind,
                        &affected_rest,
                    )?;
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &target_owned,
                    );
                    let removed_links =
                        execute_typed(conn, &delete_sql, &holder_bind, &affected_rest)?;

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
                                    .map(ResourceId::as_str)
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
            #[derive(diesel::deserialize::QueryableByName)]
            #[allow(dead_code)]
            struct ExistsRow {
                // Named exactly like the `AS dummy` projection: by-name
                // decoding matches on it (an underscore prefix broke the
                // match and made `exists` always false).
                #[diesel(sql_type = diesel::sql_types::Integer)]
                dummy: i32,
            }
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
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
            // Same swallow-as-false contract as the old get_result
            // probe: any execution error reports "no role exists".
            #[allow(clippy::question_mark)]
            let rows = match load_typed_rows::<ExistsRow>(conn, &sql, &holder_bind, &[]) {
                Ok(rows) => rows,
                Err(e) => return async move { Err(e) },
            };
            let found = !rows.is_empty();
            async move { Ok(found) }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let sql = format!(
                "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ?",
                role_table = self.role_table,
                join_table = self.join_table,
            );
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &[])?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        #[allow(clippy::too_many_lines)] // per-arity typed bind dispatch for the holder finder
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
                (1..=holder_types.len()).map(placeholder).collect();
            let type_filter = format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT {holder_projection} AS user_id \
                 FROM {holder_table} AS holder \
                 INNER JOIN {join_table} AS link ON {holder_join} \
                 INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                 WHERE {type_filter} AND {where_clause}",
                holder_table = holder_table,
                join_table = self.join_table,
                role_table = self.role_table,
                type_filter = type_filter,
                where_clause = where_clause,
                // Kind-aware join (D-08-04): typed kinds compare the
                // primary key natively; the string kind keeps the cast.
                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine, with
                // BIN_TO_UUID for the MySQL BINARY(16) uuid column.
                holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "holder.id",),
                holder_join = crate::holder::holder_join_condition(
                    self.holder_id_kind(),
                    "holder.id",
                    "link.user_id",
                ),
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "holders_where: too many holder types (max 4)".into(),
                        },
                    ));
                }
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
                (1..=holder_types.len()).map(placeholder).collect();
            let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

            // Kind-aware projection (WR-01): canonical text for the
            // Text-decoding HolderIdRow on every engine, with
            // BIN_TO_UUID for the MySQL BINARY(16) uuid column.
            let sql = format!(
                "SELECT {holder_projection} AS user_id FROM {holder_table} WHERE {type_filter}",
                holder_table = holder_table,
                type_filter = type_filter,
                holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "id"),
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "all_holders: too many holder types (max 4)".into(),
                        },
                    ));
                }
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        #[allow(clippy::too_many_lines)] // catalog filter composition with per-arity typed binds
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
                self.holder_id_kind(),
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
            let (holder_filter, _) = if has_holder {
                crate::sql::roles_matching_holder_filter(self.holder_id_kind(), idx)
            } else {
                (String::new(), idx)
            };

            let sql = base_sql
                .replace("{type_filter}", &type_filter)
                .replace("{name_filter}", &name_filter)
                .replace("{scope_filter}", &scope_filter)
                .replace("{holder_filter}", &holder_filter);

            // Leading text binds (types, optional name, optional scope
            // id); the typed holder closes the placeholder sequence
            // when present (D-08-05: parsed at the boundary, bound per
            // the kind's physical type).
            let mut text_binds: Vec<String> = query
                .types
                .iter()
                .map(|resource_type| (*resource_type).to_owned())
                .collect();
            if let Some(name) = query.name {
                text_binds.push(name.as_str().to_owned());
            }
            match &query.scope {
                CatalogScope::ClassOnly => {
                    text_binds.push(SCOPE_SENTINEL.to_owned());
                }
                CatalogScope::InstanceOnly { resource_id } => {
                    if let Some(resource_id) = resource_id {
                        text_binds.push(resource_id.as_str().to_owned());
                    }
                }
                CatalogScope::ClassAndInstance => {}
            }

            let rows: Vec<ResourceKeyRow> = if let Some(holder) = query.holder {
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return async move { Err(Error::Core(e)) },
                    };
                load_text_then_typed::<ResourceKeyRow>(conn, &sql, &text_binds, &holder_bind)?
            } else {
                load_text_only::<ResourceKeyRow>(conn, &sql, &text_binds)?
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
            let type_placeholders: Vec<String> = (1..=types.len()).map(placeholder).collect();
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "resources_find: too many types (max 4)".into(),
                        },
                    ));
                }
            };
            for row in instance_rows {
                all_keys.push(row.to_key());
            }

            // 2. Class-scoped roles: expand via resource registry
            for (type_name, resource_table, pk_column) in &self.resource_tables {
                if types.contains(&type_name.as_str()) {
                    let expansion_sql = crate::sql::select_resources_find_class_expansion(
                        &self.role_table,
                        resource_table,
                        pk_column,
                        type_name,
                    );
                    let q = diesel::sql_query(expansion_sql).bind::<Text, _>(name.as_str());
                    let class_rows: Vec<ResourceKeyRow> = q.load(conn).map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
            }

            let mut seen: std::collections::HashSet<ResourceKey> = std::collections::HashSet::new();
            all_keys.retain(|k| seen.insert(k.clone()));
            async move { Ok(all_keys) }
        }

        #[allow(clippy::too_many_lines)] // typed bind ladder for the candidate coverage read
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
            let name_placeholders: Vec<String> = (2..=names.len() + 1).map(placeholder).collect();
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

            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            // Collect the trailing name binds; the holder is
            // placeholder 1 and binds typed through the dispatch.
            let name_binds: Vec<String> =
                names.iter().map(|name| name.as_str().to_owned()).collect();
            let rows: Vec<RoleRow> =
                load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &name_binds)?;
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
    use super::{
        DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL, split_where_any_batches,
    };
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
    use crate::holder::DieselHolderBind;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // Typed holder-bind ladders for this engine (see the macro docs).
    sync_bind_ladders!(SqliteConnection, diesel::sqlite::Sqlite);

    sync_bind_dispatch!(
        SqliteConnection,
        diesel::sqlite::Sqlite,
        DieselHolderBind::BigInt => BigInt,
        DieselHolderBind::Text => Text,
    );

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
        match q.get_result::<RoleRow>(conn) {
            Ok(row) => return Ok(row.to_record()),
            Err(diesel::result::Error::NotFound) => {}
            Err(e) => return Err(Error::Diesel(e)),
        }

        let insert_sql = crate::sql::insert_role(role_table);
        let insert_q = diesel::sql_query(insert_sql)
            .bind::<Text, _>(name.as_str())
            .bind::<Text, _>(resource_type)
            .bind::<Text, _>(resource_id);
        match insert_q.execute(conn) {
            Ok(_)
            | Err(diesel::result::Error::DatabaseError(
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
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type
            // (typed bind lane: BIGINT for Integer, native UUID for Uuid).
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rest: Vec<String> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => {
                    vec![name.to_owned(), String::new(), String::new()]
                }
                rolify_core::query::ResourceFilter::Class(type_name) => {
                    vec![
                        name.to_owned(),
                        String::new(),
                        String::new(),
                        type_name.to_string(),
                        String::new(),
                    ]
                }
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    vec![
                        name.to_owned(),
                        String::new(),
                        String::new(),
                        type_name.to_string(),
                        String::new(),
                        type_name.to_string(),
                        resource_id.as_str().to_owned(),
                    ]
                }
                rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
            };
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest)?;
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
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rest: Vec<String> = match &query.filter {
                rolify_core::query::ResourceFilter::Global => {
                    vec![name.to_owned(), String::new(), String::new()]
                }
                rolify_core::query::ResourceFilter::Class(type_name) => {
                    vec![name.to_owned(), type_name.to_string(), String::new()]
                }
                rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                    vec![
                        name.to_owned(),
                        type_name.to_string(),
                        resource_id.as_str().to_owned(),
                    ]
                }
                rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
            };
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest)?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        #[allow(clippy::too_many_lines)] // typed bind ladder enumerates bind chains by arity
        fn where_any(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
            queries: &[RoleQuery<'_>],
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            if queries.is_empty() {
                return async { Ok(Vec::new()) };
            }
            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            // The typed ladder helper enumerates bind chains by arity,
            // up to 23 binds per statement, and `has_any_roles` forwards
            // any consumer query list: chunk into ladder-sized batches,
            // then union-dedup the rows (OR across batches is OR
            // within).
            let mut seen: Vec<RoleRecord> = Vec::new();
            for batch in split_where_any_batches(queries) {
                let (where_clause, _) = crate::sql::build_any_where(
                    &self.role_table,
                    &self.join_table,
                    &placeholder(1),
                    batch,
                    2,
                );
                let sql = crate::sql::select_roles_for_holder(
                    &self.role_table,
                    &self.join_table,
                    &where_clause,
                    &placeholder(1),
                );
                // Collect the batch's trailing text binds in order:
                // the holder is placeholder 1 and binds typed via the
                // dispatch below (where_any precedents, owned values).
                let mut all_values: Vec<String> = Vec::new();
                for query in batch {
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
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &all_values)?;
                for row in rows {
                    let record = row.to_record();
                    if !seen.contains(&record) {
                        seen.push(record);
                    }
                }
            }
            async move { Ok(seen) }
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
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let rt = to_storage(role.resource_type.as_deref());
            let rid = resource_id_to_storage(role.resource_id.as_ref());

            // `?` cannot replace this match: the fn returns a future,
            // so the error arm must wrap the error in one.
            #[allow(clippy::question_mark)]
            let role_record =
                match find_or_create_by_triple(conn, &self.role_table, &role.name, rt, rid) {
                    Ok(r) => r,
                    Err(e) => return async move { Err(e) },
                };

            // Same future-returning-fn shape as `add`: `?` cannot
            // produce the early-returned future.
            #[allow(clippy::question_mark)]
            let role_id = match get_role_id(
                conn,
                &self.role_table,
                &role_record.name,
                role_record.resource_type.as_deref().unwrap_or_default(),
                role_record
                    .resource_id
                    .as_ref()
                    .map(ResourceId::as_str)
                    .unwrap_or_default(),
            ) {
                Ok(id) => id,
                Err(e) => return async move { Err(e) },
            };

            let insert_sql = crate::sql::insert_link(&self.join_table);
            match execute_link_insert(conn, &insert_sql, &holder_bind, role_id) {
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
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let name_owned = name.as_str().to_owned();
            let target_owned = target;
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();

            async move {
                conn.transaction::<_, Error, _>(|conn| {
                    let affected_sql =
                        crate::sql::select_affected_roles(&role_table, &join_table, &target_owned);
                    let affected_rest: Vec<String> = match &target_owned {
                        RemovalTarget::NameOnly => vec![name_owned.clone()],
                        RemovalTarget::TypeSweep(type_name) => {
                            vec![name_owned.clone(), (*type_name).to_owned()]
                        }
                        RemovalTarget::Exact(type_name, resource_id) => vec![
                            name_owned.clone(),
                            (*type_name).to_owned(),
                            resource_id.as_str().to_owned(),
                        ],
                    };
                    let affected_rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(
                        conn,
                        &affected_sql,
                        &holder_bind,
                        &affected_rest,
                    )?;
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &target_owned,
                    );
                    let removed_links =
                        execute_typed(conn, &delete_sql, &holder_bind, &affected_rest)?;

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
                                    .map(ResourceId::as_str)
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
            #[derive(diesel::deserialize::QueryableByName)]
            #[allow(dead_code)]
            struct ExistsRow {
                // Named exactly like the `AS dummy` projection: by-name
                // decoding matches on it (an underscore prefix broke the
                // match and made `exists` always false).
                #[diesel(sql_type = diesel::sql_types::Integer)]
                dummy: i32,
            }
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
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
            // Same swallow-as-false contract as the old get_result
            // probe: any execution error reports "no role exists".
            #[allow(clippy::question_mark)]
            let rows = match load_typed_rows::<ExistsRow>(conn, &sql, &holder_bind, &[]) {
                Ok(rows) => rows,
                Err(e) => return async move { Err(e) },
            };
            let found = !rows.is_empty();
            async move { Ok(found) }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            let sql = format!(
                "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ?",
                role_table = self.role_table,
                join_table = self.join_table,
            );
            let rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &[])?;
            async move { Ok(rows.into_iter().map(|r| r.to_record()).collect()) }
        }

        #[allow(clippy::too_many_lines)] // per-arity typed bind dispatch for the holder finder
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
                (1..=holder_types.len()).map(placeholder).collect();
            let type_filter = format!("holder.rolify_type IN ({})", type_placeholders.join(", "));

            let sql = format!(
                "SELECT DISTINCT {holder_projection} AS user_id \
                 FROM {holder_table} AS holder \
                 INNER JOIN {join_table} AS link ON {holder_join} \
                 INNER JOIN {role_table} AS role_row ON role_row.id = link.role_id \
                 WHERE {type_filter} AND {where_clause}",
                holder_table = holder_table,
                join_table = self.join_table,
                role_table = self.role_table,
                type_filter = type_filter,
                where_clause = where_clause,
                // Kind-aware join (D-08-04): typed kinds compare the
                // primary key natively; the string kind keeps the cast.
                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine.
                holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "holder.id",),
                holder_join = crate::holder::holder_join_condition(
                    self.holder_id_kind(),
                    "holder.id",
                    "link.user_id",
                ),
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "holders_where: too many holder types (max 4)".into(),
                        },
                    ));
                }
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
                (1..=holder_types.len()).map(placeholder).collect();
            let type_filter = format!("rolify_type IN ({})", type_placeholders.join(", "));

            // Kind-aware projection (WR-01): canonical text for the
            // Text-decoding HolderIdRow on every engine.
            let holder_projection =
                crate::holder::holder_text_projection(self.holder_id_kind(), "id");
            let sql = format!(
                "SELECT {holder_projection} AS user_id FROM {holder_table} WHERE {type_filter}",
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "all_holders: too many holder types (max 4)".into(),
                        },
                    ));
                }
            };
            async move {
                Ok(rows
                    .into_iter()
                    .map(|r| ResourceId::new(r.user_id))
                    .collect())
            }
        }

        #[allow(clippy::too_many_lines)] // catalog filter composition with per-arity typed binds
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
                self.holder_id_kind(),
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
            let (holder_filter, _) = if has_holder {
                crate::sql::roles_matching_holder_filter(self.holder_id_kind(), idx)
            } else {
                (String::new(), idx)
            };

            let sql = base_sql
                .replace("{type_filter}", &type_filter)
                .replace("{name_filter}", &name_filter)
                .replace("{scope_filter}", &scope_filter)
                .replace("{holder_filter}", &holder_filter);

            // Leading text binds (types, optional name, optional scope
            // id); the typed holder closes the placeholder sequence
            // when present (D-08-05: parsed at the boundary, bound per
            // the kind's physical type).
            let mut text_binds: Vec<String> = query
                .types
                .iter()
                .map(|resource_type| (*resource_type).to_owned())
                .collect();
            if let Some(name) = query.name {
                text_binds.push(name.as_str().to_owned());
            }
            match &query.scope {
                CatalogScope::ClassOnly => {
                    text_binds.push(SCOPE_SENTINEL.to_owned());
                }
                CatalogScope::InstanceOnly { resource_id } => {
                    if let Some(resource_id) = resource_id {
                        text_binds.push(resource_id.as_str().to_owned());
                    }
                }
                CatalogScope::ClassAndInstance => {}
            }

            let rows: Vec<ResourceKeyRow> = if let Some(holder) = query.holder {
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return async move { Err(Error::Core(e)) },
                    };
                load_text_then_typed::<ResourceKeyRow>(conn, &sql, &text_binds, &holder_bind)?
            } else {
                load_text_only::<ResourceKeyRow>(conn, &sql, &text_binds)?
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
            let type_placeholders: Vec<String> = (1..=types.len()).map(placeholder).collect();
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
                _ => {
                    return Err(Error::Core(
                        rolify_core::error::RolifyError::InvalidConfig {
                            reason: "resources_find: too many types (max 4)".into(),
                        },
                    ));
                }
            };
            for row in instance_rows {
                all_keys.push(row.to_key());
            }

            // 2. Class-scoped roles: expand via resource registry
            for (type_name, resource_table, pk_column) in &self.resource_tables {
                if types.contains(&type_name.as_str()) {
                    let expansion_sql = crate::sql::select_resources_find_class_expansion(
                        &self.role_table,
                        resource_table,
                        pk_column,
                        type_name,
                    );
                    let q = diesel::sql_query(expansion_sql).bind::<Text, _>(name.as_str());
                    let class_rows: Vec<ResourceKeyRow> = q.load(conn).map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
            }

            let mut seen: std::collections::HashSet<ResourceKey> = std::collections::HashSet::new();
            all_keys.retain(|k| seen.insert(k.clone()));
            async move { Ok(all_keys) }
        }

        #[allow(clippy::too_many_lines)] // typed bind ladder for the candidate coverage read
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
            let name_placeholders: Vec<String> = (2..=names.len() + 1).map(placeholder).collect();
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

            // D-08-05: parse the holder id at the SPI boundary; invalid
            // input raises InvalidHolderId BEFORE any SQL runs, and the
            // parsed value binds with the kind's physical column type.
            let holder_bind = match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                Ok(parsed) => parsed,
                Err(e) => return async move { Err(Error::Core(e)) },
            };
            // Collect the trailing name binds; the holder is
            // placeholder 1 and binds typed through the dispatch.
            let name_binds: Vec<String> =
                names.iter().map(|name| name.as_str().to_owned()).collect();
            let rows: Vec<RoleRow> =
                load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &name_binds)?;
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
    use super::{
        DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL, split_where_any_batches,
    };
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
    use crate::holder::DieselHolderBind;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // Typed holder-bind ladders for this engine (see the macro docs).
    async_bind_ladders!(AsyncPgConnection, diesel::pg::Pg);

    async_bind_dispatch!(
        AsyncPgConnection,
        diesel::pg::Pg,
        DieselHolderBind::BigInt => BigInt,
        DieselHolderBind::Uuid => diesel::sql_types::Uuid,
        DieselHolderBind::Text => Text,
    );

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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rest: Vec<String> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        vec![name.to_owned(), String::new(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        vec![
                            name.to_owned(),
                            String::new(),
                            String::new(),
                            type_name.to_string(),
                            String::new(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        vec![
                            name.to_owned(),
                            String::new(),
                            String::new(),
                            type_name.to_string(),
                            String::new(),
                            type_name.to_string(),
                            resource_id.as_str().to_owned(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
                };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest).await?;
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rest: Vec<String> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        vec![name.to_owned(), String::new(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        vec![name.to_owned(), type_name.to_string(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        vec![
                            name.to_owned(),
                            type_name.to_string(),
                            resource_id.as_str().to_owned(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
                };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest).await?;
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // The typed ladder helper enumerates bind chains by
                // arity, up to 23 binds per statement, and
                // `has_any_roles` forwards any consumer query list:
                // chunk into ladder-sized batches, then union-dedup
                // the rows (OR across batches is OR within).
                let mut seen: Vec<RoleRecord> = Vec::new();
                for batch in split_where_any_batches(queries) {
                    let (where_clause, _) = crate::sql::build_any_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        batch,
                        2,
                    );
                    let sql = crate::sql::select_roles_for_holder(
                        &role_table,
                        &join_table,
                        &where_clause,
                        &placeholder(1),
                    );
                    // Collect the batch's trailing text binds in order:
                    // the holder is placeholder 1 and binds typed via
                    // the dispatch below.
                    let mut all_values: Vec<String> = Vec::new();
                    for query in batch {
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
                            rolify_core::query::ResourceFilter::Instance(
                                type_name,
                                resource_id,
                            ) => {
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
                    let rows: Vec<RoleRow> =
                        load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &all_values).await?;
                    for row in rows {
                        let record = row.to_record();
                        if !seen.contains(&record) {
                            seen.push(record);
                        }
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
            let rt = to_storage(role.resource_type.as_deref()).to_owned();
            let rid = resource_id_to_storage(role.resource_id.as_ref()).to_owned();
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let role_name = role.name.clone();

            async move {
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
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
                match execute_link_insert(conn, &insert_sql, &holder_bind, role_id).await {
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
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
                    let affected_rest: Vec<String> = match &scope_target {
                        RemovalTarget::NameOnly => vec![name_owned.clone()],
                        RemovalTarget::TypeSweep(type_name) => {
                            vec![name_owned.clone(), (*type_name).to_owned()]
                        }
                        RemovalTarget::Exact(type_name, resource_id) => vec![
                            name_owned.clone(),
                            (*type_name).to_owned(),
                            resource_id.as_str().to_owned(),
                        ],
                    };
                    let affected_rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(
                        conn,
                        &affected_sql,
                        &holder_bind,
                        &affected_rest,
                    )
                    .await?;
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &scope_target,
                    );
                    let removed_links =
                        execute_typed(conn, &delete_sql, &holder_bind, &affected_rest).await?;

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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // The probe row is local to this method and follows the
                // holder parse, which must run before any SQL.
                #[allow(clippy::items_after_statements)]
                #[derive(diesel::deserialize::QueryableByName)]
                #[allow(dead_code)]
                struct ExistsRow {
                    // Named exactly like the `AS dummy` projection: by-name
                    // decoding matches on it (an underscore prefix broke the
                    // match and made `exists` always false).
                    #[diesel(sql_type = diesel::sql_types::Integer)]
                    dummy: i32,
                }
                // Same swallow-as-false contract as the old get_result
                // probe: any execution error reports "no role exists".
                #[allow(clippy::question_mark)]
                let rows = match load_typed_rows::<ExistsRow>(conn, &sql, &holder_bind, &[]).await {
                    Ok(rows) => rows,
                    Err(e) => return Err(e),
                };
                let found = !rows.is_empty();
                Ok(found)
            }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &[]).await?;
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

                // Kind-aware join (D-08-04): typed kinds compare the
                // primary key natively; the string kind keeps the cast.
                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine.
                let holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "holder.id");
                let holder_join = crate::holder::holder_join_condition(
                    self.holder_id_kind(),
                    "holder.id",
                    "link.user_id",
                );
                let sql = format!(
                    "SELECT DISTINCT {holder_projection} AS user_id \
                     FROM {holder_table} AS holder \
                     INNER JOIN {join_table} AS link ON {holder_join} \
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "holders_where: too many holder types (max 4)".into(),
                            },
                        ));
                    }
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

                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine.
                let holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "id");
                let sql = format!(
                    "SELECT {holder_projection} AS user_id FROM {holder_table} WHERE {type_filter}",
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "all_holders: too many holder types (max 4)".into(),
                            },
                        ));
                    }
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
                    self.holder_id_kind(),
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
                let (holder_filter, _) = if has_holder {
                    crate::sql::roles_matching_holder_filter(self.holder_id_kind(), idx)
                } else {
                    (String::new(), idx)
                };

                let sql = base_sql
                    .replace("{type_filter}", &type_filter)
                    .replace("{name_filter}", &name_filter)
                    .replace("{scope_filter}", &scope_filter)
                    .replace("{holder_filter}", &holder_filter);

                // Leading text binds (types, optional name, optional
                // scope id); the typed holder closes the placeholder
                // sequence when present (D-08-05: parsed at the
                // boundary, bound per the kind's physical type).
                let mut text_binds: Vec<String> = query
                    .types
                    .iter()
                    .map(|resource_type| (*resource_type).to_owned())
                    .collect();
                if let Some(name) = query.name {
                    text_binds.push(name.as_str().to_owned());
                }
                match &query.scope {
                    CatalogScope::ClassOnly => {
                        text_binds.push(SCOPE_SENTINEL.to_owned());
                    }
                    CatalogScope::InstanceOnly { resource_id } => {
                        if let Some(resource_id) = resource_id {
                            text_binds.push(resource_id.as_str().to_owned());
                        }
                    }
                    CatalogScope::ClassAndInstance => {}
                }

                let rows: Vec<ResourceKeyRow> = if let Some(holder) = query.holder {
                    let holder_bind =
                        match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                            Ok(parsed) => parsed,
                            Err(e) => return Err(Error::Core(e)),
                        };
                    load_text_then_typed::<ResourceKeyRow>(conn, &sql, &text_binds, &holder_bind)
                        .await?
                } else {
                    load_text_only::<ResourceKeyRow>(conn, &sql, &text_binds).await?
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
                let expansions: Vec<(String, String)> = resource_tables
                    .iter()
                    .filter(|(type_name, _, _)| types.contains(&type_name.as_str()))
                    .map(|(type_name, resource_table, pk_column)| {
                        (
                            crate::sql::select_resources_find_class_expansion(
                                &role_table,
                                resource_table,
                                pk_column,
                                type_name,
                            ),
                            type_name.clone(),
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "resources_find: too many types (max 4)".into(),
                            },
                        ));
                    }
                };
                for row in instance_rows {
                    all_keys.push(row.to_key());
                }

                // 2. Class-scoped roles: expand via resource registry
                for (expansion_sql, _type_name) in expansions {
                    let class_rows: Vec<ResourceKeyRow> = diesel::sql_query(expansion_sql)
                        .bind::<Text, _>(&name_owned)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
                let mut seen: std::collections::HashSet<ResourceKey> =
                    std::collections::HashSet::new();
                all_keys.retain(|k| seen.insert(k.clone()));
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

                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // Collect the trailing name binds; the holder is
                // placeholder 1 and binds typed through the dispatch.
                let name_binds: Vec<String> =
                    names.iter().map(|name| name.as_str().to_owned()).collect();
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &name_binds).await?;
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
    use super::{
        DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL, split_where_any_batches,
    };
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
    use crate::holder::DieselHolderBind;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    // Typed holder-bind ladders for this engine (see the macro docs).
    async_bind_ladders!(AsyncMysqlConnection, diesel::mysql::Mysql);

    async_bind_dispatch!(
        AsyncMysqlConnection,
        diesel::mysql::Mysql,
        DieselHolderBind::BigInt => BigInt,
        DieselHolderBind::UuidBytes => diesel::sql_types::Binary,
        DieselHolderBind::Text => Text,
    );

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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rest: Vec<String> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        vec![name.to_owned(), String::new(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        vec![
                            name.to_owned(),
                            String::new(),
                            String::new(),
                            type_name.to_string(),
                            String::new(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        vec![
                            name.to_owned(),
                            String::new(),
                            String::new(),
                            type_name.to_string(),
                            String::new(),
                            type_name.to_string(),
                            resource_id.as_str().to_owned(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
                };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest).await?;
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rest: Vec<String> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        vec![name.to_owned(), String::new(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        vec![name.to_owned(), type_name.to_string(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        vec![
                            name.to_owned(),
                            type_name.to_string(),
                            resource_id.as_str().to_owned(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
                };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest).await?;
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // The typed ladder helper enumerates bind chains by
                // arity, up to 23 binds per statement, and
                // `has_any_roles` forwards any consumer query list:
                // chunk into ladder-sized batches, then union-dedup
                // the rows (OR across batches is OR within).
                let mut seen: Vec<RoleRecord> = Vec::new();
                for batch in split_where_any_batches(queries) {
                    let (where_clause, _) = crate::sql::build_any_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        batch,
                        2,
                    );
                    let sql = crate::sql::select_roles_for_holder(
                        &role_table,
                        &join_table,
                        &where_clause,
                        &placeholder(1),
                    );
                    // Collect the batch's trailing text binds in order:
                    // the holder is placeholder 1 and binds typed via
                    // the dispatch below.
                    let mut all_values: Vec<String> = Vec::new();
                    for query in batch {
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
                            rolify_core::query::ResourceFilter::Instance(
                                type_name,
                                resource_id,
                            ) => {
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
                    let rows: Vec<RoleRow> =
                        load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &all_values).await?;
                    for row in rows {
                        let record = row.to_record();
                        if !seen.contains(&record) {
                            seen.push(record);
                        }
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
            let rt = to_storage(role.resource_type.as_deref()).to_owned();
            let rid = resource_id_to_storage(role.resource_id.as_ref()).to_owned();
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let role_name = role.name.clone();

            async move {
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
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
                match execute_link_insert(conn, &insert_sql, &holder_bind, role_id).await {
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
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
                    let affected_rest: Vec<String> = match &scope_target {
                        RemovalTarget::NameOnly => vec![name_owned.clone()],
                        RemovalTarget::TypeSweep(type_name) => {
                            vec![name_owned.clone(), (*type_name).to_owned()]
                        }
                        RemovalTarget::Exact(type_name, resource_id) => vec![
                            name_owned.clone(),
                            (*type_name).to_owned(),
                            resource_id.as_str().to_owned(),
                        ],
                    };
                    let affected_rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(
                        conn,
                        &affected_sql,
                        &holder_bind,
                        &affected_rest,
                    )
                    .await?;
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &scope_target,
                    );
                    let removed_links =
                        execute_typed(conn, &delete_sql, &holder_bind, &affected_rest).await?;

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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // The probe row is local to this method and follows the
                // holder parse, which must run before any SQL.
                #[allow(clippy::items_after_statements)]
                #[derive(diesel::deserialize::QueryableByName)]
                #[allow(dead_code)]
                struct ExistsRow {
                    // Named exactly like the `AS dummy` projection: by-name
                    // decoding matches on it (an underscore prefix broke the
                    // match and made `exists` always false).
                    #[diesel(sql_type = diesel::sql_types::Integer)]
                    dummy: i32,
                }
                // Same swallow-as-false contract as the old get_result
                // probe: any execution error reports "no role exists".
                #[allow(clippy::question_mark)]
                let rows = match load_typed_rows::<ExistsRow>(conn, &sql, &holder_bind, &[]).await {
                    Ok(rows) => rows,
                    Err(e) => return Err(e),
                };
                let found = !rows.is_empty();
                Ok(found)
            }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let sql = format!(
                "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ?",
                role_table = self.role_table,
                join_table = self.join_table,
            );
            async move {
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &[]).await?;
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

                // Kind-aware join (D-08-04): typed kinds compare the
                // primary key natively; the string kind keeps the cast.
                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine, with
                // BIN_TO_UUID for the MySQL BINARY(16) uuid column.
                let holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "holder.id");
                let holder_join = crate::holder::holder_join_condition(
                    self.holder_id_kind(),
                    "holder.id",
                    "link.user_id",
                );
                let sql = format!(
                    "SELECT DISTINCT {holder_projection} AS user_id \
                     FROM {holder_table} AS holder \
                     INNER JOIN {join_table} AS link ON {holder_join} \
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "holders_where: too many holder types (max 4)".into(),
                            },
                        ));
                    }
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

                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine, with
                // BIN_TO_UUID for the MySQL BINARY(16) uuid column.
                let holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "id");
                let sql = format!(
                    "SELECT {holder_projection} AS user_id FROM {holder_table} WHERE {type_filter}",
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "all_holders: too many holder types (max 4)".into(),
                            },
                        ));
                    }
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
                    self.holder_id_kind(),
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
                let (holder_filter, _) = if has_holder {
                    crate::sql::roles_matching_holder_filter(self.holder_id_kind(), idx)
                } else {
                    (String::new(), idx)
                };

                let sql = base_sql
                    .replace("{type_filter}", &type_filter)
                    .replace("{name_filter}", &name_filter)
                    .replace("{scope_filter}", &scope_filter)
                    .replace("{holder_filter}", &holder_filter);

                // Leading text binds (types, optional name, optional
                // scope id); the typed holder closes the placeholder
                // sequence when present (D-08-05: parsed at the
                // boundary, bound per the kind's physical type).
                let mut text_binds: Vec<String> = query
                    .types
                    .iter()
                    .map(|resource_type| (*resource_type).to_owned())
                    .collect();
                if let Some(name) = query.name {
                    text_binds.push(name.as_str().to_owned());
                }
                match &query.scope {
                    CatalogScope::ClassOnly => {
                        text_binds.push(SCOPE_SENTINEL.to_owned());
                    }
                    CatalogScope::InstanceOnly { resource_id } => {
                        if let Some(resource_id) = resource_id {
                            text_binds.push(resource_id.as_str().to_owned());
                        }
                    }
                    CatalogScope::ClassAndInstance => {}
                }

                let rows: Vec<ResourceKeyRow> = if let Some(holder) = query.holder {
                    let holder_bind =
                        match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                            Ok(parsed) => parsed,
                            Err(e) => return Err(Error::Core(e)),
                        };
                    load_text_then_typed::<ResourceKeyRow>(conn, &sql, &text_binds, &holder_bind)
                        .await?
                } else {
                    load_text_only::<ResourceKeyRow>(conn, &sql, &text_binds).await?
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
                let expansions: Vec<(String, String)> = resource_tables
                    .iter()
                    .filter(|(type_name, _, _)| types.contains(&type_name.as_str()))
                    .map(|(type_name, resource_table, pk_column)| {
                        (
                            crate::sql::select_resources_find_class_expansion(
                                &role_table,
                                resource_table,
                                pk_column,
                                type_name,
                            ),
                            type_name.clone(),
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "resources_find: too many types (max 4)".into(),
                            },
                        ));
                    }
                };
                for row in instance_rows {
                    all_keys.push(row.to_key());
                }

                // 2. Class-scoped roles: expand via resource registry
                for (expansion_sql, _type_name) in expansions {
                    let class_rows: Vec<ResourceKeyRow> = diesel::sql_query(expansion_sql)
                        .bind::<Text, _>(&name_owned)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
                let mut seen: std::collections::HashSet<ResourceKey> =
                    std::collections::HashSet::new();
                all_keys.retain(|k| seen.insert(k.clone()));
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

                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // Collect the trailing name binds; the holder is
                // placeholder 1 and binds typed through the dispatch.
                let name_binds: Vec<String> =
                    names.iter().map(|name| name.as_str().to_owned()).collect();
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &name_binds).await?;
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
    use super::{
        DieselStore, ResourceId, ResourceRef, RoleName, SCOPE_SENTINEL, split_where_any_batches,
    };
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
    use crate::holder::DieselHolderBind;
    use crate::rows::{HolderIdRow, IdRow, ResourceKeyRow, RoleRow};
    use crate::sentinel::{resource_id_to_storage, to_storage};

    type AsyncSqliteConnection = SyncConnectionWrapper<SqliteConnection>;

    // Typed holder-bind ladders for this engine (see the macro docs).
    async_bind_ladders!(AsyncSqliteConnection, diesel::sqlite::Sqlite);

    async_bind_dispatch!(
        AsyncSqliteConnection,
        diesel::sqlite::Sqlite,
        DieselHolderBind::BigInt => BigInt,
        DieselHolderBind::Text => Text,
    );

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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rest: Vec<String> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        vec![name.to_owned(), String::new(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        vec![
                            name.to_owned(),
                            String::new(),
                            String::new(),
                            type_name.to_string(),
                            String::new(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        vec![
                            name.to_owned(),
                            String::new(),
                            String::new(),
                            type_name.to_string(),
                            String::new(),
                            type_name.to_string(),
                            resource_id.as_str().to_owned(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
                };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest).await?;
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rest: Vec<String> = match &query.filter {
                    rolify_core::query::ResourceFilter::Global => {
                        vec![name.to_owned(), String::new(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Class(type_name) => {
                        vec![name.to_owned(), type_name.to_string(), String::new()]
                    }
                    rolify_core::query::ResourceFilter::Instance(type_name, resource_id) => {
                        vec![
                            name.to_owned(),
                            type_name.to_string(),
                            resource_id.as_str().to_owned(),
                        ]
                    }
                    rolify_core::query::ResourceFilter::Any => vec![name.to_owned()],
                };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &rest).await?;
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // The typed ladder helper enumerates bind chains by
                // arity, up to 23 binds per statement, and
                // `has_any_roles` forwards any consumer query list:
                // chunk into ladder-sized batches, then union-dedup
                // the rows (OR across batches is OR within).
                let mut seen: Vec<RoleRecord> = Vec::new();
                for batch in split_where_any_batches(queries) {
                    let (where_clause, _) = crate::sql::build_any_where(
                        &role_table,
                        &join_table,
                        &placeholder(1),
                        batch,
                        2,
                    );
                    let sql = crate::sql::select_roles_for_holder(
                        &role_table,
                        &join_table,
                        &where_clause,
                        &placeholder(1),
                    );
                    // Collect the batch's trailing text binds in order:
                    // the holder is placeholder 1 and binds typed via
                    // the dispatch below.
                    let mut all_values: Vec<String> = Vec::new();
                    for query in batch {
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
                            rolify_core::query::ResourceFilter::Instance(
                                type_name,
                                resource_id,
                            ) => {
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
                    let rows: Vec<RoleRow> =
                        load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &all_values).await?;
                    for row in rows {
                        let record = row.to_record();
                        if !seen.contains(&record) {
                            seen.push(record);
                        }
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
            let rt = to_storage(role.resource_type.as_deref()).to_owned();
            let rid = resource_id_to_storage(role.resource_id.as_ref()).to_owned();
            let role_table = self.role_table.clone();
            let join_table = self.join_table.clone();
            let role_name = role.name.clone();

            async move {
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
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
                match execute_link_insert(conn, &insert_sql, &holder_bind, role_id).await {
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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
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
                    let affected_rest: Vec<String> = match &scope_target {
                        RemovalTarget::NameOnly => vec![name_owned.clone()],
                        RemovalTarget::TypeSweep(type_name) => {
                            vec![name_owned.clone(), (*type_name).to_owned()]
                        }
                        RemovalTarget::Exact(type_name, resource_id) => vec![
                            name_owned.clone(),
                            (*type_name).to_owned(),
                            resource_id.as_str().to_owned(),
                        ],
                    };
                    let affected_rows: Vec<RoleRow> = load_typed_rows::<RoleRow>(
                        conn,
                        &affected_sql,
                        &holder_bind,
                        &affected_rest,
                    )
                    .await?;
                    let affected_records: Vec<RoleRecord> =
                        affected_rows.into_iter().map(|r| r.to_record()).collect();

                    let delete_sql = crate::sql::delete_links_for_target(
                        &join_table,
                        &role_table,
                        &scope_target,
                    );
                    let removed_links =
                        execute_typed(conn, &delete_sql, &holder_bind, &affected_rest).await?;

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
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // The probe row is local to this method and follows the
                // holder parse, which must run before any SQL.
                #[allow(clippy::items_after_statements)]
                #[derive(diesel::deserialize::QueryableByName)]
                #[allow(dead_code)]
                struct ExistsRow {
                    // Named exactly like the `AS dummy` projection: by-name
                    // decoding matches on it (an underscore prefix broke the
                    // match and made `exists` always false).
                    #[diesel(sql_type = diesel::sql_types::Integer)]
                    dummy: i32,
                }
                // Same swallow-as-false contract as the old get_result
                // probe: any execution error reports "no role exists".
                #[allow(clippy::question_mark)]
                let rows = match load_typed_rows::<ExistsRow>(conn, &sql, &holder_bind, &[]).await {
                    Ok(rows) => rows,
                    Err(e) => return Err(e),
                };
                let found = !rows.is_empty();
                Ok(found)
            }
        }

        fn roles_of(
            &self,
            conn: &mut Self::Conn,
            holder: &ResourceId,
        ) -> impl Future<Output = Result<Vec<RoleRecord>, Self::Error>> + Send {
            let sql = format!(
                "SELECT role_row.name AS name, role_row.resource_type AS resource_type, role_row.resource_id AS resource_id \
                 FROM {role_table} AS role_row \
                 INNER JOIN {join_table} AS link ON link.role_id = role_row.id \
                 WHERE link.user_id = ?",
                role_table = self.role_table,
                join_table = self.join_table,
            );
            async move {
                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &[]).await?;
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

                // Kind-aware join (D-08-04): typed kinds compare the
                // primary key natively; the string kind keeps the cast.
                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine.
                let holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "holder.id");
                let holder_join = crate::holder::holder_join_condition(
                    self.holder_id_kind(),
                    "holder.id",
                    "link.user_id",
                );
                let sql = format!(
                    "SELECT DISTINCT {holder_projection} AS user_id \
                     FROM {holder_table} AS holder \
                     INNER JOIN {join_table} AS link ON {holder_join} \
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "holders_where: too many holder types (max 4)".into(),
                            },
                        ));
                    }
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

                // Kind-aware projection (WR-01): canonical text for the
                // Text-decoding HolderIdRow on every engine.
                let holder_projection =
                    crate::holder::holder_text_projection(self.holder_id_kind(), "id");
                let sql = format!(
                    "SELECT {holder_projection} AS user_id FROM {holder_table} WHERE {type_filter}",
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "all_holders: too many holder types (max 4)".into(),
                            },
                        ));
                    }
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
                    self.holder_id_kind(),
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
                let (holder_filter, _) = if has_holder {
                    crate::sql::roles_matching_holder_filter(self.holder_id_kind(), idx)
                } else {
                    (String::new(), idx)
                };

                let sql = base_sql
                    .replace("{type_filter}", &type_filter)
                    .replace("{name_filter}", &name_filter)
                    .replace("{scope_filter}", &scope_filter)
                    .replace("{holder_filter}", &holder_filter);

                // Leading text binds (types, optional name, optional
                // scope id); the typed holder closes the placeholder
                // sequence when present (D-08-05: parsed at the
                // boundary, bound per the kind's physical type).
                let mut text_binds: Vec<String> = query
                    .types
                    .iter()
                    .map(|resource_type| (*resource_type).to_owned())
                    .collect();
                if let Some(name) = query.name {
                    text_binds.push(name.as_str().to_owned());
                }
                match &query.scope {
                    CatalogScope::ClassOnly => {
                        text_binds.push(SCOPE_SENTINEL.to_owned());
                    }
                    CatalogScope::InstanceOnly { resource_id } => {
                        if let Some(resource_id) = resource_id {
                            text_binds.push(resource_id.as_str().to_owned());
                        }
                    }
                    CatalogScope::ClassAndInstance => {}
                }

                let rows: Vec<ResourceKeyRow> = if let Some(holder) = query.holder {
                    let holder_bind =
                        match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                            Ok(parsed) => parsed,
                            Err(e) => return Err(Error::Core(e)),
                        };
                    load_text_then_typed::<ResourceKeyRow>(conn, &sql, &text_binds, &holder_bind)
                        .await?
                } else {
                    load_text_only::<ResourceKeyRow>(conn, &sql, &text_binds).await?
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
                let expansions: Vec<(String, String)> = resource_tables
                    .iter()
                    .filter(|(type_name, _, _)| types.contains(&type_name.as_str()))
                    .map(|(type_name, resource_table, pk_column)| {
                        (
                            crate::sql::select_resources_find_class_expansion(
                                &role_table,
                                resource_table,
                                pk_column,
                                type_name,
                            ),
                            type_name.clone(),
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
                    _ => {
                        return Err(Error::Core(
                            rolify_core::error::RolifyError::InvalidConfig {
                                reason: "resources_find: too many types (max 4)".into(),
                            },
                        ));
                    }
                };
                for row in instance_rows {
                    all_keys.push(row.to_key());
                }

                // 2. Class-scoped roles: expand via resource registry
                for (expansion_sql, _type_name) in expansions {
                    let class_rows: Vec<ResourceKeyRow> = diesel::sql_query(expansion_sql)
                        .bind::<Text, _>(&name_owned)
                        .load(conn)
                        .await
                        .map_err(Error::Diesel)?;
                    for row in class_rows {
                        all_keys.push(row.to_key());
                    }
                }
                let mut seen: std::collections::HashSet<ResourceKey> =
                    std::collections::HashSet::new();
                all_keys.retain(|k| seen.insert(k.clone()));
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

                // D-08-05: parse the holder id at the SPI boundary;
                // invalid input raises InvalidHolderId BEFORE any SQL
                // runs, and the parsed value binds with the kind's
                // physical column type.
                let holder_bind =
                    match crate::holder::parsed_for_store(self.holder_id_kind(), holder) {
                        Ok(parsed) => parsed,
                        Err(e) => return Err(Error::Core(e)),
                    };
                // Collect the trailing name binds; the holder is
                // placeholder 1 and binds typed through the dispatch.
                let name_binds: Vec<String> =
                    names.iter().map(|name| name.as_str().to_owned()).collect();
                let rows: Vec<RoleRow> =
                    load_typed_rows::<RoleRow>(conn, &sql, &holder_bind, &name_binds).await?;
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

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "role table name must pass validation")]
    fn with_tables_rejects_role_table_quote_breakout() {
        let _ = DieselStore::with_tables("x\"; DROP TABLE users; --", "users_roles");
    }

    #[test]
    #[should_panic(expected = "join table name must pass validation")]
    fn with_tables_rejects_join_table_quote_breakout() {
        let _ = DieselStore::with_tables("roles", "links\"; DELETE FROM roles WHERE 1=1; --");
    }

    #[test]
    fn with_tables_quotes_valid_identifiers() {
        let store = DieselStore::with_tables("custom_roles", "custom_role_links");
        assert_eq!(store.role_table, quote_identifier("custom_roles"));
        assert_eq!(store.join_table, quote_identifier("custom_role_links"));
    }

    #[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
    mod where_any_batches {
        use super::super::*;

        #[test]
        fn instance_queries_split_past_the_bind_cap() {
            let visitor = RoleName::from("visitor");
            let forum_id = ResourceId::from(1_i64);
            let queries = [
                RoleQuery::with_role_and_filter(
                    &visitor,
                    ResourceFilter::Instance("Forum", &forum_id),
                ),
                RoleQuery::with_role_and_filter(
                    &visitor,
                    ResourceFilter::Instance("Forum", &forum_id),
                ),
                RoleQuery::with_role_and_filter(
                    &visitor,
                    ResourceFilter::Instance("Forum", &forum_id),
                ),
                RoleQuery::with_role_and_filter(
                    &visitor,
                    ResourceFilter::Instance("Forum", &forum_id),
                ),
            ];
            // Holder + 4 x 7 = 29 binds: three queries fit (22), the
            // fourth opens a second batch.
            let batches = split_where_any_batches(&queries);
            assert_eq!(batches.len(), 2);
            assert_eq!(batches[0].len(), 3);
            assert_eq!(batches[1].len(), 1);
        }

        #[test]
        fn global_queries_split_past_the_bind_cap() {
            let admin = RoleName::from("admin");
            let queries = [
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
            ];
            // Holder + 8 x 3 = 25 binds: seven fit (22), the eighth
            // opens a second batch.
            let batches = split_where_any_batches(&queries);
            assert_eq!(batches.len(), 2);
            assert_eq!(batches[0].len(), 7);
            assert_eq!(batches[1].len(), 1);
        }

        #[test]
        fn suite_shaped_batch_stays_whole() {
            let admin = RoleName::from("admin");
            let staff = RoleName::from("staff");
            let forum_id = ResourceId::from(1_i64);
            let queries = [
                RoleQuery::with_role_and_filter(
                    &staff,
                    ResourceFilter::Instance("Forum", &forum_id),
                ),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
                RoleQuery::with_role_and_filter(
                    &admin,
                    ResourceFilter::Instance("Forum", &forum_id),
                ),
                RoleQuery::with_role_and_filter(&admin, ResourceFilter::Class("Forum")),
            ];
            // Holder + 7 + 3 + 7 + 5 = 23 binds: exactly one batch (the
            // ported suite's largest shape).
            let batches = split_where_any_batches(&queries);
            assert_eq!(batches.len(), 1);
            assert_eq!(batches[0].len(), 4);
        }
    }
}
