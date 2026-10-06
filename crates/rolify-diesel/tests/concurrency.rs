//! Concurrency race tests (TEST-05 + SC-2).
//!
//! Two std threads on two separate connections race `find_or_create_by`
//! and `add` on the SAME triple/pair. The database UNIQUE constraint
//! is the arbiter; catch-re-read (D-04) and catch-and-ignore (D-05)
//! yield exactly ONE role row / ONE join row deterministically.
//!
//! Timing-free: threads rendezvous on `std::sync::Barrier` immediately
//! before the contested call. No `thread::sleep` anywhere.
//!
//! Each race case runs 3 iterations to smoke out flakiness.
//!
//! Runs on Postgres and `MySQL` (cfg-gated). `SQLite` excluded per D-14
//! (single-writer `SQLITE_BUSY` makes the race meaningless).

#![cfg(all(feature = "sync", any(feature = "postgres", feature = "mysql")))]

mod support;

use std::sync::{Arc, Barrier};
use std::thread;

use diesel::Connection;
use diesel::RunQueryDsl;
use diesel_migrations::MigrationHarness;
use rolify_core::config::RolifyConfig;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::store::RoleStore;
use rolify_diesel::{
    DieselStore, MIGRATIONS,
    rows::{CountRow, IdRow},
};

use testcontainers::ImageExt;
use testcontainers_modules::{mysql, postgres, testcontainers::runners::SyncRunner};

const RACE_ITERATIONS: usize = 3;

#[derive(Clone)]
enum ScopeKind {
    Global,
    Class(String),
    Instance(String, ResourceId),
}

impl ScopeKind {
    fn to_resource_ref(&self) -> ResourceRef<'_> {
        match self {
            ScopeKind::Global => ResourceRef::Global,
            ScopeKind::Class(t) => ResourceRef::Class(t),
            ScopeKind::Instance(t, id) => ResourceRef::Instance(t, id),
        }
    }

    fn sql_values(&self) -> (&str, &str) {
        match self {
            ScopeKind::Global => ("", ""),
            ScopeKind::Class(t) => (t.as_str(), ""),
            ScopeKind::Instance(t, id) => (t.as_str(), id.as_str()),
        }
    }
}

#[cfg(feature = "postgres")]
mod pg_concurrency {
    use super::*;
    use diesel::pg::PgConnection;

    fn pg_container() -> &'static testcontainers::Container<postgres::Postgres> {
        use std::sync::OnceLock;
        static CONTAINER: OnceLock<testcontainers::Container<postgres::Postgres>> = OnceLock::new();
        CONTAINER.get_or_init(|| {
            let container = postgres::Postgres::default()
                .with_tag("17")
                .start()
                .expect("Docker must be available for Postgres; postgres:17 image will be pulled");
            let host_port = container
                .get_host_port_ipv4(5432)
                .expect("Postgres port mapping");
            let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
            let mut conn =
                PgConnection::establish(&url).expect("Postgres connection for migrations");
            conn.run_pending_migrations(MIGRATIONS)
                .expect("initial migrations");
            // Set up fixture tables once
            setup_fixtures(&mut conn);
            container
        })
    }

    fn pg_conn() -> PgConnection {
        let container = pg_container();
        let host_port = container
            .get_host_port_ipv4(5432)
            .expect("Postgres port mapping");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
        PgConnection::establish(&url).expect("Postgres connection")
    }

    fn setup_fixtures(conn: &mut PgConnection) {
        // D-06: the suite owns the fixture DDL; the race only needs the
        // holder table, but executing the shared array keeps every leg on
        // identical fixtures by construction.
        for statement in rolify_test::ddl::POSTGRES {
            diesel::sql_query(*statement)
                .execute(conn)
                .expect("fixture tables");
        }
    }

    fn reset_roles(conn: &mut PgConnection) {
        diesel::sql_query("TRUNCATE TABLE users_roles, roles RESTART IDENTITY CASCADE")
            .execute(conn)
            .expect("truncate roles");
    }

    fn insert_holder(
        conn: &mut PgConnection,
        table: &str,
        holder_type: &str,
        name: &str,
    ) -> ResourceId {
        let row: IdRow = diesel::sql_query(&format!(
            "INSERT INTO {} (rolify_type, name) VALUES ($1, $2) RETURNING id",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(holder_type)
        .bind::<diesel::sql_types::Text, _>(name)
        .get_result(conn)
        .expect("insert holder");
        ResourceId::from(row.id)
    }

    fn test_find_or_create_race(role_name: &str, scope: ScopeKind) {
        let _serial = crate::support::SuiteGuard::acquire();
        let mut seed_conn = pg_conn();
        reset_roles(&mut seed_conn);

        for iteration in 0..RACE_ITERATIONS {
            reset_roles(&mut seed_conn);

            let config = Arc::new(RolifyConfig::builder().build().unwrap());
            let barrier = Arc::new(Barrier::new(2));
            let role_name_owned = role_name.to_owned();
            let scope_owned = scope.clone();

            let mut handles = vec![];
            for _ in 0..2 {
                let config = Arc::clone(&config);
                let barrier = Arc::clone(&barrier);
                let role_name = role_name_owned.clone();
                let scope = scope_owned.clone();
                let handle = thread::spawn(move || {
                    let mut conn = pg_conn();
                    let mut store = DieselStore::new(&config);
                    barrier.wait();
                    store
                        .find_or_create_by(
                            &mut conn,
                            &RoleName::from(role_name.as_str()),
                            scope.to_resource_ref(),
                        )
                        .expect("concurrent find_or_create_by")
                });
                handles.push(handle);
            }

            let mut results = vec![];
            for handle in handles {
                results.push(handle.join().expect("thread panicked"));
            }

            assert_eq!(
                results[0], results[1],
                "iteration {iteration}: both threads got same RoleRecord"
            );

            let mut verify_conn = pg_conn();
            let (rt, rid) = scope.sql_values();
            let count_row: CountRow = diesel::sql_query(
                "SELECT COUNT(*) AS count FROM roles WHERE name = $1 AND resource_type = $2 AND resource_id = $3"
            )
            .bind::<diesel::sql_types::Text, _>(role_name)
            .bind::<diesel::sql_types::Text, _>(rt)
            .bind::<diesel::sql_types::Text, _>(rid)
            .get_result(&mut verify_conn)
            .expect("count role rows");
            assert_eq!(
                count_row.count, 1,
                "iteration {iteration}: exactly one role row for '{}'",
                role_name
            );
        }
    }

    fn test_add_race(role_name: &str, scope: ScopeKind) {
        let _serial = crate::support::SuiteGuard::acquire();
        let mut seed_conn = pg_conn();
        reset_roles(&mut seed_conn);

        for iteration in 0..RACE_ITERATIONS {
            reset_roles(&mut seed_conn);

            let config = Arc::new(RolifyConfig::builder().build().unwrap());
            let mut store = DieselStore::new(&config);
            let role = store
                .find_or_create_by(
                    &mut seed_conn,
                    &RoleName::from(role_name),
                    scope.to_resource_ref(),
                )
                .expect("seed find_or_create_by");
            let holder_id = insert_holder(
                &mut seed_conn,
                "users",
                "User",
                &format!("race_holder_{iteration}"),
            );

            let config = Arc::new(config);
            let barrier = Arc::new(Barrier::new(2));
            let role_clone = role.clone();
            let holder_clone = holder_id.clone();
            let scope_clone = scope.clone();

            let mut handles = vec![];
            for _ in 0..2 {
                let config = Arc::clone(&config);
                let barrier = Arc::clone(&barrier);
                let role = role_clone.clone();
                let holder = holder_clone.clone();
                let scope = scope_clone.clone();
                let handle = thread::spawn(move || {
                    let mut conn = pg_conn();
                    let mut store = DieselStore::new(&config);
                    barrier.wait();
                    store
                        .add(&mut conn, &holder, &role)
                        .expect("concurrent add")
                });
                handles.push(handle);
            }

            let mut results = vec![];
            for handle in handles {
                results.push(handle.join().expect("thread panicked"));
            }

            results.sort();
            assert_eq!(
                results,
                vec![false, true],
                "iteration {iteration}: one Ok(true), one Ok(false)"
            );

            let mut verify_conn = pg_conn();
            let (rt, rid) = scope.sql_values();
            let role_id_row: IdRow = diesel::sql_query(
                "SELECT id FROM roles WHERE name = $1 AND resource_type = $2 AND resource_id = $3",
            )
            .bind::<diesel::sql_types::Text, _>(role_name)
            .bind::<diesel::sql_types::Text, _>(rt)
            .bind::<diesel::sql_types::Text, _>(rid)
            .get_result(&mut verify_conn)
            .expect("get role id");
            let role_id = role_id_row.id;

            let link_count_row: CountRow = diesel::sql_query(
                "SELECT COUNT(*) AS count FROM users_roles WHERE user_id = $1 AND role_id = $2",
            )
            .bind::<diesel::sql_types::Text, _>(holder_id.as_str())
            .bind::<diesel::sql_types::BigInt, _>(role_id)
            .get_result(&mut verify_conn)
            .expect("count join rows");
            assert_eq!(
                link_count_row.count, 1,
                "iteration {iteration}: exactly one join row for holder+role"
            );
        }
    }

    #[test]
    fn concurrent_find_or_create_by_global_role() {
        test_find_or_create_race("concurrent_global_admin", ScopeKind::Global);
    }

    #[test]
    fn concurrent_find_or_create_by_class_role() {
        test_find_or_create_race(
            "concurrent_class_manager",
            ScopeKind::Class("Forum".to_owned()),
        );
    }

    #[test]
    fn concurrent_find_or_create_by_instance_role() {
        test_find_or_create_race(
            "concurrent_instance_moderator",
            ScopeKind::Instance("Forum".to_owned(), ResourceId::from("77")),
        );
    }

    #[test]
    fn concurrent_add_global_role() {
        test_add_race("concurrent_add_admin", ScopeKind::Global);
    }

    #[test]
    fn concurrent_add_class_role() {
        test_add_race(
            "concurrent_add_manager",
            ScopeKind::Class("Forum".to_owned()),
        );
    }

    #[test]
    fn concurrent_add_instance_role() {
        test_add_race(
            "concurrent_add_moderator",
            ScopeKind::Instance("Forum".to_owned(), ResourceId::from("77")),
        );
    }
}

#[cfg(feature = "mysql")]
mod mysql_concurrency {
    use super::*;
    use diesel::mysql::MysqlConnection;

    fn mysql_container() -> &'static testcontainers::Container<mysql::Mysql> {
        use std::sync::OnceLock;
        static CONTAINER: OnceLock<testcontainers::Container<mysql::Mysql>> = OnceLock::new();
        CONTAINER.get_or_init(|| {
            let container = mysql::Mysql::default()
                .with_tag("8.4")
                .start()
                .expect("Docker must be available for MySQL; mysql:8.4 image will be pulled");
            let host_port = container
                .get_host_port_ipv4(3306)
                .expect("MySQL port mapping");
            let url = format!("mysql://root@127.0.0.1:{host_port}/test");
            let mut conn =
                MysqlConnection::establish(&url).expect("MySQL connection for migrations");
            conn.run_pending_migrations(MIGRATIONS)
                .expect("initial migrations");
            // Set up fixture tables once
            setup_fixtures(&mut conn);
            container
        })
    }

    fn mysql_conn() -> MysqlConnection {
        let container = mysql_container();
        let host_port = container
            .get_host_port_ipv4(3306)
            .expect("MySQL port mapping");
        let url = format!("mysql://root@127.0.0.1:{host_port}/test");
        MysqlConnection::establish(&url).expect("MySQL connection")
    }

    fn setup_fixtures(conn: &mut MysqlConnection) {
        // D-06: the suite owns the fixture DDL (backticked `groups` for
        // the MySQL 8.4 reserved word); executed statement by statement.
        for statement in rolify_test::ddl::MYSQL {
            diesel::sql_query(*statement)
                .execute(conn)
                .expect("fixture tables");
        }
    }

    fn reset_roles(conn: &mut MysqlConnection) {
        // MySQL: TRUNCATE is single-table only and refuses `roles` while
        // the users_roles FK references it (error 1701, even with the
        // child table empty; verified against mysql:8.4), so the reset
        // uses FK-ordered DELETEs instead of the Postgres multi-table
        // TRUNCATE. No assertion depends on AUTO_INCREMENT state.
        diesel::sql_query("DELETE FROM users_roles")
            .execute(conn)
            .expect("clear users_roles");
        diesel::sql_query("DELETE FROM roles")
            .execute(conn)
            .expect("clear roles");
    }

    fn insert_holder(
        conn: &mut MysqlConnection,
        table: &str,
        holder_type: &str,
        name: &str,
    ) -> ResourceId {
        diesel::sql_query(&format!(
            "INSERT INTO {} (rolify_type, name) VALUES (?, ?)",
            table
        ))
        .bind::<diesel::sql_types::Text, _>(holder_type)
        .bind::<diesel::sql_types::Text, _>(name)
        .execute(conn)
        .expect("insert holder");
        let row: IdRow = diesel::sql_query("SELECT LAST_INSERT_ID() AS id")
            .get_result(conn)
            .expect("last insert id");
        ResourceId::from(row.id)
    }

    fn test_find_or_create_race(role_name: &str, scope: ScopeKind) {
        let _serial = crate::support::SuiteGuard::acquire();
        let mut seed_conn = mysql_conn();
        reset_roles(&mut seed_conn);

        for iteration in 0..RACE_ITERATIONS {
            reset_roles(&mut seed_conn);

            let config = Arc::new(RolifyConfig::builder().build().unwrap());
            let barrier = Arc::new(Barrier::new(2));
            let role_name_owned = role_name.to_owned();
            let scope_owned = scope.clone();

            let mut handles = vec![];
            for _ in 0..2 {
                let config = Arc::clone(&config);
                let barrier = Arc::clone(&barrier);
                let role_name = role_name_owned.clone();
                let scope = scope_owned.clone();
                let handle = thread::spawn(move || {
                    let mut conn = mysql_conn();
                    let mut store = DieselStore::new(&config);
                    barrier.wait();
                    store
                        .find_or_create_by(
                            &mut conn,
                            &RoleName::from(role_name.as_str()),
                            scope.to_resource_ref(),
                        )
                        .expect("concurrent find_or_create_by")
                });
                handles.push(handle);
            }

            let mut results = vec![];
            for handle in handles {
                results.push(handle.join().expect("thread panicked"));
            }

            assert_eq!(
                results[0], results[1],
                "iteration {iteration}: both threads got same RoleRecord"
            );

            let mut verify_conn = mysql_conn();
            let (rt, rid) = scope.sql_values();
            let count_row: CountRow = diesel::sql_query(
                "SELECT COUNT(*) AS count FROM roles WHERE name = ? AND resource_type = ? AND resource_id = ?"
            )
            .bind::<diesel::sql_types::Text, _>(role_name)
            .bind::<diesel::sql_types::Text, _>(rt)
            .bind::<diesel::sql_types::Text, _>(rid)
            .get_result(&mut verify_conn)
            .expect("count role rows");
            assert_eq!(
                count_row.count, 1,
                "iteration {iteration}: exactly one role row for '{}'",
                role_name
            );
        }
    }

    fn test_add_race(role_name: &str, scope: ScopeKind) {
        let _serial = crate::support::SuiteGuard::acquire();
        let mut seed_conn = mysql_conn();
        reset_roles(&mut seed_conn);

        for iteration in 0..RACE_ITERATIONS {
            reset_roles(&mut seed_conn);

            let config = Arc::new(RolifyConfig::builder().build().unwrap());
            let mut store = DieselStore::new(&config);
            let role = store
                .find_or_create_by(
                    &mut seed_conn,
                    &RoleName::from(role_name),
                    scope.to_resource_ref(),
                )
                .expect("seed find_or_create_by");
            let holder_id = insert_holder(
                &mut seed_conn,
                "users",
                "User",
                &format!("race_holder_{iteration}"),
            );

            let config = Arc::new(config);
            let barrier = Arc::new(Barrier::new(2));
            let role_clone = role.clone();
            let holder_clone = holder_id.clone();
            let scope_clone = scope.clone();

            let mut handles = vec![];
            for _ in 0..2 {
                let config = Arc::clone(&config);
                let barrier = Arc::clone(&barrier);
                let role = role_clone.clone();
                let holder = holder_clone.clone();
                let scope = scope_clone.clone();
                let handle = thread::spawn(move || {
                    let mut conn = mysql_conn();
                    let mut store = DieselStore::new(&config);
                    barrier.wait();
                    store
                        .add(&mut conn, &holder, &role)
                        .expect("concurrent add")
                });
                handles.push(handle);
            }

            let mut results = vec![];
            for handle in handles {
                results.push(handle.join().expect("thread panicked"));
            }

            results.sort();
            assert_eq!(
                results,
                vec![false, true],
                "iteration {iteration}: one Ok(true), one Ok(false)"
            );

            let mut verify_conn = mysql_conn();
            let (rt, rid) = scope.sql_values();
            let role_id_row: IdRow = diesel::sql_query(
                "SELECT id FROM roles WHERE name = ? AND resource_type = ? AND resource_id = ?",
            )
            .bind::<diesel::sql_types::Text, _>(role_name)
            .bind::<diesel::sql_types::Text, _>(rt)
            .bind::<diesel::sql_types::Text, _>(rid)
            .get_result(&mut verify_conn)
            .expect("get role id");
            let role_id = role_id_row.id;

            let link_count_row: CountRow = diesel::sql_query(
                "SELECT COUNT(*) AS count FROM users_roles WHERE user_id = ? AND role_id = ?",
            )
            .bind::<diesel::sql_types::Text, _>(holder_id.as_str())
            .bind::<diesel::sql_types::BigInt, _>(role_id)
            .get_result(&mut verify_conn)
            .expect("count join rows");
            assert_eq!(
                link_count_row.count, 1,
                "iteration {iteration}: exactly one join row for holder+role"
            );
        }
    }

    #[test]
    fn concurrent_find_or_create_by_global_role() {
        test_find_or_create_race("concurrent_global_admin", ScopeKind::Global);
    }

    #[test]
    fn concurrent_find_or_create_by_class_role() {
        test_find_or_create_race(
            "concurrent_class_manager",
            ScopeKind::Class("Forum".to_owned()),
        );
    }

    #[test]
    fn concurrent_find_or_create_by_instance_role() {
        test_find_or_create_race(
            "concurrent_instance_moderator",
            ScopeKind::Instance("Forum".to_owned(), ResourceId::from("77")),
        );
    }

    #[test]
    fn concurrent_add_global_role() {
        test_add_race("concurrent_add_admin", ScopeKind::Global);
    }

    #[test]
    fn concurrent_add_class_role() {
        test_add_race(
            "concurrent_add_manager",
            ScopeKind::Class("Forum".to_owned()),
        );
    }

    #[test]
    fn concurrent_add_instance_role() {
        test_add_race(
            "concurrent_add_moderator",
            ScopeKind::Instance("Forum".to_owned(), ResourceId::from("77")),
        );
    }
}
