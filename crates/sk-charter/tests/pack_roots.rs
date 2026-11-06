//! Acceptance tests for built-in pack root discovery and tier precedence roots.
//!
//! Contract mirror: `src/charter/offering/pack_paths.py` plus
//! `src/kernel/paths.py::get_built_in_pack_root`. Resolution is fail-closed: an
//! unsynced pinned checkout is `BuiltInPackMissing`, a genuinely absent tree is
//! `PackRootNotFound`, and a kind with no shipped content directory is refused
//! by identity rather than joined to a non-existent path.

mod support;

use std::fs;
use std::path::Path;

use sk_charter::{
    ArtifactKind, BuiltInSearch, CharterError, Limits, PackRoots, PackTier, PathExpansion,
    ProjectConfig,
};
use support::{BUILTIN_PACK_ID, mkdir, pack_yaml, write};

fn canonical(path: &Path) -> std::path::PathBuf {
    fs::canonicalize(path).expect("canonical fixture path")
}

fn built_in_tree(root: &Path) {
    write(
        &root.join("packs/built-in/pack.yaml"),
        &pack_yaml("built-in", "built-in", BUILTIN_PACK_ID, Some("1.0.0")),
    );
    mkdir(&root.join("packs/built-in/directives"));
}

#[test]
fn ancestor_walk_finds_the_nearest_built_in_tree() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    built_in_tree(&checkout);
    let anchor = checkout.join("src/nested/file.rs");
    mkdir(anchor.parent().unwrap());

    let search = BuiltInSearch::new(&anchor);
    assert_eq!(
        search.find_root().unwrap(),
        canonical(&checkout.join("packs/built-in"))
    );
}

#[test]
fn env_override_wins_over_the_ancestor_walk() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    built_in_tree(&checkout);
    let override_packs = temp.path().join("relocated/packs");
    write(
        &override_packs.join("built-in/pack.yaml"),
        &pack_yaml("built-in", "built-in", BUILTIN_PACK_ID, Some("9.9.9")),
    );

    let search =
        BuiltInSearch::new(checkout.join("src/lib.rs")).with_packs_root_env(&override_packs);
    assert_eq!(
        search.find_root().unwrap(),
        canonical(&override_packs.join("built-in"))
    );
}

#[test]
fn a_broken_env_override_falls_back_with_a_recorded_warning() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    built_in_tree(&checkout);
    let missing = temp.path().join("does-not-exist");

    let search = BuiltInSearch::new(checkout.join("src")).with_packs_root_env(&missing);
    let root = search.find_root().unwrap();
    assert_eq!(root, canonical(&checkout.join("packs/built-in")));
    assert_eq!(search.warnings().len(), 1);
    assert!(search.warnings()[0].contains("does-not-exist"));
}

#[test]
fn pinned_but_unsynced_checkout_is_built_in_pack_missing() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    write(
        &checkout.join("packs/built-in.source.yaml"),
        "pack: built-in\nref: 0123456789abcdef\n",
    );

    let error = BuiltInSearch::new(checkout.join("src"))
        .find_root()
        .unwrap_err();
    assert!(matches!(error, CharterError::BuiltInPackMissing));
    assert!(error.to_string().contains("make sync-packs"));
}

#[test]
fn a_tree_without_pack_yaml_beside_a_pin_is_treated_as_unsynced() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    mkdir(&checkout.join("packs/built-in/__pycache__"));
    write(
        &checkout.join("packs/built-in.source.yaml"),
        "pack: built-in\nref: 0123456789abcdef\n",
    );

    let error = BuiltInSearch::new(checkout.join("src"))
        .find_root()
        .unwrap_err();
    assert!(matches!(error, CharterError::BuiltInPackMissing));
}

#[test]
fn absent_tree_without_a_pin_is_pack_root_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let error = BuiltInSearch::new(temp.path().join("somewhere/src"))
        .find_root()
        .unwrap_err();
    match error {
        CharterError::PackRootNotFound { tier } => assert_eq!(tier, PackTier::BuiltIn),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn built_in_content_dirs_follow_the_artifact_kind_authority() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    built_in_tree(&checkout);
    let search = BuiltInSearch::new(checkout.join("src"));
    let config = ProjectConfig::absent(checkout.join(".kittify/config.yaml"));
    let roots =
        PackRoots::resolve(&checkout, &config, &PathExpansion::empty(), Some(&search)).unwrap();

    assert_eq!(
        roots.built_in_content_dir(ArtifactKind::Directive).unwrap(),
        canonical(&checkout.join("packs/built-in")).join("directives")
    );
    for kind in [
        ArtifactKind::Template,
        ArtifactKind::Asset,
        ArtifactKind::AntiPattern,
    ]
    .iter()
    .filter(|kind| !kind.has_built_in_content_dir())
    {
        let error = roots.built_in_content_dir(*kind).unwrap_err();
        match error {
            CharterError::BuiltInContentDirNotAvailable { kind: refused } => {
                assert_eq!(refused, *kind);
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}

#[test]
fn org_pack_roots_keep_config_order_and_project_pack_dir_is_optional() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    mkdir(&project.join(".kittify"));
    let one = project.join("packs/one");
    let two = project.join("packs/two");
    mkdir(&one.join("core"));
    mkdir(&two);
    write(
        &project.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: one\n        local_path: packs/one\n        subdir: core\n      - name: two\n        local_path: packs/two\n",
    );
    let config = sk_charter::load_project_config(&project, &Limits::default()).unwrap();
    let roots = PackRoots::resolve(&project, &config, &PathExpansion::empty(), None).unwrap();

    assert!(roots.built_in.is_none());
    assert_eq!(roots.organizations.len(), 2);
    assert_eq!(roots.organizations[0].name, "one");
    assert_eq!(roots.organizations[0].root, canonical(&one.join("core")));
    assert_eq!(roots.organizations[1].root, canonical(&two));
    assert!(roots.project.is_none());
}

#[test]
fn a_declared_org_pack_that_is_missing_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &project.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: gone\n        local_path: packs/gone\n",
    );
    let config = sk_charter::load_project_config(&project, &Limits::default()).unwrap();
    let error = PackRoots::resolve(&project, &config, &PathExpansion::empty(), None).unwrap_err();
    match error {
        CharterError::OrgPackMissing { name, root, .. } => {
            assert_eq!(name, "gone");
            assert_eq!(root, project.join("packs/gone"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn subdir_containment_rejects_traversal_and_symlink_escape() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let outside = temp.path().join("outside");
    mkdir(&outside);
    mkdir(&project.join("packs/one"));
    write(
        &project.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: one\n        local_path: packs/one\n        subdir: ../escape\n",
    );
    let error = sk_charter::load_project_config(&project, &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::InvalidConfig { .. }));

    #[cfg(unix)]
    {
        let project = temp.path().join("project2");
        mkdir(&project.join("packs/one"));
        support::symlink_dir(&outside, &project.join("packs/one/link"));
        write(
            &project.join(".kittify/config.yaml"),
            "charter_packs:\n  org:\n    packs:\n      - name: one\n        local_path: packs/one\n        subdir: link\n",
        );
        let config = sk_charter::load_project_config(&project, &Limits::default()).unwrap();
        let error =
            PackRoots::resolve(&project, &config, &PathExpansion::empty(), None).unwrap_err();
        assert!(matches!(error, CharterError::PathEscape { .. }));
    }
}
