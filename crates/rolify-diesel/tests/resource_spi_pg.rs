//! Postgres-backed acceptance tests for `ResourceStore` and finder SPI members.
//!
//! Tests cover:
//! - STI class-scope expansion
//! - String PK (`team_code`) round-trip
//! - Scoped delete + cascade sweep (SC-3)
//! - `holders_where` strict vs non-strict
//! - `all_holders` full-table semantics
//! - `roles_matching` filter matrix
//! - Custom join/role table names (CONF-04)

#![cfg(all(feature = "sync", feature = "postgres"))]

mod support;

mod tests {
    use std::collections::HashSet;

    use diesel::Connection;
    use diesel::RunQueryDsl;
    use pretty_assertions::assert_eq;
    use rolify_core::catalog::{CatalogScope, RoleCatalogQuery};
    use rolify_core::query::{ResourceFilter, RoleQuery};
    use rolify_core::resource::ResourceRef;
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::{ResourceKey, ResourceStore, RoleStore};
    use rolify_diesel::rows::IdRow;
    use rolify_diesel::{DieselStore, MIGRATIONS};

    use crate::support::{
        insert_holder, insert_resource, pg_conn, pg_container, reset_fixtures, reset_roles,
        run_migrations, setup_fixtures, test_config,
    };

    /// Helper to create a store with all resource types registered
    fn make_store() -> DieselStore {
        DieselStore::new(&test_config())
            .for_holder_table("users")
            .register_resource_table("Forum", "forums", "id")
            .register_resource_table("Group", "groups", "id")
            .register_resource_table("Team", "teams", "team_code")
            .register_resource_table("Organization", "organizations", "id")
    }

    #[test]
    fn resources_find_returns_instance_keys_for_instance_roles() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        // Insert a forum
        let forum_key = insert_resource(&mut conn, "forums", "Test Forum");
        let forum_id = forum_key.resource_id.clone();

        // Grant instance-scoped role
        let role = RoleRecord::for_instance("moderator", "Forum", forum_id.clone());
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let role_rec = store
                .find_or_create_by(conn, &role.name, ResourceRef::Instance("Forum", &forum_id))
                .unwrap();
            store
                .add(conn, &ResourceId::from(1_i64), &role_rec)
                .unwrap();
            Ok(())
        })
        .unwrap();

        // Query resources_find
        let found = store
            .resources_find(&mut conn, &["Forum"], &RoleName::from("moderator"))
            .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].resource_type, "Forum");
        assert_eq!(found[0].resource_id, forum_id);
    }

    #[test]
    fn resources_find_expands_class_role_to_all_instances() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        // Insert multiple forums
        let forum1_key = insert_resource(&mut conn, "forums", "Forum 1");
        let forum2_key = insert_resource(&mut conn, "forums", "Forum 2");
        let forum3_key = insert_resource(&mut conn, "forums", "Forum 3");

        // Grant class-scoped role on Forum
        let role = RoleRecord::for_class("admin", "Forum");
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let role_rec = store
                .find_or_create_by(conn, &role.name, ResourceRef::Class("Forum"))
                .unwrap();
            store
                .add(conn, &ResourceId::from(1_i64), &role_rec)
                .unwrap();
            Ok(())
        })
        .unwrap();

        // Query resources_find - should return all three forums
        let found = store
            .resources_find(&mut conn, &["Forum"], &RoleName::from("admin"))
            .unwrap();

        let found_ids: HashSet<_> = found.iter().map(|k| k.resource_id.as_str()).collect();
        let expected_ids: HashSet<_> = [
            forum1_key.resource_id.as_str(),
            forum2_key.resource_id.as_str(),
            forum3_key.resource_id.as_str(),
        ]
        .into();

        assert_eq!(found.len(), 3);
        assert_eq!(found_ids, expected_ids);
        for key in &found {
            assert_eq!(key.resource_type, "Forum");
        }
    }

    #[test]
    fn resources_find_respects_sti_family() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        // Insert organizations with different types
        diesel::sql_query("INSERT INTO organizations (type) VALUES ('Company'), ('Department')")
            .execute(&mut conn)
            .unwrap();

        let org1_row: IdRow =
            diesel::sql_query("SELECT id FROM organizations WHERE type = 'Company'")
                .get_result(&mut conn)
                .unwrap();
        let org2_row: IdRow =
            diesel::sql_query("SELECT id FROM organizations WHERE type = 'Department'")
                .get_result(&mut conn)
                .unwrap();
        let org1_id = org1_row.id;
        let org2_id = org2_row.id;

        // Grant class-scoped role on Organization (parent type)
        let role = RoleRecord::for_class("owner", "Organization");
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let role_rec = store
                .find_or_create_by(conn, &role.name, ResourceRef::Class("Organization"))
                .unwrap();
            store
                .add(conn, &ResourceId::from(1_i64), &role_rec)
                .unwrap();
            Ok(())
        })
        .unwrap();

        // Query with STI family (Organization + descendants)
        // Note: descendant_types for Organization would include Company, Department
        // For this test, we query with both types explicitly
        let found = store
            .resources_find(
                &mut conn,
                &["Organization", "Company", "Department"],
                &RoleName::from("owner"),
            )
            .unwrap();

        // Should find both organizations since they're in the STI family
        let found_ids: HashSet<_> = found.iter().map(|k| k.resource_id.as_str()).collect();
        assert!(found_ids.contains(org1_id.to_string().as_str()));
        assert!(found_ids.contains(org2_id.to_string().as_str()));
    }

    #[test]
    fn resources_find_string_pk_team_code_roundtrip() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        // Insert team with string PK (the shared helper only serves
        // integer-PK tables, so the team row is inserted explicitly
        // with its `team_code` key).
        diesel::sql_query("INSERT INTO teams (team_code, name) VALUES ('alpha', 'Team Alpha')")
            .execute(&mut conn)
            .unwrap();
        let team_key = ResourceKey::new("Team", "alpha");
        assert_eq!(team_key.resource_type, "Team");

        // Grant instance-scoped role
        let role = RoleRecord::for_instance("member", "Team", team_key.resource_id.clone());
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let role_rec = store
                .find_or_create_by(
                    conn,
                    &role.name,
                    ResourceRef::Instance("Team", &team_key.resource_id),
                )
                .unwrap();
            store
                .add(conn, &ResourceId::from(1_i64), &role_rec)
                .unwrap();
            Ok(())
        })
        .unwrap();

        // Query resources_find
        let found = store
            .resources_find(&mut conn, &["Team"], &RoleName::from("member"))
            .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].resource_type, "Team");
        assert_eq!(found[0].resource_id, team_key.resource_id);
    }

    #[test]
    fn in_list_filters_candidates_by_holder_roles() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        // Insert forums
        let forum1_key = insert_resource(&mut conn, "forums", "Forum 1");
        let forum2_key = insert_resource(&mut conn, "forums", "Forum 2");
        let forum3_key = insert_resource(&mut conn, "forums", "Forum 3");

        let holder1 = ResourceId::from(1_i64);
        let holder2 = ResourceId::from(2_i64);

        // Grant roles: holder1 has moderator on forum1 and forum2 (class-scoped)
        // holder2 has moderator on forum3 (instance-scoped)
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            // Class-scoped role for holder1
            let class_role = store
                .find_or_create_by(
                    conn,
                    &RoleName::from("moderator"),
                    ResourceRef::Class("Forum"),
                )
                .unwrap();
            store.add(conn, &holder1, &class_role).unwrap();

            // Instance-scoped role for holder2 on forum3
            let inst_role = store
                .find_or_create_by(
                    conn,
                    &RoleName::from("moderator"),
                    ResourceRef::Instance("Forum", &forum3_key.resource_id),
                )
                .unwrap();
            store.add(conn, &holder2, &inst_role).unwrap();
            Ok(())
        })
        .unwrap();

        let candidates = vec![forum1_key.clone(), forum2_key.clone(), forum3_key.clone()];

        // holder1 should see forum1, forum2, and forum3: the
        // class-scoped role covers every instance (the gem's `in`
        // filter matches NULL-scoped rows against all candidates,
        // resource_adapter.rb:27-30).
        let holder1_results = store
            .in_list(
                &mut conn,
                &candidates,
                &holder1,
                &[RoleName::from("moderator")],
            )
            .unwrap();
        let holder1_ids: HashSet<_> = holder1_results
            .iter()
            .map(|k| k.resource_id.as_str())
            .collect();
        assert_eq!(holder1_results.len(), 3);
        assert!(holder1_ids.contains(&forum1_key.resource_id.as_str()));
        assert!(holder1_ids.contains(&forum2_key.resource_id.as_str()));
        assert!(holder1_ids.contains(&forum3_key.resource_id.as_str()));

        // holder2 should see only forum3 (instance-scoped)
        let holder2_results = store
            .in_list(
                &mut conn,
                &candidates,
                &holder2,
                &[RoleName::from("moderator")],
            )
            .unwrap();
        assert_eq!(holder2_results.len(), 1);
        assert_eq!(holder2_results[0].resource_id, forum3_key.resource_id);
    }

    #[test]
    fn holders_where_distinguishes_strict_vs_non_strict() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        let holder1 = ResourceId::from(1_i64);
        let holder2 = ResourceId::from(2_i64);

        // holder1: global admin role
        // holder2: class-scoped admin on Forum
        // The holder rows must exist: the holder-table scan finds no
        // link-only identities.
        insert_holder(&mut conn, "users", "User", "holder1");
        insert_holder(&mut conn, "users", "User", "holder2");
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let global_role = store
                .find_or_create_by(conn, &RoleName::from("admin"), ResourceRef::Global)
                .unwrap();
            store.add(conn, &holder1, &global_role).unwrap();

            let class_role = store
                .find_or_create_by(conn, &RoleName::from("admin"), ResourceRef::Class("Forum"))
                .unwrap();
            store.add(conn, &holder2, &class_role).unwrap();
            Ok(())
        })
        .unwrap();

        // Non-strict: global role matches class query (override ladder)
        let non_strict = store
            .holders_where(
                &mut conn,
                &["User"],
                &RoleQuery::with_role_and_filter(
                    &RoleName::from("admin"),
                    ResourceFilter::Class("Forum"),
                ),
                false,
            )
            .unwrap();
        let non_strict_ids: HashSet<_> = non_strict.iter().map(|id| id.as_str()).collect();
        assert_eq!(non_strict.len(), 2);
        assert!(non_strict_ids.contains("1"));
        assert!(non_strict_ids.contains("2"));

        // Strict: only exact class-scoped matches
        let strict = store
            .holders_where(
                &mut conn,
                &["User"],
                &RoleQuery::with_role_and_filter(
                    &RoleName::from("admin"),
                    ResourceFilter::Class("Forum"),
                ),
                true,
            )
            .unwrap();
        let strict_ids: HashSet<_> = strict.iter().map(|id| id.as_str()).collect();
        assert_eq!(strict.len(), 1);
        assert!(strict_ids.contains("2"));
        assert!(!strict_ids.contains("1"));
    }

    #[test]
    fn all_holders_includes_never_rolified_users() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        // The granted holder needs its row (id 1 on the restarted
        // sequence) plus two never-rolified rows for the universe.
        insert_holder(&mut conn, "users", "User", "holder1");
        insert_holder(&mut conn, "users", "User", "never_rolified_1");
        insert_holder(&mut conn, "users", "User", "never_rolified_2");

        // Grant role to one user
        let holder1 = ResourceId::from(1_i64);
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let role = store
                .find_or_create_by(conn, &RoleName::from("admin"), ResourceRef::Global)
                .unwrap();
            store.add(conn, &holder1, &role).unwrap();
            Ok(())
        })
        .unwrap();

        // all_holders should return all users including never-rolified
        let all = store.all_holders(&mut conn, &["User"]).unwrap();
        let all_ids: HashSet<_> = all.iter().map(|id| id.as_str()).collect();

        // Should have at least 3 users (original + 2 never_rolified)
        assert!(all.len() >= 3);
        assert!(all_ids.contains("1"));
        // The never_rolified users have auto-generated IDs, check they're included
    }

    #[test]
    fn roles_matching_honors_name_scope_holder_filters() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        let holder1 = ResourceId::from(1_i64);

        // The holder-filtered query joins the holder table: the row
        // must exist.
        insert_holder(&mut conn, "users", "User", "holder1");
        // The instance-scoped editor below sits on this forum; the key
        // lives outside the transaction so later filters reuse it.
        let forum_key = insert_resource(&mut conn, "forums", "Test Forum");

        // Create various roles
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            // Global admin
            let r1 = store
                .find_or_create_by(conn, &RoleName::from("admin"), ResourceRef::Global)
                .unwrap();
            store.add(conn, &holder1, &r1).unwrap();

            // Class-scoped moderator on Forum
            let r2 = store
                .find_or_create_by(
                    conn,
                    &RoleName::from("moderator"),
                    ResourceRef::Class("Forum"),
                )
                .unwrap();
            store.add(conn, &holder1, &r2).unwrap();

            // Instance-scoped editor on the forum above
            let r3 = store
                .find_or_create_by(
                    conn,
                    &RoleName::from("editor"),
                    ResourceRef::Instance("Forum", &forum_key.resource_id),
                )
                .unwrap();
            store.add(conn, &holder1, &r3).unwrap();

            // Another global role (viewer)
            let r4 = store
                .find_or_create_by(conn, &RoleName::from("viewer"), ResourceRef::Global)
                .unwrap();
            store.add(conn, &holder1, &r4).unwrap();

            Ok(())
        })
        .unwrap();

        // Filter by name (the name binding outlives the query it feeds)
        let moderator_name = RoleName::from("moderator");
        let query = RoleCatalogQuery::for_types(&["Forum"]).with_name(&moderator_name);
        let results = store.roles_matching(&mut conn, &query).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name.as_str(), "moderator");
        assert!(results[0].is_class_scoped_to("Forum"));

        // Filter by scope: ClassOnly
        let query = RoleCatalogQuery::for_types(&["Forum"]).class_only();
        let results = store.roles_matching(&mut conn, &query).unwrap();
        let class_roles: Vec<_> = results
            .iter()
            .filter(|r| r.is_class_scoped_to("Forum"))
            .collect();
        assert_eq!(class_roles.len(), 1);
        assert_eq!(class_roles[0].name.as_str(), "moderator");

        // Filter by scope: InstanceOnly (specific instance).
        // The editor row granted above sits on this same forum: the
        // query must reuse its key (a fresh insert would point at an
        // unrelated instance).
        let query =
            RoleCatalogQuery::for_types(&["Forum"]).instance_only(Some(&forum_key.resource_id));
        let results = store.roles_matching(&mut conn, &query).unwrap();
        let inst_roles: Vec<_> = results
            .iter()
            .filter(|r| r.is_instance_scoped_to("Forum", &forum_key.resource_id))
            .collect();
        assert_eq!(inst_roles.len(), 1);
        assert_eq!(inst_roles[0].name.as_str(), "editor");

        // Filter by holder
        let query = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&holder1);
        let results = store.roles_matching(&mut conn, &query).unwrap();
        assert_eq!(results.len(), 2); // moderator (class) + editor (instance)
    }

    #[test]
    fn scoped_delete_removes_only_instance_bound_rows() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        let mut store = make_store();

        let forum_key = insert_resource(&mut conn, "forums", "Test Forum");
        let forum_id = forum_key.resource_id.clone();

        // Create roles: one instance-scoped, one class-scoped on same type
        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let inst_role = store
                .find_or_create_by(
                    conn,
                    &RoleName::from("moderator"),
                    ResourceRef::Instance("Forum", &forum_id),
                )
                .unwrap();
            store
                .add(conn, &ResourceId::from(1_i64), &inst_role)
                .unwrap();

            let class_role = store
                .find_or_create_by(conn, &RoleName::from("admin"), ResourceRef::Class("Forum"))
                .unwrap();
            store
                .add(conn, &ResourceId::from(1_i64), &class_role)
                .unwrap();

            Ok(())
        })
        .unwrap();

        // Verify both roles exist before delete
        let before = store
            .roles_matching(&mut conn, &RoleCatalogQuery::for_types(&["Forum"]))
            .unwrap();
        assert_eq!(before.len(), 2);

        // Delete instance-scoped roles for this forum
        let deleted = store
            .remove_roles_for_scope(&mut conn, "Forum", &forum_id)
            .unwrap();
        assert_eq!(deleted, 1);

        // Verify only instance-scoped role was deleted, class-scoped survives
        let after = store
            .roles_matching(&mut conn, &RoleCatalogQuery::for_types(&["Forum"]))
            .unwrap();
        assert_eq!(after.len(), 1);
        assert!(after[0].is_class_scoped_to("Forum"));
        assert_eq!(after[0].name.as_str(), "admin");
    }

    #[test]
    fn custom_join_role_tables_work_end_to_end() {
        let _container = pg_container();
        let mut conn = pg_conn();
        // Serialized with the other tests sharing this database.
        let _serial = crate::support::SuiteGuard::acquire();
        run_migrations(&mut conn);
        setup_fixtures(&mut conn);
        reset_roles(&mut conn);
        reset_fixtures(&mut conn);

        // Create custom pair: custom_roles / custom_role_links.
        // The tables are created here (roles-shaped role table plus a
        // two-column join): the shared `rights` fixture table carries
        // only (id, name) and cannot serve as a role table, per the
        // gem's schema where custom role tables keep the full
        // name-plus-polymorphic-reference shape (schema.rb:4-10).
        diesel::sql_query(
            "CREATE TABLE custom_roles (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                resource_type VARCHAR(191) NOT NULL DEFAULT '',
                resource_id VARCHAR(191) NOT NULL DEFAULT '',
                CONSTRAINT custom_roles_triple_unique UNIQUE (name, resource_type, resource_id)
            )",
        )
        .execute(&mut conn)
        .unwrap();
        diesel::sql_query(
            "CREATE TABLE custom_role_links (
                user_id VARCHAR(191) NOT NULL,
                role_id BIGINT NOT NULL REFERENCES custom_roles(id) ON DELETE CASCADE,
                CONSTRAINT custom_role_links_pair_unique UNIQUE (user_id, role_id)
            )",
        )
        .execute(&mut conn)
        .unwrap();
        let mut custom_store = DieselStore::with_tables("custom_roles", "custom_role_links")
            .for_holder_table("users")
            .register_resource_table("Forum", "forums", "id");

        let forum_key = insert_resource(&mut conn, "forums", "Custom Forum");
        let holder = ResourceId::from(1_i64);

        conn.transaction::<_, diesel::result::Error, _>(|conn| {
            let role = custom_store
                .find_or_create_by(
                    conn,
                    &RoleName::from("moderator"),
                    ResourceRef::Instance("Forum", &forum_key.resource_id),
                )
                .unwrap();
            custom_store.add(conn, &holder, &role).unwrap();
            Ok(())
        })
        .unwrap();

        // Verify the role was created and linked
        let results = custom_store
            .resources_find(&mut conn, &["Forum"], &RoleName::from("moderator"))
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].resource_id, forum_key.resource_id);

        // Verify in_list works with custom tables
        let in_results = custom_store
            .in_list(
                &mut conn,
                &[forum_key.clone()],
                &holder,
                &[RoleName::from("moderator")],
            )
            .unwrap();
        assert_eq!(in_results.len(), 1);
    }
}
