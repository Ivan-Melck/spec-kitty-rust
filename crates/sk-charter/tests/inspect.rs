//! End-to-end acceptance test for the bounded, read-only inspection surface.
//!
//! One project fixture exercises every layer: `.kittify/config.yaml`,
//! `.kittify/charter/`, a built-in pack found by ancestor walk, an org pack
//! declared in config (with `subdir`), and the project pack overlay. The test
//! proves precedence, typed output, and that no user file is rewritten.

mod support;

use std::fs;
use std::path::Path;

use sk_charter::{
    ArtifactKind, BuiltInSearch, CharterBundle, CharterError, InspectionOptions, Limits, PackTier,
    PathExpansion, inspect_project,
};
use support::{BUILTIN_PACK_ID, asset_yaml, mkdir, pack_yaml, tree_snapshot, write};

fn write_pack(root: &Path, name: &str, level: &str, pack_id: &str, version: Option<&str>) {
    write(
        &root.join("pack.yaml"),
        &pack_yaml(name, level, pack_id, version),
    );
}

fn fixture(temp: &Path) -> std::path::PathBuf {
    // A checkout that holds the built-in pack, separate from the consumer project.
    let checkout = temp.join("checkout");
    write_pack(
        &checkout.join("packs/built-in"),
        "built-in",
        "built-in",
        BUILTIN_PACK_ID,
        Some("4.0.0"),
    );
    write(
        &checkout.join("packs/built-in/assets/logo.asset.yaml"),
        &asset_yaml("logo", "image/png", "assets/logo.png"),
    );
    write(
        &checkout.join("packs/built-in/assets/logo.png"),
        "built-in-png",
    );

    let project = temp.join("project");
    write(
        &project.join(".kittify/config.yaml"),
        "charter: .kittify/charter/charter.yaml\ncharter_packs:\n  org:\n    packs:\n      - name: team\n        pack_id: 01ARWG13C000000000000000F2\n        local_path: packs/team\n        subdir: pack-core\n  project:\n    skill_namespace: acme\n",
    );
    write(&project.join(".kittify/charter/charter.md"), "# Charter\n");
    write(
        &project.join(".kittify/charter/charter.yaml"),
        "schema_version: '1'\n",
    );

    write_pack(
        &project.join("packs/team/pack-core"),
        "team",
        "org",
        "01ARWG13C000000000000000F2",
        None,
    );
    write(
        &project.join("packs/team/pack-core/assets/logo.asset.yaml"),
        &asset_yaml("logo", "image/svg+xml", "logo.svg"),
    );
    write(
        &project.join("packs/team/pack-core/assets/logo.svg"),
        "org-svg",
    );

    write_pack(
        &project.join(".kittify/charter-packs"),
        "project",
        "project",
        "01ARWG13C000000000000000F3",
        None,
    );
    write(
        &project.join(".kittify/charter-packs/assets/logo.asset.yaml"),
        &asset_yaml("logo", "image/webp", "logo.webp"),
    );
    write(
        &project.join(".kittify/charter-packs/assets/logo.webp"),
        "project-webp",
    );

    project
}

#[test]
fn inspection_loads_config_charter_packs_and_assets_in_precedence_order() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(temp.path());
    let options = InspectionOptions::new()
        .with_built_in(BuiltInSearch::new(temp.path().join("checkout/src")));

    let inspection = inspect_project(&project, &options).unwrap();
    assert_eq!(inspection.project_root, project);
    assert!(inspection.config.present);
    assert_eq!(inspection.config.org_packs.len(), 1);
    assert!(inspection.charter.is_present());
    assert_eq!(
        inspection
            .charter
            .charter_md
            .as_ref()
            .unwrap()
            .relative_path,
        ".kittify/charter/charter.md"
    );

    assert_eq!(
        inspection
            .packs
            .iter()
            .map(|pack| (pack.name.as_str(), pack.tier))
            .collect::<Vec<_>>(),
        vec![
            ("built-in", PackTier::BuiltIn),
            ("team", PackTier::Org),
            ("project", PackTier::Project),
        ]
    );

    assert_eq!(
        inspection
            .roots
            .built_in
            .as_ref()
            .map(|root| root.ends_with("checkout/packs/built-in")),
        Some(true)
    );
    assert_eq!(inspection.roots.organizations[0].name, "team");

    let logo = inspection.assets.resolve("logo").unwrap();
    assert_eq!(logo.tier, PackTier::Project);
    assert_eq!(logo.mime, "image/webp");
    assert!(
        logo.path
            .ends_with(".kittify/charter-packs/assets/logo.webp")
    );
    assert!(logo.exists);
    assert_eq!(
        inspection
            .assets
            .shadowed()
            .iter()
            .map(|shadow| shadow.tier)
            .collect::<Vec<_>>(),
        vec![PackTier::BuiltIn, PackTier::Org]
    );
}

#[test]
fn built_in_content_dir_refusal_is_reachable_from_the_inspection() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(temp.path());
    let options = InspectionOptions::new()
        .with_built_in(BuiltInSearch::new(temp.path().join("checkout/src")));
    let inspection = inspect_project(&project, &options).unwrap();

    let error = inspection
        .roots
        .built_in_content_dir(ArtifactKind::Template)
        .unwrap_err();
    assert!(matches!(
        error,
        CharterError::BuiltInContentDirNotAvailable { .. }
    ));
    assert!(
        inspection
            .roots
            .built_in_content_dir(ArtifactKind::Directive)
            .is_ok()
    );
}

#[test]
fn inspection_serializes_to_plain_json_for_the_cli() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(temp.path());
    let options = InspectionOptions::new()
        .with_built_in(BuiltInSearch::new(temp.path().join("checkout/src")));
    let inspection = inspect_project(&project, &options).unwrap();

    let value = serde_json::to_value(&inspection).unwrap();
    assert_eq!(value["config"]["org_packs"][0]["name"], "team");
    assert_eq!(value["config"]["project_skill_namespace"], "acme");
    assert_eq!(
        value["charter"]["charter_md"]["relative_path"],
        ".kittify/charter/charter.md"
    );
    assert_eq!(value["packs"][0]["tier"], "built-in");
    assert_eq!(value["packs"][1]["tier"], "org");
    assert_eq!(value["assets"]["entries"]["logo"]["tier"], "project");
    assert_eq!(value["assets"]["entries"]["logo"]["mime"], "image/webp");
    assert!(value["roots"]["built_in"].is_string());
    assert!(value["roots"]["organizations"][0]["root"].is_string());
}

#[test]
fn inspection_never_rewrites_user_yaml_or_markdown() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(temp.path());
    let before = tree_snapshot(temp.path());
    let options = InspectionOptions::new()
        .with_built_in(BuiltInSearch::new(temp.path().join("checkout/src")));
    let _ = inspect_project(&project, &options).unwrap();
    let after = tree_snapshot(temp.path());
    assert_eq!(before, after, "inspection must be read-only");
}

#[test]
fn inspection_without_a_built_in_search_skips_the_built_in_tier() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(temp.path());
    let inspection = inspect_project(&project, &InspectionOptions::new()).unwrap();
    assert!(inspection.roots.built_in.is_none());
    assert_eq!(inspection.packs.len(), 2);
    assert_eq!(
        inspection.assets.resolve("logo").unwrap().tier,
        PackTier::Project
    );
}

#[test]
fn inspection_fails_closed_when_a_declared_org_pack_does_not_exist() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &project.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: gone\n        local_path: packs/gone\n",
    );
    let error = inspect_project(&project, &InspectionOptions::new()).unwrap_err();
    assert!(matches!(error, CharterError::OrgPackMissing { .. }));
}

#[test]
fn inspection_fails_closed_when_a_pack_descriptor_declares_the_wrong_tier() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write_pack(
        &project.join(".kittify/charter-packs"),
        "project",
        "built-in",
        "01ARWG13C000000000000000F3",
        Some("4.0.0"),
    );
    let error = inspect_project(&project, &InspectionOptions::new()).unwrap_err();
    assert!(matches!(error, CharterError::PackLevelMismatch { .. }));
}

#[test]
fn inspection_accepts_an_explicit_limits_budget() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &project.join(".kittify/config.yaml"),
        &format!("charter: {}\n", "x".repeat(4096)),
    );
    let options =
        InspectionOptions::new().with_limits(Limits::default().with_max_config_bytes(512));
    let error = inspect_project(&project, &options).unwrap_err();
    assert!(matches!(error, CharterError::TooLarge { .. }));
}

#[test]
fn inspection_does_not_require_a_charter_bundle_or_project_pack() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("bare");
    mkdir(&project);
    let inspection = inspect_project(&project, &InspectionOptions::new()).unwrap();
    assert!(!inspection.charter.is_present());
    assert!(!inspection.config.present);
    assert!(inspection.packs.is_empty());
    assert_eq!(inspection.assets.entries().count(), 0);
    assert!(
        CharterBundle::load(&project, &Limits::default())
            .unwrap()
            .files()
            .next()
            .is_none()
    );
    let _ = PathExpansion::empty();
    let _ = fs::metadata(&project).unwrap();
}
