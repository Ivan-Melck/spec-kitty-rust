#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives under crates/sk-mission")
        .to_path_buf()
}

pub fn fixtures_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project")
}

pub fn temp_project() -> TempDir {
    let temp = tempfile::tempdir().expect("temporary project");
    fs::create_dir_all(temp.path().join("kitty-specs")).expect("kitty-specs");
    temp
}

pub fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).expect("write fixture");
}

pub fn mission_dir(root: &Path, slug: &str) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    fs::create_dir_all(&dir).expect("create mission directory");
    dir
}

pub fn meta_json(id: &str, slug: &str, number: Option<i64>, topology: &str) -> String {
    let number = match number {
        Some(value) => value.to_string(),
        None => "null".to_owned(),
    };
    format!(
        r#"{{
  "created_at": "2026-11-02T10:00:00Z",
  "flattened": false,
  "friendly_name": "{slug}",
  "mission_id": "{id}",
  "mission_number": {number},
  "mission_slug": "{slug}",
  "mission_type": "software-dev",
  "slug": "{slug}",
  "target_branch": "main",
  "topology": "{topology}"
}}
"#
    )
}

pub fn write_mission(root: &Path, slug: &str, id: &str) -> PathBuf {
    let dir = mission_dir(root, slug);
    write(
        &dir.join("meta.json"),
        &meta_json(id, slug, None, "single_branch"),
    );
    dir
}

pub fn wp_source(id: &str, dependencies_yaml: &str) -> String {
    format!(
        "---\nwork_package_id: {id}\ntitle: Fixture {id}\ndependencies: {dependencies_yaml}\nmission_id: 01KN2371WNZX28ME4MN97B67PJ\n---\n\n# Work Package {id}\n\nBody text.\n"
    )
}
