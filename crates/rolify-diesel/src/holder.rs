//! Kind-aware holder-id bind helper (D-08-05).
//!
//! Single choke point for parsing and binding holder ids per the configured
//! `HolderIdKind`. The pure parse lives in `rolify_core::holder::parse_holder_id`
//! (DRY, single source of truth); this module does the per-engine typed bind
//! on top of it. Every holder-id bind site in `store.rs` routes through this
//! helper so no raw `bind::<Text, _>(holder.as_str())` remains (grep-pinned).

use diesel::sql_types::{BigInt, Nullable, Text};
use rolify_core::config::HolderIdKind;
use rolify_core::error::RolifyError;
use rolify_core::holder::{ParsedHolderId, holder_id_to_string, parse_holder_id};
use rolify_core::role::ResourceId;

/// The parsed, engine-ready holder id: the typed value plus the SQL types
/// used to bind it per engine.
///
/// Consumers use [`holder_id_kind_from_config`] to resolve the kind from
/// the config, then call [`parsed_for_store`] to obtain this, then bind the
/// typed value at the site.
#[derive(Clone, Debug)]
pub enum DieselHolderBind {
    /// 64-bit integer: bindable as `BigInt` on all engines.
    BigInt(i64),
    /// Postgres native UUID: binds via `diesel/uuid`'s `Uuid` codec.
    #[cfg(feature = "postgres")]
    Uuid(uuid::Uuid),
    /// MySQL native binary UUID: binds as `Vec<u8>` over `BINARY(16)`.
    #[cfg(feature = "mysql")]
    UuidBytes(Vec<u8>),
    /// SQLite/any: canonical hyphenated UUID string bound as `Text`.
    Text(String),
}

/// Resolve the configured holder-id kind from the rolify config (D-08-01).
#[must_use]
pub fn holder_id_kind_from_config(config: &rolify_core::config::RolifyConfig) -> HolderIdKind {
    config.holder_id_kind()
}

/// Parse the holder id once for the configured kind, returning the typed
/// bind envelope for the compiled engine.
///
/// # Errors
///
/// Returns [`RolifyError::InvalidHolderId`] before any SQL executes when the
/// input does not match the configured kind (D-08-05).
pub fn parsed_for_store(
    kind: HolderIdKind,
    holder: &ResourceId,
) -> Result<DieselHolderBind, RolifyError> {
    let parsed = parse_holder_id(kind, holder.as_str())?;
    Ok(match (kind, parsed) {
        (HolderIdKind::Integer, ParsedHolderId::Integer(value)) => DieselHolderBind::BigInt(value),
        (HolderIdKind::Uuid, ParsedHolderId::Uuid(uuid)) => uuid_bind(uuid),
        (HolderIdKind::String, _) => DieselHolderBind::Text(holder.as_str().to_owned()),
        // Mismatch should be impossible: parse_holder_id guarantees the arm.
        _ => DieselHolderBind::Text(holder.as_str().to_owned()),
    })
}

/// Per-engine UUID bind: native `UUID` on Postgres, `BINARY(16)` bytes on
/// MySQL, hyphenated `TEXT` on SQLite (and as the inert default).
#[cfg(any(feature = "postgres", feature = "mysql"))]
#[must_use]
fn uuid_bind(uuid: uuid::Uuid) -> DieselHolderBind {
    #[cfg(feature = "postgres")]
    {
        return DieselHolderBind::Uuid(uuid);
    }
    #[cfg(all(feature = "mysql", not(feature = "postgres")))]
    {
        return DieselHolderBind::UuidBytes(uuid.as_bytes().to_vec());
    }
}

/// SQLite/default: UUIDs bind as hyphenated text (no native UUID type).
#[cfg(any(feature = "sqlite", not(any(feature = "postgres", feature = "mysql"))))]
#[must_use]
fn uuid_bind(uuid: uuid::Uuid) -> DieselHolderBind {
    DieselHolderBind::Text(uuid.hyphenated().to_string())
}

/// Kind-aware select-list projection for the holder id column in the finder
/// queries (RESEARCH Pattern 3).
///
/// | Kind     | Postgres                     | MySQL                      | SQLite       |
/// |----------|------------------------------|----------------------------|--------------|
/// | Integer  | `holder.id` (typed `BigInt`) | `holder.id` (typed `BigInt`) | `holder.id`  |
/// | Uuid     | `CAST(holder.id AS TEXT)`    | `holder.id` (binary bytes) | `holder.id`  |
/// | String   | `CAST(holder.id AS TEXT)`    | `CAST(holder.id AS CHAR)`  | `holder.id`  |
///
/// String keeps today's `cast_to_text` (gem parity); Integer removes the cast
/// so the typed column compares directly against `link.user_id BIGINT`.
#[must_use]
pub fn holder_id_projection(kind: HolderIdKind, holder_pk_column: &str) -> String {
    match kind {
        HolderIdKind::Integer => holder_pk_column.to_owned(),
        HolderIdKind::Uuid => {
            #[cfg(feature = "postgres")]
            {
                crate::dialect::cast_to_text(holder_pk_column)
            }
            #[cfg(any(feature = "mysql", feature = "sqlite", not(feature = "postgres")))]
            {
                holder_pk_column.to_owned()
            }
        }
        HolderIdKind::String => crate::dialect::cast_to_text(holder_pk_column),
    }
}

/// Kind-aware join-condition predicate comparing the holder id column against
/// the join's `user_id` (both typed for integer). Only the integer kind
/// changes the comparison; uuid/string keep today's string-equality path.
#[must_use]
pub fn holder_join_condition(kind: HolderIdKind, holder_pk_column: &str, user_id_column: &str) -> String {
    match kind {
        HolderIdKind::Integer => format!("{holder_pk_column} = {user_id_column}"),
        HolderIdKind::Uuid | HolderIdKind::String => {
            format!("{} = {}", crate::dialect::cast_to_text(holder_pk_column), user_id_column)
        }
    }
}

/// Parse a holder id against the configured kind and return its canonical
/// string (D-08-05: typed parse runs at the SPI boundary; bind stays TEXT for
/// legacy string columns. Plan 07's per-kind migrations swap the column type
/// and this helper becomes a simple pass-through for the typed bind).
///
/// # Errors
///
/// Returns [`RolifyError::InvalidHolderId`] when the input does not match
/// the kind's grammar (e.g., "abc" for `Integer`, "not-a-uuid" for `Uuid`).
pub fn parse_canonical_holder(kind: HolderIdKind, holder: &ResourceId) -> Result<String, RolifyError> {
    let parsed = parse_holder_id(kind, holder.as_str())?;
    Ok(holder_id_to_string(&parsed))
}

/// The Diesel SQL type used by the string-holder arm's spiked references
/// (kept public for glue sites that hard-bind text).
pub type HolderText = Text;

/// The nullable wrapper for a left-join's optional holder id projection.
pub type NullableBigInt = Nullable<BigInt>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_parse_and_stringify() {
        let holder = ResourceId::from(42_i64);
        let parsed = parsed_for_store(HolderIdKind::Integer, &holder).unwrap();
        assert!(matches!(parsed, DieselHolderBind::BigInt(42)));
    }

    #[test]
    fn uuid_parse_round_trip_via_holder_id_to_string() {
        let uuid_str = "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8";
        let holder = ResourceId::from(uuid_str);
        let parsed = parsed_for_store(HolderIdKind::Uuid, &holder).unwrap();
        #[cfg(feature = "sqlite")]
        {
            // SQLite binds hyphenated text.
            match parsed {
                DieselHolderBind::Text(s) => assert_eq!(s, uuid_str),
                _ => panic!("expected Text bind for sqlite uuid"),
            }
        }
        #[cfg(feature = "postgres")]
        {
            match parsed {
                DieselHolderBind::Uuid(u) => assert_eq!(u.to_string(), uuid_str),
                _ => panic!("expected native Uuid bind for postgres"),
            }
        }
        #[cfg(all(feature = "mysql", not(feature = "postgres")))]
        {
            match parsed {
                DieselHolderBind::UuidBytes(bytes) => {
                    let decoded = uuid::Uuid::from_bytes(bytes.try_into().expect("16 bytes"));
                    assert_eq!(decoded.to_string(), uuid_str);
                }
                _ => panic!("expected UuidBytes bind for mysql"),
            }
        }
    }

    #[test]
    fn string_parse_passes_through() {
        let holder = ResourceId::from("team-alpha");
        let parsed = parsed_for_store(HolderIdKind::String, &holder).unwrap();
        match parsed {
            DieselHolderBind::Text(s) => assert_eq!(s, "team-alpha"),
            _ => panic!("expected Text bind for string kind"),
        }
    }

    #[test]
    fn invalid_holder_id_at_boundary() {
        let holder = ResourceId::from("not-a-uuid");
        let result = parsed_for_store(HolderIdKind::Uuid, &holder);
        assert!(matches!(
            result,
            Err(RolifyError::InvalidHolderId { expected: "uuid", .. })
        ));
    }

    #[test]
    fn integer_projection_omits_cast_for_integer_kind() {
        match holder_id_projection(HolderIdKind::Integer, "holder.id").as_str() {
            "holder.id" => {}
            other => panic!("integer kind should not cast, got {other}"),
        }
    }

    #[test]
    fn string_projection_keeps_the_cast_today() {
        // Pittfall 8 / gem parity: string kind preserves the historical cast.
        assert!(
            holder_id_projection(HolderIdKind::String, "holder.id").contains("CAST"),
            "string kind keeps the legacy text cast"
        );
    }
}
