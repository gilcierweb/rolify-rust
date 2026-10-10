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

/// SQLite leg: no container, runs locally for all three kinds.
#[cfg(feature = "sqlite")]
mod sqlite {
    use rolify_core::config::HolderIdKind;
    use rolify_core::error::RolifyError;
    use rolify_core::kernel::RemovalTarget;
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName};
    use rolify_core::store::RoleStore;
    use rolify_sqlx::{SqlxStore, Error};
    use sqlx::SqliteConnection;

    use crate::support::sqlite_conn;
    use crate::{all_kinds, holder_id_for, invalid_holder_id_for, rejecting_kinds, config_for};

    /// Canonical SQLite DDL for the kind's join column type (D-08-04).
    fn schema_ddl(kind: HolderIdKind) -> &'static str {
        match kind {
            HolderIdKind::Integer => include_str!(
                "../migrations/sqlite/integer/0000000001_rolify_create_tables.sql"
            ),
            HolderIdKind::Uuid => include_str!(
                "../migrations/sqlite/uuid/0000000001_rolify_create_tables.sql"
            ),
            HolderIdKind::String => include_str!(
                "../migrations/sqlite/string/0000000001_rolify_create_tables.sql"
            ),
        }
    }

    fn store_for(kind: HolderIdKind) -> SqlxStore<sqlx::Sqlite> {
        SqlxStore::new(&config_for(kind))
    }

    #[sqlx::test]
    async fn lifecycle_all_kinds() {
        for kind in all_kinds() {
            let mut conn = sqlite_conn().await;
            sqlx::query(schema_ddl(kind))
                .execute(&mut conn)
                .await
                .expect("apply kind-specific DDL");

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

    #[sqlx::test]
    async fn invalid_holder_id_rejected_all_kinds() {
        for kind in rejecting_kinds() {
            let mut conn = sqlite_conn().await;
            sqlx::query(schema_ddl(kind))
                .execute(&mut conn)
                .await
                .expect("apply kind-specific DDL");

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
    use rolify_sqlx::{SqlxStore, Error};
    use sqlx::PgConnection;

    use crate::support::pg_conn;
    use crate::{all_kinds, holder_id_for, invalid_holder_id_for, rejecting_kinds, config_for};

    fn schema_ddl(kind: HolderIdKind) -> &'static str {
        match kind {
            HolderIdKind::Integer => include_str!(
                "../migrations/postgres/integer/0000000001_rolify_create_tables.sql"
            ),
            HolderIdKind::Uuid => include_str!(
                "../migrations/postgres/uuid/0000000001_rolify_create_tables.sql"
            ),
            HolderIdKind::String => include_str!(
                "../migrations/postgres/string/0000000001_rolify_create_tables.sql"
            ),
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
        sqlx::query(schema_ddl(kind))
            .execute(&mut conn)
            .await
            .expect("apply kind-specific DDL");
        conn
    }

    #[sqlx::test]
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

    #[sqlx::test]
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
    use rolify_sqlx::{SqlxStore, Error};
    use sqlx::MySqlConnection;

    use crate::support::mysql_conn;
    use crate::{all_kinds, holder_id_for, invalid_holder_id_for, rejecting_kinds, config_for};

    fn schema_ddl(kind: HolderIdKind) -> &'static str {
        match kind {
            HolderIdKind::Integer => include_str!(
                "../migrations/mysql/integer/0000000001_rolify_create_tables.sql"
            ),
            HolderIdKind::Uuid => include_str!(
                "../migrations/mysql/uuid/0000000001_rolify_create_tables.sql"
            ),
            HolderIdKind::String => include_str!(
                "../migrations/mysql/string/0000000001_rolify_create_tables.sql"
            ),
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
        sqlx::query(schema_ddl(kind))
            .execute(&mut conn)
            .await
            .expect("apply kind-specific DDL");
        conn
    }

    #[sqlx::test]
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

    #[sqlx::test]
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