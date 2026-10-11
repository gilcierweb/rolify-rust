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
        (HolderIdKind::String, ParsedHolderId::Text(s)) => DieselHolderBind::Text(s),
        // Mismatch should be impossible: parse_holder_id guarantees the arm.
        _ => DieselHolderBind::Text(holder.as_str().to_owned()),
    })
}

/// Per-engine UUID bind: native `UUID` on Postgres, `BINARY(16)` bytes on
/// `MySQL`, hyphenated `TEXT` on `SQLite` (and as the inert default).
#[cfg(any(feature = "postgres", feature = "mysql"))]
#[must_use]
fn uuid_bind(uuid: uuid::Uuid) -> DieselHolderBind {
    #[cfg(feature = "postgres")]
    {
        DieselHolderBind::Uuid(uuid)
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

/// The holder primary-key expression for a COMPARISON against the
/// join's `user_id` column or a typed holder bind (the sqlx analog's
/// `holder_compare_expr`, RESEARCH Pattern 3): typed kinds compare
/// natively (`BIGINT`, `UUID`/`BINARY(16)`, or `TEXT` on both sides),
/// so the column stands bare; the string kind keeps the historical
/// text cast because the fixture holder keys are integers while the
/// string link column stores text (Postgres rejects
/// `integer = text`).
///
/// The SELECT-list projection is [`holder_text_projection`] instead:
/// `HolderIdRow` decodes `Text`, so the read path projects canonical
/// text on every engine (WR-01).
#[must_use]
pub fn holder_id_projection(kind: HolderIdKind, holder_pk_column: &str) -> String {
    match kind {
        HolderIdKind::Integer | HolderIdKind::Uuid => holder_pk_column.to_owned(),
        HolderIdKind::String => crate::dialect::cast_to_text(holder_pk_column),
    }
}

/// The holder primary-key PROJECTION feeding `HolderIdRow`'s text
/// decode (08 review WR-01): integer and string kinds keep the
/// uniform text cast on every engine, and the uuid kind keeps it on
/// Postgres and `SQLite` (canonical hyphenated text both). `MySQL`
/// stores the uuid kind as `BINARY(16)`, where `CAST(BINARY(16) AS
/// CHAR)` decodes the raw 16 bytes instead of the canonical string, so
/// that engine canonicalizes through `BIN_TO_UUID` (`MySQL` 8+, the
/// documented uuid column; sqlx solves the same corner in its row
/// decoder, which the compile-time-typed `QueryableByName` rows here
/// cannot do).
#[must_use]
pub fn holder_text_projection(kind: HolderIdKind, holder_pk_column: &str) -> String {
    match (kind, cfg!(feature = "mysql")) {
        (HolderIdKind::Uuid, true) => format!("BIN_TO_UUID({holder_pk_column})"),
        _ => crate::dialect::cast_to_text(holder_pk_column),
    }
}

/// Kind-aware join-condition predicate comparing the holder id column
/// against the join's `user_id`: typed kinds compare natively through
/// [`holder_id_projection`]; the string kind keeps today's text-cast
/// repair.
#[must_use]
pub fn holder_join_condition(
    kind: HolderIdKind,
    holder_pk_column: &str,
    user_id_column: &str,
) -> String {
    format!(
        "{} = {}",
        holder_id_projection(kind, holder_pk_column),
        user_id_column
    )
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
pub fn parse_canonical_holder(
    kind: HolderIdKind,
    holder: &ResourceId,
) -> Result<String, RolifyError> {
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
            Err(RolifyError::InvalidHolderId {
                expected: "uuid",
                ..
            })
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
    fn uuid_compare_stays_native_on_every_engine() {
        // Gate-fix semantics (sqlx analog `holder_compare_expr`): the
        // uuid kind joins against the native UUID/BINARY(16) column, so
        // a text cast would make Postgres reject `text = uuid`.
        assert_eq!(
            holder_id_projection(HolderIdKind::Uuid, "holder.id"),
            "holder.id"
        );
        assert_eq!(
            holder_join_condition(HolderIdKind::Uuid, "holder.id", "link.user_id"),
            "holder.id = link.user_id"
        );
    }

    #[test]
    fn join_condition_is_kind_aware() {
        assert_eq!(
            holder_join_condition(HolderIdKind::Integer, "holder.id", "link.user_id"),
            "holder.id = link.user_id"
        );
        assert!(
            holder_join_condition(HolderIdKind::String, "holder.id", "link.user_id")
                .starts_with("CAST(holder.id")
        );
    }

    #[test]
    fn string_projection_keeps_the_cast_today() {
        // Pittfall 8 / gem parity: string kind preserves the historical cast.
        assert!(
            holder_id_projection(HolderIdKind::String, "holder.id").contains("CAST"),
            "string kind keeps the legacy text cast"
        );
    }

    #[test]
    fn text_projection_is_canonical_text_per_engine() {
        // WR-01: the finder read path decodes Text on every engine.
        // Integer and string kinds keep the uniform cast; the uuid kind
        // keeps it on Postgres/SQLite (canonical text) while MySQL
        // canonicalizes its BINARY(16) column through BIN_TO_UUID.
        for kind in [
            HolderIdKind::Integer,
            HolderIdKind::String,
            HolderIdKind::Uuid,
        ] {
            let projection = holder_text_projection(kind, "holder.id");
            #[cfg(feature = "mysql")]
            let expected_uuid_arm = matches!(kind, HolderIdKind::Uuid);
            #[cfg(not(feature = "mysql"))]
            let expected_uuid_arm = false;
            if expected_uuid_arm {
                assert_eq!(projection, "BIN_TO_UUID(holder.id)");
            } else {
                assert!(
                    projection.starts_with("CAST(holder.id"),
                    "kind {kind:?} should project the text cast, got {projection}"
                );
            }
        }
    }
}
