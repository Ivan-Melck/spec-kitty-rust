use sk_charter::{AssetCatalog, AssetRoots, CharterBundle, Limits, OrgAssetRoot};
use std::fs;

#[test]
fn a_partial_overlay_inherits_fields_and_changes_only_its_title() {
    let temp = tempfile::tempdir().unwrap();
    let built = temp.path().join("built-in/assets");
    let project = temp.path().join("project/assets");
    fs::create_dir_all(&built).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        built.join("logo.asset.yaml"),
        "id: logo\nmime: image/png\npath: assets/logo.png\ntitle: Original\n",
    )
    .unwrap();
    fs::write(
        project.join("logo.asset.yaml"),
        "id: logo\ntitle: Project title\n",
    )
    .unwrap();
    let roots = AssetRoots {
        built_in: Some(built),
        project: Some(project),
        ..Default::default()
    };
    let catalog = AssetCatalog::load(&roots, &Limits::default()).unwrap();
    let logo = catalog.get("logo").unwrap();
    assert_eq!(logo.mime, "image/png");
    assert_eq!(logo.path, "assets/logo.png");
    assert_eq!(logo.title.as_deref(), Some("Project title"));
}

#[cfg(unix)]
#[test]
fn an_assets_directory_cannot_escape_its_pack_in_any_tier() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(
        outside.join("secret.asset.yaml"),
        "id: secret\nmime: text/plain\npath: secret.txt\n",
    )
    .unwrap();
    for tier in ["built-in", "org", "project"] {
        let pack = temp.path().join(tier);
        fs::create_dir(&pack).unwrap();
        let assets = pack.join("assets");
        std::os::unix::fs::symlink(&outside, &assets).unwrap();
        let mut roots = AssetRoots::default();
        match tier {
            "built-in" => roots.built_in = Some(assets),
            "org" => roots.organizations.push(OrgAssetRoot {
                name: "team".into(),
                assets_dir: assets,
            }),
            _ => roots.project = Some(assets),
        }
        assert!(
            AssetCatalog::load(&roots, &Limits::default()).is_err(),
            "{tier}"
        );
    }
}

#[cfg(unix)]
#[test]
fn charter_reads_reject_fifos_even_without_default_features() {
    let temp = tempfile::tempdir().unwrap();
    let charter = temp.path().join(".kittify/charter");
    fs::create_dir_all(&charter).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(charter.join("charter.md"))
            .status()
            .unwrap()
            .success()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(CharterBundle::load(temp.path(), &Limits::default()));
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("reader must not block on FIFO")
            .is_err()
    );
}
