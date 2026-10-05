//! Executor uniformity over `ConnectionTrait` (SC-5, D-05): the SAME
//! `SeaormStore` methods run on a bare autocommit `DatabaseConnection`,
//! on a pooled `DatabaseConnection`, and inside a caller-owned
//! `DatabaseTransaction` (commit and rollback). Four legs, both engines.
//!
//! Run: `cargo test -p rolify-seaorm --features postgres,suite --test executor`
//!      `cargo test -p rolify-seaorm --features mysql,suite --test executor`

#![cfg(all(feature = "suite", any(feature = "postgres", feature = "mysql")))]

mod support;

use rolify_core::kernel::RemovalTarget;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use sea_orm::{ConnectionTrait, TransactionTrait};

use crate::support::SuiteStore;
use rolify_seaorm::SeaormStore;

/// Four-leg matrix over any executor the store accepts. Asserts identical
/// semantics on each leg.
async fn run_executor_matrix() {
    prepare().await;
    reset().await;

    let admin = RoleName::from("admin");
    let admin_id = ResourceId::from(1_i64);

    // LEG 1: bare connection, autocommit.
    let mut store = suite_store();
    let mut bare = crate::support::connect().await;
    let role = store
        .find_or_create_by(&mut bare, &admin, ResourceRef::Global)
        .await
        .expect("leg1 find_or_create");
    assert!(
        store
            .add(&mut bare, &admin_id, &role)
            .await
            .expect("leg1 first add")
    );
    assert!(
        !store
            .add(&mut bare, &admin_id, &role)
            .await
            .expect("leg1 second add is idempotent-false")
    );

    // LEG 2: a *pooled* checkout is another DatabaseConnection (the pool
    // surface DatabaseConnection already is); same ops, same results.
    let mut pooled = crate::support::connect().await;
    let rows = store
        .where_(
            &mut pooled,
            &admin_id,
            &RoleQuery::with_role_and_filter(&admin, ResourceFilter::Global),
        )
        .await
        .expect("leg2 read");
    assert_eq!(rows.len(), 1, "pooled checkout reads the bare commit");

    // LEG 3: caller-owned transaction, commit. The SAME store code runs
    // on DatabaseTransaction because both entity connections share the
    // ConnectionTrait surface the store is generic over.
    let tx = bare.begin().await.expect("begin tx");
    let mut tx = tx;
    let mut tx_store = new_tx_store();
    let scoped = tx_store
        .find_or_create_by(
            &mut tx,
            &RoleName::from("moderator"),
            ResourceRef::Instance("Forum", &ResourceId::from(1_i64)),
        )
        .await
        .expect("leg3 find_or_create");
    tx_store
        .add(&mut tx, &admin_id, &scoped)
        .await
        .expect("leg3 add");
    let in_tx = tx_store
        .where_(
            &mut tx,
            &admin_id,
            &RoleQuery::with_role_and_filter(&RoleName::from("moderator"), ResourceFilter::Any),
        )
        .await
        .expect("leg3 in-tx read");
    assert_eq!(in_tx.len(), 1, "in-transaction read sees the staged rows");
    tx.commit().await.expect("commit");
    let after_commit = store
        .where_any(
            &mut pooled,
            &admin_id,
            &[RoleQuery {
                name: &RoleName::from("moderator"),
                filter: ResourceFilter::Any,
            }],
        )
        .await
        .expect("leg3 post-commit read");
    assert_eq!(after_commit.len(), 1, "committed rows visible outside");

    // LEG 4: caller-owned transaction rolled back: nothing leaks.
    let tx2 = bare.begin().await.expect("begin tx2");
    let mut tx2 = tx2;
    let mut tx2_store = new_tx_store();
    let ghost = tx2_store
        .find_or_create_by(&mut tx2, &RoleName::from("ghost"), ResourceRef::Global)
        .await
        .expect("leg4 find_or_create");
    tx2_store
        .add(&mut tx2, &admin_id, &ghost)
        .await
        .expect("leg4 add");
    tx2.rollback().await.expect("rollback");
    let after_rollback = store
        .where_any(
            &mut pooled,
            &admin_id,
            &[RoleQuery {
                name: &RoleName::from("ghost"),
                filter: ResourceFilter::Any,
            }],
        )
        .await
        .expect("leg4 post-rollback read");
    assert_eq!(after_rollback.len(), 0, "rollback leaves zero rows");

    // Remove through the same surface (public path) to keep the matrix
    // writable-symmetric.
    store
        .remove(
            &mut bare,
            &admin_id,
            &RoleName::from("moderator"),
            RemovalTarget::NameOnly,
            true,
        )
        .await
        .expect("remove");
    assert_eq!(
        store
            .where_any(
                &mut bare,
                &admin_id,
                &[RoleQuery {
                    name: &RoleName::from("moderator"),
                    filter: ResourceFilter::Any,
                }],
            )
            .await
            .expect("post-remove read")
            .len(),
        0
    );
}

use rolify_core::query::RoleQuery;

/// Schema + fixtures on the shared container (idempotent).
async fn prepare() {
    let conn = crate::support::connect().await;
    crate::support::setup_schema(&conn).await;
}

/// Remove role and link rows (fixtures untouched).
async fn reset() {
    let conn = crate::support::connect().await;
    conn.execute_unprepared("DELETE FROM users_roles")
        .await
        .expect("reset users_roles");
    conn.execute_unprepared("DELETE FROM roles")
        .await
        .expect("reset roles");
}

fn suite_store() -> SuiteStore {
    let config = <rolify_test::fixtures::DefaultUser as rolify_test::fixtures::UserClass>::config();
    crate::support::build_store(&config)
}

/// The transaction-typed store leg (same registrations; the executor
/// type parameter is the whole point of SC-5).
fn new_tx_store() -> SeaormStore<sea_orm::DatabaseTransaction> {
    let config = <rolify_test::fixtures::DefaultUser as rolify_test::fixtures::UserClass>::config();
    crate::support::build_store(&config)
}

#[cfg(feature = "postgres")]
mod pg_executor {
    #[tokio::test]
    async fn executor_matrix_postgres() {
        super::run_executor_matrix().await;
    }
}

#[cfg(all(feature = "mysql", not(feature = "postgres")))]
mod mysql_executor {
    #[tokio::test]
    async fn executor_matrix_mysql() {
        super::run_executor_matrix().await;
    }
}
