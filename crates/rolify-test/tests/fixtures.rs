//! Shared test fixtures for Phase 08 holder id kind matrix.
//!
//! Provides valid and invalid holder id vectors per kind (Integer, Uuid, String)
//! for use across all adapter test crates. Uses `faker-rust` behind the
//! `faker-rust` feature for dynamic generation; provides const fallbacks when
//! the feature is off.

/// Valid integer holder ids (i64 range).
#[must_use]
pub fn valid_integer_ids() -> Vec<String> {
    vec!["1".to_owned(), "42".to_owned(), "9223372036854775807".to_owned()]
}

/// Valid UUID holder ids (simple, URN, and braced forms).
#[must_use]
pub fn valid_uuid_ids() -> Vec<String> {
    vec![
        "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8".to_owned(),
        "urn:uuid:a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8".to_owned(),
        "{a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8}".to_owned(),
    ]
}

/// Valid string holder ids (arbitrary strings accepted by String kind).
#[must_use]
pub fn valid_string_ids() -> Vec<String> {
    vec!["user-1".to_owned(), "team-alpha".to_owned(), "custom-pk".to_owned()]
}

/// Invalid integer inputs (should produce InvalidHolderId).
#[must_use]
pub fn invalid_integer_inputs() -> Vec<String> {
    vec![
        "abc".to_owned(),
        "007".to_owned(),
        "+7".to_owned(),
        " 7 ".to_owned(),
        "9223372036854775808".to_owned(), // i64::MAX + 1
        "-0".to_owned(),
        "".to_owned(),
    ]
}

/// Invalid UUID inputs (should produce InvalidHolderId).
#[must_use]
pub fn invalid_uuid_inputs() -> Vec<String> {
    vec!["not-a-uuid".to_owned(), "".to_owned(), "123".to_owned()]
}

/// Invalid string inputs (String kind accepts all, so empty).
#[must_use]
pub fn invalid_string_inputs() -> Vec<String> {
    vec![]
}

/// Generate a random valid integer id (requires `faker-rust` feature).
#[cfg(feature = "faker-rust")]
#[must_use]
pub fn random_valid_integer_id() -> String {
    faker_rust::number::between(1, i64::MAX as u64).to_string()
}

/// Generate a random valid UUID id (requires `faker-rust` feature).
#[cfg(feature = "faker-rust")]
#[must_use]
pub fn random_valid_uuid_id() -> String {
    use uuid::Uuid;
    Uuid::new_v4().to_string()
}

/// Generate a random valid string id (requires `faker-rust` feature).
#[cfg(feature = "faker-rust")]
#[must_use]
pub fn random_valid_string_id() -> String {
    format!(
        "{}-{}",
        faker_rust::internet::username(None),
        faker_rust::number::between(1, 9999)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_integer_ids_not_empty() {
        let ids = valid_integer_ids();
        assert!(!ids.is_empty());
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn valid_uuid_ids_not_empty() {
        let ids = valid_uuid_ids();
        assert!(!ids.is_empty());
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn valid_string_ids_not_empty() {
        let ids = valid_string_ids();
        assert!(!ids.is_empty());
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn invalid_integer_inputs_not_empty() {
        let ids = invalid_integer_inputs();
        assert!(!ids.is_empty());
        assert_eq!(ids.len(), 7);
    }

    #[test]
    fn invalid_uuid_inputs_not_empty() {
        let ids = invalid_uuid_inputs();
        assert!(!ids.is_empty());
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn invalid_string_inputs_is_empty() {
        let ids = invalid_string_inputs();
        assert!(ids.is_empty());
    }
}