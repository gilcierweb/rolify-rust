//! BSON document shapes for `rolify-mongodb`.
//!
//! Port of the gem's Mongoid models
//! (`rolify/lib/generators/rolify/templates/role-mongoid.rb`, joined with the
//! runtime writes in `rolify/lib/rolify/adapters/mongoid/role_adapter.rb`):
//! the role document carries `name`, the scope pair, and `user_ids`; the
//! consumer document gains `role_ids` (D-08 two-sided HABTM).
//!
//! Scope rule (D-07): absent scope is stored as an **explicit null**, never
//! an empty string, and the fields are ALWAYS written. Mongo matches
//! `{field: null}` against both null and missing, so writes stay total and
//! filters stay stable; the SQL adapters' `''` sentinel never appears here.

use bson::{Bson, Document};
use rolify_core::role::{ResourceId, RoleName, RoleRecord};

use crate::error::Error;

/// The role document's `_id` type: driver-minted `ObjectId`, kept distinct
/// from the stringified `resource_id` (RESEARCH Q3: integer PKs and UUIDs
/// are stringified at the BSON boundary; `_id` is never conflated with them).
pub use bson::oid::ObjectId;

/// Wired schema field names. The BSON field-name injection boundary
/// (T-05.1-04) is closed by construction: every filter/update is assembled
/// from these literals plus typed values, never from raw consumer strings.
mod fields {
    pub(super) const ID: &str = "_id";
    pub(super) const NAME: &str = "name";
    pub(super) const RESOURCE_TYPE: &str = "resource_type";
    pub(super) const RESOURCE_ID: &str = "resource_id";
    pub(super) const USER_IDS: &str = "user_ids";
}

/// Role document: one role row as stored in Mongo.
///
/// Layout mirrors the SQL `roles` table columns. `user_ids` holds
/// stringified holder PKs (D-08): holder identity in rolify is
/// [`ResourceId`] (string-native, CORE-01), so the SQL
/// `users_roles.user_id` text conversion carries over 1:1.
///
/// `serde` derives are opt-in (QUAL-03 mirror); the store binds typed
/// values through [`RoleDoc::to_document`] / [`RoleDoc::from_document`], so
/// the derives stay a pure consumer convenience.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct RoleDoc {
    /// Driver-generated document id (`ObjectId`), distinct from `resource_id`."
    #[cfg_attr(feature = "serde", serde(rename = "_id"))]
    pub id: ObjectId,
    /// Role name, byte-exact (CORE-01; no normalization).
    pub name: String,
    /// Resource class name; explicit null (BSON) when `None`.
    pub resource_type: Option<String>,
    /// Stringified resource PK; explicit null (BSON) when `None`.
    pub resource_id: Option<String>,
    /// Stringified holder PKs linked to this role.
    pub user_ids: Vec<String>,
}

/// Minimal consumer-side link shape used by the adapter's two-sided writes
/// and by the suite's seeding: every consumer document gains a `role_ids`
/// array (`$addToSet`/`$pull` kept in step with the role document's
/// `user_ids`).
///
/// Consumer `_id` values stay opaque to the adapter: holders are addressed
/// by their stringified PK only (RESEARCH Q3), so this shape carries typed
/// `ObjectId` role ids plus no knowledge of the consumer's own id type.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HolderLinkDoc {
    /// Stringified holder PK (the consumer's own identity in rolify).
    pub holder_id: String,
    /// Role document `_id`s linked to this holder.
    pub role_ids: Vec<ObjectId>,
}

impl RoleDoc {
    /// Create a document for `record`, minting a fresh `_id`. Insert is the
    /// only entry point for new roles: the unique compound index (plus the
    /// 11000 race catch, D-14) is the dedupe arbiter.
    #[must_use]
    pub fn from_record(record: &RoleRecord) -> Self {
        Self {
            id: ObjectId::new(),
            name: record.name.to_string(),
            resource_type: record.resource_type.clone(),
            resource_id: record.resource_id.as_ref().map(ToString::to_string),
            user_ids: Vec::new(),
        }
    }

    /// Convert back to a core [`RoleRecord`]; `user_ids`/`_id` do not exist
    /// on the core type and stay document-local.
    #[must_use]
    pub fn to_record(&self) -> RoleRecord {
        RoleRecord::new(
            RoleName::new(self.name.clone()),
            self.resource_type.clone(),
            self.resource_id.clone().map(ResourceId::new),
        )
    }

    /// Assemble the full document for insertion. Scope fields are ALWAYS
    /// present: `None` maps to `Bson::Null` (D-07), never to a missing key
    /// and never to the SQL `''` sentinel. Public for consumers that drive
    /// raw `Collection<bson::Document>` work alongside the store.
    #[must_use]
    pub fn to_document(&self) -> Document {
        let mut document = Document::new();
        document.insert(fields::ID, self.id);
        document.insert(fields::NAME, self.name.clone());
        document.insert(
            fields::RESOURCE_TYPE,
            scope_field(self.resource_type.as_deref()),
        );
        document.insert(
            fields::RESOURCE_ID,
            scope_field(self.resource_id.as_deref()),
        );
        document.insert(fields::USER_IDS, self.user_ids.clone());
        document
    }

    /// Read a persisted document back. Missing scope keys read as `None`
    /// (Mongo `{field: null}` equality keeps filters total either way).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Mongo`] when a stored field carries an unexpected
    /// BSON type (that is a schema-corruption path, not a normal lookup).
    pub fn from_document(document: &Document) -> Result<Self, Error> {
        Ok(Self {
            id: match document.get(fields::ID) {
                Some(Bson::ObjectId(id)) => *id,
                Some(other) => return Err(unexpected(fields::ID, "objectId", other)),
                None => return Err(unexpected(fields::ID, "objectId", &Bson::Undefined)),
            },
            name: match document.get(fields::NAME) {
                Some(Bson::String(name)) => name.clone(),
                other => {
                    return Err(unexpected(
                        fields::NAME,
                        "string",
                        other.unwrap_or(&Bson::Undefined),
                    ));
                }
            },
            resource_type: scope_option(document, fields::RESOURCE_TYPE)?,
            resource_id: scope_option(document, fields::RESOURCE_ID)?,
            user_ids: match document.get(fields::USER_IDS) {
                Some(Bson::Array(ids)) => ids
                    .iter()
                    .map(|id| match id {
                        Bson::String(holder) => Ok(holder.clone()),
                        other => Err(unexpected(fields::USER_IDS, "string", other)),
                    })
                    .collect::<Result<Vec<String>, Error>>()?,
                Some(other) => return Err(unexpected(fields::USER_IDS, "array", other)),
                None => Vec::new(),
            },
        })
    }
}

/// Explicit-null scope rule: `Option<&str>` -> BSON, `None` as `Bson::Null`.
fn scope_field(value: Option<&str>) -> Bson {
    value.map_or(Bson::Null, |value| Bson::String(value.to_owned()))
}

/// Read a scope field: `Bson::Null` and missing both map to `None`,
/// matching Mongo null equality, so a half-applied legacy write still reads.
fn scope_option(document: &Document, field: &str) -> Result<Option<String>, Error> {
    match document.get(field) {
        Some(Bson::Null) | None => Ok(None),
        Some(Bson::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(unexpected(field, "string|null", other)),
    }
}

/// Schema corruption surfaces as a driver-shaped custom error; the plan's
/// error contract (`Error::Mongo` + `Error::Core`, `non_exhaustive`) keeps
/// deserialization failures inside the Mongo variant.
fn unexpected(field: &str, expected: &str, actual: &Bson) -> Error {
    Error::Mongo(mongodb::error::Error::custom(format!(
        "role document field `{field}`: expected {expected}, found {actual:?}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolify_core::role::RoleRecord;

    #[test]
    fn global_role_doc_writes_explicit_null_scope() {
        let doc = RoleDoc::from_record(&RoleRecord::global("admin")).to_document();
        assert_eq!(doc.get(fields::RESOURCE_TYPE), Some(&Bson::Null));
        assert_eq!(doc.get(fields::RESOURCE_ID), Some(&Bson::Null));
        // The SQL `''` sentinel must never appear (D-07).
        assert_ne!(
            doc.get(fields::RESOURCE_TYPE),
            Some(&Bson::String(String::new()))
        );
        // Scope keys are always present, never omitted.
        assert!(doc.get(fields::RESOURCE_TYPE).is_some());
        assert!(doc.get(fields::RESOURCE_ID).is_some());
    }

    #[test]
    fn roundtrip_preserves_name_and_scope() {
        let record = RoleRecord::for_class("manager", "Forum");
        let doc = RoleDoc::from_record(&record);
        assert_eq!(doc.to_record(), record);
    }

    #[test]
    fn stringified_integer_and_uuid_resource_ids_roundtrip() {
        for raw in ["42", "3d5b2f1e-2f34-4f3e-9a3e-9f7d2c1a5b6d"] {
            let record = RoleRecord::for_instance("viewer", "Team", raw);
            let doc = RoleDoc::from_record(&record);
            assert_eq!(doc.resource_id.as_deref(), Some(raw));
            assert_eq!(doc.to_record(), record);
        }
    }

    #[test]
    fn document_roundtrip_through_bson_keeps_explicit_nulls() {
        let record = RoleRecord::global("admin");
        let doc = RoleDoc::from_record(&record).to_document();
        let back = RoleDoc::from_document(&doc).expect("document deserializes");
        assert_eq!(back.to_record(), record);
        assert_eq!(back.resource_type, None);
        assert_eq!(back.resource_id, None);
        assert!(back.user_ids.is_empty());
    }

    #[test]
    fn document_roundtrip_keeps_user_ids_and_scope_values() {
        let mut doc = RoleDoc::from_record(&RoleRecord::for_class("manager", "Forum"));
        doc.user_ids = vec!["1".to_owned(), "external-key".to_owned()];
        let back = RoleDoc::from_document(&doc.to_document()).expect("document deserializes");
        assert_eq!(back.user_ids, ["1".to_owned(), "external-key".to_owned()]);
        assert_eq!(back.resource_type.as_deref(), Some("Forum"));
    }

    #[test]
    fn holder_link_doc_collects_object_ids() {
        let first = ObjectId::new();
        let second = ObjectId::new();
        let link = HolderLinkDoc {
            holder_id: "7".to_owned(),
            role_ids: vec![first, second],
        };
        assert_eq!(link.role_ids.len(), 2);
        assert!(link.role_ids.contains(&first));
    }

    /// QUAL-03/D-07 axis: with the `serde` feature the derives round-trip
    /// `resource_id` as a BSON string (or explicit null) while `_id` stays
    /// an ObjectId - RESEARCH Q3 on the serde axis.
    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip_keeps_nulls_and_objectId() {
        let mut doc = RoleDoc::from_record(&RoleRecord::for_instance("viewer", "Team", "team-7"));
        doc.user_ids = vec!["1".to_owned()];
        let serialized = bson::to_document(&doc).expect("serialize");
        assert_eq!(
            serialized.get(fields::RESOURCE_TYPE),
            Some(&Bson::String("Team".to_owned()))
        );
        assert_eq!(
            serialized.get(fields::RESOURCE_ID),
            Some(&Bson::String("team-7".to_owned()))
        );
        assert!(matches!(
            serialized.get(fields::ID),
            Some(Bson::ObjectId(_))
        ));
        let class_doc = RoleDoc::from_record(&RoleRecord::for_class("manager", "Forum"));
        let class_serialized = bson::to_document(&class_doc).expect("serialize class doc");
        assert_eq!(
            class_serialized.get(fields::RESOURCE_ID),
            Some(&Bson::Null),
            "absent scope persists as explicit null (serde axis)"
        );
        assert!(matches!(
            class_serialized.get(fields::ID),
            Some(Bson::ObjectId(_))
        ));
        let decoded: RoleDoc = bson::from_document(class_serialized).expect("deserialize");
        assert_eq!(decoded.to_record(), class_doc.to_record());
    }
}
