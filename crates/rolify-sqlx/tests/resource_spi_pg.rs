//! Postgres acceptance suite for the resource-side SPI (04-04, ADPT-04):
//! the gem's `resource.rb` / `resource_adapter.rb` semantics proven on a
//! real postgres:17 container through the PUBLIC [`Resource`] provided
//! statics (`with_role` / `find_roles` / `applied_roles` /
//! `roles_of_instance` / `applied_roles_of_instance`), the same surface
//! consumers use.
//!
//! The matrix mirrors `rolify-diesel/tests/resource_spi_pg.rs` (the
//! 03-03 acceptance shape) and the portable-suite pin structure of
//! `rolify-test`'s `resource_reads` / `resource_queries` modules:
//!
//! - (a) the gem's join asymmetry (resource_adapter.rb:21-23), both
//!   directions: a class-scoped role covers every instance key of the
//!   family while a global row never surfaces one (the family join
//!   constrains `resource_type IN (..)`, which the sentinel type can
//!   never match);
//! - (b) the `find_roles` name/user `:any` matrix (resource.rb:8-10):
//!   the catalog branch (user `None`) and the holder-association branch
//!   (user `Some`, `resource_adapter.rb:7`), each with name `None`
//!   (`:any`) or a concrete name;
//! - (c) `applied_roles`: instance form composes instance-bound plus
//!   class-scoped rows (resource.rb:44-47), and the class-level children
//!   flag honors STI `descendant_types()` (resource_adapter.rb:32-38)
//!   with the parent-class row covering child-typed table rows through
//!   the registry-driven expansion;
//! - (d) instance `roles` returns only instance-bound rows
//!   (resource_spec.rb:481-496), class rows excluded, every holder's
//!   instance rows included;
//! - (e) string primary keys: grants on a `Team` row keyed by its
//!   `team_code` read back through the resource-side statics
//!   (resource_spec.rb:144-150).
//!
//! Plus one raw-SPI supplement: [`RoleStore::roles_matching`] with a
//! holder filter, pinning the text cast on the holder join against
//! integer fixture primary keys (the 04-04 deviation fix - the public
//! statics never route through the holder branch, so the case calls the
//! store directly, mirroring the diesel reference's holder cell).
//!
//! Results compare as SETS (D-04): no `ORDER BY` anywhere. All cases
//! serialize through [`support::suite_lock`] and start from truncated
//! role + fixture tables.

#![cfg(feature = "postgres")]

// The shared support module ships helpers for several test binaries;
// this binary uses the Postgres resource-side subset (the unused-arm
// precedent of tests/tracer_pg.rs).
#[allow(dead_code)]
mod support;

mod tests {
    use rolify_core::catalog::RoleCatalogQuery;
    use rolify_core::config::RolifyConfig;
    use rolify_core::manager::Rolify;
    use rolify_core::resource::{Resource, ResourceRef};
    use rolify_core::role::{ResourceId, RoleName, RoleRecord};
    use rolify_core::store::{ResourceKey, RoleStore};
    use rolify_core::user::RolifyUser;
    use rolify_sqlx::SqlxStore;
    use rolify_test::fixtures::{Company, Forum, Organization, Team};

    use crate::support::{
        FixtureUser, apply_migrations_pg, insert_holder_pg, insert_organization_pg,
        insert_resource_pg, insert_team_pg, pg_conn, pg_pool, reset_fixtures_pg, reset_roles_pg,
        setup_fixtures_pg, suite_lock,
    };

    /// Assert two resource-key lists as SETS (D-04): sort plus dedup the
    /// `(type, id)` pairs on both sides, then equality (the portable
    /// suite's `assert_key_set` idiom, `resource_queries.rs`).
    fn assert_key_set(actual: &[ResourceKey], expected: &[ResourceKey]) {
        let mut sorted_actual: Vec<(&str, &str)> = actual
            .iter()
            .map(|key| (key.resource_type.as_str(), key.resource_id.as_str()))
            .collect();
        sorted_actual.sort_unstable();
        sorted_actual.dedup();
        let mut sorted_expected: Vec<(&str, &str)> = expected
            .iter()
            .map(|key| (key.resource_type.as_str(), key.resource_id.as_str()))
            .collect();
        sorted_expected.sort_unstable();
        sorted_expected.dedup();
        assert_eq!(
            sorted_actual, sorted_expected,
            "resource query results compare as (type, id) sets (D-04); actual {actual:?} expected {expected:?}"
        );
    }

    /// Assert two role-record lists as sets (D-04): same length, every
    /// expected row present, order ignored (the portable suite's
    /// `assert_record_set` idiom, `resource_reads.rs`).
    fn assert_record_set(found: &[RoleRecord], expected: &[RoleRecord]) {
        assert_eq!(
            found.len(),
            expected.len(),
            "row count mismatch; found {found:?}, expected {expected:?}"
        );
        for record in expected {
            assert!(
                found.contains(record),
                "missing expected row {record:?} in {found:?}"
            );
        }
    }

    /// Boot the container database into a clean slate and build the
    /// read engine over its own direct connection (migrations + the
    /// D-06 fixture DDL arrays, then both resets). The suite lock is
    /// acquired FIRST and returned so the case body stays serialized.
    #[expect(clippy::type_complexity)]
    async fn boot() -> (
        tokio::sync::MutexGuard<'static, ()>,
        sqlx::PgPool,
        Rolify<SqlxStore<sqlx::Postgres>>,
    ) {
        let serial = suite_lock().await;
        let pool = pg_pool().await;
        apply_migrations_pg(&pool).await;
        setup_fixtures_pg(&pool).await;
        reset_roles_pg(&pool).await;
        reset_fixtures_pg(&pool).await;

        let config = RolifyConfig::default();
        let store: SqlxStore<sqlx::Postgres> = SqlxStore::new(&config)
            .for_holder_table("users")
            .register_resource_table("Forum", "forums", "id")
            .register_resource_table("Group", "groups", "id")
            .register_resource_table("Team", "teams", "team_code")
            .register_resource_table("Organization", "organizations", "id")
            .register_resource_table("Company", "organizations", "id");
        let engine = Rolify::new(store, pg_conn().await, config);
        (serial, pool, engine)
    }

    /// Seat a fixture holder: insert the `users` row (the holder-table
    /// scan of the finder reads finds no link-only identities) and
    /// return its id plus the public [`RolifyUser`] handle that grants
    /// through `add_role` (the consumer surface; Config callbacks ride
    /// along exactly like a real consumer's grants).
    async fn seat_user(name: &str) -> (ResourceId, FixtureUser<sqlx::Postgres>) {
        let mut seed_conn = pg_conn().await;
        let holder_id = insert_holder_pg(&mut seed_conn, "users", "User", name).await;
        let config = RolifyConfig::default();
        let store: SqlxStore<sqlx::Postgres> = SqlxStore::new(&config);
        let user = FixtureUser::new(holder_id.clone(), store, pg_conn().await, config);
        (holder_id, user)
    }

    /// Case (a): the join asymmetry, both directions - a CLASS-scoped
    /// role covers every instance key of the family while a GLOBAL row
    /// of any name never surfaces one (resource_adapter.rb:21-23).
    #[tokio::test]
    async fn class_scope_covers_instances_and_global_is_excluded() {
        let (_serial, _pool, mut engine) = boot().await;

        let mut seed = pg_conn().await;
        let forum_first = insert_resource_pg(&mut seed, "forums", "Forum", "forum-first").await;
        let forum_second = insert_resource_pg(&mut seed, "forums", "Forum", "forum-second").await;
        let forum_last = insert_resource_pg(&mut seed, "forums", "Forum", "forum-last").await;
        let universe = vec![forum_first, forum_second, forum_last];

        let (_admin_id, mut admin) = seat_user("admin").await;
        let watcher = RoleName::from("watcher");
        let curator = RoleName::from("curator");
        admin
            .add_role(&watcher, ResourceRef::Global)
            .await
            .expect("grant global watcher");
        admin
            .add_role(&curator, ResourceRef::Class("Forum"))
            .await
            .expect("grant class curator on Forum");

        // Global excluded by the join: the sentinel type never matches
        // the family filter.
        let found = Forum::with_role(&mut engine, std::slice::from_ref(&watcher), None)
            .await
            .expect("with_role watcher");
        assert!(found.is_empty(), "a global row surfaces no resources");

        // Class covers every instance key of the family.
        let found = Forum::with_role(&mut engine, std::slice::from_ref(&curator), None)
            .await
            .expect("with_role curator");
        assert_key_set(&found, &universe);

        // Complements pin both directions: the class row empties the
        // universe; the global row excludes nothing.
        let found =
            Forum::without_role(&mut engine, std::slice::from_ref(&curator), None, &universe)
                .await
                .expect("without_role curator");
        assert!(found.is_empty(), "the class row empties the universe");
        let found = Forum::without_role(&mut engine, &[watcher], None, &universe)
            .await
            .expect("without_role watcher");
        assert_key_set(&found, &universe);
    }

    /// Case (b): the `find_roles` name/user `:any` matrix
    /// (resource.rb:8-10): catalog branch (user `None`) and the
    /// holder-association branch (user `Some`), each with and without
    /// a name. A same-name GLOBAL row never surfaces in any cell.
    #[tokio::test]
    async fn find_roles_answers_the_name_user_any_matrix() {
        let (_serial, _pool, mut engine) = boot().await;

        let mut seed = pg_conn().await;
        let forum_first = insert_resource_pg(&mut seed, "forums", "Forum", "forum-first").await;
        let forum_last = insert_resource_pg(&mut seed, "forums", "Forum", "forum-last").await;

        let (admin_id, mut admin) = seat_user("admin").await;
        let (zombie_id, mut zombie) = seat_user("zombie").await;
        let forum = RoleName::from("forum");
        let godfather = RoleName::from("godfather");
        admin
            .add_role(
                &forum,
                ResourceRef::Instance("Forum", &forum_first.resource_id),
            )
            .await
            .expect("admin: instance forum on Forum first");
        admin
            .add_role(&godfather, ResourceRef::Class("Forum"))
            .await
            .expect("admin: class godfather on Forum");
        zombie
            .add_role(
                &forum,
                ResourceRef::Instance("Forum", &forum_last.resource_id),
            )
            .await
            .expect("zombie: instance forum on Forum last");
        // A global row sharing the name of an instance grant: the
        // resource-side reads structurally exclude it.
        let sneaky = RoleName::from("sneaky");
        admin
            .add_role(&sneaky, ResourceRef::Global)
            .await
            .expect("admin: global sneaky");

        let admin_forum_first =
            RoleRecord::for_instance("forum", "Forum", forum_first.resource_id.as_str());
        let admin_godfather_class = RoleRecord::for_class("godfather", "Forum");
        let zombie_forum_last =
            RoleRecord::for_instance("forum", "Forum", forum_last.resource_id.as_str());

        // (None, None): the whole Forum family regardless of holder.
        let rows = Forum::find_roles(&mut engine, None, None)
            .await
            .expect("find_roles any/any");
        assert_record_set(
            &rows,
            &[
                admin_forum_first.clone(),
                admin_godfather_class.clone(),
                zombie_forum_last.clone(),
            ],
        );
        assert!(
            rows.iter().all(|row| !row.is_global()),
            "the catalog branch never returns global rows"
        );

        // (Some name, None): the name narrows the family read.
        let rows = Forum::find_roles(&mut engine, Some(&forum), None)
            .await
            .expect("find_roles forum/any");
        assert_record_set(
            &rows,
            &[admin_forum_first.clone(), zombie_forum_last.clone()],
        );

        // (None, Some user): only the holder's rows, still inside the
        // family - the zombie's Forum row and the global row stay out.
        let rows = Forum::find_roles(&mut engine, None, Some(&admin_id))
            .await
            .expect("find_roles any/admin");
        assert_record_set(
            &rows,
            &[admin_forum_first.clone(), admin_godfather_class.clone()],
        );

        // (Some name, Some user): the intersection.
        let rows = Forum::find_roles(&mut engine, Some(&godfather), Some(&admin_id))
            .await
            .expect("find_roles godfather/admin");
        assert_record_set(&rows, std::slice::from_ref(&admin_godfather_class));

        // The holder branch never sees a global row of the same name:
        // the family filter drops it before the name filter runs.
        let rows = Forum::find_roles(&mut engine, Some(&sneaky), Some(&admin_id))
            .await
            .expect("find_roles sneaky/admin");
        assert!(
            rows.is_empty(),
            "the global sneaky row is invisible to the resource family"
        );

        // Sanity: the other holder really sits on the last forum.
        let rows = Forum::find_roles(&mut engine, Some(&forum), Some(&zombie_id))
            .await
            .expect("find_roles forum/zombie");
        assert_record_set(&rows, std::slice::from_ref(&zombie_forum_last));
    }

    /// Case (c) part 1: instance `applied_roles` composes the
    /// instance-bound rows UNION the class-scoped rows of the family
    /// (`self.roles + self.class.applied_roles(true)`,
    /// resource.rb:44-47), set-deduped, with the other instance's rows
    /// staying out.
    #[tokio::test]
    async fn applied_roles_of_instance_composes_class_and_instance() {
        let (_serial, _pool, mut engine) = boot().await;

        let mut seed = pg_conn().await;
        let forum_first = insert_resource_pg(&mut seed, "forums", "Forum", "forum-first").await;
        let forum_second = insert_resource_pg(&mut seed, "forums", "Forum", "forum-second").await;

        let (_admin_id, mut admin) = seat_user("admin").await;
        let (_zombie_id, mut zombie) = seat_user("zombie").await;
        admin
            .add_role(
                &RoleName::from("forum"),
                ResourceRef::Instance("Forum", &forum_first.resource_id),
            )
            .await
            .expect("admin: instance forum on Forum first");
        admin
            .add_role(&RoleName::from("godfather"), ResourceRef::Class("Forum"))
            .await
            .expect("admin: class godfather on Forum");
        zombie
            .add_role(
                &RoleName::from("roomate"),
                ResourceRef::Instance("Forum", &forum_second.resource_id),
            )
            .await
            .expect("zombie: instance roomate on Forum second");

        let first = Forum {
            id: forum_first.resource_id.clone(),
            name: "Forum First".to_owned(),
        };
        let rows = Forum::applied_roles_of_instance(&mut engine, &first)
            .await
            .expect("applied_roles of Forum first");
        assert_record_set(
            &rows,
            &[
                RoleRecord::for_instance("forum", "Forum", forum_first.resource_id.as_str()),
                RoleRecord::for_class("godfather", "Forum"),
            ],
        );
    }

    /// Case (c) part 2: the class-level children flag honors
    /// `descendant_types()` (resource_adapter.rb:32-38): a class row on
    /// the CHILD type is visible from the parent's `applied_roles` only
    /// when children is set, a class row on the PARENT type is visible
    /// either way, and the parent's class row covers child-typed rows
    /// of the shared STI table through the registry class expansion.
    #[tokio::test]
    async fn applied_roles_children_flag_honors_sti_family() {
        let (_serial, _pool, mut engine) = boot().await;

        // One row per STI type on the shared organizations table.
        let mut seed = pg_conn().await;
        let organization = insert_organization_pg(&mut seed, "Organization").await;
        let company = insert_organization_pg(&mut seed, "Company").await;

        let (_admin_id, mut admin) = seat_user("admin").await;
        admin
            .add_role(
                &RoleName::from("keeper"),
                ResourceRef::Class("Organization"),
            )
            .await
            .expect("admin: class keeper on the STI parent");
        admin
            .add_role(&RoleName::from("hr"), ResourceRef::Class("Company"))
            .await
            .expect("admin: class hr on the STI child");

        let keeper_row = RoleRecord::for_class("keeper", "Organization");
        let hr_row = RoleRecord::for_class("hr", "Company");

        // children = true: the family is [Organization, Company], so
        // both class rows surface.
        let rows = Organization::applied_roles(&mut engine, true)
            .await
            .expect("applied_roles with children");
        assert_record_set(&rows, &[keeper_row.clone(), hr_row.clone()]);

        // children = false: exactly the parent type, so the child-typed
        // class row drops out.
        let rows = Organization::applied_roles(&mut engine, false)
            .await
            .expect("applied_roles without children");
        assert_record_set(&rows, std::slice::from_ref(&keeper_row));

        // The child's own class read sees its own row only.
        let rows = Company::applied_roles(&mut engine, true)
            .await
            .expect("Company applied_roles with children");
        assert_record_set(&rows, std::slice::from_ref(&hr_row));

        // The registry class expansion: the parent's class row covers
        // EVERY row of the shared organizations table, including the
        // child-typed one, surfaced as parent-typed keys.
        let found = Organization::with_role(&mut engine, &[RoleName::from("keeper")], None)
            .await
            .expect("with_role keeper over the STI family");
        assert_key_set(
            &found,
            &[
                ResourceKey::new("Organization", organization.resource_id.clone()),
                ResourceKey::new("Organization", company.resource_id.clone()),
            ],
        );
    }

    /// Case (d): instance `roles` returns only instance-bound rows
    /// (resource_spec.rb:481-496): BOTH holders' rows on the instance,
    /// the class row excluded.
    #[tokio::test]
    async fn instance_roles_returns_only_instance_bound_rows() {
        let (_serial, _pool, mut engine) = boot().await;

        let mut seed = pg_conn().await;
        let forum_first = insert_resource_pg(&mut seed, "forums", "Forum", "forum-first").await;

        let (_admin_id, mut admin) = seat_user("admin").await;
        let (_zombie_id, mut zombie) = seat_user("zombie").await;
        admin
            .add_role(
                &RoleName::from("forum"),
                ResourceRef::Instance("Forum", &forum_first.resource_id),
            )
            .await
            .expect("admin: instance forum on Forum first");
        zombie
            .add_role(
                &RoleName::from("group"),
                ResourceRef::Instance("Forum", &forum_first.resource_id),
            )
            .await
            .expect("zombie: instance group on Forum first (the sneaky name)");
        admin
            .add_role(&RoleName::from("godfather"), ResourceRef::Class("Forum"))
            .await
            .expect("admin: class godfather on Forum");

        let first = Forum {
            id: forum_first.resource_id.clone(),
            name: "Forum First".to_owned(),
        };
        let rows = Forum::roles_of_instance(&mut engine, &first)
            .await
            .expect("roles of Forum first");
        assert_record_set(
            &rows,
            &[
                RoleRecord::for_instance("forum", "Forum", forum_first.resource_id.as_str()),
                RoleRecord::for_instance("group", "Forum", forum_first.resource_id.as_str()),
            ],
        );
    }

    /// Case (e): the string-PK family (resource_spec.rb:144-150): a
    /// `Team` row is keyed by its `team_code` string, grants ride it
    /// untouched, and the registry class expansion reads every team
    /// row through the string pk column.
    #[tokio::test]
    async fn team_string_pk_reads_round_trip() {
        let (_serial, _pool, mut engine) = boot().await;

        let mut seed = pg_conn().await;
        insert_team_pg(&mut seed, "alpha", "Team Alpha").await;
        insert_team_pg(&mut seed, "beta", "Team Beta").await;
        let team_alpha = ResourceKey::new("Team", "alpha");
        let team_beta = ResourceKey::new("Team", "beta");

        let (god_id, mut god) = seat_user("god").await;
        god.add_role(
            &RoleName::from("captain"),
            ResourceRef::Instance("Team", &team_alpha.resource_id),
        )
        .await
        .expect("god: captain on Team alpha");
        god.add_role(&RoleName::from("member"), ResourceRef::Class("Team"))
            .await
            .expect("god: class member on Team");

        // Instance read: the stringified PK travels untouched.
        let found = Team::with_role(&mut engine, &[RoleName::from("captain")], None)
            .await
            .expect("with_role captain");
        assert_key_set(&found, std::slice::from_ref(&team_alpha));

        // Class expansion over the string pk column covers both teams.
        let found = Team::with_role(&mut engine, &[RoleName::from("member")], None)
            .await
            .expect("with_role member");
        assert_key_set(&found, &[team_alpha.clone(), team_beta.clone()]);

        // The holder filter (`in_list`) over string-keyed candidates.
        let found = Team::with_role(&mut engine, &[RoleName::from("captain")], Some(&god_id))
            .await
            .expect("with_role captain and god");
        assert_key_set(&found, std::slice::from_ref(&team_alpha));

        let alpha_instance = Team {
            team_code: team_alpha.resource_id.as_str().to_owned(),
            name: "Team Alpha".to_owned(),
        };
        let rows = Team::roles_of_instance(&mut engine, &alpha_instance)
            .await
            .expect("roles of Team alpha");
        assert_record_set(
            &rows,
            &[RoleRecord::for_instance("captain", "Team", "alpha")],
        );
    }

    /// Raw-SPI supplement pinning the 04-04 holder-join fix:
    /// [`RoleStore::roles_matching`] with a holder joins the integer-keyed
    /// fixture holder table against the text link column; without the
    /// `cast_to_text` on the join and the filter, Postgres rejects the
    /// comparison (`integer = text`). The public statics never route
    /// through the holder branch (the find_roles user leg reads
    /// `roles_of` instead), so the pin calls the store directly, like
    /// the diesel reference's holder cell (resource_spi_pg.rs there).
    #[tokio::test]
    async fn roles_matching_holder_join_casts_integer_holder_ids() {
        let (_serial, _pool, mut engine) = boot().await;

        let mut seed = pg_conn().await;
        let forum_first = insert_resource_pg(&mut seed, "forums", "Forum", "forum-first").await;

        let (admin_id, mut admin) = seat_user("admin").await;
        let (_zombie_id, mut zombie) = seat_user("zombie").await;
        admin
            .add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum"))
            .await
            .expect("admin: class moderator on Forum");
        admin
            .add_role(
                &RoleName::from("editor"),
                ResourceRef::Instance("Forum", &forum_first.resource_id),
            )
            .await
            .expect("admin: instance editor on Forum first");
        zombie
            .add_role(
                &RoleName::from("ghost"),
                ResourceRef::Instance("Forum", &forum_first.resource_id),
            )
            .await
            .expect("zombie: instance ghost on Forum first");

        let query = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&admin_id);
        let (store, conn) = engine.store_with_conn();
        let rows = store
            .roles_matching(conn, &query)
            .await
            .expect("holder-filtered catalog read");
        assert_record_set(
            &rows,
            &[
                RoleRecord::for_class("moderator", "Forum"),
                RoleRecord::for_instance("editor", "Forum", forum_first.resource_id.as_str()),
            ],
        );
    }

    /// `instance_only(None)` is the catalog scope's documented "every
    /// instance row in types" query: class rows stay out, instance rows
    /// stay in (the None corner of the scope filter, pinned against the
    /// same reference semantics as the diesel acceptance file and the
    /// in-memory grid).
    #[tokio::test]
    async fn roles_matching_instance_only_none_excludes_class_rows() {
        let (_serial, _pool, mut engine) = boot().await;

        let mut seed = pg_conn().await;
        let forum_first = insert_resource_pg(&mut seed, "forums", "Forum", "forum-first").await;
        let forum_second = insert_resource_pg(&mut seed, "forums", "Forum", "forum-second").await;

        let (_, mut admin) = seat_user("admin").await;
        admin
            .add_role(&RoleName::from("moderator"), ResourceRef::Class("Forum"))
            .await
            .expect("admin: class moderator on Forum");
        admin
            .add_role(
                &RoleName::from("editor"),
                ResourceRef::Instance("Forum", &forum_first.resource_id),
            )
            .await
            .expect("admin: instance editor on Forum first");
        admin
            .add_role(
                &RoleName::from("reviewer"),
                ResourceRef::Instance("Forum", &forum_second.resource_id),
            )
            .await
            .expect("admin: instance reviewer on Forum second");

        let query = RoleCatalogQuery::for_types(&["Forum"]).instance_only(None);
        let (store, conn) = engine.store_with_conn();
        let rows = store
            .roles_matching(conn, &query)
            .await
            .expect("instance_only(None) catalog read");
        assert_record_set(
            &rows,
            &[
                RoleRecord::for_instance("editor", "Forum", forum_first.resource_id.as_str()),
                RoleRecord::for_instance("reviewer", "Forum", forum_second.resource_id.as_str()),
            ],
        );
    }
}
