//! Acceptance tests for fail-closed asset-manifest resolution.
//!
//! Contract mirror: `src/charter/offering/assets/repository.py`. Built-in
//! blobs anchor at the *parent* of the shipped `assets/` directory (the authored
//! path carries the pack-relative segment); org/project blobs anchor at the
//! overlay's own `assets/` directory. Containment rejects absolute paths, `..`
//! traversal and symlink escapes. Same-precedence duplicate ids are ambiguous
//! and fail closed; cross-tier shadowing is explicit and recorded.

mod support;

use std::fs;
use std::path::Path;

use sk_charter::{AssetCatalog, AssetRoots, CharterError, Limits, OrgAssetRoot, PackTier};
use support::{asset_yaml, mkdir, write};

fn roots(built_in: Option<&Path>, orgs: &[(&str, &Path)], project: Option<&Path>) -> AssetRoots {
    AssetRoots {
        built_in: built_in.map(Path::to_path_buf),
        organizations: orgs
            .iter()
            .map(|(name, path)| OrgAssetRoot {
                name: (*name).to_owned(),
                assets_dir: path.to_path_buf(),
            })
            .collect(),
        project: project.map(Path::to_path_buf),
    }
}

#[test]
fn resolves_the_built_in_blob_without_doubling_the_anchor_segment() {
    let temp = tempfile::tempdir().unwrap();
    let pack_root = temp.path().join("built-in");
    let assets = pack_root.join("assets");
    write(
        &assets.join("logo.asset.yaml"),
        &asset_yaml("logo", "image/png", "assets/logo.png"),
    );
    write(&assets.join("logo.png"), "png");

    let catalog = AssetCatalog::load(&roots(Some(&assets), &[], None), &Limits::default()).unwrap();
    let entry = catalog.get("logo").expect("loaded asset");
    assert_eq!(entry.tier, PackTier::BuiltIn);
    assert_eq!(entry.path, "assets/logo.png");
    assert_eq!(entry.manifest_path, assets.join("logo.asset.yaml"));

    let resolved = catalog.resolve("logo").unwrap();
    assert_eq!(resolved.path, assets.join("logo.png"));
    assert!(resolved.exists);
    assert!(!resolved.path.to_string_lossy().contains("assets/assets"));
}

#[test]
fn org_and_project_blobs_anchor_at_the_overlay_assets_directory() {
    let temp = tempfile::tempdir().unwrap();
    let built_in = temp.path().join("built-in/assets");
    mkdir(&built_in);
    let org_assets = temp.path().join("org/assets");
    write(
        &org_assets.join("mypack/deep.asset.yaml"),
        &asset_yaml("deep", "text/plain", "mypack/deep.txt"),
    );
    write(&org_assets.join("mypack/deep.txt"), "org");
    let project_assets = temp.path().join("project/assets");
    write(
        &project_assets.join("p.asset.yaml"),
        &asset_yaml("p", "text/plain", "p.txt"),
    );
    write(&project_assets.join("p.txt"), "project");

    let catalog = AssetCatalog::load(
        &roots(
            Some(&built_in),
            &[("team", &org_assets)],
            Some(&project_assets),
        ),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(
        catalog.resolve("deep").unwrap().path,
        org_assets.join("mypack/deep.txt")
    );
    assert_eq!(
        catalog.resolve("p").unwrap().path,
        project_assets.join("p.txt")
    );
}

#[test]
fn precedence_is_project_over_org_over_built_in_and_shadowing_is_recorded() {
    let temp = tempfile::tempdir().unwrap();
    let built_in = temp.path().join("built-in/assets");
    let org_assets = temp.path().join("org/assets");
    let project_assets = temp.path().join("project/assets");
    write(
        &built_in.join("icon.asset.yaml"),
        &asset_yaml("icon", "image/png", "assets/icon.png"),
    );
    write(&built_in.join("icon.png"), "built-in");
    write(
        &org_assets.join("icon.asset.yaml"),
        &asset_yaml("icon", "image/svg+xml", "icon.svg"),
    );
    write(&org_assets.join("icon.svg"), "org");
    write(
        &project_assets.join("icon.asset.yaml"),
        &asset_yaml("icon", "image/webp", "icon.webp"),
    );
    write(&project_assets.join("icon.webp"), "project");

    let catalog = AssetCatalog::load(
        &roots(
            Some(&built_in),
            &[("team", &org_assets)],
            Some(&project_assets),
        ),
        &Limits::default(),
    )
    .unwrap();

    let entry = catalog.get("icon").unwrap();
    assert_eq!(entry.tier, PackTier::Project);
    assert_eq!(entry.mime, "image/webp");
    let resolved = catalog.resolve("icon").unwrap();
    assert_eq!(resolved.path, project_assets.join("icon.webp"));
    assert_eq!(
        catalog
            .shadowed()
            .iter()
            .map(|shadow| shadow.tier)
            .collect::<Vec<_>>(),
        vec![PackTier::BuiltIn, PackTier::Org]
    );
}

#[test]
fn a_later_org_pack_overrides_an_earlier_one_by_explicit_config_order() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first/assets");
    let second = temp.path().join("second/assets");
    write(
        &first.join("a.asset.yaml"),
        &asset_yaml("a", "text/plain", "a.txt"),
    );
    write(&first.join("a.txt"), "first");
    write(
        &second.join("a.asset.yaml"),
        &asset_yaml("a", "text/plain", "a.txt"),
    );
    write(&second.join("a.txt"), "second");

    let catalog = AssetCatalog::load(
        &roots(None, &[("first", &first), ("second", &second)], None),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(catalog.resolve("a").unwrap().path, second.join("a.txt"));
    assert_eq!(catalog.shadowed().len(), 1);
}

#[test]
fn two_manifests_with_the_same_id_at_the_same_precedence_are_ambiguous() {
    let temp = tempfile::tempdir().unwrap();
    let project_assets = temp.path().join("project/assets");
    write(
        &project_assets.join("a.asset.yaml"),
        &asset_yaml("dup", "text/plain", "a.txt"),
    );
    write(
        &project_assets.join("nested/b.asset.yaml"),
        &asset_yaml("dup", "text/plain", "b.txt"),
    );

    let error = AssetCatalog::load(&roots(None, &[], Some(&project_assets)), &Limits::default())
        .unwrap_err();
    match error {
        CharterError::AmbiguousAsset { id, sources } => {
            assert_eq!(id, "dup");
            assert_eq!(sources.len(), 2);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn an_unknown_asset_id_is_a_typed_missing_error() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("assets");
    mkdir(&assets);
    let catalog = AssetCatalog::load(&roots(None, &[], Some(&assets)), &Limits::default()).unwrap();
    assert!(catalog.get("ghost").is_none());
    let error = catalog.resolve("ghost").unwrap_err();
    match error {
        CharterError::AssetNotFound { ref id } => assert_eq!(id, "ghost"),
        other => panic!("unexpected error: {other}"),
    }
    assert!(error.to_string().contains("ghost"));
}

#[test]
fn traversal_and_absolute_blob_paths_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    for (label, blob_path) in [
        ("parent", "../../../../etc/passwd"),
        ("nested-parent", "assets/../../secret.txt"),
        ("absolute", "/etc/passwd"),
        ("windows", "C:\\Windows\\win.ini"),
        ("unc", "\\\\server\\share\\file"),
    ] {
        let root = temp.path().join(label);
        write(
            &root.join("evil.asset.yaml"),
            &asset_yaml("evil", "text/plain", blob_path),
        );
        let catalog =
            AssetCatalog::load(&roots(None, &[], Some(&root)), &Limits::default()).unwrap();
        let error = catalog.resolve("evil").unwrap_err();
        match error {
            CharterError::AssetPathEscape {
                id,
                blob_path: refused,
                ..
            } => {
                assert_eq!(id, "evil");
                assert_eq!(refused, blob_path);
            }
            other => panic!("{label}: unexpected error: {other}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_that_escapes_the_anchor_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    write(&outside.join("secret.txt"), "top secret");
    let assets = temp.path().join("assets");
    mkdir(&assets);
    support::symlink_dir(&outside, &assets.join("link"));
    write(
        &assets.join("s.asset.yaml"),
        &asset_yaml("s", "text/plain", "link/secret.txt"),
    );

    let catalog = AssetCatalog::load(&roots(None, &[], Some(&assets)), &Limits::default()).unwrap();
    let error = catalog.resolve("s").unwrap_err();
    assert!(matches!(error, CharterError::AssetPathEscape { .. }));
}

#[test]
fn a_missing_blob_is_reported_not_opened_or_faked() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("assets");
    write(
        &assets.join("ghost.asset.yaml"),
        &asset_yaml("ghost", "text/plain", "ghost.txt"),
    );
    let catalog = AssetCatalog::load(&roots(None, &[], Some(&assets)), &Limits::default()).unwrap();
    let resolved = catalog.resolve("ghost").unwrap();
    assert_eq!(resolved.path, assets.join("ghost.txt"));
    assert!(!resolved.exists);
}

#[test]
fn malformed_or_over_budget_manifests_fail_closed() {
    let temp = tempfile::tempdir().unwrap();

    let corrupt = temp.path().join("corrupt");
    write(&corrupt.join("a.asset.yaml"), "id: [unterminated\n");
    let error =
        AssetCatalog::load(&roots(None, &[], Some(&corrupt)), &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::InvalidYaml { .. }));

    let list = temp.path().join("list");
    write(&list.join("a.asset.yaml"), "- a\n- b\n");
    let error = AssetCatalog::load(&roots(None, &[], Some(&list)), &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::ExpectedMapping { .. }));

    for (label, body) in [
        ("missing path", "id: a\nmime: text/plain\n"),
        ("missing mime", "id: a\npath: a.txt\n"),
        ("missing id", "mime: text/plain\npath: a.txt\n"),
        ("blank id", "id: ''\nmime: text/plain\npath: a.txt\n"),
        (
            "unknown field",
            "id: a\nmime: text/plain\npath: a.txt\nsurprise: 1\n",
        ),
    ] {
        let root = temp.path().join(label.replace(' ', "-"));
        write(&root.join("a.asset.yaml"), body);
        let error =
            AssetCatalog::load(&roots(None, &[], Some(&root)), &Limits::default()).unwrap_err();
        assert!(
            matches!(error, CharterError::InvalidAssetManifest { .. }),
            "{label}: {error}"
        );
    }

    let big = temp.path().join("big");
    write(
        &big.join("a.asset.yaml"),
        &format!(
            "id: a\nmime: text/plain\npath: a.txt\ntitle: '{}'\n",
            "x".repeat(2048)
        ),
    );
    let limits = Limits::default().with_max_asset_manifest_bytes(1024);
    let error = AssetCatalog::load(&roots(None, &[], Some(&big)), &limits).unwrap_err();
    assert!(matches!(error, CharterError::TooLarge { .. }));
}

#[test]
fn the_manifest_count_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("assets");
    for index in 0..3 {
        write(
            &assets.join(format!("{index}.asset.yaml")),
            &asset_yaml(&format!("a{index}"), "text/plain", &format!("{index}.txt")),
        );
    }
    let limits = Limits::default().with_max_asset_manifests(2);
    let error = AssetCatalog::load(&roots(None, &[], Some(&assets)), &limits).unwrap_err();
    match error {
        CharterError::TooManyAssets { limit } => assert_eq!(limit, 2),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn catalogs_serialize_as_sorted_plain_data() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("assets");
    write(
        &assets.join("b.asset.yaml"),
        &asset_yaml("b", "text/plain", "b.txt"),
    );
    write(
        &assets.join("a.asset.yaml"),
        &asset_yaml("a", "text/plain", "a.txt"),
    );
    let catalog = AssetCatalog::load(&roots(None, &[], Some(&assets)), &Limits::default()).unwrap();
    let value = serde_json::to_value(&catalog).unwrap();
    assert_eq!(value["entries"]["a"]["tier"], "project");
    assert_eq!(value["entries"]["a"]["path"], "a.txt");
    assert_eq!(value["entries"]["b"]["mime"], "text/plain");
    assert!(value["shadowed"].as_array().unwrap().is_empty());
    assert_eq!(
        catalog
            .entries()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[test]
fn an_unreadable_asset_root_is_absence_not_a_crash() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing/assets");
    let catalog =
        AssetCatalog::load(&roots(None, &[], Some(&missing)), &Limits::default()).unwrap();
    assert_eq!(catalog.entries().count(), 0);
    assert!(fs::metadata(&missing).is_err());
}

#[cfg(unix)]
#[test]
fn a_symlinked_sidecar_is_reported_rather_than_silently_skipped() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside.asset.yaml");
    write(&outside, &asset_yaml("hidden", "text/plain", "hidden.txt"));
    let assets = temp.path().join("assets");
    mkdir(&assets);
    support::symlink_file(&outside, &assets.join("hidden.asset.yaml"));

    let error =
        AssetCatalog::load(&roots(None, &[], Some(&assets)), &Limits::default()).unwrap_err();
    match error {
        CharterError::InvalidAssetManifest { path, reason } => {
            assert!(path.ends_with("hidden.asset.yaml"));
            assert!(reason.contains("symlink"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn an_empty_asset_title_is_tolerated_like_the_python_model() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("assets");
    write(
        &assets.join("a.asset.yaml"),
        "id: a\nmime: text/plain\npath: a.txt\ntitle: ''\n",
    );
    let catalog = AssetCatalog::load(&roots(None, &[], Some(&assets)), &Limits::default()).unwrap();
    assert_eq!(catalog.get("a").unwrap().title.as_deref(), Some(""));
}
