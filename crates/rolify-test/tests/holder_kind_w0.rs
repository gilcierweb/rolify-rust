//! Wave 0 validation stubs for Phase 08 decisions D-08-01 through D-08-08.
//!
//! Each stub is a parametrized test over `HolderIdKind` variants that initially
//! panics with `unimplemented!()`. The test signatures and fixtures are the
//! contract that all adapter plans (02-04) extend. When a decision's
//! implementation lands, the corresponding stub is replaced with a real test.

use rolify_core::config::HolderIdKind;
use rstest::rstest;

#[cfg(feature = "faker")]
use faker_rust as _; // ensure feature compiles

/// D-08-01: CLI flag + config knob surface exists.
#[rstest]
#[case(HolderIdKind::Integer)]
#[case(HolderIdKind::Uuid)]
#[case(HolderIdKind::String)]
fn holder_id_kind_surface_exists(#[case] _kind: HolderIdKind) {
    unimplemented!("D-08-01: HolderIdKind enum exposed in public config API")
}

/// D-08-02: Default is Integer in both CLI and config.
#[test]
fn default_is_integer() {
    unimplemented!("D-08-02: Default `integer` in config builder and CLI scaffold")
}

/// D-08-03: Coherence — CLI default matches config default.
#[test]
fn coherence_cli_config_default() {
    unimplemented!("D-08-03: One default shared by generator and runtime")
}

/// D-08-04: Column type map per engine/kind.
#[rstest]
#[case(HolderIdKind::Integer, "postgres")]
#[case(HolderIdKind::Uuid, "postgres")]
#[case(HolderIdKind::String, "postgres")]
#[case(HolderIdKind::Integer, "mysql")]
#[case(HolderIdKind::Uuid, "mysql")]
#[case(HolderIdKind::String, "mysql")]
#[case(HolderIdKind::Integer, "sqlite")]
#[case(HolderIdKind::Uuid, "sqlite")]
#[case(HolderIdKind::String, "sqlite")]
fn column_type_map(#[case] _kind: HolderIdKind, #[case] _engine: &str) {
    unimplemented!("D-08-04: Column type map per engine: BIGINT / UUID / BINARY(16) / TEXT / VARCHAR(191)")
}

/// D-08-05: InvalidHolderId at adapter boundary before DB.
#[rstest]
#[case(HolderIdKind::Integer, "abc")]
#[case(HolderIdKind::Integer, "007")]
#[case(HolderIdKind::Integer, "+7")]
#[case(HolderIdKind::Integer, " 7 ")]
#[case(HolderIdKind::Integer, "9223372036854775808")]
#[case(HolderIdKind::Integer, "-0")]
#[case(HolderIdKind::Integer, "")]
#[case(HolderIdKind::Uuid, "not-a-uuid")]
#[case(HolderIdKind::Uuid, "")]
#[case(HolderIdKind::Uuid, "123")]
fn invalid_holder_id_at_boundary(#[case] _kind: HolderIdKind, #[case] _bad_input: &str) {
    unimplemented!("D-08-05: InvalidHolderId error at adapter boundary before DB")
}

/// D-08-06: SeaORM raw Statement with runtime Value.
#[rstest]
#[case(HolderIdKind::Integer)]
#[case(HolderIdKind::Uuid)]
#[case(HolderIdKind::String)]
fn seaorm_raw_statement_runtime_value(#[case] _kind: HolderIdKind) {
    unimplemented!("D-08-06: SeaORM raw Statement with runtime-typed Value per kind")
}

/// D-08-07: Optional --with-holder-fk emits REFERENCES.
#[rstest]
#[case(HolderIdKind::Integer, true)]
#[case(HolderIdKind::Integer, false)]
#[case(HolderIdKind::Uuid, true)]
#[case(HolderIdKind::Uuid, false)]
#[case(HolderIdKind::String, true)]
#[case(HolderIdKind::String, false)]
fn with_holder_fk_emits_ref(#[case] _kind: HolderIdKind, #[case] _enabled: bool) {
    unimplemented!("D-08-07: Optional --with-holder-fk emits REFERENCES in DDL")
}

/// D-08-08: MongoDB stores canonical strings regardless.
#[rstest]
#[case(HolderIdKind::Integer)]
#[case(HolderIdKind::Uuid)]
#[case(HolderIdKind::String)]
fn mongo_canonical_strings(#[case] _kind: HolderIdKind) {
    unimplemented!("D-08-08: MongoDB stores canonical strings regardless of SQL kind")
}