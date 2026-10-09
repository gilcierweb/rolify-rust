//! Concurrency race tests, the async `SQLx` port of the diesel race
//! design (TEST-05 + SC-2): two parallel grants of the SAME role
//! name/triple for the SAME holder must resolve to EXACTLY one role
//! row and exactly one link row, deterministically.
//!
//! Design (ported from the repaired rolify-diesel race test, 04-10;
//! never from the defective pre-repair history recorded as the Phase 3
//! gap 2 disclosure):
//!
//! - Timing-free rendezvous: each racing tokio task acquires its OWN
//!   pool checkout first, then waits on a `tokio::sync::Barrier`
//!   placed immediately before the contested grant, so both tasks hit
//!   the database with the window maximally contested. No sleeps, no
//!   timing assumptions anywhere.
//! - UNIQUE as arbiter: the roles-triple and join-pair UNIQUE
//!   constraints decide the winner; the store's portable catch
//!   (Postgres 23505 / `MySQL` 23000 with native 1062, plus `SQLite` 2067
//!   for completeness) absorbs the loser. The loser's
//!   `find_or_create_by` violation resolves through the re-SELECT
//!   path (both tasks observe the SAME record), and the loser's `add`
//!   pair violation resolves through catch-and-ignore (`Ok(false)`).
//! - `InnoDB` deadlock victims are a retryable race outcome, not a
//!   store bug: when both grants hit the engine in the same instant
//!   `MySQL` can pick one task as the victim (error 1213, SQLSTATE
//!   40001) and documents client-side retry as the contract. The
//!   racing grant retries the whole pass within a bounded budget; the
//!   victim re-enters through the SELECT-first leg and the UNIQUE
//!   arbiter keeps the one-row/one-link guarantees intact across
//!   retries, so the assertions never change.
//! - The contested grant is the full `add_role` shape through the
//!   public store members: `find_or_create_by` followed by `add`.
//! - Three iterations per scope kind (global, class, instance) smoke
//!   out flakiness, matching the diesel template's iteration count.
//!
//! One test function per engine runs the whole matrix sequentially:
//! every iteration resets the shared role tables, so parallel cases
//! inside one binary would wipe each other's rows. A single case per
//! engine eliminates that interference by construction (the async
//! counterpart of the diesel suite's process-wide `SuiteGuard`).
//!
//! `SQLite` is excluded per D-11 (the locked non-gate posture): the
//! single-writer engine makes a parallel-grant race meaningless, the
//! same exclusion the diesel race tests carry.
//!
//! Runs:
//! - `cargo test -p rolify-sqlx --features postgres,suite --test concurrency`
//! - `cargo test -p rolify-sqlx --features mysql,suite --test concurrency`

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod support;

use std::sync::Arc;

use rolify_core::config::RolifyConfig;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::RoleStore;
use rolify_sqlx::SqlxStore;

/// Race iterations per scope kind (the diesel template's count).
const RACE_ITERATIONS: usize = 3;

/// The contested scope kinds: one per scope level, mirroring the
/// diesel race matrix.
#[derive(Clone)]
enum ScopeKind {
    /// A global role (empty scope columns).
    Global,
    /// A class-scoped role (`resource_type` set, empty `resource_id`).
    Class(String),
    /// An instance-scoped role (both scope columns set).
    Instance(String, ResourceId),
}

impl ScopeKind {
    /// The grant target the racing stores receive.
    fn to_resource_ref(&self) -> ResourceRef<'_> {
        match self {
            ScopeKind::Global => ResourceRef::Global,
            ScopeKind::Class(type_name) => ResourceRef::Class(type_name),
            ScopeKind::Instance(type_name, resource_id) => {
                ResourceRef::Instance(type_name, resource_id)
            }
        }
    }

    /// The storage scope pair the verification probes bind.
    fn sql_values(&self) -> (&str, &str) {
        match self {
            ScopeKind::Global => ("", ""),
            ScopeKind::Class(type_name) => (type_name.as_str(), ""),
            ScopeKind::Instance(type_name, resource_id) => {
                (type_name.as_str(), resource_id.as_str())
            }
        }
    }
}

/// What one racing task brings back from the contested grant.
struct GrantOutcome {
    record: RoleRecord,
    added: bool,
}

#[cfg(feature = "postgres")]
mod postgres_race {
    use super::*;
    use sqlx::Row as _;

    use crate::support::{
        apply_migrations_pg, insert_holder_pg, pg_conn, pg_pool, reset_roles_pg, setup_fixtures_pg,
    };

    /// Count the role rows matching the exact triple on a verification
    /// connection (every value is a runtime bind).
    async fn count_role_rows(
        conn: &mut sqlx::PgConnection,
        role_name: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> i64 {
        sqlx::query(
            "SELECT COUNT(*) FROM roles WHERE name = $1 AND resource_type = $2 AND resource_id = $3",
        )
        .bind(role_name)
        .bind(resource_type)
        .bind(resource_id)
        .fetch_one(conn)
        .await
        .expect("count the raced role rows on Postgres")
        .get::<i64, _>(0)
    }

    /// Resolve the raced role row's generated id on a verification
    /// connection.
    async fn resolve_role_row_id(
        conn: &mut sqlx::PgConnection,
        role_name: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> i64 {
        sqlx::query(
            "SELECT id FROM roles WHERE name = $1 AND resource_type = $2 AND resource_id = $3",
        )
        .bind(role_name)
        .bind(resource_type)
        .bind(resource_id)
        .fetch_one(conn)
        .await
        .expect("resolve the raced role row id on Postgres")
        .get::<i64, _>(0)
    }

    /// Count the holder's link rows for the raced role on a
    /// verification connection.
    async fn count_link_rows(
        conn: &mut sqlx::PgConnection,
        holder: &str,
        raced_role_id: i64,
    ) -> i64 {
        sqlx::query("SELECT COUNT(*) FROM users_roles WHERE user_id = $1 AND role_id = $2")
            .bind(holder)
            .bind(raced_role_id)
            .fetch_one(conn)
            .await
            .expect("count the raced link rows on Postgres")
            .get::<i64, _>(0)
    }

    /// One racing task: acquire a pool checkout, rendezvous on the
    /// barrier, then run the contested grant (the `add_role` shape:
    /// `find_or_create_by` followed by `add`) through the public
    /// store members.
    ///
    /// The checkout is acquired BEFORE the barrier wait so both tasks
    /// arrive at the contested window with a live connection in hand;
    /// the barrier sits immediately before the grant. The store calls
    /// take reborrows of the inner connection (sqlx 0.9 deleted the
    /// wrapper Executor impls, RESEARCH Pitfall 4).
    async fn contested_grant(
        pool: sqlx::PgPool,
        barrier: Arc<tokio::sync::Barrier>,
        role_name: String,
        scope: ScopeKind,
        holder: ResourceId,
    ) -> GrantOutcome {
        let mut checkout = pool
            .acquire()
            .await
            .expect("pool checkout for the racing task on Postgres");
        let mut store = SqlxStore::new(&RolifyConfig::default());
        barrier.wait().await;
        let record = store
            .find_or_create_by(
                &mut *checkout,
                &RoleName::from(role_name.as_str()),
                scope.to_resource_ref(),
            )
            .await
            .expect("concurrent find_or_create_by on Postgres");
        let added = store
            .add(&mut *checkout, &holder, &record)
            .await
            .expect("concurrent add on Postgres");
        GrantOutcome { record, added }
    }

    /// The grant race matrix on the Postgres container: every scope
    /// kind, three iterations, UNIQUE-as-arbiter assertions.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_grant_race_yields_exactly_one_row_pair_postgres() {
        let pool = pg_pool().await;
        apply_migrations_pg(&pool).await;
        setup_fixtures_pg(&pool).await;

        let races = [
            ("concurrent_global_admin", ScopeKind::Global),
            (
                "concurrent_class_manager",
                ScopeKind::Class("Forum".to_owned()),
            ),
            (
                "concurrent_instance_moderator",
                ScopeKind::Instance("Forum".to_owned(), ResourceId::from("77")),
            ),
        ];

        for (role_name, scope) in races {
            for iteration in 0..RACE_ITERATIONS {
                // Fresh slate per iteration: the reset drops every role
                // and link row, so each race starts empty.
                reset_roles_pg(&pool).await;

                let mut seed_conn = pg_conn().await;
                let holder = insert_holder_pg(
                    &mut seed_conn,
                    "users",
                    "User",
                    &format!("race_{role_name}_holder_{iteration}"),
                )
                .await;

                let barrier = Arc::new(tokio::sync::Barrier::new(2));
                let first_handle = tokio::spawn(contested_grant(
                    pool.clone(),
                    Arc::clone(&barrier),
                    role_name.to_owned(),
                    scope.clone(),
                    holder.clone(),
                ));
                let second_handle = tokio::spawn(contested_grant(
                    pool.clone(),
                    Arc::clone(&barrier),
                    role_name.to_owned(),
                    scope.clone(),
                    holder.clone(),
                ));

                let first = first_handle
                    .await
                    .expect("the first racing task must not panic");
                let second = second_handle
                    .await
                    .expect("the second racing task must not panic");

                assert_eq!(
                    first.record, second.record,
                    "{role_name} iteration {iteration}: both tasks resolved the same record (the re-SELECT catch)"
                );
                let mut added_results = [first.added, second.added];
                added_results.sort_unstable();
                assert_eq!(
                    added_results,
                    [false, true],
                    "{role_name} iteration {iteration}: exactly one grant inserted the link, the loser's pair violation was absorbed"
                );

                // Deterministic probes on a third connection: exactly
                // one role row and exactly one link row survive.
                let mut verify_conn = pg_conn().await;
                let (resource_type, resource_id) = scope.sql_values();
                let role_rows =
                    count_role_rows(&mut verify_conn, role_name, resource_type, resource_id).await;
                assert_eq!(
                    role_rows, 1,
                    "{role_name} iteration {iteration}: exactly one role row for the triple"
                );
                let raced_role_id =
                    resolve_role_row_id(&mut verify_conn, role_name, resource_type, resource_id)
                        .await;
                let link_rows =
                    count_link_rows(&mut verify_conn, holder.as_str(), raced_role_id).await;
                assert_eq!(
                    link_rows, 1,
                    "{role_name} iteration {iteration}: exactly one link row for the holder and the raced role"
                );
            }
        }
    }
}

#[cfg(feature = "mysql")]
mod mysql_race {
    use super::*;
    use sqlx::Row as _;

    use crate::support::{
        apply_migrations_mysql, insert_holder_mysql, mysql_conn, mysql_pool, reset_roles_mysql,
        setup_fixtures_mysql,
    };

    /// Count the role rows matching the exact triple on a verification
    /// connection (every value is a runtime bind).
    async fn count_role_rows(
        conn: &mut sqlx::MySqlConnection,
        role_name: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> i64 {
        sqlx::query(
            "SELECT COUNT(*) FROM roles WHERE name = ? AND resource_type = ? AND resource_id = ?",
        )
        .bind(role_name)
        .bind(resource_type)
        .bind(resource_id)
        .fetch_one(conn)
        .await
        .expect("count the raced role rows on MySQL")
        .get::<i64, _>(0)
    }

    /// Resolve the raced role row's generated id on a verification
    /// connection.
    async fn resolve_role_row_id(
        conn: &mut sqlx::MySqlConnection,
        role_name: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> i64 {
        sqlx::query("SELECT id FROM roles WHERE name = ? AND resource_type = ? AND resource_id = ?")
            .bind(role_name)
            .bind(resource_type)
            .bind(resource_id)
            .fetch_one(conn)
            .await
            .expect("resolve the raced role row id on MySQL")
            .get::<i64, _>(0)
    }

    /// Count the holder's link rows for the raced role on a
    /// verification connection.
    async fn count_link_rows(
        conn: &mut sqlx::MySqlConnection,
        holder: &str,
        raced_role_id: i64,
    ) -> i64 {
        sqlx::query("SELECT COUNT(*) FROM users_roles WHERE user_id = ? AND role_id = ?")
            .bind(holder)
            .bind(raced_role_id)
            .fetch_one(conn)
            .await
            .expect("count the raced link rows on MySQL")
            .get::<i64, _>(0)
    }

    /// Bound for the victim-retry loop in [`contested_grant`]: one
    /// deadlock is a normal race outcome, two in the same iteration
    /// would already be extraordinary, five exhausts any plausible
    /// interleaving.
    const DEADLOCK_RETRIES: usize = 5;

    /// Which grant step an error escaped from (kept for panic context).
    #[derive(Debug)]
    enum GrantStep {
        FindOrCreate,
        Add,
    }

    /// One full contested grant pass through the public store members
    /// (`find_or_create_by` then `add`), with the step preserved so the
    /// caller can classify the failure.
    async fn run_grant(
        conn: &mut sqlx::MySqlConnection,
        store: &mut SqlxStore<sqlx::MySql>,
        role_name: &str,
        scope: &ScopeKind,
        holder: &ResourceId,
    ) -> Result<GrantOutcome, (GrantStep, rolify_sqlx::Error)> {
        let record = store
            .find_or_create_by(conn, &RoleName::from(role_name), scope.to_resource_ref())
            .await
            .map_err(|error| (GrantStep::FindOrCreate, error))?;
        let added = store
            .add(conn, holder, &record)
            .await
            .map_err(|error| (GrantStep::Add, error))?;
        Ok(GrantOutcome { record, added })
    }

    /// Match the `InnoDB` deadlock victim signal: the native error 1213
    /// surfaces as SQLSTATE 40001 through `DatabaseError::code()`.
    fn is_deadlock_victim(error: &rolify_sqlx::Error) -> bool {
        match error {
            rolify_sqlx::Error::Sqlx(sqlx::Error::Database(database_error)) => {
                database_error.code().as_deref() == Some("40001")
            }
            _ => false,
        }
    }

    /// One racing task: acquire a pool checkout, rendezvous on the
    /// barrier, then run the contested grant (the `add_role` shape:
    /// `find_or_create_by` followed by `add`) through the public
    /// store members.
    ///
    /// The checkout is acquired BEFORE the barrier wait so both tasks
    /// arrive at the contested window with a live connection in hand;
    /// the barrier sits immediately before the grant. The store calls
    /// take reborrows of the inner connection (sqlx 0.9 deleted the
    /// wrapper Executor impls, RESEARCH Pitfall 4).
    ///
    /// A deadlock victim (1213) retries the whole pass: the barrier
    /// rendezvous has already happened, the retry needs no new
    /// synchronization, and the UNIQUE arbiter keeps the outcome
    /// assertions valid no matter which pass wins.
    async fn contested_grant(
        pool: sqlx::MySqlPool,
        barrier: Arc<tokio::sync::Barrier>,
        role_name: String,
        scope: ScopeKind,
        holder: ResourceId,
    ) -> GrantOutcome {
        let mut checkout = pool
            .acquire()
            .await
            .expect("pool checkout for the racing task on MySQL");
        let mut store = SqlxStore::new(&RolifyConfig::default());
        barrier.wait().await;
        let mut retries_left = DEADLOCK_RETRIES;
        loop {
            match run_grant(&mut checkout, &mut store, &role_name, &scope, &holder).await {
                Ok(outcome) => return outcome,
                Err((_step, error)) if is_deadlock_victim(&error) => {
                    retries_left -= 1;
                    assert!(
                        retries_left > 0,
                        "the contested grant stayed deadlocked after {DEADLOCK_RETRIES} retries"
                    );
                }
                Err((step, error)) => {
                    panic!("contested grant on MySQL, {step:?} step: {error}");
                }
            }
        }
    }

    /// The grant race matrix on the `MySQL` container: every scope kind,
    /// three iterations, UNIQUE-as-arbiter assertions.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_grant_race_yields_exactly_one_row_pair_mysql() {
        let pool = mysql_pool().await;
        apply_migrations_mysql(&pool).await;
        setup_fixtures_mysql(&pool).await;

        let races = [
            ("concurrent_global_admin", ScopeKind::Global),
            (
                "concurrent_class_manager",
                ScopeKind::Class("Forum".to_owned()),
            ),
            (
                "concurrent_instance_moderator",
                ScopeKind::Instance("Forum".to_owned(), ResourceId::from("77")),
            ),
        ];

        for (role_name, scope) in races {
            for iteration in 0..RACE_ITERATIONS {
                // Fresh slate per iteration: the FK-safe DELETE pair
                // drops every link and role row (child links before
                // parent rows, MySQL error 1701 forbids TRUNCATE on
                // the referenced side).
                reset_roles_mysql(&pool).await;

                let mut seed_conn = mysql_conn().await;
                let holder = insert_holder_mysql(
                    &mut seed_conn,
                    "users",
                    "User",
                    &format!("race_{role_name}_holder_{iteration}"),
                )
                .await;

                let barrier = Arc::new(tokio::sync::Barrier::new(2));
                let first_handle = tokio::spawn(contested_grant(
                    pool.clone(),
                    Arc::clone(&barrier),
                    role_name.to_owned(),
                    scope.clone(),
                    holder.clone(),
                ));
                let second_handle = tokio::spawn(contested_grant(
                    pool.clone(),
                    Arc::clone(&barrier),
                    role_name.to_owned(),
                    scope.clone(),
                    holder.clone(),
                ));

                let first = first_handle
                    .await
                    .expect("the first racing task must not panic");
                let second = second_handle
                    .await
                    .expect("the second racing task must not panic");

                assert_eq!(
                    first.record, second.record,
                    "{role_name} iteration {iteration}: both tasks resolved the same record (the re-SELECT catch)"
                );
                let mut added_results = [first.added, second.added];
                added_results.sort_unstable();
                assert_eq!(
                    added_results,
                    [false, true],
                    "{role_name} iteration {iteration}: exactly one grant inserted the link, the loser's pair violation was absorbed"
                );

                // Deterministic probes on a third connection: exactly
                // one role row and exactly one link row survive.
                let mut verify_conn = mysql_conn().await;
                let (resource_type, resource_id) = scope.sql_values();
                let role_rows =
                    count_role_rows(&mut verify_conn, role_name, resource_type, resource_id).await;
                assert_eq!(
                    role_rows, 1,
                    "{role_name} iteration {iteration}: exactly one role row for the triple"
                );
                let raced_role_id =
                    resolve_role_row_id(&mut verify_conn, role_name, resource_type, resource_id)
                        .await;
                let link_rows =
                    count_link_rows(&mut verify_conn, holder.as_str(), raced_role_id).await;
                assert_eq!(
                    link_rows, 1,
                    "{role_name} iteration {iteration}: exactly one link row for the holder and the raced role"
                );
            }
        }
    }
}
