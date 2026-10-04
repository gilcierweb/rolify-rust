//! Tracer test: end-to-end "grant global then class then instance role,
//! read it back" on real MySQL - one path through every layer.
//!
//! This is the Phase 4 engine-expansion leg (04-03 Task 2): it mirrors the
//! Postgres tracer assertions on a real mysql:8.4 container (pinned,
//! module-readiness waits, never sleeps) through the PUBLIC [`RolifyUser`]
//! provided methods, and additionally pins the byte-exactness falsifier
//! where the collation risk lives (the DDL's `utf8mb4_bin` collation:
//! `Admin` and `admin` are two distinct role rows).
//!
//! D-13 proof, repeated per engine: the leg instantiates
//! `SqlxStore<MySql>` without spelling a single generic bound.

#![cfg(feature = "mysql")]

use rolify_core::config::RolifyConfig;
use rolify_core::kernel::RemovalTarget;
use rolify_core::query::ResourceFilter;
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName};
use rolify_core::user::RolifyUser;
use rolify_sqlx::SqlxStore;
use sqlx::Row;

use crate::support::{
    FixtureUser, apply_migrations_mysql, insert_holder_mysql, mysql_conn, mysql_pool,
    reset_roles_mysql, setup_fixtures_mysql,
};

// The shared support module ships helpers for several test binaries;
// this binary uses the MySQL subset, so the unused arms are allowed to
// sit idle here (same precedent as tests/migrations.rs).
#[allow(dead_code)]
mod support;

async fn count_rows(pool: &sqlx::MySqlPool, sql_text: &'static str) -> i64 {
    sqlx::query(sql_text)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("count query failed ({sql_text}): {error}"))
        .get::<i64, _>(0)
}

#[tokio::test]
async fn tracer_grant_check_revoke_lifecycle_on_mysql() {
    // Bootstrap: container (implicit readiness), migrations, fixtures.
    let pool = mysql_pool().await;
    apply_migrations_mysql(&pool).await;
    setup_fixtures_mysql(&pool).await;
    reset_roles_mysql(&pool).await;

    let config = RolifyConfig::default();

    // The store: generic over the engine, zero bounds spelled here (D-13).
    let mut seed_conn = mysql_conn().await;
    let user_id = insert_holder_mysql(&mut seed_conn, "users", "User", "tracer_user").await;
    let mut user = FixtureUser::new(
        user_id.clone(),
        SqlxStore::new(&config),
        mysql_conn().await,
        config.clone(),
    );

    // ============================================================
    // GRANT PHASE: global, then class, then instance (the tracer path)
    // ============================================================

    let admin = RoleName::from("admin");
    let admin_record = user
        .add_role(&admin, ResourceRef::Global)
        .await
        .expect("grant global admin");
    assert!(admin_record.is_global());
    assert_eq!(admin_record.name.as_str(), "admin");

    // Double add is idempotent: find_or_create_by returns the SAME
    // record and the join UNIQUE pair rejects the duplicate link.
    let admin_record_again = user
        .add_role(&admin, ResourceRef::Global)
        .await
        .expect("second grant of global admin");
    assert_eq!(
        admin_record, admin_record_again,
        "find_or_create_by is idempotent on the triple"
    );
    let admin_role_rows = count_rows(
        &pool,
        "SELECT COUNT(*) FROM roles WHERE name = 'admin' AND resource_type = '' AND resource_id = ''",
    )
    .await;
    assert_eq!(admin_role_rows, 1, "exactly one global admin role row");
    let admin_links: i64 = sqlx::query(
        "SELECT COUNT(*) FROM users_roles \
         WHERE user_id = ? AND role_id IN \
           (SELECT id FROM roles WHERE name = 'admin' \
             AND resource_type = '' AND resource_id = '')",
    )
    .bind(user_id.as_str())
    .fetch_one(&pool)
    .await
    .expect("count the holder's links to the global admin")
    .get(0);
    assert_eq!(admin_links, 1, "exactly one link for the global admin");

    let moderator = RoleName::from("moderator");
    let forum_42 = ResourceId::from("42");
    let moderator_class = user
        .add_role(&moderator, ResourceRef::Class("Forum"))
        .await
        .expect("grant class moderator on Forum");
    assert!(moderator_class.is_class_scoped_to("Forum"));

    let moderator_instance = user
        .add_role(&moderator, ResourceRef::Instance("Forum", &forum_42))
        .await
        .expect("grant instance moderator on Forum#42");
    assert!(moderator_instance.is_instance_scoped_to("Forum", &forum_42));

    // An instance-ONLY role: the unambiguous probe for the "reverse
    // never holds" negatives below.
    let editor = RoleName::from("editor");
    user.add_role(&editor, ResourceRef::Instance("Forum", &forum_42))
        .await
        .expect("grant instance editor on Forum#42");

    // ============================================================
    // CHECK PHASE: every RoleQuery shape through the public methods
    // ============================================================

    // Global matches the global grant.
    assert!(
        user.has_role(&admin, ResourceFilter::Global)
            .await
            .expect("has_role global admin")
    );

    // The global override: class and instance queries see the global row.
    assert!(
        user.has_role(&admin, ResourceFilter::Class("Forum"))
            .await
            .expect("has_role class admin")
    );
    assert!(
        user.has_role(&admin, ResourceFilter::Instance("Forum", &forum_42))
            .await
            .expect("has_role instance admin")
    );

    // Class matches the class grant...
    assert!(
        user.has_role(&moderator, ResourceFilter::Class("Forum"))
            .await
            .expect("has_role class moderator")
    );
    // ...and class-covers-instance: the instance query sees the class
    // grant for ANY instance id.
    assert!(
        user.has_role(&moderator, ResourceFilter::Instance("Forum", &forum_42))
            .await
            .expect("has_role instance moderator (own id)")
    );
    let forum_99 = ResourceId::from("99");
    assert!(
        user.has_role(&moderator, ResourceFilter::Instance("Forum", &forum_99))
            .await
            .expect("has_role instance moderator (any id: class covers instance)")
    );

    // Instance matches the instance grant exactly.
    assert!(
        user.has_role(&editor, ResourceFilter::Instance("Forum", &forum_42))
            .await
            .expect("has_role instance editor")
    );
    // Reverse never holds: an instance row is not the class row...
    assert!(
        !user
            .has_role(&editor, ResourceFilter::Class("Forum"))
            .await
            .expect("has_role class editor is false")
    );
    // ...and a different instance id matches nothing.
    assert!(
        !user
            .has_role(&editor, ResourceFilter::Instance("Forum", &forum_99))
            .await
            .expect("has_role instance editor on the wrong id is false")
    );

    // Any includes the global grant (D-02: the gem's database path).
    assert!(
        user.has_role(&admin, ResourceFilter::Any)
            .await
            .expect("has_role any admin")
    );
    let ghost = RoleName::from("ghost");
    assert!(
        !user
            .has_role(&ghost, ResourceFilter::Any)
            .await
            .expect("has_role any ghost is false")
    );

    // Strict checks match exactly and nothing more.
    assert!(
        user.has_strict_role(&moderator, ResourceFilter::Class("Forum"))
            .await
            .expect("strict class moderator")
    );
    assert!(
        !user
            .has_strict_role(&admin, ResourceFilter::Class("Forum"))
            .await
            .expect("strict class does not see the global row")
    );
    assert!(
        user.has_strict_role(&moderator, ResourceFilter::Instance("Forum", &forum_42))
            .await
            .expect("strict instance moderator")
    );
    assert!(
        !user
            .has_strict_role(&moderator, ResourceFilter::Instance("Forum", &forum_99))
            .await
            .expect("strict instance on the wrong id is false")
    );
    assert!(
        !user
            .has_strict_role(&moderator, ResourceFilter::Global)
            .await
            .expect("strict global does not see the class row")
    );

    // One OR-joined round-trip for has_any (never N sequential checks).
    assert!(
        user.has_any_roles(&[
            rolify_core::query::RoleQuery {
                name: &ghost,
                filter: ResourceFilter::Global,
            },
            rolify_core::query::RoleQuery {
                name: &admin,
                filter: ResourceFilter::Global,
            },
        ])
        .await
        .expect("has_any_roles finds admin among ghost")
    );
    assert!(
        !user
            .has_any_roles(&[])
            .await
            .expect("has_any_roles over no queries is false")
    );

    // The whole association, compared as a SET (no ORDER BY anywhere):
    // four links - global admin, class moderator, instance moderator,
    // instance editor - so the name multiset carries moderator twice.
    let mut names: Vec<String> = user
        .roles_name()
        .await
        .expect("roles_name")
        .into_iter()
        .map(|name| name.as_str().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["admin", "editor", "moderator", "moderator"],
        "one name per linked role row"
    );
    assert!(
        !user
            .only_has_role(&admin, ResourceFilter::Global)
            .await
            .expect("only_has_role is false while the holder has several roles")
    );

    // ============================================================
    // REVOKE PHASE: exact, then the -2 cross-scope TypeSweep
    // ============================================================

    // Exact target: one link, the orphan row destroyed (no other holder).
    let outcome = user
        .revoke(&editor, RemovalTarget::Exact("Forum", &forum_42))
        .await
        .expect("revoke editor exactly");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(outcome.removed_roles.len(), 1);
    assert_eq!(outcome.removed_roles[0].name.as_str(), "editor");
    assert!(
        !user
            .has_role(&editor, ResourceFilter::Any)
            .await
            .expect("editor fully revoked")
    );

    // TypeSweep: the class AND the instance moderator rows fall in one
    // sweep: -2 links, -2 orphan rows.
    let outcome = user
        .revoke(&moderator, RemovalTarget::TypeSweep("Forum"))
        .await
        .expect("revoke moderator with the cross-scope sweep");
    assert_eq!(outcome.removed_links, 2, "class + instance links swept");
    assert_eq!(outcome.removed_roles.len(), 2, "both orphan rows destroyed");
    let mut swept_names: Vec<&str> = outcome
        .removed_roles
        .iter()
        .map(|record| record.name.as_str())
        .collect();
    swept_names.sort_unstable();
    assert_eq!(swept_names, ["moderator", "moderator"]);
    assert!(
        !user
            .has_role(&moderator, ResourceFilter::Any)
            .await
            .expect("moderator fully revoked")
    );
    assert!(
        user.has_role(&admin, ResourceFilter::Any)
            .await
            .expect("admin survives the moderator sweep")
    );

    // A repeated remove removes nothing (idempotence edge).
    let outcome = user
        .revoke(&moderator, RemovalTarget::TypeSweep("Forum"))
        .await
        .expect("repeated revoke succeeds");
    assert_eq!(outcome.removed_links, 0, "no links left to remove");
    assert!(outcome.removed_roles.is_empty());

    // Only the global admin remains.
    assert!(
        user.only_has_role(&admin, ResourceFilter::Global)
            .await
            .expect("only_has_role is true once admin is the only role")
    );

    // ============================================================
    // ORPHAN SEMANTICS: the row survives while another holder has it
    // ============================================================

    let mut other_seed_conn = mysql_conn().await;
    let other_id = insert_holder_mysql(&mut other_seed_conn, "users", "User", "tracer_other").await;
    let mut other_user = FixtureUser::new(
        other_id,
        SqlxStore::new(&config),
        mysql_conn().await,
        config.clone(),
    );

    // The second holder links the SAME global admin row (level-1 dedupe
    // across holders, then the level-2 link).
    let shared_record = other_user
        .add_role(&admin, ResourceRef::Global)
        .await
        .expect("second holder grants the shared admin");
    assert_eq!(shared_record, admin_record);

    // Removing from the first holder leaves the row (the second still
    // references it).
    let outcome = user
        .revoke(&admin, RemovalTarget::NameOnly)
        .await
        .expect("revoke admin from the first holder");
    assert_eq!(outcome.removed_links, 1);
    assert!(
        outcome.removed_roles.is_empty(),
        "the role row survives: another holder still references it"
    );
    let surviving_rows = count_rows(
        &pool,
        "SELECT COUNT(*) FROM roles WHERE name = 'admin' AND resource_type = '' AND resource_id = ''",
    )
    .await;
    assert_eq!(surviving_rows, 1, "the shared admin row survives");
    assert!(
        other_user
            .has_role(&admin, ResourceFilter::Global)
            .await
            .expect("the second holder still holds the shared admin")
    );

    // Removing from the last holder destroys the orphan row.
    let outcome = other_user
        .revoke(&admin, RemovalTarget::NameOnly)
        .await
        .expect("revoke admin from the last holder");
    assert_eq!(outcome.removed_links, 1);
    assert_eq!(
        outcome.removed_roles.len(),
        1,
        "the orphan row is destroyed"
    );
    assert_eq!(outcome.removed_roles[0].name.as_str(), "admin");
    let remaining_rows = count_rows(
        &pool,
        "SELECT COUNT(*) FROM roles WHERE name = 'admin' AND resource_type = '' AND resource_id = ''",
    )
    .await;
    assert_eq!(remaining_rows, 0, "no admin row remains");

    // ============================================================
    // BYTE-EXACT NAMES: Admin and admin are two distinct rows
    // ============================================================
    //
    // This is the collation falsifier where the risk lives (MySQL):
    // granting `Admin` globally must NOT satisfy the lowercase query,
    // and a second grant of the lowercase spelling must create a
    // SEPARATE role row (two rows total), proving the DDL's
    // `utf8mb4_bin` collation.

    let admin_capital = RoleName::from("Admin");
    user.add_role(&admin_capital, ResourceRef::Global)
        .await
        .expect("grant Admin (capital)");
    assert!(
        !user
            .has_role(&admin, ResourceFilter::Global)
            .await
            .expect("byte-exact: 'admin' does not match 'Admin'")
    );
    user.add_role(&admin, ResourceRef::Global)
        .await
        .expect("grant admin (lowercase)");
    let both_admin_rows = count_rows(
        &pool,
        "SELECT COUNT(*) FROM roles WHERE name IN ('Admin', 'admin') AND resource_type = '' AND resource_id = ''",
    )
    .await;
    assert_eq!(
        both_admin_rows, 2,
        "byte-exact: 'Admin' and 'admin' are two distinct role rows"
    );

    // Leave the container DB clean for the next test binary.
    reset_roles_mysql(&pool).await;
}
