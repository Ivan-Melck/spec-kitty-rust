#![allow(dead_code)]

//! Shared helpers for the native status-writer tests.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub const MISSION_SLUG: &str = "034-mutation-fixture";
pub const MISSION_ID: &str = "01K9M0R3W00000000000000000";

pub fn mutation_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mutation")
}

pub fn fixture(kind: &str) -> PathBuf {
    mutation_root().join(kind)
}

pub fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read_text(path))
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

pub fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create destination");
    for entry in fs::read_dir(source).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

/// A temporary repository root plus the mission feature directory inside it.
pub struct TempMission {
    pub _temp: tempfile::TempDir,
    pub repo_root: PathBuf,
    pub feature_dir: PathBuf,
}

/// Create `<tmp>/kitty-specs/<slug>` with the fixture meta and WP file.
pub fn temp_mission_with(subtasks: &[&str], dependencies: &[&str]) -> TempMission {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_root = temp.path().to_path_buf();
    let feature_dir = repo_root.join("kitty-specs").join(MISSION_SLUG);
    fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
    fs::write(
        feature_dir.join("meta.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "mission_slug": MISSION_SLUG,
            "mission_id": MISSION_ID,
            "mission_number": null,
            "mission_type": "software-dev",
            "target_branch": "main",
        }))
        .unwrap()
            + "\n",
    )
    .expect("meta.json");
    let subtasks_yaml = if subtasks.is_empty() {
        "[]".to_owned()
    } else {
        let items = subtasks
            .iter()
            .map(|id| format!("- {id}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n{items}")
    };
    let dependencies_yaml = if dependencies.is_empty() {
        "[]".to_owned()
    } else {
        format!(
            "[{}]",
            dependencies
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    fs::write(
        feature_dir.join("tasks/WP01.md"),
        format!(
            "---\nwork_package_id: WP01\ntitle: Mutation fixture work package\ndependencies: {dependencies_yaml}\nsubtasks:{subtasks_yaml}\n---\n\n# WP01\n"
        ),
    )
    .expect("WP01.md");
    TempMission {
        _temp: temp,
        repo_root,
        feature_dir,
    }
}

pub fn temp_mission() -> TempMission {
    temp_mission_with(&[], &[])
}

/// Copy a fixture `input/` tree into a fresh temp repository root.
pub fn copy_fixture_input(kind: &str) -> TempMission {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_root = temp.path().to_path_buf();
    copy_dir(&fixture(kind).join("input"), &repo_root);
    let feature_dir = repo_root.join("kitty-specs").join(MISSION_SLUG);
    TempMission {
        _temp: temp,
        repo_root,
        feature_dir,
    }
}

pub fn expected(kind: &str, name: &str) -> PathBuf {
    fixture(kind).join("expected").join(name)
}
