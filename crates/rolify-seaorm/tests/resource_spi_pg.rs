//! Postgres acceptance pins for the raw-SQL SPI corners the frozen
//! parity suite cannot see (05 code review CR-01..CR-05): the
//! `where_any` holder gate (SQL operator precedence), the holder-filter
//! text cast against integer holder ids, the `instance_only(None)`
//! scope filter, the fail-loud holder-table guard, and the `in_list`
//! no-type-check coverage (gem `resource_adapter.rb:27-30`).
//!
//! Mirrors the sqlx crate's `resource_spi_pg.rs` supplement posture:
//! the public statics never route through these branches (the
//! `find_roles` holder leg reads `roles_of`), so the pins call the
//! store directly.
//!
//! Run: `cargo test -p rolify-seaorm --features postgres,suite --test resource_spi_pg`

#![cfg(all(feature = "postgres", feature = "suite"))]

mod support;

use rolify_core::catalog::RoleCatalogQuery;
use rolify_core::config::RolifyConfig;
use rolify_core::error::RolifyError;
use rolify_core::query::{ResourceFilter, RoleQuery};
use rolify_core::resource::ResourceRef;
use rolify_core::role::{ResourceId, RoleName, RoleRecord};
use rolify_core::store::{ResourceKey, ResourceStore, RoleStore};
use rolify_test::backend::TestBackend;
use rolify_test::fixtures::DefaultUser;
use sea_orm::DatabaseConnection;

use rolify_seaorm::{Error, SeaormStore};

use crate::support::SeaormBackend;

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

/// Assert two resource-key lists as sets (D-04): sort plus dedup the
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

/// CR-01 pin: with two ladders OR-joined inside the holder gate, a role
/// matching ONLY the second query and held by ANOTHER holder must stay
/// invisible. Unparenthesized, SQL parses the composition as
/// `(holder AND ladder1) OR ladder2` and leaks the other holder's row;
/// the parity suite's negative rows use unheld names, so only this
/// content pin can see the leak.
#[tokio::test]
async fn where_any_second_ladder_stays_gated_on_the_holder() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    let alpha = RoleName::from("alpha");
    let beta = RoleName::from("beta");
    let forum_first = ResourceId::from(1_i64);
    backend
        .grant_to("admin", &alpha, ResourceRef::Global)
        .await
        .expect("admin: global alpha");
    backend
        .grant_to(
            "zombie",
            &beta,
            ResourceRef::Instance("Forum", &forum_first),
        )
        .await
        .expect("zombie: instance beta on Forum first");

    let queries = [
        RoleQuery {
            name: &alpha,
            filter: ResourceFilter::Global,
        },
        RoleQuery {
            name: &beta,
            filter: ResourceFilter::Instance("Forum", &forum_first),
        },
    ];
    let admin_id = backend.holder_id("admin").expect("admin fixture");
    let (store, conn) = backend.engine().store_with_conn();
    let rows = store
        .where_any(conn, &admin_id, &queries)
        .await
        .expect("where_any two ladders");
    assert_record_set(&rows, &[RoleRecord::global("alpha")]);
}

/// CR-02 pin: the holder-filtered catalog read joins the integer-keyed
/// fixture holder table and filters by it; without the text cast on
/// BOTH the join and the filter, Postgres rejects the comparison
/// (`operator does not exist: integer = text`). Ported from the sqlx
/// crate's `roles_matching_holder_join_casts_integer_holder_ids`.
#[tokio::test]
async fn roles_matching_holder_filter_casts_integer_holder_ids() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    backend
        .grant_to(
            "admin",
            &RoleName::from("moderator"),
            ResourceRef::Class("Forum"),
        )
        .await
        .expect("admin: class moderator on Forum");
    backend
        .grant_to(
            "admin",
            &RoleName::from("editor"),
            ResourceRef::Instance("Forum", &ResourceId::from(1_i64)),
        )
        .await
        .expect("admin: instance editor on Forum first");

    let admin_id = backend.holder_id("admin").expect("admin fixture");
    let query = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&admin_id);
    let (store, conn) = backend.engine().store_with_conn();
    let rows = store
        .roles_matching(conn, &query)
        .await
        .expect("holder-filtered catalog read");
    assert_record_set(
        &rows,
        &[
            RoleRecord::for_class("moderator", "Forum"),
            RoleRecord::for_instance("editor", "Forum", 1_i64),
        ],
    );
}

/// CR-03 pin: `instance_only(None)` is the catalog scope's documented
/// "every instance row in types" query: class rows stay out, instance
/// rows stay in (mirror of the sqlx pin and the in-memory grid).
#[tokio::test]
async fn roles_matching_instance_only_none_excludes_class_rows() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    backend
        .grant_to(
            "admin",
            &RoleName::from("moderator"),
            ResourceRef::Class("Forum"),
        )
        .await
        .expect("admin: class moderator on Forum");
    backend
        .grant_to(
            "admin",
            &RoleName::from("editor"),
            ResourceRef::Instance("Forum", &ResourceId::from(1_i64)),
        )
        .await
        .expect("admin: instance editor on Forum first");
    backend
        .grant_to(
            "admin",
            &RoleName::from("reviewer"),
            ResourceRef::Instance("Forum", &ResourceId::from(2_i64)),
        )
        .await
        .expect("admin: instance reviewer on Forum second");

    let query = RoleCatalogQuery::for_types(&["Forum"]).instance_only(None);
    let (store, conn) = backend.engine().store_with_conn();
    let rows = store
        .roles_matching(conn, &query)
        .await
        .expect("instance_only(None) catalog read");
    assert_record_set(
        &rows,
        &[
            RoleRecord::for_instance("editor", "Forum", 1_i64),
            RoleRecord::for_instance("reviewer", "Forum", 2_i64),
        ],
    );
}

/// CR-04 pin: a holder filter on a store built without
/// `for_holder_table(..)` must fail loudly with `InvalidConfig` (the
/// same posture as `holders_where` / `all_holders`), never silently
/// return every holder's rows.
#[tokio::test]
async fn roles_matching_holder_filter_without_holder_table_fails_loudly() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    let config = RolifyConfig::default();
    let bare_store = SeaormStore::<DatabaseConnection>::new(&config);
    let holder_id = ResourceId::from(1_i64);
    let query = RoleCatalogQuery::for_types(&["Forum"]).with_holder(&holder_id);
    let (_, conn) = backend.engine().store_with_conn();
    let error = bare_store
        .roles_matching(conn, &query)
        .await
        .expect_err("holder filter without a holder table must fail loudly");
    assert!(
        matches!(error, Error::Core(RolifyError::InvalidConfig { .. })),
        "expected InvalidConfig, got {error:?}"
    );
}

/// CR-05 pin: the gem's `in` carries no `resource_type` condition
/// (`resource_adapter.rb:27-30`), so a same-id instance row covers
/// same-id candidates of OTHER types, and class plus global rows
/// (sentinel id) cover every candidate (mirror of the in-memory pin,
/// `rolify-test/src/tests.rs`: "gem `in` applies no `resource_type`
/// condition").
#[tokio::test]
async fn in_list_covers_cross_type_class_and_global_rows() {
    let mut backend = SeaormBackend::<DefaultUser>::build()
        .await
        .expect("backend build (container + migrator + fixtures)");

    let editor = RoleName::from("editor");
    let manager = RoleName::from("manager");
    let boss = RoleName::from("boss");
    backend
        .grant_to(
            "admin",
            &editor,
            ResourceRef::Instance("Forum", &ResourceId::from(1_i64)),
        )
        .await
        .expect("admin: instance editor on Forum first");
    backend
        .grant_to("admin", &manager, ResourceRef::Class("Forum"))
        .await
        .expect("admin: class manager on Forum");
    backend
        .grant_to("admin", &boss, ResourceRef::Global)
        .await
        .expect("admin: global boss");

    let admin_id = backend.holder_id("admin").expect("admin fixture");
    let candidates = vec![
        ResourceKey::new("Forum", ResourceId::from(1_i64)),
        ResourceKey::new("Forum", ResourceId::from(2_i64)),
        ResourceKey::new("Group", ResourceId::from(1_i64)),
        ResourceKey::new("Team", ResourceId::from(2_i64)),
    ];
    let (store, conn) = backend.engine().store_with_conn();

    // Instance row: same-id coverage crosses types (Group "1" is
    // covered by the Forum "1" instance row); the id "2" candidates
    // are not covered under this name.
    let found = store
        .in_list(conn, &candidates, &admin_id, std::slice::from_ref(&editor))
        .await
        .expect("in_list editor");
    assert_key_set(
        &found,
        &[
            ResourceKey::new("Forum", ResourceId::from(1_i64)),
            ResourceKey::new("Group", ResourceId::from(1_i64)),
        ],
    );

    // Class row: the sentinel id covers every candidate, including
    // the Group and Team candidates of other types.
    let found = store
        .in_list(conn, &candidates, &admin_id, std::slice::from_ref(&manager))
        .await
        .expect("in_list manager");
    assert_key_set(&found, &candidates);

    // Global row: same sentinel coverage.
    let found = store
        .in_list(conn, &candidates, &admin_id, std::slice::from_ref(&boss))
        .await
        .expect("in_list boss");
    assert_key_set(&found, &candidates);
}
