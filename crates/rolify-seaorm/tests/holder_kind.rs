//! Holder id kind matrix for the `SeaORM` adapter (Phase 08 D-08-04/D-08-05).
//!
//! Each compiled engine runs the grant/check/revoke lifecycle against a
//! schema whose `users_roles.user_id` column carries the kind's physical
//! type (D-08-04). The kind-specific DDL is the canonical migration tree
//! for that engine, applied via the `SeaORM` migrator, so this suite doubles
//! as a local confirmation that the trees apply.
//!
//! Engine features are additive for the lib but the test suite runs one
//! engine at a time via `--features`. The `SQLite` leg is out of scope for
//! `SeaORM` (no `SeaORM` `SQLite` async support in this crate). Postgres and
//! `MySQL` legs use testcontainers (`MySQL` is CI-only where libmysqlclient
//! is present).

#[cfg(any(feature = "postgres", feature = "mysql"))]
mod support;

use rolify_core::config::{HolderIdKind, RolifyConfig};
use rolify_seaorm::migration::m20261003_000001_create_roles::{get_statements, MigrationDirection};
use sea_orm::{ConnectionTrait, DbBackend};

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

/// Execute the migration for the given backend and kind.
async fn apply_migration(
    conn: &sea_orm::DatabaseConnection,
    backend: DbBackend,
    kind: HolderIdKind,
) {
    let stmts = get_statements(backend, kind, MigrationDirection::Up)
        .expect("get migration statements");
    for stmt in stmts {
        conn.execute_unprepared(stmt)
            .await
            .expect("apply migration statement");
    }
}

/// Drop all tables for the given backend.
async fn drop_tables(
    conn: &sea_orm::DatabaseConnection,
    backend: DbBackend,
) {
    let stmts = get_statements(backend, HolderIdKind::Integer, MigrationDirection::Down)
        .expect("get down migration statements");
    for stmt in stmts {
        let _ = conn.execute_raw(sea_orm::Statement::from_sql_and_values(backend, *stmt, vec![])).await; // ignore errors
    }
}

/// Postgres leg: testcontainers (runs where Docker is available).
#[cfg(feature = "postgres")]
mod postgres {
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName};
    use rolify_core::store::RoleStore;
    use rolify_seaorm::{SeaormStore, Error};
    use sea_orm::DatabaseConnection;

    use crate::support::pg::connect as pg_conn;
    #[cfg(feature = "mysql")]
    use crate::support::mysql::connect as mysql_conn;
    use crate::{all_kinds, holder_id_for, invalid_holder_id_for, rejecting_kinds, config_for, apply_migration, drop_tables};

    fn store_for(kind: HolderIdKind) -> SeaormStore<DatabaseConnection> {
        SeaormStore::new(&config_for(kind))
    }

    async fn fresh_conn(kind: HolderIdKind) -> DatabaseConnection {
        let conn = pg_conn().await;
        drop_tables(&conn, sea_orm::DbBackend::Postgres).await;
        apply_migration(&conn, sea_orm::DbBackend::Postgres, kind).await;
        conn
    }

    #[tokio::test]
    async fn lifecycle_all_kinds() {
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

            // add
            assert!(
                store.add(&mut conn, &holder, &admin).await.expect("add admin"),
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

    #[tokio::test]
    async fn invalid_holder_id_rejected_all_kinds() {
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
                matches!(
                    error,
                    Error::Core(RolifyError::InvalidHolderId { .. })
                ),
                "expected InvalidHolderId for {kind:?}, got {error:?}"
            );
        }
    }
}

/// MySQL leg: testcontainers (CI-only where libmysqlclient is installed).
#[cfg(feature = "mysql")]
mod mysql {
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName};
    use rolify_core::store::RoleStore;
    use rolify_seaorm::{SeaormStore, Error};
    use sea_orm::DatabaseConnection;

    use crate::support::mysql_conn;
    use crate::{all_kinds, holder_id_for, invalid_holder_id_for, rejecting_kinds, config_for, apply_migration, drop_tables};

    fn store_for(kind: HolderIdKind) -> SeaormStore<DatabaseConnection> {
        SeaormStore::new(&config_for(kind))
    }

    async fn fresh_conn(kind: HolderIdKind) -> DatabaseConnection {
        let mut conn = mysql_conn().await;
        drop_tables(&conn, sea_orm::DbBackend::MySql).await;
        apply_migration(&conn, sea_orm::DbBackend::MySql, kind).await;
        conn
    }

    #[tokio::test]
    async fn lifecycle_all_kinds() {
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
                store.add(&mut conn, &holder, &admin).await.expect("add admin"),
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

    #[tokio::test]
    async fn invalid_holder_id_rejected_all_kinds() {
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