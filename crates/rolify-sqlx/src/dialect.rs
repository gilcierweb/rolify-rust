//! Runtime dialect helpers: placeholder syntax and identifier quoting.
//!
//! Unlike the diesel reference adapter (whose engines are mutually
//! exclusive features, so its switches compile-time `cfg`), sqlx engine
//! features are ADDITIVE (D-11/D-13): one build may enable Postgres,
//! `MySQL`, and `SQLite` at once. The engine is therefore a runtime property
//! of the `DB` type parameter, and both helpers switch on
//! [`Database::NAME`] (`"PostgreSQL"` / `"MySQL"` / `"SQLite"`, per the
//! backend crates' own consts).
//!
//! The query TEXT must carry the engine's native syntax: sqlx writes `?`
//! by trait default and only the Postgres backend overrides it with `$N`
//! ([`PgArguments::format_placeholder`]), so hand-written SQL aimed at a
//! prepared statement must already carry `$N` for Postgres and `?` for
//! MySQL/SQLite.

use sqlx::database::Database;

/// Return the positional placeholder for `DB` at the 1-based `index`.
///
/// - `PostgreSQL`: `$1`, `$2`, ...
/// - `MySQL` / `SQLite`: `?` (anonymous positional)
///
/// Every placeholder OCCURRENCE in a template consumes exactly one bind.
/// Postgres allows reusing `$N` across occurrences, but `MySQL` and `SQLite`
/// positional `?` marks cannot share a bind, so the adapter's uniform
/// discipline is one bind per occurrence on every engine: the same SQL
/// text then works on all three with one ordered bind list.
///
/// # Panics
///
/// Panics if `index` is zero (placeholders are 1-based).
#[must_use]
pub fn placeholder<DB: Database>(index: usize) -> String {
    assert!(index > 0, "placeholder index must be 1-based");
    match DB::NAME {
        "PostgreSQL" => format!("${index}"),
        _ => "?".to_owned(),
    }
}

/// Quote an identifier (table or column name) for `DB`.
///
/// - `PostgreSQL` / `SQLite`: double quotes (`"identifier"`)
/// - `MySQL`: backticks (`` `identifier` ``)
///
/// The input must have already passed the D-08 allow-list validation
/// (`RolifyConfigBuilder::validate_identifier`,
/// `^[A-Za-z_][A-Za-z0-9_]*$`). This function only adds the engine's
/// quoting characters; together with the allow-list it forms the
/// two-layer identifier safety of Phase 3 D-08.
#[must_use]
pub fn quote_identifier<DB: Database>(name: &str) -> String {
    match DB::NAME {
        "MySQL" => format!("`{name}`"),
        _ => format!("\"{name}\""),
    }
}

/// Wrap an integer-key column so it decodes as text
/// (`CAST(... AS TEXT)`; `CAST(... AS CHAR)` on `MySQL`, which rejects
/// `TEXT` as a cast target).
///
/// The holder and resource primary keys are integers on most fixture
/// tables while the SPI carries stringified ids; every projection or
/// comparison involving such a key casts here so the text-first decode
/// never sees a binary integer. Mirrors the diesel reference's
/// per-engine `cast_to_text`, with the engine selected at runtime.
#[must_use]
pub fn cast_to_text<DB: Database>(column: &str) -> String {
    match DB::NAME {
        "MySQL" => format!("CAST({column} AS CHAR)"),
        _ => format!("CAST({column} AS TEXT)"),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "postgres")]
    mod postgres {
        use super::super::*;
        use sqlx::Postgres;

        #[test]
        fn placeholder_is_dollar_numbered() {
            assert_eq!(placeholder::<Postgres>(1), "$1");
            assert_eq!(placeholder::<Postgres>(42), "$42");
        }

        #[test]
        fn quote_identifier_uses_double_quotes() {
            assert_eq!(quote_identifier::<Postgres>("roles"), "\"roles\"");
            assert_eq!(
                quote_identifier::<Postgres>("users_roles"),
                "\"users_roles\""
            );
        }

        #[test]
        #[should_panic(expected = "placeholder index must be 1-based")]
        fn placeholder_index_zero_panics() {
            let _ = placeholder::<Postgres>(0);
        }

        #[test]
        fn cast_to_text_uses_text_target() {
            assert_eq!(cast_to_text::<Postgres>("res.id"), "CAST(res.id AS TEXT)");
        }
    }

    #[cfg(feature = "mysql")]
    mod mysql {
        use super::super::*;
        use sqlx::MySql;

        #[test]
        fn placeholder_is_question_mark() {
            assert_eq!(placeholder::<MySql>(1), "?");
            assert_eq!(placeholder::<MySql>(999), "?");
        }

        #[test]
        fn quote_identifier_uses_backticks() {
            assert_eq!(quote_identifier::<MySql>("roles"), "`roles`");
            assert_eq!(quote_identifier::<MySql>("users_roles"), "`users_roles`");
        }

        #[test]
        fn cast_to_text_uses_char_target() {
            assert_eq!(cast_to_text::<MySql>("res.id"), "CAST(res.id AS CHAR)");
        }
    }

    #[cfg(feature = "sqlite")]
    mod sqlite {
        use super::super::*;
        use sqlx::Sqlite;

        #[test]
        fn placeholder_is_question_mark() {
            assert_eq!(placeholder::<Sqlite>(1), "?");
            assert_eq!(placeholder::<Sqlite>(999), "?");
        }

        #[test]
        fn quote_identifier_uses_double_quotes() {
            assert_eq!(quote_identifier::<Sqlite>("roles"), "\"roles\"");
            assert_eq!(
                quote_identifier::<Sqlite>("custom_table"),
                "\"custom_table\""
            );
        }
    }
}
