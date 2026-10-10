//! Holder id parsing and canonicalization.
//!
//! Provides the single source of truth for parsing holder ids according to
//! the configured `HolderIdKind`. All adapters must use this helper instead
//! of implementing their own parsing logic (DRY, D-08-05).

use crate::config::HolderIdKind;
use crate::error::RolifyError;
use uuid::Uuid;

/// A parsed holder id, typed by kind.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "kind", content = "value", rename_all = "lowercase")
)]
pub enum ParsedHolderId {
    /// 64-bit integer holder id.
    Integer(i64),
    /// UUID holder id.
    Uuid(Uuid),
    /// String holder id (canonical string form).
    Text(String),
}

/// Parses a holder id string according to the configured kind.
///
/// # Errors
///
/// Returns [`RolifyError::InvalidHolderId`] if the input does not match
/// the expected format for the kind:
/// - `Integer`: must be a valid i64 (no leading zeros, no + sign, no whitespace, within i64 range)
/// - `Uuid`: must be a valid UUID (simple, URN, or braced forms accepted)
/// - `String`: accepts any string (no validation)
///
/// # Example
///
/// ```
/// use rolify_core::config::HolderIdKind;
/// use rolify_core::holder::parse_holder_id;
///
/// let parsed = parse_holder_id(HolderIdKind::Integer, "42").unwrap();
/// assert_eq!(parsed, ParsedHolderId::Integer(42));
/// ```
pub fn parse_holder_id(kind: HolderIdKind, input: &str) -> Result<ParsedHolderId, RolifyError> {
    match kind {
        HolderIdKind::Integer => parse_integer(input),
        HolderIdKind::Uuid => parse_uuid(input),
        HolderIdKind::String => Ok(ParsedHolderId::Text(input.to_owned())),
    }
}

/// Converts a parsed holder id back to its canonical string representation.
///
/// This is the inverse of `parse_holder_id` for valid inputs:
/// - `Integer(42)` -> "42"
/// - `Uuid(u)` -> canonical hyphenated UUID string
/// - `Text(s)` -> s (unchanged)
///
/// # Example
///
/// ```
/// use rolify_core::config::HolderIdKind;
/// use rolify_core::holder::{parse_holder_id, holder_id_to_string};
///
/// let parsed = parse_holder_id(HolderIdKind::Integer, "007").unwrap();
/// assert_eq!(holder_id_to_string(&parsed), "7");
/// ```
#[must_use]
pub fn holder_id_to_string(parsed: &ParsedHolderId) -> String {
    match parsed {
        ParsedHolderId::Integer(i) => i.to_string(),
        ParsedHolderId::Uuid(u) => u.to_string(),
        ParsedHolderId::Text(s) => s.clone(),
    }
}

fn parse_integer(input: &str) -> Result<ParsedHolderId, RolifyError> {
    // Reject empty
    if input.is_empty() {
        return Err(RolifyError::InvalidHolderId {
            expected: "integer",
            got: input.to_owned(),
        });
    }
    // Check for + sign
    if input.starts_with('+') {
        return Err(RolifyError::InvalidHolderId {
            expected: "integer",
            got: input.to_owned(),
        });
    }
    // Check for whitespace
    if input.chars().any(char::is_whitespace) {
        return Err(RolifyError::InvalidHolderId {
            expected: "integer",
            got: input.to_owned(),
        });
    }
    // Check for -0
    if input == "-0" {
        return Err(RolifyError::InvalidHolderId {
            expected: "integer",
            got: input.to_owned(),
        });
    }
    let value = input.parse::<i64>().map_err(|_| RolifyError::InvalidHolderId {
        expected: "integer",
        got: input.to_owned(),
    })?;
    Ok(ParsedHolderId::Integer(value))
}

fn parse_uuid(input: &str) -> Result<ParsedHolderId, RolifyError> {
    // uuid::Uuid::parse_str accepts simple, URN, and braced forms
    let uuid = Uuid::parse_str(input).map_err(|_| RolifyError::InvalidHolderId {
        expected: "uuid",
        got: input.to_owned(),
    })?;
    Ok(ParsedHolderId::Uuid(uuid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HolderIdKind;

    #[test]
    fn parse_integer_valid() {
        assert_eq!(
            parse_holder_id(HolderIdKind::Integer, "42"),
            Ok(ParsedHolderId::Integer(42))
        );
        assert_eq!(
            parse_holder_id(HolderIdKind::Integer, "0"),
            Ok(ParsedHolderId::Integer(0))
        );
        assert_eq!(
            parse_holder_id(HolderIdKind::Integer, "9223372036854775807"),
            Ok(ParsedHolderId::Integer(i64::MAX))
        );
        assert_eq!(
            parse_holder_id(HolderIdKind::Integer, "-1"),
            Ok(ParsedHolderId::Integer(-1))
        );
        assert_eq!(
            parse_holder_id(HolderIdKind::Integer, "-9223372036854775808"),
            Ok(ParsedHolderId::Integer(i64::MIN))
        );
    }

    #[test]
    fn parse_integer_re_canonicalizes_leading_zeros() {
        // "007" -> 7 (re-canonicalized)
        assert_eq!(
            parse_holder_id(HolderIdKind::Integer, "007"),
            Ok(ParsedHolderId::Integer(7))
        );
    }

    #[test]
    fn parse_integer_rejects_invalid() {
        // Empty
        assert!(matches!(
            parse_holder_id(HolderIdKind::Integer, ""),
            Err(RolifyError::InvalidHolderId { expected: "integer", .. })
        ));
        // Leading zeros - now re-canonicalized (e.g., "007" -> 7)
        assert_eq!(
            parse_holder_id(HolderIdKind::Integer, "007"),
            Ok(ParsedHolderId::Integer(7))
        );
        // Plus sign
        assert!(matches!(
            parse_holder_id(HolderIdKind::Integer, "+7"),
            Err(RolifyError::InvalidHolderId { expected: "integer", .. })
        ));
        // Whitespace
        assert!(matches!(
            parse_holder_id(HolderIdKind::Integer, " 7 "),
            Err(RolifyError::InvalidHolderId { expected: "integer", .. })
        ));
        // Overflow (i64::MAX + 1)
        assert!(matches!(
            parse_holder_id(HolderIdKind::Integer, "9223372036854775808"),
            Err(RolifyError::InvalidHolderId { expected: "integer", .. })
        ));
        // -0
        assert!(matches!(
            parse_holder_id(HolderIdKind::Integer, "-0"),
            Err(RolifyError::InvalidHolderId { expected: "integer", .. })
        ));
        // Non-numeric
        assert!(matches!(
            parse_holder_id(HolderIdKind::Integer, "abc"),
            Err(RolifyError::InvalidHolderId { expected: "integer", .. })
        ));
    }

    #[test]
    fn parse_uuid_valid() {
        let uuid_str = "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8";
        let parsed = parse_holder_id(HolderIdKind::Uuid, uuid_str).unwrap();
        assert_eq!(
            parsed,
            ParsedHolderId::Uuid(Uuid::parse_str(uuid_str).unwrap())
        );

        // URN form
        let urn = format!("urn:uuid:{uuid_str}");
        let parsed = parse_holder_id(HolderIdKind::Uuid, &urn).unwrap();
        assert_eq!(
            parsed,
            ParsedHolderId::Uuid(Uuid::parse_str(uuid_str).unwrap())
        );

        // Braced form
        let braced = format!("{{{uuid_str}}}");
        let parsed = parse_holder_id(HolderIdKind::Uuid, &braced).unwrap();
        assert_eq!(
            parsed,
            ParsedHolderId::Uuid(Uuid::parse_str(uuid_str).unwrap())
        );
    }

    #[test]
    fn parse_uuid_rejects_invalid() {
        assert!(matches!(
            parse_holder_id(HolderIdKind::Uuid, "not-a-uuid"),
            Err(RolifyError::InvalidHolderId { expected: "uuid", .. })
        ));
        assert!(matches!(
            parse_holder_id(HolderIdKind::Uuid, ""),
            Err(RolifyError::InvalidHolderId { expected: "uuid", .. })
        ));
        assert!(matches!(
            parse_holder_id(HolderIdKind::Uuid, "123"),
            Err(RolifyError::InvalidHolderId { expected: "uuid", .. })
        ));
    }

    #[test]
    fn parse_string_accepts_all() {
        assert_eq!(
            parse_holder_id(HolderIdKind::String, "anything"),
            Ok(ParsedHolderId::Text("anything".to_owned()))
        );
        assert_eq!(
            parse_holder_id(HolderIdKind::String, ""),
            Ok(ParsedHolderId::Text(String::new()))
        );
        assert_eq!(
            parse_holder_id(HolderIdKind::String, "007"),
            Ok(ParsedHolderId::Text("007".to_owned()))
        );
    }

    #[test]
    fn holder_id_to_string_round_trips() {
        // Integer
        let parsed = parse_holder_id(HolderIdKind::Integer, "42").unwrap();
        assert_eq!(holder_id_to_string(&parsed), "42");

        // Integer with leading zeros -> canonicalized
        let parsed = parse_holder_id(HolderIdKind::Integer, "007").unwrap();
        assert_eq!(holder_id_to_string(&parsed), "7");

        // UUID -> canonical hyphenated
        let uuid_str = "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8";
        let parsed = parse_holder_id(HolderIdKind::Uuid, uuid_str).unwrap();
        assert_eq!(holder_id_to_string(&parsed), uuid_str);

        // URN form -> canonical hyphenated
        let urn = format!("urn:uuid:{uuid_str}");
        let parsed = parse_holder_id(HolderIdKind::Uuid, &urn).unwrap();
        assert_eq!(holder_id_to_string(&parsed), uuid_str);

        // String -> unchanged
        let parsed = parse_holder_id(HolderIdKind::String, "user-1").unwrap();
        assert_eq!(holder_id_to_string(&parsed), "user-1");
    }

    #[test]
    fn default_holder_id_kind_is_integer() {
        assert_eq!(HolderIdKind::default(), HolderIdKind::Integer);
    }
}