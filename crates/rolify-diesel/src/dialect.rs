//! Per-backend SQL dialect helpers: placeholder syntax and identifier quoting.
//!
//! These are compile-time switches via mutually exclusive backend features
//! (`postgres` / `mysql` / `sqlite`). Each function is a diameter-of-one
//! helper — no runtime branches, the correct variant is selected by the
//! feature that enables the crate.
//!
//! This mirrors the gem's per-adapter placeholder handling (AR uses `?` for
//! all three via its bind logic; we must be explicit because Diesel's
//! `sql_query().bind()` does NOT rewrite placeholders across backends — the
//! query text must already carry the correct syntax: `$n` for Postgres, `?`
//! for MySQL/SQLite).

/// Return the positional placeholder for the currently compiled backend.
///
/// - Postgres: `$1`, `$2`, ... (1-based, Postgres PREPARE syntax)
/// - MySQL / SQLite: `?` (anonymous positional)
///
/// # Panics
///
/// Panics if called with `index == 0` (placeholders are 1-based).
#[cfg(feature = "postgres")]
#[must_use]
pub fn placeholder(index: usize) -> String {
    assert!(index > 0, "placeholder index must be 1-based");
    format!("${index}")
}

/// Return the positional placeholder for MySQL/SQLite (always `?`).
///
/// Returns an owned `String` like the Postgres variant so every call site
/// shares one shape (the split return type broke the `mysql`/`sqlite`
/// builds: `.map(placeholder).collect::<Vec<String>>()` rejects `&str`).
#[cfg(any(feature = "mysql", feature = "sqlite"))]
#[must_use]
pub fn placeholder(_index: usize) -> String {
    "?".to_owned()
}

/// Quote an identifier (table or column name) for the currently compiled backend.
///
/// - Postgres / SQLite: double quotes (`"identifier"`)
/// - MySQL: backticks (`` `identifier` ``)
///
/// The input must have already passed the allow-list validation in
/// `RolifyConfig::build()` (`^[A-Za-z_][A-Za-z0-9_]*$`). This function only
/// adds the engine-specific quoting characters.
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[must_use]
pub fn quote_identifier(name: &str) -> String {
    format!("\"{name}\"")
}

/// Quote an identifier for MySQL (backticks).
#[cfg(feature = "mysql")]
#[must_use]
pub fn quote_identifier(name: &str) -> String {
    format!("`{name}`")
}

/// Wrap an integer-key column so it decodes as text.
///
/// The holder and resource primary keys are integers on most tables
/// while the SPI carries stringified ids; every projection of such a
/// key casts here so the `Text` by-name decoding never sees a binary
/// integer (which decodes as garbage instead of erroring).
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[must_use]
pub fn cast_to_text(column: &str) -> String {
    format!("CAST({column} AS TEXT)")
}

/// Wrap an integer-key column so it decodes as text on MySQL
/// (`CAST AS CHAR`: MySQL rejects `TEXT` as a cast target).
#[cfg(feature = "mysql")]
#[must_use]
pub fn cast_to_text(column: &str) -> String {
    format!("CAST({column} AS CHAR)")
}

/// Fallback placeholder when no backend feature is enabled (inert stub).
/// Returns a generic placeholder that won't be used since SQL generation
/// is only called when a backend feature is active.
#[cfg(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")))]
#[must_use]
pub fn placeholder(_index: usize) -> String {
    "?".to_string()
}

/// Fallback quote_identifier when no backend feature is enabled (inert stub).
/// Returns the name unquoted since this is only used for the inert stub
/// compilation path where no actual SQL is executed.
#[cfg(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")))]
#[must_use]
pub fn quote_identifier(name: &str) -> String {
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_postgres_is_dollar_numbered() {
        // This test only compiles under `cfg(feature = "postgres")`
        #[cfg(feature = "postgres")]
        {
            assert_eq!(placeholder(1), "$1");
            assert_eq!(placeholder(42), "$42");
        }
    }

    #[test]
    fn placeholder_mysql_sqlite_is_question_mark() {
        #[cfg(any(feature = "mysql", feature = "sqlite"))]
        {
            assert_eq!(placeholder(1), "?");
            assert_eq!(placeholder(999), "?");
        }
    }

    #[test]
    fn quote_identifier_postgres_sqlite_double_quotes() {
        #[cfg(any(feature = "postgres", feature = "sqlite"))]
        {
            assert_eq!(quote_identifier("roles"), "\"roles\"");
            assert_eq!(quote_identifier("users_roles"), "\"users_roles\"");
            assert_eq!(quote_identifier("custom_table"), "\"custom_table\"");
        }
    }

    #[test]
    fn quote_identifier_mysql_backticks() {
        #[cfg(feature = "mysql")]
        {
            assert_eq!(quote_identifier("roles"), "`roles`");
            assert_eq!(quote_identifier("users_roles"), "`users_roles`");
            assert_eq!(quote_identifier("custom_table"), "`custom_table`");
        }
    }
}
