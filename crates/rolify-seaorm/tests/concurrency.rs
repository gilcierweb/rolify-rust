//! Concurrency races (D-14 analog on SeaORM): two concurrent
//! `find_or_create_by` calls converge to exactly one role row; two
//! concurrent `add` calls yield exactly `(false, true)` (one link row).
//! Barrier rendezvous, three iterations, no sleeps; each racer talks to
//! the container over its OWN `DatabaseConnection` (mirrors the diesel
//! harness's two-thread shape). Postgres and `MySQL` legs.
//!
//! Run: `cargo test -p rolify-seaorm --features postgres,suite --test concurrency`
//!      `cargo test -p rolify-seaorm --features mysql,suite --test concurrency`

#![cfg(all(feature = "suite", any(feature = "postgres", feature = "mysql")))]

mod support;

use std::sync::Arc;
// Async-aware barrier (std Barrier::wait would park the current_thread runtime).

use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;

use crate::support::SuiteStore;

const ROLE_NAME: &str = "racecourier";

/// Ensure the shared container has the schema + fixture rows (idempotent
/// per process).
async fn prepare_schema() {
    let conn = crate::support::connect().await;
    crate::support::setup_schema(&conn).await;
}

fn new_store() -> SuiteStore {
    let config = <rolify_test::fixtures::DefaultUser as rolify_test::fixtures::UserClass>::config();
    crate::support::build_store(&config)
}

async fn reset_roles() {
    let conn = crate::support::connect().await;
    use sea_orm::ConnectionTrait;
    conn.execute_unprepared("DELETE FROM users_roles")
        .await
        .expect("reset users_roles");
    conn.execute_unprepared("DELETE FROM roles")
        .await
        .expect("reset roles");
}

async fn role_row_count() -> i64 {
    let conn = crate::support::connect().await;
    use sea_orm::ConnectionTrait;
    let row = conn
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT COUNT(*) AS count FROM roles".to_owned(),
            Vec::new(),
        ))
        .await
        .expect("count roles")
        .expect("COUNT(*) returns one row");
    row.try_get("", "count").expect("count column")
}

async fn link_row_count() -> i64 {
    let conn = crate::support::connect().await;
    use sea_orm::ConnectionTrait;
    let row = conn
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT COUNT(*) AS count FROM users_roles".to_owned(),
            Vec::new(),
        ))
        .await
        .expect("count links")
        .expect("COUNT(*) returns one row");
    row.try_get("", "count").expect("count column")
}

/// Owned scope payload so each spawned task builds its own `ResourceRef`
/// (borrows cannot cross `tokio::spawn` boundaries).
#[derive(Clone)]
enum ScopeShape {
    Global,
    Class(&'static str),
    Instance(&'static str, ResourceId),
}

impl ScopeShape {
    fn as_ref(&self) -> ResourceRef<'_> {
        match self {
            ScopeShape::Global => ResourceRef::Global,
            ScopeShape::Class(type_name) => ResourceRef::Class(type_name),
            ScopeShape::Instance(type_name, id) => ResourceRef::Instance(type_name, id),
        }
    }
}

/// Two concurrent `find_or_create_by` calls on one triple: barrier
/// rendezvous, both callers get the same row, exactly one row exists.
async fn find_or_create_race(shape: ScopeShape) {
    // Serialize against sibling tests in this binary: a shared
    // container schema means another case's reset must not interleave
    // (same posture as the suite backend's SuiteGuard).
    let _guard = crate::support::SuiteGuard::acquire();
    prepare_schema().await;
    for _iteration in 0..3 {
        reset_roles().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for _racer in 0..2 {
            let barrier = Arc::clone(&barrier);
            let mut store = new_store();
            let shape = shape.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let mut conn = crate::support::connect().await;
                store
                    .find_or_create_by(&mut conn, &RoleName::from(ROLE_NAME), shape.as_ref())
                    .await
                    .map(|record| record.name.as_str() == ROLE_NAME)
                    .unwrap_or(false)
            }));
        }
        let mut outcomes = Vec::with_capacity(2);
        for handle in handles {
            outcomes.push(handle.await.expect("join"));
        }
        assert!(
            outcomes.iter().all(|hit| *hit),
            "both racers observe the same role row"
        );
        assert_eq!(
            role_row_count().await,
            1,
            "the unique triple converges the race to exactly one row"
        );
    }
}

/// Two concurrent `add` calls on one holder+role: exactly one wins
/// (`true`), the loser sees `false` (pair-unique guard), one link row.
async fn add_race(shape: ScopeShape) {
    let _guard = crate::support::SuiteGuard::acquire();
    prepare_schema().await;
    for _iteration in 0..3 {
        reset_roles().await;
        let holder = ResourceId::from(1_i64);
        let role = {
            let mut store = new_store();
            let mut conn = crate::support::connect().await;
            store
                .find_or_create_by(&mut conn, &RoleName::from(ROLE_NAME), shape.as_ref())
                .await
                .expect("provision role")
        };

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for _racer in 0..2 {
            let barrier = Arc::clone(&barrier);
            let mut store = new_store();
            let role = role.clone();
            let holder = holder.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let mut conn = crate::support::connect().await;
                store
                    .add(&mut conn, &holder, &role)
                    .await
                    .expect("add thrives")
            }));
        }
        let mut outcomes: Vec<bool> = Vec::with_capacity(2);
        for handle in handles {
            outcomes.push(handle.await.expect("join"));
        }
        outcomes.sort_unstable();
        assert_eq!(
            outcomes,
            vec![false, true],
            "exactly one add wins the pair-unique race"
        );
        assert_eq!(link_row_count().await, 1, "exactly one link row");
    }
}

#[cfg(feature = "postgres")]
mod pg_races {
    use super::*;

    #[tokio::test]
    async fn find_or_create_race_global_postgres() {
        find_or_create_race(ScopeShape::Global).await;
    }

    #[tokio::test]
    async fn find_or_create_race_class_postgres() {
        find_or_create_race(ScopeShape::Class("Forum")).await;
    }

    #[tokio::test]
    async fn find_or_create_race_instance_postgres() {
        find_or_create_race(ScopeShape::Instance("Forum", ResourceId::from(1_i64))).await;
    }

    #[tokio::test]
    async fn add_race_global_postgres() {
        add_race(ScopeShape::Global).await;
    }

    #[tokio::test]
    async fn add_race_class_postgres() {
        add_race(ScopeShape::Class("Forum")).await;
    }

    #[tokio::test]
    async fn add_race_instance_postgres() {
        add_race(ScopeShape::Instance("Forum", ResourceId::from(1_i64))).await;
    }
}

#[cfg(all(feature = "mysql", not(feature = "postgres")))]
mod mysql_races {
    use super::*;

    #[tokio::test]
    async fn find_or_create_race_global_mysql() {
        find_or_create_race(ScopeShape::Global).await;
    }

    #[tokio::test]
    async fn find_or_create_race_class_mysql() {
        find_or_create_race(ScopeShape::Class("Forum")).await;
    }

    #[tokio::test]
    async fn find_or_create_race_instance_mysql() {
        find_or_create_race(ScopeShape::Instance("Forum", ResourceId::from(1_i64))).await;
    }

    #[tokio::test]
    async fn add_race_global_mysql() {
        add_race(ScopeShape::Global).await;
    }

    #[tokio::test]
    async fn add_race_class_mysql() {
        add_race(ScopeShape::Class("Forum")).await;
    }

    #[tokio::test]
    async fn add_race_instance_mysql() {
        add_race(ScopeShape::Instance("Forum", ResourceId::from(1_i64))).await;
    }
}
