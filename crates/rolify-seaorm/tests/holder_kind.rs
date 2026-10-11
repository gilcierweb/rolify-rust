//! Holder id kind matrix for the `SeaORM` adapter (Phase 08 D-08-04/D-08-05).
//!
//! Each compiled engine runs the grant/check/revoke lifecycle against a
//! schema whose `users_roles.user_id` column carries the kind's physical
//! type (D-08-04). The kind-specific DDL is the canonical migration tree
//! for that engine, applied via the `SeaORM` migrator, so this suite doubles
//! as a local confirmation that the trees apply. The finder leg registers
//! a per-kind holder table and exercises `holders_where` / `all_holders`
//! / the `roles_matching` holder filter per kind, so the kind-aware
//! comparison and projection SQL (08 review CR-01/WR-01) is covered on
//! the gem's finder paths, not just the bind lane.
//!
//! Engine features are additive for the lib but the test suite runs one
//! engine at a time via `--features`. The `SQLite` leg is out of scope for
//! `SeaORM` (no `SeaORM` `SQLite` async support in this crate). Postgres and
//! `MySQL` legs use testcontainers (`MySQL` is CI-only where libmysqlclient
//! is present).

#[cfg(any(feature = "postgres", feature = "mysql"))]
mod support;

use rolify_core::config::{HolderIdKind, RolifyConfig};
use rolify_seaorm::migration::m20261003_000001_create_roles::{MigrationDirection, get_statements};
use sea_orm::{ConnectionTrait, DbBackend};

/// The registered holder table for the finder matrix (the finders read
/// a consumer holder table through `for_holder_table`).
const HOLDER_TABLE: &str = "kind_holders";

/// Canonical holder id string per kind (matches `data.rs` shapes).
fn holder_id_for(kind: HolderIdKind) -> &'static str {
    match kind {
        HolderIdKind::Integer => "42",
        HolderIdKind::Uuid => "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8",
        HolderIdKind::String => "user-1",
    }
}

/// A second, ungranted holder id per kind: `all_holders` lists it while
/// `holders_where` and the holder-filtered catalog exclude it, pinning
/// that the finder join and filter isolate the requested id per kind.
fn bystander_id_for(kind: HolderIdKind) -> &'static str {
    match kind {
        HolderIdKind::Integer => "43",
        HolderIdKind::Uuid => "b4b5b6b7-c8c9-d0e1-f2a3-b4c5d6e7f8a9",
        HolderIdKind::String => "user-2",
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
    let stmts =
        get_statements(backend, kind, MigrationDirection::Up).expect("get migration statements");
    for stmt in stmts {
        conn.execute_unprepared(stmt)
            .await
            .expect("apply migration statement");
    }
}

/// Drop all tables for the given backend.
async fn drop_tables(conn: &sea_orm::DatabaseConnection, backend: DbBackend) {
    let stmts = get_statements(backend, HolderIdKind::Integer, MigrationDirection::Down)
        .expect("get down migration statements");
    for stmt in stmts {
        let _ = conn
            .execute_raw(sea_orm::Statement::from_sql_and_values(
                backend,
                *stmt,
                vec![],
            ))
            .await; // ignore errors
    }
}

/// `Postgres` leg: testcontainers (runs where Docker is available).
#[cfg(feature = "postgres")]
mod postgres {
    use rolify_core::catalog::RoleCatalogQuery;
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::RoleStore;
    use rolify_seaorm::{Error, SeaormStore};
    use sea_orm::{ConnectionTrait, DatabaseConnection};

    use crate::support::pg::connect as pg_conn;
    use crate::{
        HOLDER_TABLE, all_kinds, apply_migration, bystander_id_for, config_for, drop_tables,
        holder_id_for, invalid_holder_id_for, rejecting_kinds,
    };

    fn store_for(kind: HolderIdKind) -> SeaormStore<DatabaseConnection> {
        SeaormStore::new(&config_for(kind))
    }

    async fn fresh_conn(kind: HolderIdKind) -> DatabaseConnection {
        let conn = pg_conn().await;
        drop_tables(&conn, sea_orm::DbBackend::Postgres).await;
        apply_migration(&conn, sea_orm::DbBackend::Postgres, kind).await;
        conn
    }

    /// Statements creating and seeding the per-kind holder table: the
    /// id column carries the kind's physical type (D-08-04), and the
    /// two rows carry the granted holder plus the ungranted bystander.
    /// Integers stay bare; the uuid and string kinds quote their
    /// literals (`Postgres` casts the quoted uuid to the native type).
    fn holder_table_setup(kind: HolderIdKind) -> Vec<String> {
        let id_type = match kind {
            HolderIdKind::Integer => "BIGINT",
            HolderIdKind::Uuid => "UUID",
            HolderIdKind::String => "VARCHAR(191)",
        };
        let holder_literal = match kind {
            HolderIdKind::Integer => holder_id_for(kind).to_owned(),
            _ => format!("'{}'", holder_id_for(kind)),
        };
        let bystander_literal = match kind {
            HolderIdKind::Integer => bystander_id_for(kind).to_owned(),
            _ => format!("'{}'", bystander_id_for(kind)),
        };
        vec![
            format!(
                "CREATE TABLE {HOLDER_TABLE} (id {id_type} PRIMARY KEY, \
                 rolify_type VARCHAR(191) NOT NULL DEFAULT '', \
                 name VARCHAR(255) NOT NULL DEFAULT '')"
            ),
            format!(
                "INSERT INTO {HOLDER_TABLE} (id, rolify_type, name) \
                 VALUES ({holder_literal}, 'User', 'kind-a'), \
                 ({bystander_literal}, 'User', 'kind-b')"
            ),
        ]
    }

    #[tokio::test]
    async fn lifecycle_all_kinds() {
        // The fresh_conn drop+apply sequence races against the sibling test
        // on this engine when both target the SHARED container database;
        // the suite guard serializes the two for the whole test body
        // (diesel holder_kind.rs precedent, commit 52be22e).
        let _suite_guard = crate::support::SuiteGuard::acquire();
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

    #[tokio::test]
    async fn invalid_holder_id_rejected_all_kinds() {
        // Same shared-container serialization as the lifecycle leg.
        let _suite_guard = crate::support::SuiteGuard::acquire();
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

    /// Finder paths per kind (08 review CR-01/WR-01): `holders_where`,
    /// `all_holders`, and the `roles_matching` holder filter run
    /// against a registered holder table whose id column carries the
    /// kind's physical type, so the kind-aware join key, the holder
    /// filter bind, and the canonical-text projection are exercised
    /// per kind on the gem's finder surface, not just the bind lane.
    #[tokio::test]
    async fn finder_paths_all_kinds() {
        // Same shared-container serialization as the lifecycle legs:
        // this test re-applies the kind schema per iteration.
        let _suite_guard = crate::support::SuiteGuard::acquire();
        for kind in all_kinds() {
            let mut conn = fresh_conn(kind).await;
            let mut store = store_for(kind).for_holder_table(HOLDER_TABLE);
            let holder = ResourceId::from(holder_id_for(kind));
            let bystander = ResourceId::from(bystander_id_for(kind));

            // The per-kind holder table must not survive a previous
            // iteration's column type (BIGINT -> UUID and back).
            conn.execute_unprepared(&format!("DROP TABLE IF EXISTS {HOLDER_TABLE}"))
                .await
                .expect("drop prior holder table");
            for statement in holder_table_setup(kind) {
                conn.execute_unprepared(&statement)
                    .await
                    .expect("apply holder table statement");
            }

            // Class-scoped role: the catalog read filters
            // resource_type != '', so the finder matrix grants a
            // non-global row.
            let moderator = RoleName::from("moderator");
            let class_row = store
                .find_or_create_by(&mut conn, &moderator, ResourceRef::Class("Forum"))
                .await
                .expect("find_or_create_by class moderator");
            assert!(
                store
                    .add(&mut conn, &holder, &class_row)
                    .await
                    .expect("add link"),
                "grant inserts the link for {kind:?}"
            );

            // holders_where: only the role-holding holder matches the
            // role query (the bystander never enters the result).
            let query = RoleQuery {
                name: &moderator,
                filter: ResourceFilter::Class("Forum"),
            };
            let found = store
                .holders_where(&mut conn, &["User"], &query, false)
                .await
                .expect("holders_where per kind");
            assert_eq!(found, vec![holder.clone()], "holders_where for {kind:?}");

            // all_holders: the FULL per-type universe, granted or not
            // (gem `finders.rb:13` `all_except` feed).
            let universe = store
                .all_holders(&mut conn, &["User"])
                .await
                .expect("all_holders per kind");
            assert_eq!(universe.len(), 2, "all_holders universe for {kind:?}");
            assert!(
                universe.contains(&holder) && universe.contains(&bystander),
                "all_holders must list both holders as canonical ids for {kind:?}"
            );

            // roles_matching holder filter: the granted holder's class
            // row; the bystander's read stays empty, pinning the
            // typed filter bind per kind.
            let catalog = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&holder);
            let rows = store
                .roles_matching(&mut conn, &catalog)
                .await
                .expect("roles_matching holder filter per kind");
            assert_eq!(
                rows,
                vec![RoleRecord::for_class("moderator", "Forum")],
                "roles_matching holder rows for {kind:?}"
            );
            let bystander_catalog = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&bystander);
            let rows = store
                .roles_matching(&mut conn, &bystander_catalog)
                .await
                .expect("roles_matching bystander filter per kind");
            assert!(
                rows.is_empty(),
                "the ungranted bystander must read no rows for {kind:?}"
            );
        }
    }
}

/// `MySQL` leg: testcontainers (CI-only where libmysqlclient is installed).
#[cfg(feature = "mysql")]
mod mysql {
    use rolify_core::catalog::RoleCatalogQuery;
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::RoleStore;
    use rolify_seaorm::{Error, SeaormStore};
    use sea_orm::{ConnectionTrait, DatabaseConnection};

    use crate::support::mysql::connect as mysql_conn;
    use crate::{
        HOLDER_TABLE, all_kinds, apply_migration, bystander_id_for, config_for, drop_tables,
        holder_id_for, invalid_holder_id_for, rejecting_kinds,
    };

    fn store_for(kind: HolderIdKind) -> SeaormStore<DatabaseConnection> {
        SeaormStore::new(&config_for(kind))
    }

    async fn fresh_conn(kind: HolderIdKind) -> DatabaseConnection {
        let conn = mysql_conn().await;
        drop_tables(&conn, sea_orm::DbBackend::MySql).await;
        apply_migration(&conn, sea_orm::DbBackend::MySql, kind).await;
        conn
    }

    /// Statements creating and seeding the per-kind holder table: the
    /// id column carries the kind's physical type (D-08-04), and the
    /// two rows carry the granted holder plus the ungranted bystander.
    /// Integers stay bare; the uuid kind seeds through `UUID_TO_BIN`
    /// (the documented `BINARY(16)` column, `MySQL` 8+); the string kind
    /// quotes its literal.
    fn holder_table_setup(kind: HolderIdKind) -> Vec<String> {
        let id_type = match kind {
            HolderIdKind::Integer => "BIGINT",
            HolderIdKind::Uuid => "BINARY(16)",
            HolderIdKind::String => "VARCHAR(191)",
        };
        let id_literal = |id: &str| match kind {
            HolderIdKind::Integer => id.to_owned(),
            HolderIdKind::Uuid => format!("UUID_TO_BIN('{id}')"),
            HolderIdKind::String => format!("'{id}'"),
        };
        vec![
            format!(
                "CREATE TABLE {HOLDER_TABLE} (id {id_type} PRIMARY KEY, \
                 rolify_type VARCHAR(191) NOT NULL DEFAULT '', \
                 name VARCHAR(255) NOT NULL DEFAULT '')"
            ),
            format!(
                "INSERT INTO {HOLDER_TABLE} (id, rolify_type, name) \
                 VALUES ({}, 'User', 'kind-a'), \
                 ({}, 'User', 'kind-b')",
                id_literal(holder_id_for(kind)),
                id_literal(bystander_id_for(kind)),
            ),
        ]
    }

    #[tokio::test]
    async fn lifecycle_all_kinds() {
        // The fresh_conn drop+apply sequence races against the sibling test
        // on this engine when both target the SHARED container database;
        // the suite guard serializes the two for the whole test body
        // (diesel holder_kind.rs precedent, commit 52be22e).
        let _suite_guard = crate::support::SuiteGuard::acquire();
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

    #[tokio::test]
    async fn invalid_holder_id_rejected_all_kinds() {
        // Same shared-container serialization as the lifecycle leg.
        let _suite_guard = crate::support::SuiteGuard::acquire();
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

    /// Finder paths per kind (08 review CR-01/WR-01), `MySQL` leg: the
    /// uuid kind is the WR-01 proof on this engine - the holder
    /// projection must return the canonical hyphenated uuid string
    /// (`BIN_TO_UUID` over the BINARY(16) column), never the raw bytes
    /// a `CAST(BINARY(16) AS CHAR)` would leak.
    #[tokio::test]
    async fn finder_paths_all_kinds() {
        // Same shared-container serialization as the lifecycle legs:
        // this test re-applies the kind schema per iteration.
        let _suite_guard = crate::support::SuiteGuard::acquire();
        for kind in all_kinds() {
            let mut conn = fresh_conn(kind).await;
            let mut store = store_for(kind).for_holder_table(HOLDER_TABLE);
            let holder = ResourceId::from(holder_id_for(kind));
            let bystander = ResourceId::from(bystander_id_for(kind));

            // The per-kind holder table must not survive a previous
            // iteration's column type (BIGINT -> BINARY(16) and back).
            conn.execute_unprepared(&format!("DROP TABLE IF EXISTS {HOLDER_TABLE}"))
                .await
                .expect("drop prior holder table");
            for statement in holder_table_setup(kind) {
                conn.execute_unprepared(&statement)
                    .await
                    .expect("apply holder table statement");
            }

            // Class-scoped role: the catalog read filters
            // resource_type != '', so the finder matrix grants a
            // non-global row.
            let moderator = RoleName::from("moderator");
            let class_row = store
                .find_or_create_by(&mut conn, &moderator, ResourceRef::Class("Forum"))
                .await
                .expect("find_or_create_by class moderator");
            assert!(
                store
                    .add(&mut conn, &holder, &class_row)
                    .await
                    .expect("add link"),
                "grant inserts the link for {kind:?}"
            );

            // holders_where: only the role-holding holder matches the
            // role query, as a canonical id string.
            let query = RoleQuery {
                name: &moderator,
                filter: ResourceFilter::Class("Forum"),
            };
            let found = store
                .holders_where(&mut conn, &["User"], &query, false)
                .await
                .expect("holders_where per kind");
            assert_eq!(found, vec![holder.clone()], "holders_where for {kind:?}");

            // all_holders: the FULL per-type universe, granted or not,
            // every id canonical (the uuid leg is the WR-01 pin).
            let universe = store
                .all_holders(&mut conn, &["User"])
                .await
                .expect("all_holders per kind");
            assert_eq!(universe.len(), 2, "all_holders universe for {kind:?}");
            assert!(
                universe.contains(&holder) && universe.contains(&bystander),
                "all_holders must list both holders as canonical ids for {kind:?}"
            );

            // roles_matching holder filter: the granted holder's class
            // row; the bystander's read stays empty, pinning the typed
            // filter bind per kind.
            let catalog = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&holder);
            let rows = store
                .roles_matching(&mut conn, &catalog)
                .await
                .expect("roles_matching holder filter per kind");
            assert_eq!(
                rows,
                vec![RoleRecord::for_class("moderator", "Forum")],
                "roles_matching holder rows for {kind:?}"
            );
            let bystander_catalog = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&bystander);
            let rows = store
                .roles_matching(&mut conn, &bystander_catalog)
                .await
                .expect("roles_matching bystander filter per kind");
            assert!(
                rows.is_empty(),
                "the ungranted bystander must read no rows for {kind:?}"
            );
        }
    }
}
