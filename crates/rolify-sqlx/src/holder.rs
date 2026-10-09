//! Kind-aware holder-id bind construction (D-08-04/D-08-05).
//!
//! Single seam between the SPI's stringified [`ResourceId`] and the typed
//! sqlx binds the configured [`HolderIdKind`] demands (D-08-01): the pure
//! parse lives once in [`rolify_core::holder::parse_holder_id`] (DRY), and
//! this module maps the parsed value onto the engine's bind arms.
//!
//! ## Bind arms per (kind, engine)
//!
//! | Kind      | Postgres                | `MySQL`                   | `SQLite`                 |
//! |-----------|-------------------------|---------------------------|--------------------------|
//! | `Integer` | `BIGINT` <- `i64`       | `BIGINT` <- `i64`         | `INTEGER` <- `i64`       |
//! | `Uuid`    | `UUID` <- native `Uuid` | `BINARY(16)` <- `Uuid`    | `TEXT` <- hyphenated     |
//! | `String`  | `VARCHAR(191)` <- text  | `VARCHAR(191)` <- text    | `TEXT` <- text           |
//!
//! The `SQLite` uuid branch binds the canonical hyphenated text instead of a
//! bare `Uuid`: sqlx maps `Uuid` to a 16-byte BLOB on `SQLite`, which never
//! compares equal to the `TEXT` column of D-08-04 (RESEARCH Pitfall 2).

use rolify_core::config::HolderIdKind;
use rolify_core::error::RolifyError;
use rolify_core::holder::{ParsedHolderId, parse_holder_id};
use rolify_core::role::ResourceId;
use sqlx::database::Database;
use sqlx::query::Query;

/// One runtime bind value: the typed, ordered payload of a statement.
///
/// Role names, scope columns, and resource ids are text; generated role
/// ids are the integer binds; holder ids flow through
/// [`holder_bind_value`], which selects the arm by kind and engine.
#[derive(Debug, Clone)]
pub(crate) enum BindValue {
    /// A text bind (names, scope columns, stringified ids, `SQLite`
    /// hyphenated uuids).
    Text(String),
    /// A generated role-row id bind (and integer-kind holder binds).
    Integer(i64),
    /// A native uuid bind: `UUID` on Postgres, `BINARY(16)` on `MySQL`.
    ///
    /// Never constructed for `SQLite` (the driver maps it to BLOB there,
    /// RESEARCH Pitfall 2): that engine binds the hyphenated text arm.
    Uuid(sqlx::types::Uuid),
}

impl BindValue {
    /// Attach this value to a statement, in order.
    pub(crate) fn apply<'q, DB>(
        &self,
        statement: Query<'q, DB, <DB as Database>::Arguments>,
    ) -> Query<'q, DB, <DB as Database>::Arguments>
    where
        DB: Database,
        for<'e> String: sqlx::Encode<'e, DB> + sqlx::Type<DB>,
        for<'e> i64: sqlx::Encode<'e, DB> + sqlx::Type<DB>,
        for<'e> sqlx::types::Uuid: sqlx::Encode<'e, DB> + sqlx::Type<DB>,
    {
        match self {
            BindValue::Text(text) => statement.bind(text.clone()),
            BindValue::Integer(number) => statement.bind(*number),
            BindValue::Uuid(uuid) => statement.bind(*uuid),
        }
    }
}

/// Parse and wrap `holder` into the typed bind the configured `kind`
/// demands on `backend` (the `Database::NAME` string: `"PostgreSQL"`,
/// `"MySQL"`, or `"SQLite"`).
///
/// This is the ONLY holder-id bind construction in the crate: every SPI
/// path in `store.rs` calls it, so an id that does not parse for the kind
/// fails here - as [`RolifyError::InvalidHolderId`] raised BEFORE any
/// statement executes (D-08-05) - and no engine ever receives a value its
/// column cannot store (T-08-10).
///
/// # Errors
///
/// Returns [`RolifyError::InvalidHolderId`] when the id does not match the
/// kind's grammar (e.g. `"abc"` under `Integer`, `"not-a-uuid"` under
/// `Uuid`).
pub(crate) fn holder_bind_value(
    kind: HolderIdKind,
    holder: &ResourceId,
    backend: &str,
) -> Result<BindValue, RolifyError> {
    let parsed = parse_holder_id(kind, holder.as_str())?;
    Ok(match parsed {
        ParsedHolderId::Integer(number) => BindValue::Integer(number),
        ParsedHolderId::Uuid(uuid) if backend == "SQLite" => {
            BindValue::Text(uuid.hyphenated().to_string())
        }
        ParsedHolderId::Uuid(uuid) => BindValue::Uuid(uuid),
        ParsedHolderId::Text(text) => BindValue::Text(text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_kind_builds_integer_bind() {
        let bind = holder_bind_value(HolderIdKind::Integer, &ResourceId::from("42"), "PostgreSQL")
            .expect("integer parse");
        assert!(matches!(bind, BindValue::Integer(42)));
    }

    #[test]
    fn uuid_kind_builds_native_bind_on_postgres_and_mysql() {
        let uuid_text = "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8";
        for backend in ["PostgreSQL", "MySQL"] {
            let bind = holder_bind_value(HolderIdKind::Uuid, &ResourceId::from(uuid_text), backend)
                .expect("uuid parse");
            match bind {
                BindValue::Uuid(uuid) => assert_eq!(uuid.to_string(), uuid_text),
                other => panic!("expected native Uuid bind on {backend}, got {other:?}"),
            }
        }
    }

    #[test]
    fn uuid_kind_builds_hyphenated_text_bind_on_sqlite() {
        // RESEARCH Pitfall 2: a bare Uuid encodes as a 16-byte BLOB on
        // SQLite while the D-08-04 column is TEXT; the canonical
        // hyphenated text is the only comparable bind.
        let uuid_text = "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8";
        let bind = holder_bind_value(HolderIdKind::Uuid, &ResourceId::from(uuid_text), "SQLite")
            .expect("uuid parse");
        match bind {
            BindValue::Text(text) => assert_eq!(text, uuid_text),
            other => panic!("expected hyphenated Text bind on SQLite, got {other:?}"),
        }
    }

    #[test]
    fn string_kind_passes_the_text_through_on_every_backend() {
        for backend in ["PostgreSQL", "MySQL", "SQLite"] {
            let bind = holder_bind_value(
                HolderIdKind::String,
                &ResourceId::from("team-alpha"),
                backend,
            )
            .expect("string parse");
            match bind {
                BindValue::Text(text) => assert_eq!(text, "team-alpha"),
                other => panic!("expected Text bind on {backend}, got {other:?}"),
            }
        }
    }

    #[test]
    fn invalid_id_raises_invalid_holder_id_before_any_sql() {
        let result = holder_bind_value(
            HolderIdKind::Integer,
            &ResourceId::from("abc"),
            "PostgreSQL",
        );
        assert!(matches!(
            result,
            Err(RolifyError::InvalidHolderId {
                expected: "integer",
                ..
            })
        ));
        let result = holder_bind_value(
            HolderIdKind::Uuid,
            &ResourceId::from("not-a-uuid"),
            "SQLite",
        );
        assert!(matches!(
            result,
            Err(RolifyError::InvalidHolderId {
                expected: "uuid",
                ..
            })
        ));
    }
}
