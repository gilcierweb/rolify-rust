//! Emitter dispatch for the four backends.
//!
//! Diesel and Sqlx share ONE renderer (`sql::render_sql`), so D-11 identity
//! holds by construction: `render_all` calls it directly per engine. `SeaORM`
//! and `MongoDB` have their own hand-maintained-template emitters. All
//! table-name substitution flows through ONE re-scan-free helper (D-14).

pub mod mongo;
pub mod seaorm;
pub mod sql;

use crate::render::RenderPlan;
use rolify_core::config::HolderIdKind;

/// Canonical join-table sentinel in the vendored templates.
pub(crate) const JOIN_TABLE_SENTINEL: &str = "users_roles";

/// Canonical roles-table stem in the vendored templates.
pub(crate) const ROLES_TABLE_SENTINEL: &str = "roles";

/// Canonical holder-id-type sentinel in the vendored templates.
pub(crate) const HOLDER_ID_TYPE_SENTINEL: &str = "{{holder_id_type}}";

/// Re-scan-free table-name substitution (D-14).
///
/// The naive two-pass chain
/// (`replace(JOIN_SENTINEL, join).replace(ROLES_SENTINEL, roles)`) is wrong:
/// the second pass re-scans text the first pass inserted, so an explicit
/// `--join-table` embedding the roles stem (e.g. `member_roles_archive`)
/// is silently mangled (`member_privileges_archive`) whenever a custom
/// `--roles-table` is also in play (WR-01). The split here guarantees that
/// can never happen: the template is split on the canonical join sentinel
/// FIRST, the roles replacement only ever sees ORIGINAL template segments,
/// and the join with the requested name happens LAST, after every other
/// substitution, so no pass can touch it. For every name pair the previous
/// longest-first chain handled correctly, the output is byte-identical
/// (the equivalence unit test pins that).
pub(crate) fn substitute_table_names(template: &str, plan: &RenderPlan, engine: &str) -> String {
    let holder_id_type = holder_id_type_sql(plan.holder_id_kind, engine);
    template
        .replace(HOLDER_ID_TYPE_SENTINEL, holder_id_type)
        .split(JOIN_TABLE_SENTINEL)
        .map(|segment| segment.replace(ROLES_TABLE_SENTINEL, &plan.roles_table))
        .collect::<Vec<_>>()
        .join(&plan.join_table)
}

/// Returns the SQL column type for the given holder id kind and engine.
pub(crate) fn holder_id_type_sql(kind: HolderIdKind, engine: &str) -> &'static str {
    match (kind, engine) {
        (HolderIdKind::Integer, "postgres") => "BIGINT",
        (HolderIdKind::Uuid, "postgres") => "UUID",
        (HolderIdKind::String, "postgres") => "VARCHAR(191)",
        (HolderIdKind::Integer, "mysql") => "BIGINT",
        (HolderIdKind::Uuid, "mysql") => "BINARY(16)",
        (HolderIdKind::String, "mysql") => "VARCHAR(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin",
        (HolderIdKind::Integer, "sqlite") => "INTEGER",
        (HolderIdKind::Uuid, "sqlite") => "TEXT",
        (HolderIdKind::String, "sqlite") => "TEXT",
        _ => "VARCHAR(191)", // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Backend;
    use crate::templates;

    fn plan(roles_table: &str, join_table: &str, holder_id_kind: rolify_core::config::HolderIdKind) -> RenderPlan {
        RenderPlan {
            backend: Backend::Diesel,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: roles_table.to_string(),
            join_table: join_table.to_string(),
            holder_id_kind,
            with_holder_fk: false,
        }
    }

    /// Default names with String holder_id_kind: the rendered bytes equal the
    /// template bytes with {{holder_id_type}} replaced by the engine-specific
    /// type for String kind (VARCHAR(191) for PG, full charset for MySQL, TEXT for SQLite).
    #[test]
    fn substitute_table_names_default_identity() {
        let default_plan = plan("roles", "users_roles", rolify_core::config::HolderIdKind::String);

        for engine in ["postgres", "mysql", "sqlite"] {
            for template in [templates::up(engine), templates::down(engine)] {
                let expected_type = holder_id_type_sql(rolify_core::config::HolderIdKind::String, engine);
                let expected = template.replace("{{holder_id_type}}", expected_type);
                assert_eq!(
                    substitute_table_names(template, &default_plan, engine),
                    expected,
                    "default-name substitution with String kind must produce correct type for engine {engine}"
                );
            }
        }
    }

    /// The derived-join shape (holder plural + _ + roles table) agrees with
    /// the documented derivation: both sentinels become the same stem.
    #[test]
    fn substitute_table_names_derived_join() {
        let derived_plan = plan("privileges", "users_privileges", rolify_core::config::HolderIdKind::Integer);
        let rendered = substitute_table_names(templates::up("postgres"), &derived_plan, "postgres");

        assert!(
            rendered.contains("CREATE TABLE users_privileges"),
            "derived join table missing from the rendered up.sql"
        );
        assert!(
            rendered.contains("privileges_triple_unique"),
            "roles table stem not substituted"
        );
        assert!(
            !rendered.contains("users_roles"),
            "stale derived join name must not survive"
        );
    }

    /// An explicit join table embedding the roles stem survives verbatim:
    /// the roles pass can never re-scan the inserted join name.
    #[test]
    fn substitute_table_names_explicit_join_embeds_roles_stem() {
        let explicit_plan = plan("privileges", "member_roles_archive", rolify_core::config::HolderIdKind::Integer);
        let rendered = substitute_table_names(templates::up("postgres"), &explicit_plan, "postgres");

        assert!(
            rendered.contains("CREATE TABLE member_roles_archive"),
            "explicit join name missing from the rendered up.sql"
        );
        assert!(
            !rendered.contains("member_privileges_archive"),
            "explicit join name was mangled by a re-scan"
        );
    }

    /// Byte-equivalence with the previous longest-first chain for the tested
    /// custom pair: the mechanism swap preserves shipped bytes (D-14).
    #[test]
    fn substitute_table_names_custom_pair_matches_longest_first_output() {
        let custom_plan = plan("privileges", "customers_privileges", rolify_core::config::HolderIdKind::Integer);

        for engine in ["postgres", "mysql", "sqlite"] {
            for template in [templates::up(engine), templates::down(engine)] {
                let expected_type = holder_id_type_sql(rolify_core::config::HolderIdKind::Integer, engine);
                let longest_first = template
                    .replace("{{holder_id_type}}", expected_type)
                    .replace("users_roles", &custom_plan.join_table)
                    .replace("roles", &custom_plan.roles_table);
                assert_eq!(
                    substitute_table_names(template, &custom_plan, engine),
                    longest_first,
                    "helper output diverged from the longest-first bytes for engine {engine}"
                );
            }
        }
    }
}
