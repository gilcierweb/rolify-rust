#![cfg(feature = "serde")]
//! serde roundtrips (QUAL-03): JSON the types survive EXACTLY - byte-exact
//! name/id preservation, no normalization anywhere (case-fold would be a
//! role-name spoofing vector).

use rolify_core::role::{ResourceId, RoleName, RoleRecord, RoleSet};

#[test]
fn role_name_roundtrips_byte_exact() {
    let name = RoleName::from("Admin");
    let json = serde_json::to_string(&name).unwrap();
    assert_eq!(json, "\"Admin\"");
    assert_eq!(serde_json::from_str::<RoleName>(&json).unwrap(), name);
}

#[test]
fn resource_id_roundtrips_byte_exact() {
    let id = ResourceId::from("T-42");
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"T-42\"");
    assert_eq!(serde_json::from_str::<ResourceId>(&json).unwrap(), id);
}

#[test]
fn role_record_roundtrips_all_scopes() {
    let id = ResourceId::from(7_i64);
    for record in [
        RoleRecord::global("admin"),
        RoleRecord::for_class("manager", "Forum"),
        RoleRecord::for_instance("moderator", "Forum", id),
    ] {
        let json = serde_json::to_string(&record).unwrap();
        let back: RoleRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }
    // explicit shape: names stay byte-exact inside the record
    let json = serde_json::to_string(&RoleRecord::for_class("Admin", "Forum")).unwrap();
    assert!(json.contains("\"Admin\""));
    assert!(!json.contains("\"admin\""));
}

#[test]
fn role_set_serializes_its_snapshot() {
    let rows = vec![
        RoleRecord::global("admin"),
        RoleRecord::for_class("manager", "Forum"),
    ];
    let set = RoleSet::new(&rows);
    let json = serde_json::to_value(set).unwrap();
    let expected = serde_json::json!({
        "rows": [
            {"name": "admin", "resource_type": null, "resource_id": null},
            {"name": "manager", "resource_type": "Forum", "resource_id": null},
        ]
    });
    assert_eq!(json, expected);
}
