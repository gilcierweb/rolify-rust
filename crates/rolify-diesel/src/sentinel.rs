//! Sentinel translation: `None` ↔ `''` at the adapter boundary (D-02).
//!
//! The core SPI uses `Option<String>` / `Option<ResourceId>` for scope
//! columns (semantic `None` = global/class). The physical storage uses
//! the sentinel empty string `''` for both columns so the `UNIQUE` triple
//! constraint deduplicates identically on all three engines.
//!
//! This module owns BOTH directions:
//! - Write side: `Option<&str>` → `&str` (None → SCOPE_SENTINEL)
//! - Read side: `&str` → `Option<String>` (SCOPE_SENTINEL → None)
//!
//! The sentinel never crosses the SPI boundary — `RoleRecord` keeps
//! `Option` semantics throughout the kernel and consumer API.

use rolify_core::role::SCOPE_SENTINEL;
use rolify_core::role::ResourceId;

/// Convert an optional scope column value to its physical storage form.
///
/// `None` (global/class scope) becomes the sentinel `''`.
/// `Some(value)` becomes `value` unchanged.
#[inline]
pub fn to_storage(value: Option<&str>) -> &str {
    value.unwrap_or(SCOPE_SENTINEL)
}

/// Convert an optional ResourceId to its physical storage form.
#[inline]
pub fn resource_id_to_storage(value: Option<&ResourceId>) -> &str {
    value.map(ResourceId::as_str).unwrap_or(SCOPE_SENTINEL)
}

/// Convert a physical storage value back to the semantic `Option`.
///
/// The sentinel `''` becomes `None`. Any other value becomes `Some(value)`.
#[inline]
pub fn from_storage(value: &str) -> Option<String> {
    if value == SCOPE_SENTINEL {
        None
    } else {
        Some(value.to_owned())
    }
}

/// Convert a physical storage value back to `Option<ResourceId>`.
#[inline]
pub fn resource_id_from_storage(value: &str) -> Option<ResourceId> {
    if value == SCOPE_SENTINEL {
        None
    } else {
        Some(ResourceId::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolify_core::role::ResourceId;

    #[test]
    fn to_storage_none_returns_sentinel() {
        assert_eq!(to_storage(None), "");
        assert_eq!(resource_id_to_storage(None), "");
    }

    #[test]
    fn to_storage_some_returns_value() {
        assert_eq!(to_storage(Some("Forum")), "Forum");
        assert_eq!(resource_id_to_storage(Some(&ResourceId::from("42"))), "42");
    }

    #[test]
    fn from_storage_sentinel_returns_none() {
        assert_eq!(from_storage(""), None);
        assert_eq!(resource_id_from_storage(""), None);
    }

    #[test]
    fn from_storage_value_returns_some() {
        assert_eq!(from_storage("Forum"), Some("Forum".to_owned()));
        assert_eq!(resource_id_from_storage("42"), Some(ResourceId::from("42")));
    }

    #[test]
    fn roundtrip_preserves_semantics() {
        // None → sentinel → None
        assert_eq!(from_storage(&to_storage(None)), None);
        // Some → value → Some
        assert_eq!(from_storage(&to_storage(Some("Forum"))), Some("Forum".to_owned()));
        // ResourceId roundtrip
        let id = ResourceId::from("123");
        assert_eq!(
            resource_id_from_storage(&resource_id_to_storage(Some(&id))),
            Some(id)
        );
    }
}