//! Acceptance tests for `pack.yaml` descriptor inspection.
//!
//! Contract mirror: `src/charter/offering/packs/pack_descriptor.py`. The
//! descriptor carries identity (`pack_id`), the declared tier, and optional
//! lineage/layout pointers. A declared tier that disagrees with the configured
//! position is a fail-closed error, not a silent reclassification.

mod support;

use std::path::Path;

use sk_charter::{CharterError, Limits, PackTier, load_pack_descriptor};
use support::{BUILTIN_PACK_ID, mkdir, write};

fn descriptor_path(root: &Path) -> std::path::PathBuf {
    root.join("pack.yaml")
}

#[test]
fn loads_a_built_in_descriptor_with_identity_and_layout_pointers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("built-in");
    write(
        &descriptor_path(&root),
        &format!(
            "pack_id: {BUILTIN_PACK_ID}\npack_version: 4.0.0\nname: built-in\npack_level: built-in\nrequires_core: '>=4.0,<5.0'\nactive_charter: org-charter.yaml\npack_drg: drg/graph.yaml\nparent_pack: null\n"
        ),
    );

    let descriptor = load_pack_descriptor(PackTier::BuiltIn, &root, &Limits::default()).unwrap();
    assert_eq!(descriptor.tier, PackTier::BuiltIn);
    assert_eq!(descriptor.pack_level, PackTier::BuiltIn);
    assert!(!descriptor.level_inferred);
    assert_eq!(descriptor.pack_id.as_deref(), Some(BUILTIN_PACK_ID));
    assert_eq!(descriptor.pack_version.as_deref(), Some("4.0.0"));
    assert_eq!(descriptor.name, "built-in");
    assert_eq!(descriptor.requires_core.as_deref(), Some(">=4.0,<5.0"));
    assert_eq!(
        descriptor.active_charter.as_deref(),
        Some("org-charter.yaml")
    );
    assert_eq!(descriptor.pack_drg.as_deref(), Some("drg/graph.yaml"));
    assert_eq!(descriptor.parent_pack, None);
    assert_eq!(descriptor.root, root);
    assert_eq!(descriptor.path, descriptor_path(&root));
}

#[test]
fn a_missing_descriptor_is_a_typed_missing_error() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("org");
    mkdir(&root);
    let error = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap_err();
    match error {
        CharterError::PackDescriptorMissing {
            tier,
            root: missing,
        } => {
            assert_eq!(tier, PackTier::Org);
            assert_eq!(missing, root);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_declared_tier_that_disagrees_with_the_configured_tier_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("org");
    write(
        &descriptor_path(&root),
        &format!("pack_id: {BUILTIN_PACK_ID}\nname: team\npack_level: built-in\n"),
    );
    let error = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap_err();
    match error {
        CharterError::PackLevelMismatch {
            declared,
            expected,
            path,
        } => {
            assert_eq!(declared, PackTier::BuiltIn);
            assert_eq!(expected, PackTier::Org);
            assert_eq!(path, descriptor_path(&root));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_missing_pack_level_is_inferred_from_the_configured_position() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("org");
    write(
        &descriptor_path(&root),
        &format!("pack_id: {BUILTIN_PACK_ID}\nname: team\n"),
    );
    let descriptor = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap();
    assert_eq!(descriptor.pack_level, PackTier::Org);
    assert!(descriptor.level_inferred);
}

#[test]
fn a_built_in_descriptor_requires_a_pack_version() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("built-in");
    write(
        &descriptor_path(&root),
        &format!("pack_id: {BUILTIN_PACK_ID}\nname: built-in\npack_level: built-in\n"),
    );
    let error = load_pack_descriptor(PackTier::BuiltIn, &root, &Limits::default()).unwrap_err();
    match error {
        CharterError::InvalidPackDescriptor { reason, .. } => {
            assert!(reason.contains("pack_version"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn an_invalid_pack_id_is_rejected_but_a_missing_one_is_tolerated_during_backfill() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("org");
    write(
        &descriptor_path(&root),
        "pack_id: not-a-ulid\nname: team\npack_level: org\n",
    );
    let error = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap_err();
    match error {
        CharterError::InvalidPackId { value, .. } => assert_eq!(value, "not-a-ulid"),
        other => panic!("unexpected error: {other}"),
    }

    let root = temp.path().join("org2");
    write(&descriptor_path(&root), "name: team\npack_level: org\n");
    let descriptor = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap();
    assert_eq!(descriptor.pack_id, None);
}

#[test]
fn corrupt_or_non_mapping_descriptors_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bad");
    write(&descriptor_path(&root), "name: ['unterminated\n");
    let error = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::InvalidYaml { .. }));

    let root = temp.path().join("list");
    write(&descriptor_path(&root), "- a\n- b\n");
    let error = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::ExpectedMapping { .. }));
}

#[test]
fn unknown_or_malformed_descriptor_fields_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for (label, body) in [
        ("unknown field", "name: x\npack_level: org\nsurprise: 1\n"),
        ("missing name", "pack_level: org\n"),
        ("bad level", "name: x\npack_level: staging\n"),
    ] {
        let root = temp.path().join(label.replace(' ', "-"));
        write(&descriptor_path(&root), body);
        let error = load_pack_descriptor(PackTier::Org, &root, &Limits::default()).unwrap_err();
        match error {
            CharterError::InvalidPackDescriptor { .. } => {}
            other => panic!("{label}: unexpected error: {other}"),
        }
    }
}

#[test]
fn oversized_descriptors_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("org");
    write(
        &descriptor_path(&root),
        &format!(
            "name: team\npack_level: org\npadding: '{}'\n",
            "x".repeat(2048)
        ),
    );
    let limits = Limits::default().with_max_pack_descriptor_bytes(1024);
    let error = load_pack_descriptor(PackTier::Org, &root, &limits).unwrap_err();
    match error {
        CharterError::TooLarge { path, limit } => {
            assert_eq!(path, descriptor_path(&root));
            assert_eq!(limit, 1024);
        }
        other => panic!("unexpected error: {other}"),
    }
}
