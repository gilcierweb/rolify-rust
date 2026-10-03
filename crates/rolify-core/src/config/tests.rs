//! Unit tests for `config.rs` (struct/defaults/validation) - callback
//! choreography lives in `config/callbacks.rs` (CONF-05).

use super::*;

#[test]
fn defaults_match_the_gem() {
    let config = RolifyConfig::default();
    assert!(!config.strict()); // rolify.rb:35 - strict is opt-in
    assert!(config.remove_role_if_empty()); // configure.rb:5
    assert_eq!(config.role_table(), "roles");
    assert_eq!(config.join_table(), "users_roles");
}

#[test]
fn builder_defaults_via_build() {
    let config = RolifyConfig::builder().build().unwrap();
    assert!(!config.strict());
    assert!(config.remove_role_if_empty());
}

#[test]
fn builder_applies_overrides() {
    let config = RolifyConfig::builder()
        .strict(true)
        .remove_role_if_empty(false)
        .role_table("privileges")
        .join_table("customers_privileges")
        .build()
        .unwrap();
    assert!(config.strict());
    assert!(!config.remove_role_if_empty());
    assert_eq!(config.role_table(), "privileges");
    assert_eq!(config.join_table(), "customers_privileges");
}

#[test]
fn empty_role_table_is_invalid_config() {
    let outcome = RolifyConfig::builder().role_table("").build();
    assert!(matches!(
        outcome,
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("role_table")
    ));
}

#[test]
fn empty_join_table_is_invalid_config() {
    let outcome = RolifyConfig::builder().join_table("").build();
    assert!(matches!(
        outcome,
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("join_table")
    ));
}

#[test]
fn strict_engages_for_only_on_class_or_instance() {
    let id = crate::role::ResourceId::from(1_i64);
    let strict = RolifyConfig::builder().strict(true).build().unwrap();
    let relaxed = RolifyConfig::default();

    assert!(strict.strict_engages_for(&ResourceFilter::Class("Forum")));
    assert!(strict.strict_engages_for(&ResourceFilter::Instance("Forum", &id)));
    assert!(!strict.strict_engages_for(&ResourceFilter::Global));
    assert!(!strict.strict_engages_for(&ResourceFilter::Any));

    assert!(!relaxed.strict_engages_for(&ResourceFilter::Class("Forum")));
    assert!(!relaxed.strict_engages_for(&ResourceFilter::Instance("Forum", &id)));
}

#[test]
fn unset_hooks_no_op_successfully() {
    let config = RolifyConfig::default();
    let record = RoleRecord::global("admin");
    config.run_before_add(&record).unwrap();
    config.run_before_remove(&record).unwrap();
    config.run_after_add(&record);
    config.run_after_remove(&record);
}

#[test]
fn config_and_builder_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RolifyConfig>();
    assert_send_sync::<RolifyConfigBuilder>();
    assert_send_sync::<BeforeHook>();
    assert_send_sync::<AfterHook>();
}

#[test]
fn validate_identifier_accepts_valid_names() {
    assert!(RolifyConfigBuilder::validate_identifier("roles").is_ok());
    assert!(RolifyConfigBuilder::validate_identifier("users_roles").is_ok());
    assert!(RolifyConfigBuilder::validate_identifier("Admin_Moderator_rights").is_ok());
    assert!(RolifyConfigBuilder::validate_identifier("_private").is_ok());
    assert!(RolifyConfigBuilder::validate_identifier("table123").is_ok());
}

#[test]
fn validate_identifier_rejects_invalid_names() {
    // starts with digit
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("1roles"),
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("1roles")
    ));
    // contains space
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("ta ble"),
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("ta ble")
    ));
    // contains hyphen
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("bad-name"),
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("bad-name")
    ));
    // contains quote
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier("tab\"le"),
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("tab\"le")
    ));
    // empty string
    assert!(matches!(
        RolifyConfigBuilder::validate_identifier(""),
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("empty")
    ));
}

#[test]
fn build_rejects_invalid_role_table() {
    let outcome = RolifyConfig::builder().role_table("bad-name").build();
    assert!(matches!(
        outcome,
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("bad-name")
    ));
}

#[test]
fn build_rejects_invalid_join_table() {
    let outcome = RolifyConfig::builder().join_table("bad-name").build();
    assert!(matches!(
        outcome,
        Err(RolifyError::InvalidConfig { ref reason }) if reason.contains("bad-name")
    ));
}

#[test]
fn build_accepts_valid_custom_names() {
    let config = RolifyConfig::builder()
        .role_table("privileges")
        .join_table("customers_privileges")
        .build()
        .unwrap();
    assert_eq!(config.role_table(), "privileges");
    assert_eq!(config.join_table(), "customers_privileges");
}

#[test]
fn validate_identifier_is_public_api() {
    // This test ensures validate_identifier is reachable from the crate's public API
    use crate::config::RolifyConfigBuilder;
    assert!(RolifyConfigBuilder::validate_identifier("valid_name").is_ok());
}
