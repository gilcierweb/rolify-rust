//! Phase 08 decision ledger for D-08-01 through D-08-08.
//!
//! Wave 0 (08-00) created one parametrized stub per locked decision as the
//! acceptance contract. The adapter plans (08-01 through 08-05) landed the
//! implementations together with their real coverage, so this file now:
//!
//! - asserts the four core-reachable contracts directly (config surface,
//!   shared default, generator/runtime coherence, boundary parse), and
//! - documents the four adapter/CLI-owned contracts with pointers to the
//!   suites that pin them:
//!   - D-08-04 column type map per engine and kind:
//!     `rolify-cli/tests/drift.rs::renderer_matches_canonical_all_kinds`
//!     (3 engines x 3 kinds render sweep against the canonical trees).
//!   - D-08-06 `SeaORM` runtime-typed `Value`:
//!     `rolify-seaorm/src/store.rs::holder_value` (`Value::BigInt`,
//!     `Value::Uuid` or `Value::String` per kind), exercised by the store's
//!     bind-path tests.
//!   - D-08-07 optional `--with-holder-fk` DDL emission:
//!     `rolify-cli/tests/generate.rs::generate_with_holder_fk` and
//!     `generate_with_holder_fk_string_noop`.
//!   - D-08-08 `MongoDB` canonical strings regardless of SQL kind:
//!     `rolify-mongodb/src/lib.rs::mongo_unchanged` regression guard (the
//!     document and bind path never branches on the kind).

use pretty_assertions::assert_eq;
use rolify_core::config::{HolderIdKind, RolifyConfig};
use rolify_core::error::RolifyError;
use rolify_core::holder::parse_holder_id;
use rstest::rstest;

#[cfg(feature = "faker")]
use faker_rust as _; // ensure the feature compiles for this binary

/// D-08-01: `HolderIdKind` is exposed through the public config API: every
/// kind builds through the builder knob and round-trips via the getter.
#[rstest]
#[case(HolderIdKind::Integer)]
#[case(HolderIdKind::Uuid)]
#[case(HolderIdKind::String)]
fn holder_id_kind_surface_exists(#[case] kind: HolderIdKind) {
    let config = RolifyConfig::builder()
        .holder_id_kind(kind)
        .build()
        .expect("config builds for every holder id kind");
    assert_eq!(config.holder_id_kind(), kind, "builder knob round-trips");
}

/// D-08-02: the shared default kind is `Integer` on the config side. The CLI
/// scaffold leg of the default is pinned by rolify-cli's generate suite
/// (the scaffold renders the integer tree without --holder-id-type).
#[test]
fn default_is_integer() {
    assert_eq!(HolderIdKind::default(), HolderIdKind::Integer);
    let config = RolifyConfig::builder()
        .build()
        .expect("default config builds");
    assert_eq!(config.holder_id_kind(), HolderIdKind::Integer);
}

/// D-08-03: one default shared by generator and runtime: the runtime
/// default equals the enum default that the generator reads. The CLI-side
/// leg (the default render matches the integer canonical tree) is pinned
/// by rolify-cli tests/drift.rs and tests/generate.rs.
#[test]
fn coherence_cli_config_default() {
    let config = RolifyConfig::builder()
        .build()
        .expect("default config builds");
    assert_eq!(
        config.holder_id_kind(),
        HolderIdKind::default(),
        "runtime and generator share the enum default"
    );
}

/// D-08-05: `InvalidHolderId` fires at the boundary parse, before any SQL
/// runs: every adapter routes holder ids through `parse_holder_id`
/// (diesel `parsed_for_store`, sqlx `holder_bind_value`, seaorm
/// `holder_value`), so rejecting here rejects pre-SQL everywhere.
///
/// Note: "007" is deliberately valid for the Integer kind (canonicalized
/// to "7", pinned by the `parse_holder_id` doctest), so it is absent from
/// the rejecting list despite appearing in the wave 0 draft.
#[rstest]
#[case(HolderIdKind::Integer, "abc")]
#[case(HolderIdKind::Integer, "+7")]
#[case(HolderIdKind::Integer, " 7 ")]
#[case(HolderIdKind::Integer, "9223372036854775808")]
#[case(HolderIdKind::Integer, "-0")]
#[case(HolderIdKind::Integer, "")]
#[case(HolderIdKind::Uuid, "not-a-uuid")]
#[case(HolderIdKind::Uuid, "")]
#[case(HolderIdKind::Uuid, "123")]
fn invalid_holder_id_at_boundary(#[case] kind: HolderIdKind, #[case] bad_input: &str) {
    let error = parse_holder_id(kind, bad_input)
        .expect_err("malformed holder id is rejected by the boundary parse");
    assert!(
        matches!(error, RolifyError::InvalidHolderId { .. }),
        "expected InvalidHolderId for kind {kind:?} and input {bad_input:?}, got {error:?}"
    );
}
