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

/// Canonical join-table sentinel in the vendored templates.
const JOIN_TABLE_SENTINEL: &str = "users_roles";

/// Canonical roles-table stem in the vendored templates.
const ROLES_TABLE_SENTINEL: &str = "roles";

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
pub(crate) fn substitute_table_names(template: &str, plan: &RenderPlan) -> String {
    template
        .split(JOIN_TABLE_SENTINEL)
        .map(|segment| segment.replace(ROLES_TABLE_SENTINEL, &plan.roles_table))
        .collect::<Vec<_>>()
        .join(&plan.join_table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Backend;
    use crate::templates;

    fn plan(roles_table: &str, join_table: &str) -> RenderPlan {
        RenderPlan {
            backend: Backend::Diesel,
            role_name: "Role".to_string(),
            holder_name: "User".to_string(),
            roles_table: roles_table.to_string(),
            join_table: join_table.to_string(),
        }
    }

    /// Default names leave every canonical sentinel untouched: the rendered
    /// bytes equal the template bytes.
    #[test]
    fn substitute_table_names_default_identity() {
        let default_plan = plan("roles", "users_roles");

        for template in [
            templates::up("postgres"),
            templates::down("postgres"),
            templates::up("mysql"),
            templates::down("mysql"),
            templates::up("sqlite"),
            templates::down("sqlite"),
        ] {
            assert_eq!(
                substitute_table_names(template, &default_plan),
                template,
                "default-name substitution must be the identity"
            );
        }
    }

    /// The derived-join shape (holder plural + _ + roles table) agrees with
    /// the documented derivation: both sentinels become the same stem.
    #[test]
    fn substitute_table_names_derived_join() {
        let derived_plan = plan("privileges", "users_privileges");
        let rendered = substitute_table_names(templates::up("postgres"), &derived_plan);

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
        let explicit_plan = plan("privileges", "member_roles_archive");
        let rendered = substitute_table_names(templates::up("postgres"), &explicit_plan);

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
        let custom_plan = plan("privileges", "customers_privileges");

        for template in [
            templates::up("postgres"),
            templates::down("postgres"),
            templates::up("mysql"),
            templates::down("mysql"),
            templates::up("sqlite"),
            templates::down("sqlite"),
        ] {
            let longest_first = template
                .replace("users_roles", &custom_plan.join_table)
                .replace("roles", &custom_plan.roles_table);
            assert_eq!(
                substitute_table_names(template, &custom_plan),
                longest_first,
                "helper output diverged from the longest-first bytes"
            );
        }
    }
}
