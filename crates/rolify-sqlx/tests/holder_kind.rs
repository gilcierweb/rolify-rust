//! Holder id kind matrix for the `SQLx` adapter (Phase 08 D-08-04/D-08-05).
//!
//! Each compiled engine runs the grant/check/revoke lifecycle against a
//! schema whose `users_roles.user_id` column carries the kind's physical
//! type (D-08-04). The kind-specific DDL is the canonical migration tree
//! for that engine, embedded byte-for-byte via `include_str!`, so this
//! suite doubles as a local confirmation that the trees apply.
//!
//! Engine features are additive for the lib but the test suite runs one
//! engine at a time via `--features`. The `SQLite` leg runs locally without
//! Docker; Postgres and `MySQL` legs use testcontainers (`MySQL` is CI-only
//! where libmysqlclient is present).

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod support;

use rolify_core::config::{HolderIdKind, RolifyConfig};

#[cfg(any(feature = "postgres", feature = "mysql"))]
use std::sync::atomic::{AtomicBool, Ordering};

/// Canonical holder id string per kind (matches `data.rs` shapes).
fn holder_id_for(kind: HolderIdKind) -> &'static str {
    match kind {
        HolderIdKind::Integer => "42",
        HolderIdKind::Uuid => "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8",
        HolderIdKind::String => "user-1",
    }
}

/// The three kinds the matrix exercises.
fn all_kinds() -> [HolderIdKind; 3] {
    [
        HolderIdKind::Integer,
        HolderIdKind::Uuid,
        HolderIdKind::String,
    ]
}

/// The kinds that reject malformed input: `String` accepts every value
/// (including the empty string), so it has no invalid sample (D-08-05).
fn rejecting_kinds() -> [HolderIdKind; 2] {
    [HolderIdKind::Integer, HolderIdKind::Uuid]
}

/// A holder id string that must be rejected under `kind` (D-08-05).
fn invalid_holder_id_for(kind: HolderIdKind) -> &'static str {
    match kind {
        HolderIdKind::Integer => "abc",
        HolderIdKind::Uuid => "not-a-uuid",
        // `String` accepts anything; never reached (see `rejecting_kinds`).
        HolderIdKind::String => unreachable!("String kind rejects nothing"),
    }
}

/// Create a config with the specified holder id kind.
fn config_for(kind: HolderIdKind) -> RolifyConfig {
    RolifyConfig::builder()
        .holder_id_kind(kind)
        .build()
        .expect("default config")
}

/// Split a multi-statement SQL file into individual statements, skipping
/// comment lines (`cli/tests/e2e_engines.rs` precedent): Postgres and
/// `MySQL` reject multiple commands inside one prepared statement
/// (SQLSTATE 42601 on Postgres), so the canonical per-kind migration file
/// is applied one statement at a time. `SQLite` takes the same split
/// path identically.
fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in sql.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("--") {
            continue;
        }

        current.push_str(line);
        current.push('\n');

        if trimmed.ends_with(';') {
            statements.push(current.trim().to_string());
            current.clear();
        }
    }

    statements
}

/// Process-wide serializer for the shared-database legs: the pg and mysql
/// tests each drop and re-create the roles tables on the SHARED container
/// database, so without the guard they race and wipe each other's schema
/// mid-test (diesel `holder_kind.rs` `SuiteGuard` precedent, commit 52be22e).
/// `SQLite` legs use fresh per-connection in-memory databases and need no
/// guard.
#[cfg(any(feature = "postgres", feature = "mysql"))]
static SUITE_SERIAL: AtomicBool = AtomicBool::new(false);

/// Held for one shared-database test's whole body; releases on drop.
#[cfg(any(feature = "postgres", feature = "mysql"))]
struct SuiteGuard {
    flag: &'static AtomicBool,
}

#[cfg(any(feature = "postgres", feature = "mysql"))]
impl SuiteGuard {
    /// Acquire the process-wide lock (spins on `yield_now`:
    /// timing-free, liveness-only).
    fn acquire() -> Self {
        while SUITE_SERIAL.swap(true, Ordering::Acquire) {
            std::thread::yield_now();
        }
        Self {
            flag: &SUITE_SERIAL,
        }
    }
}

#[cfg(any(feature = "postgres", feature = "mysql"))]
impl Drop for SuiteGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// `SQLite` leg: no container, runs locally for all three kinds.
#[cfg(feature = "sqlite")]
mod sqlite {
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName};
    use rolify_core::store::RoleStore;
    use rolify_sqlx::{Error, SqlxStore};

    use crate::support::sqlite_conn;
    use crate::{
        all_kinds, config_for, holder_id_for, invalid_holder_id_for, rejecting_kinds,
        split_sql_statements,
    };

    /// Canonical `SQLite` DDL for the kind's join column type (D-08-04).
    fn schema_ddl(kind: HolderIdKind) -> &'static str {
        match kind {
            HolderIdKind::Integer => {
                include_str!("../migrations/sqlite/integer/0000000001_rolify_create_tables.sql")
            }
            HolderIdKind::Uuid => {
                include_str!("../migrations/sqlite/uuid/0000000001_rolify_create_tables.sql")
            }
            HolderIdKind::String => {
                include_str!("../migrations/sqlite/string/0000000001_rolify_create_tables.sql")
            }
        }
    }

    /// Apply the kind's canonical schema one statement at a time:
    /// Postgres and `MySQL` reject multiple commands inside one prepared
    /// statement (SQLSTATE 42601 on Postgres), so the canonical migration
    /// file is split before execution; `SQLite` takes the split path
    /// identically (`cli/tests/e2e_engines.rs` precedent).
    async fn apply_schema(conn: &mut sqlx::SqliteConnection, sql: &'static str) {
        for statement in split_sql_statements(sql) {
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .execute(&mut *conn)
                .await
                .expect("apply kind-specific DDL statement");
        }
    }

    fn store_for(kind: HolderIdKind) -> SqlxStore<sqlx::Sqlite> {
        SqlxStore::new(&config_for(kind))
    }

    #[sqlx::test]
    async fn lifecycle_all_kinds() {
        for kind in all_kinds() {
            let mut conn = sqlite_conn().await;
            apply_schema(&mut conn, schema_ddl(kind)).await;

            let mut store = store_for(kind);
            let holder = ResourceId::from(holder_id_for(kind));

            let admin = store
                .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
                .await
                .expect("find_or_create_by global admin");
            let query = RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            };

            // add
            assert!(
                store
                    .add(&mut conn, &holder, &admin)
                    .await
                    .expect("add admin"),
                "first add inserts a link for {kind:?}"
            );
            // idempotent add
            assert!(
                !store
                    .add(&mut conn, &holder, &admin)
                    .await
                    .expect("add admin again"),
                "second add is a no-op for {kind:?}"
            );
            // has_role (non-strict ladder)
            assert_eq!(
                store
                    .where_(&mut conn, &holder, &query)
                    .await
                    .expect("where_ admin")
                    .len(),
                1,
                "where_ finds the granted role for {kind:?}"
            );
            // finder
            assert_eq!(
                store
                    .roles_of(&mut conn, &holder)
                    .await
                    .expect("roles_of")
                    .len(),
                1,
                "roles_of lists the granted role for {kind:?}"
            );
            // remove
            let outcome = store
                .remove(
                    &mut conn,
                    &holder,
                    &RoleName::from("admin"),
                    RemovalTarget::NameOnly,
                    true,
                )
                .await
                .expect("remove admin");
            assert_eq!(outcome.removed_links, 1, "removed one link for {kind:?}");
            assert_eq!(
                store
                    .roles_of(&mut conn, &holder)
                    .await
                    .expect("roles_of after remove")
                    .len(),
                0,
                "no roles remain for {kind:?}"
            );
        }
    }

    #[sqlx::test]
    async fn invalid_holder_id_rejected_all_kinds() {
        for kind in rejecting_kinds() {
            let mut conn = sqlite_conn().await;
            apply_schema(&mut conn, schema_ddl(kind)).await;

            let mut store = store_for(kind);
            let admin = store
                .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
                .await
                .expect("find_or_create_by global admin");
            let bad_holder = ResourceId::from(invalid_holder_id_for(kind));

            let error = store
                .add(&mut conn, &bad_holder, &admin)
                .await
                .expect_err("invalid holder id must be rejected pre-SQL");
            assert!(
                matches!(error, Error::Core(RolifyError::InvalidHolderId { .. })),
                "expected InvalidHolderId for {kind:?}, got {error:?}"
            );
        }
    }
}

/// `Postgres` leg: testcontainers (runs where Docker is available).
#[cfg(feature = "postgres")]
mod postgres {
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName};
    use rolify_core::store::RoleStore;
    use rolify_sqlx::{Error, SqlxStore};
    use sqlx::PgConnection;

    use crate::support::pg_conn;
    use crate::{
        SuiteGuard, all_kinds, config_for, holder_id_for, invalid_holder_id_for, rejecting_kinds,
        split_sql_statements,
    };

    fn schema_ddl(kind: HolderIdKind) -> &'static str {
        match kind {
            HolderIdKind::Integer => {
                include_str!("../migrations/postgres/integer/0000000001_rolify_create_tables.sql")
            }
            HolderIdKind::Uuid => {
                include_str!("../migrations/postgres/uuid/0000000001_rolify_create_tables.sql")
            }
            HolderIdKind::String => {
                include_str!("../migrations/postgres/string/0000000001_rolify_create_tables.sql")
            }
        }
    }

    /// Apply the kind's canonical schema one statement at a time
    /// (see the `SQLite` module's `apply_schema` note: Postgres rejects
    /// multiple commands inside one prepared statement, SQLSTATE 42601).
    async fn apply_schema(conn: &mut sqlx::PgConnection, sql: &'static str) {
        for statement in split_sql_statements(sql) {
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .execute(&mut *conn)
                .await
                .expect("apply kind-specific DDL statement");
        }
    }

    fn store_for(kind: HolderIdKind) -> SqlxStore<sqlx::Postgres> {
        SqlxStore::new(&config_for(kind))
    }

    async fn fresh_conn(kind: HolderIdKind) -> PgConnection {
        let mut conn = pg_conn().await;
        sqlx::query("DROP TABLE IF EXISTS users_roles, roles CASCADE")
            .execute(&mut conn)
            .await
            .expect("drop prior tables");
        apply_schema(&mut conn, schema_ddl(kind)).await;
        conn
    }

    #[sqlx::test]
    async fn lifecycle_all_kinds() {
        // Both tests on this engine drop and re-create the roles tables on
        // the SHARED container database; the guard serializes them for
        // the whole test body (diesel holder_kind.rs precedent, 52be22e).
        let _suite_guard = SuiteGuard::acquire();
        for kind in all_kinds() {
            let mut conn = fresh_conn(kind).await;
            let mut store = store_for(kind);
            let holder = ResourceId::from(holder_id_for(kind));

            let admin = store
                .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
                .await
                .expect("find_or_create_by global admin");
            let query = RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            };

            assert!(
                store
                    .add(&mut conn, &holder, &admin)
                    .await
                    .expect("add admin"),
                "first add inserts a link for {kind:?}"
            );
            assert_eq!(
                store
                    .where_(&mut conn, &holder, &query)
                    .await
                    .expect("where_ admin")
                    .len(),
                1,
                "where_ finds the granted role for {kind:?}"
            );
            assert_eq!(
                store
                    .roles_of(&mut conn, &holder)
                    .await
                    .expect("roles_of")
                    .len(),
                1,
                "roles_of lists the granted role for {kind:?}"
            );
            let outcome = store
                .remove(
                    &mut conn,
                    &holder,
                    &RoleName::from("admin"),
                    RemovalTarget::NameOnly,
                    true,
                )
                .await
                .expect("remove admin");
            assert_eq!(outcome.removed_links, 1, "removed one link for {kind:?}");
        }
    }

    #[sqlx::test]
    async fn invalid_holder_id_rejected_all_kinds() {
        // Same shared-container serialization as the lifecycle leg.
        let _suite_guard = SuiteGuard::acquire();
        for kind in rejecting_kinds() {
            let mut conn = fresh_conn(kind).await;
            let mut store = store_for(kind);
            let admin = store
                .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
                .await
                .expect("find_or_create_by global admin");
            let bad_holder = ResourceId::from(invalid_holder_id_for(kind));

            let error = store
                .add(&mut conn, &bad_holder, &admin)
                .await
                .expect_err("invalid holder id must be rejected pre-SQL");
            assert!(
                matches!(error, Error::Core(RolifyError::InvalidHolderId { .. })),
                "expected InvalidHolderId for {kind:?}, got {error:?}"
            );
        }
    }
}

/// `MySQL` leg: testcontainers (CI-only where libmysqlclient is installed).
#[cfg(feature = "mysql")]
mod mysql {
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName};
    use rolify_core::store::RoleStore;
    use rolify_sqlx::{Error, SqlxStore};
    use sqlx::MySqlConnection;

    use crate::support::mysql_conn;
    use crate::{
        SuiteGuard, all_kinds, config_for, holder_id_for, invalid_holder_id_for, rejecting_kinds,
        split_sql_statements,
    };

    fn schema_ddl(kind: HolderIdKind) -> &'static str {
        match kind {
            HolderIdKind::Integer => {
                include_str!("../migrations/mysql/integer/0000000001_rolify_create_tables.sql")
            }
            HolderIdKind::Uuid => {
                include_str!("../migrations/mysql/uuid/0000000001_rolify_create_tables.sql")
            }
            HolderIdKind::String => {
                include_str!("../migrations/mysql/string/0000000001_rolify_create_tables.sql")
            }
        }
    }

    /// Apply the kind's canonical schema one statement at a time
    /// (see the `SQLite` module's `apply_schema` note: `MySQL` rejects
    /// multiple commands inside one prepared statement).
    async fn apply_schema(conn: &mut sqlx::MySqlConnection, sql: &'static str) {
        for statement in split_sql_statements(sql) {
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .execute(&mut *conn)
                .await
                .expect("apply kind-specific DDL statement");
        }
    }

    fn store_for(kind: HolderIdKind) -> SqlxStore<sqlx::MySql> {
        SqlxStore::new(&config_for(kind))
    }

    async fn fresh_conn(kind: HolderIdKind) -> MySqlConnection {
        let mut conn = mysql_conn().await;
        sqlx::query("DROP TABLE IF EXISTS users_roles")
            .execute(&mut conn)
            .await
            .expect("drop join");
        sqlx::query("DROP TABLE IF EXISTS roles")
            .execute(&mut conn)
            .await
            .expect("drop roles");
        apply_schema(&mut conn, schema_ddl(kind)).await;
        conn
    }

    #[sqlx::test]
    async fn lifecycle_all_kinds() {
        // Both tests on this engine drop and re-create the roles tables on
        // the SHARED container database; the guard serializes them for
        // the whole test body (diesel holder_kind.rs precedent, 52be22e).
        let _suite_guard = SuiteGuard::acquire();
        for kind in all_kinds() {
            let mut conn = fresh_conn(kind).await;
            let mut store = store_for(kind);
            let holder = ResourceId::from(holder_id_for(kind));

            let admin = store
                .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
                .await
                .expect("find_or_create_by global admin");
            let query = RoleQuery {
                name: &RoleName::from("admin"),
                filter: ResourceFilter::Global,
            };

            assert!(
                store
                    .add(&mut conn, &holder, &admin)
                    .await
                    .expect("add admin"),
                "first add inserts a link for {kind:?}"
            );
            assert_eq!(
                store
                    .where_(&mut conn, &holder, &query)
                    .await
                    .expect("where_ admin")
                    .len(),
                1,
                "where_ finds the granted role for {kind:?}"
            );
            assert_eq!(
                store
                    .roles_of(&mut conn, &holder)
                    .await
                    .expect("roles_of")
                    .len(),
                1,
                "roles_of lists the granted role for {kind:?}"
            );
            let outcome = store
                .remove(
                    &mut conn,
                    &holder,
                    &RoleName::from("admin"),
                    RemovalTarget::NameOnly,
                    true,
                )
                .await
                .expect("remove admin");
            assert_eq!(outcome.removed_links, 1, "removed one link for {kind:?}");
        }
    }

    #[sqlx::test]
    async fn invalid_holder_id_rejected_all_kinds() {
        // Same shared-container serialization as the lifecycle leg.
        let _suite_guard = SuiteGuard::acquire();
        for kind in rejecting_kinds() {
            let mut conn = fresh_conn(kind).await;
            let mut store = store_for(kind);
            let admin = store
                .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
                .await
                .expect("find_or_create_by global admin");
            let bad_holder = ResourceId::from(invalid_holder_id_for(kind));

            let error = store
                .add(&mut conn, &bad_holder, &admin)
                .await
                .expect_err("invalid holder id must be rejected pre-SQL");
            assert!(
                matches!(error, Error::Core(RolifyError::InvalidHolderId { .. })),
                "expected InvalidHolderId for {kind:?}, got {error:?}"
            );
        }
    }
}
