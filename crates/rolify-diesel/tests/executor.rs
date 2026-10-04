//! Executor uniformity acceptance tests (ROADMAP SC-5).
//!
//! Proves the same `DieselStore` calls work identically across three
//! connection surfaces:
//! 1. Bare `PgConnection` / `MysqlConnection` (autocommit)
//! 2. `r2d2` pool checkout (`PooledConnection`)
//! 3. Caller-owned `conn.transaction(|c| ...)` — commit leg
//! 4. Caller-owned transaction — rollback leg (savepoint absorption)
//!
//! The rollback leg proves that role writes inside a nested transaction
//! (savepoint) are fully absorbed on rollback — cross-connection visibility
//! confirms zero leaked rows.
//!
//! Runs on Postgres and MySQL (cfg-gated). SQLite excluded per D-14
//! (locking-sensitive acceptance not meaningful on single-writer engine).

#![cfg(all(feature = "sync", any(feature = "postgres", feature = "mysql")))]

use diesel::Connection;
use diesel::RunQueryDsl;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel_migrations::MigrationHarness;
use rolify_core::config::RolifyConfig;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord, SCOPE_SENTINEL};
use rolify_core::store::RoleStore;
use rolify_diesel::{DieselStore, MIGRATIONS, rows::CountRow};

use diesel::connection::SimpleConnection;
use rolify_diesel::rows::IdRow;
use testcontainers::ImageExt;
use testcontainers_modules::{mysql, postgres, testcontainers::runners::SyncRunner};

#[cfg(feature = "postgres")]
mod pg_executor {
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
            // Run migrations once when container starts
            let host_port = container
                .get_host_port_ipv4(5432)
                .expect("Postgres port mapping");
            let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
            let mut conn =
                PgConnection::establish(&url).expect("Postgres connection for migrations");
            conn.run_pending_migrations(MIGRATIONS)
                .expect("initial migrations");
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

    fn pg_pool() -> Pool<ConnectionManager<PgConnection>> {
        let container = pg_container();
        let host_port = container.get_host_port_ipv4(5432).expect("Postgres port");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
        let manager = ConnectionManager::<PgConnection>::new(url);
        Pool::builder().max_size(4).build(manager).expect("pg pool")
    }

    fn setup_fixtures(conn: &mut PgConnection) {
        let sql = r#"
            CREATE TABLE IF NOT EXISTS users (
                id BIGSERIAL PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'User',
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS customers (
                id BIGSERIAL PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'Customer',
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS forums (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS groups (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS teams (
                team_code VARCHAR(191) PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS organizations (
                id BIGSERIAL PRIMARY KEY,
                type VARCHAR(191) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS rights (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            );
            CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            );
            "#;
        conn.batch_execute(sql).expect("fixture tables");
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

    fn run_executor_matrix() {
        let _container = pg_container();
        let mut conn = pg_conn();
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);

        let config = RolifyConfig::builder().build().unwrap();
        let mut store = DieselStore::new(&config);
        let user_id = insert_holder(&mut conn, "users", "User", "executor_user");

        // --- LEG 1: Bare connection (autocommit) ---
        let admin = store
            .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
            .expect("bare: find_or_create_by global admin");
        assert!(admin.is_global());
        let added = store
            .add(&mut conn, &user_id, &admin)
            .expect("bare: add global admin");
        assert!(added, "bare: first add creates link");
        let added_again = store
            .add(&mut conn, &user_id, &admin)
            .expect("bare: add again");
        assert!(!added_again, "bare: second add returns false");

        // Verify via fresh connection (cross-connection visibility)
        let mut verify_conn = pg_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id,
                &RoleQuery {
                    name: &RoleName::from("admin"),
                    filter: ResourceFilter::Global,
                },
            )
            .expect("bare: where_ verify");
        assert_eq!(roles.len(), 1, "bare: role visible cross-connection");
        assert!(roles[0].is_global());
        reset_roles(&mut conn);

        // --- LEG 2: r2d2 pool checkout ---
        let pool = pg_pool();
        let mut pooled = pool.get().expect("pool checkout");
        setup_fixtures(&mut pooled);
        reset_roles(&mut pooled);

        let mut store_pooled = DieselStore::new(&config);
        let user_id_pooled = insert_holder(&mut pooled, "users", "User", "executor_user_pooled");

        let manager = store_pooled
            .find_or_create_by(
                &mut pooled,
                &RoleName::from("manager"),
                ResourceRef::Class("Forum"),
            )
            .expect("pooled: find_or_create_by class manager");
        assert!(manager.is_class_scoped_to("Forum"));
        let added = store_pooled
            .add(&mut pooled, &user_id_pooled, &manager)
            .expect("pooled: add class manager");
        assert!(added, "pooled: first add creates link");

        // Cross-connection verify via bare connection
        let mut verify_conn = pg_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id_pooled,
                &RoleQuery {
                    name: &RoleName::from("manager"),
                    filter: ResourceFilter::Class("Forum"),
                },
            )
            .expect("pooled: where_ verify");
        assert_eq!(roles.len(), 1, "pooled: role visible cross-connection");
        assert!(roles[0].is_class_scoped_to("Forum"));
        reset_roles(&mut pooled);

        // --- LEG 3: Caller-owned transaction (commit) ---
        let mut tx_conn = pg_conn();
        setup_fixtures(&mut tx_conn);
        reset_roles(&mut tx_conn);

        let mut store_tx = DieselStore::new(&config);
        let user_id_tx = insert_holder(&mut tx_conn, "users", "User", "executor_user_tx");

        tx_conn
            .transaction::<_, rolify_diesel::Error, _>(|conn| {
                let moderator = store_tx
                    .find_or_create_by(
                        conn,
                        &RoleName::from("moderator"),
                        ResourceRef::Instance("Forum", &ResourceId::from("99")),
                    )
                    .expect("tx: find_or_create_by instance moderator");
                assert!(moderator.is_instance_scoped_to("Forum", &ResourceId::from("99")));
                let added = store_tx
                    .add(conn, &user_id_tx, &moderator)
                    .expect("tx: add instance moderator");
                assert!(added, "tx: first add creates link");

                // Read inside same transaction
                let roles = store_tx
                    .where_(
                        conn,
                        &user_id_tx,
                        &RoleQuery {
                            name: &RoleName::from("moderator"),
                            filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
                        },
                    )
                    .expect("tx: where_ inside transaction");
                assert_eq!(roles.len(), 1, "tx: role visible inside transaction");
                assert!(roles[0].is_instance_scoped_to("Forum", &ResourceId::from("99")));

                Ok(())
            })
            .expect("tx: commit");

        // Cross-connection verify after commit
        let mut verify_conn = pg_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id_tx,
                &RoleQuery {
                    name: &RoleName::from("moderator"),
                    filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
                },
            )
            .expect("tx: where_ verify after commit");
        assert_eq!(
            roles.len(),
            1,
            "tx: role visible cross-connection after commit"
        );
        assert!(roles[0].is_instance_scoped_to("Forum", &ResourceId::from("99")));
        reset_roles(&mut tx_conn);

        // --- LEG 4: Caller-owned transaction (rollback) ---
        let mut rb_conn = pg_conn();
        setup_fixtures(&mut rb_conn);
        reset_roles(&mut rb_conn);

        let mut store_rb = DieselStore::new(&config);
        let user_id_rb = insert_holder(&mut rb_conn, "users", "User", "executor_user_rb");

        let rb_result: Result<(), rolify_diesel::Error> = rb_conn.transaction(|conn| {
            let editor = store_rb
                .find_or_create_by(conn, &RoleName::from("editor"), ResourceRef::Global)
                .expect("rb: find_or_create_by global editor");
            assert!(editor.is_global());
            let added = store_rb
                .add(conn, &user_id_rb, &editor)
                .expect("rb: add global editor");
            assert!(added, "rb: first add creates link");

            // Force rollback
            Err(rolify_diesel::Error::Core(
                rolify_core::RolifyError::InvalidConfig {
                    reason: "intentional rollback for test".into(),
                },
            ))
        });

        assert!(rb_result.is_err(), "rb: transaction rolled back");

        // Cross-connection verify: NO rows should exist after rollback
        let mut verify_conn = pg_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id_rb,
                &RoleQuery {
                    name: &RoleName::from("editor"),
                    filter: ResourceFilter::Global,
                },
            )
            .expect("rb: where_ verify after rollback");
        assert_eq!(
            roles.len(),
            0,
            "rb: NO role rows leaked after rollback (savepoint absorption)"
        );

        // Also verify roles table directly
        let count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM roles WHERE name = 'editor' AND resource_type = '' AND resource_id = ''")
            .get_result(&mut verify_conn)
            .expect("rb: count editor rows");
        assert_eq!(
            count_row.count, 0,
            "rb: NO role row in roles table after rollback"
        );

        let link_count_row: CountRow =
            diesel::sql_query("SELECT COUNT(*) AS count FROM users_roles WHERE user_id = $1")
                .bind::<diesel::sql_types::Text, _>(user_id_rb.as_str())
                .get_result(&mut verify_conn)
                .expect("rb: count links");
        assert_eq!(
            link_count_row.count, 0,
            "rb: NO join rows leaked after rollback"
        );
    }

    #[test]
    fn executor_uniformity_bare_pool_tx_commit_rollback() {
        run_executor_matrix();
    }
}

#[cfg(feature = "mysql")]
mod mysql_executor {
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
            // Run migrations once when container starts
            let host_port = container
                .get_host_port_ipv4(3306)
                .expect("MySQL port mapping");
            let url = format!("mysql://root@127.0.0.1:{host_port}/test");
            let mut conn =
                MysqlConnection::establish(&url).expect("MySQL connection for migrations");
            conn.run_pending_migrations(MIGRATIONS)
                .expect("initial migrations");
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

    fn mysql_pool() -> Pool<ConnectionManager<MysqlConnection>> {
        let container = mysql_container();
        let host_port = container.get_host_port_ipv4(3306).expect("MySQL port");
        let url = format!("mysql://root@127.0.0.1:{host_port}/test");
        let manager = ConnectionManager::<MysqlConnection>::new(url);
        Pool::builder()
            .max_size(4)
            .build(manager)
            .expect("mysql pool")
    }

    fn setup_fixtures(conn: &mut MysqlConnection) {
        let sql = r#"
            CREATE TABLE IF NOT EXISTS users (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'User',
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS customers (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                rolify_type VARCHAR(191) NOT NULL DEFAULT 'Customer',
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS forums (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS groups (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS teams (
                team_code VARCHAR(191) PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS organizations (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                type VARCHAR(191) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS rights (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS moderators_rights (
                moderator_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (moderator_id, right_id)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            CREATE TABLE IF NOT EXISTS admin_rights (
                admin_id VARCHAR(191) NOT NULL,
                right_id BIGINT NOT NULL,
                PRIMARY KEY (admin_id, right_id)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
            "#;
        conn.batch_execute(sql).expect("fixture tables");
    }

    fn reset_roles(conn: &mut MysqlConnection) {
        diesel::sql_query("TRUNCATE TABLE users_roles, roles")
            .execute(conn)
            .expect("truncate roles");
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

    fn run_executor_matrix() {
        let _container = mysql_container();
        let mut conn = mysql_conn();
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);

        let config = RolifyConfig::builder().build().unwrap();
        let mut store = DieselStore::new(&config);
        let user_id = insert_holder(&mut conn, "users", "User", "executor_user");

        // --- LEG 1: Bare connection (autocommit) ---
        let admin = store
            .find_or_create_by(&mut conn, &RoleName::from("admin"), ResourceRef::Global)
            .expect("bare: find_or_create_by global admin");
        assert!(admin.is_global());
        let added = store
            .add(&mut conn, &user_id, &admin)
            .expect("bare: add global admin");
        assert!(added, "bare: first add creates link");
        let added_again = store
            .add(&mut conn, &user_id, &admin)
            .expect("bare: add again");
        assert!(!added_again, "bare: second add returns false");

        let mut verify_conn = mysql_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id,
                &RoleQuery {
                    name: &RoleName::from("admin"),
                    filter: ResourceFilter::Global,
                },
            )
            .expect("bare: where_ verify");
        assert_eq!(roles.len(), 1, "bare: role visible cross-connection");
        assert!(roles[0].is_global());
        reset_roles(&mut conn);

        // --- LEG 2: r2d2 pool checkout ---
        let pool = mysql_pool();
        let mut pooled = pool.get().expect("pool checkout");
        setup_fixtures(&mut pooled);
        reset_roles(&mut pooled);

        let mut store_pooled = DieselStore::new(&config);
        let user_id_pooled = insert_holder(&mut pooled, "users", "User", "executor_user_pooled");

        let manager = store_pooled
            .find_or_create_by(
                &mut pooled,
                &RoleName::from("manager"),
                ResourceRef::Class("Forum"),
            )
            .expect("pooled: find_or_create_by class manager");
        assert!(manager.is_class_scoped_to("Forum"));
        let added = store_pooled
            .add(&mut pooled, &user_id_pooled, &manager)
            .expect("pooled: add class manager");
        assert!(added, "pooled: first add creates link");

        let mut verify_conn = mysql_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id_pooled,
                &RoleQuery {
                    name: &RoleName::from("manager"),
                    filter: ResourceFilter::Class("Forum"),
                },
            )
            .expect("pooled: where_ verify");
        assert_eq!(roles.len(), 1, "pooled: role visible cross-connection");
        assert!(roles[0].is_class_scoped_to("Forum"));
        reset_roles(&mut pooled);

        // --- LEG 3: Caller-owned transaction (commit) ---
        let mut tx_conn = mysql_conn();
        setup_fixtures(&mut tx_conn);
        reset_roles(&mut tx_conn);

        let mut store_tx = DieselStore::new(&config);
        let user_id_tx = insert_holder(&mut tx_conn, "users", "User", "executor_user_tx");

        tx_conn
            .transaction::<_, rolify_diesel::Error, _>(|conn| {
                let moderator = store_tx
                    .find_or_create_by(
                        conn,
                        &RoleName::from("moderator"),
                        ResourceRef::Instance("Forum", &ResourceId::from("99")),
                    )
                    .expect("tx: find_or_create_by instance moderator");
                assert!(moderator.is_instance_scoped_to("Forum", &ResourceId::from("99")));
                let added = store_tx
                    .add(conn, &user_id_tx, &moderator)
                    .expect("tx: add instance moderator");
                assert!(added, "tx: first add creates link");

                let roles = store_tx
                    .where_(
                        conn,
                        &user_id_tx,
                        &RoleQuery {
                            name: &RoleName::from("moderator"),
                            filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
                        },
                    )
                    .expect("tx: where_ inside transaction");
                assert_eq!(roles.len(), 1, "tx: role visible inside transaction");
                assert!(roles[0].is_instance_scoped_to("Forum", &ResourceId::from("99")));

                Ok(())
            })
            .expect("tx: commit");

        let mut verify_conn = mysql_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id_tx,
                &RoleQuery {
                    name: &RoleName::from("moderator"),
                    filter: ResourceFilter::Instance("Forum", &ResourceId::from("99")),
                },
            )
            .expect("tx: where_ verify after commit");
        assert_eq!(
            roles.len(),
            1,
            "tx: role visible cross-connection after commit"
        );
        assert!(roles[0].is_instance_scoped_to("Forum", &ResourceId::from("99")));
        reset_roles(&mut tx_conn);

        // --- LEG 4: Caller-owned transaction (rollback) ---
        let mut rb_conn = mysql_conn();
        setup_fixtures(&mut rb_conn);
        reset_roles(&mut rb_conn);

        let mut store_rb = DieselStore::new(&config);
        let user_id_rb = insert_holder(&mut rb_conn, "users", "User", "executor_user_rb");

        let rb_result: Result<(), rolify_diesel::Error> = rb_conn.transaction(|conn| {
            let editor = store_rb
                .find_or_create_by(conn, &RoleName::from("editor"), ResourceRef::Global)
                .expect("rb: find_or_create_by global editor");
            assert!(editor.is_global());
            let added = store_rb
                .add(conn, &user_id_rb, &editor)
                .expect("rb: add global editor");
            assert!(added, "rb: first add creates link");

            Err(rolify_diesel::Error::Core(
                rolify_core::RolifyError::InvalidConfig {
                    reason: "intentional rollback for test".into(),
                },
            ))
        });

        assert!(rb_result.is_err(), "rb: transaction rolled back");

        let mut verify_conn = mysql_conn();
        let roles = store
            .where_(
                &mut verify_conn,
                &user_id_rb,
                &RoleQuery {
                    name: &RoleName::from("editor"),
                    filter: ResourceFilter::Global,
                },
            )
            .expect("rb: where_ verify after rollback");
        assert_eq!(
            roles.len(),
            0,
            "rb: NO role rows leaked after rollback (savepoint absorption)"
        );

        let count_row: CountRow = diesel::sql_query("SELECT COUNT(*) AS count FROM roles WHERE name = 'editor' AND resource_type = '' AND resource_id = ''")
            .get_result(&mut verify_conn)
            .expect("rb: count editor rows");
        assert_eq!(
            count_row.count, 0,
            "rb: NO role row in roles table after rollback"
        );

        let link_count_row: CountRow =
            diesel::sql_query("SELECT COUNT(*) AS count FROM users_roles WHERE user_id = ?")
                .bind::<diesel::sql_types::Text, _>(user_id_rb.as_str())
                .get_result(&mut verify_conn)
                .expect("rb: count links");
        assert_eq!(
            link_count_row.count, 0,
            "rb: NO join rows leaked after rollback"
        );
    }

    #[test]
    fn executor_uniformity_bare_pool_tx_commit_rollback() {
        run_executor_matrix();
    }
}
