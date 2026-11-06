//! `4.0.0rc7` pack descriptor and charter-pack ignore-rule migrations.

use std::path::Path;
use std::process::Command;

use sk_install::migrations::charter_pack_gitignore;
use sk_install::migrations::pack_level;

fn project() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp")
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("git");
    assert!(status.success());
}

fn load_yaml(path: &Path) -> serde_yaml_ng::Value {
    serde_yaml_ng::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn pack_level_never_creates_the_project_pack_directory() {
    let directory = project();
    let root = directory.path();
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    assert!(!pack_level::detect(root).unwrap());
    let outcome = pack_level::apply(root).unwrap();
    assert!(!outcome.changed);
    assert!(!root.join(".kittify/charter-packs").exists());
}

#[test]
fn pack_level_writes_a_minimal_project_descriptor_once() {
    let directory = project();
    let root = directory.path();
    let pack = root.join(".kittify/charter-packs");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("graph.yaml"), "nodes: []\n").unwrap();

    assert!(pack_level::detect(root).unwrap());
    let outcome = pack_level::apply(root).unwrap();
    assert!(outcome.changed);
    assert!(outcome.changes[0].contains(".kittify/charter-packs/pack.yaml"));
    assert!(!outcome.changes[0].contains(&root.to_string_lossy().to_string()));

    let descriptor = load_yaml(&pack.join("pack.yaml"));
    let mapping = descriptor.as_mapping().unwrap();
    let id = mapping
        .get(serde_yaml_ng::Value::from("pack_id"))
        .and_then(serde_yaml_ng::Value::as_str)
        .unwrap();
    assert_eq!(id.len(), 26);
    assert_eq!(
        mapping.get(serde_yaml_ng::Value::from("pack_level")),
        Some(&serde_yaml_ng::Value::from("project"))
    );
    assert_eq!(
        mapping.get(serde_yaml_ng::Value::from("name")),
        Some(&serde_yaml_ng::Value::from(
            root.file_name().unwrap().to_string_lossy().to_string()
        ))
    );
    assert!(!pack_level::detect(root).unwrap());
    let again = pack_level::apply(root).unwrap();
    assert!(!again.changed);
    assert_eq!(load_yaml(&pack.join("pack.yaml")), descriptor);
}

#[test]
fn pack_level_adds_only_the_missing_key_and_preserves_authored_content() {
    let directory = project();
    let root = directory.path();
    let pack = root.join(".kittify/charter-packs");
    std::fs::create_dir_all(&pack).unwrap();
    let original = "# keep this comment\npack_id: 01ARWG13C000000000000000BB\nname: authored-handle  # inline\n";
    std::fs::write(pack.join("pack.yaml"), original).unwrap();

    let outcome = pack_level::apply(root).unwrap();
    assert!(outcome.changed);
    let text = std::fs::read_to_string(pack.join("pack.yaml")).unwrap();
    assert!(text.contains("# keep this comment"));
    assert!(text.contains("name: authored-handle  # inline"));
    let descriptor = load_yaml(&pack.join("pack.yaml"));
    assert_eq!(
        descriptor
            .as_mapping()
            .unwrap()
            .get(serde_yaml_ng::Value::from("pack_id")),
        Some(&serde_yaml_ng::Value::from("01ARWG13C000000000000000BB"))
    );
    assert_eq!(
        descriptor
            .as_mapping()
            .unwrap()
            .get(serde_yaml_ng::Value::from("pack_level")),
        Some(&serde_yaml_ng::Value::from("project"))
    );
}

#[test]
fn pack_level_refuses_a_malformed_descriptor_without_touching_it() {
    let directory = project();
    let root = directory.path();
    let pack = root.join(".kittify/charter-packs");
    std::fs::create_dir_all(&pack).unwrap();
    let path = pack.join("pack.yaml");
    std::fs::write(&path, "pack_id: [unterminated\n").unwrap();
    assert!(pack_level::detect(root).unwrap());
    assert!(pack_level::can_apply(root).unwrap().is_some());
    assert!(pack_level::apply(root).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "pack_id: [unterminated\n"
    );
}

#[test]
fn pack_level_leaves_a_non_mapping_descriptor_for_validate() {
    let directory = project();
    let root = directory.path();
    let pack = root.join(".kittify/charter-packs");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("pack.yaml"), "- a list\n").unwrap();
    assert!(!pack_level::detect(root).unwrap());
    assert!(!pack_level::apply(root).unwrap().changed);
}

fn cutover_project() -> tempfile::TempDir {
    let directory = project();
    let root = directory.path();
    let pack = root.join(".kittify/charter-packs");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("graph.yaml"), "nodes: []\n").unwrap();
    std::fs::write(pack.join("pack.yaml"), "pack_id: x\npack_level: project\n").unwrap();
    std::fs::write(
        root.join(".gitignore"),
        ".kittify/charter-packs/**\n!.kittify/charter-packs/graph.yaml\n",
    )
    .unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    directory
}

#[test]
fn track_charter_pack_yaml_groups_the_allow_rule_and_is_idempotent() {
    let directory = cutover_project();
    let root = directory.path();
    assert!(charter_pack_gitignore::detect(root).unwrap());
    let outcome = charter_pack_gitignore::apply(root).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        ".kittify/charter-packs/**\n!.kittify/charter-packs/graph.yaml\n!.kittify/charter-packs/pack.yaml\n"
    );
    assert!(!charter_pack_gitignore::detect(root).unwrap());
    assert!(!charter_pack_gitignore::apply(root).unwrap().changed);
}

#[test]
fn track_charter_pack_yaml_matches_an_anchored_blanket_rule() {
    let directory = project();
    let root = directory.path();
    std::fs::write(root.join(".gitignore"), "/.kittify/charter-packs/**\r\n").unwrap();
    let outcome = charter_pack_gitignore::apply(root).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        "/.kittify/charter-packs/**\r\n!/.kittify/charter-packs/pack.yaml\r\n"
    );
}

#[test]
fn track_charter_pack_yaml_leaves_unrelated_ignore_files_alone() {
    let directory = project();
    let root = directory.path();
    let original = "target/\n";
    std::fs::write(root.join(".gitignore"), original).unwrap();
    assert!(!charter_pack_gitignore::detect(root).unwrap());
    assert!(!charter_pack_gitignore::apply(root).unwrap().changed);
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        original
    );
}

#[test]
fn track_charter_pack_yaml_warns_when_a_later_rule_still_hides_the_file() {
    let directory = cutover_project();
    let root = directory.path();
    std::fs::write(
        root.join(".gitignore"),
        ".kittify/charter-packs/**\n!.kittify/charter-packs/graph.yaml\n.kittify/\n",
    )
    .unwrap();
    let outcome = charter_pack_gitignore::apply(root).unwrap();
    assert!(outcome.changed);
    assert!(outcome.warnings.iter().any(|warning| {
        warning.contains(
            ".kittify/charter-packs/pack.yaml is still ignored by another .gitignore rule",
        )
    }));
}

#[test]
fn track_charter_pack_yaml_warns_instead_of_crashing_on_an_unreadable_file() {
    let directory = project();
    let root = directory.path();
    std::fs::create_dir_all(root.join(".gitignore")).unwrap();
    assert!(charter_pack_gitignore::detect(root).unwrap());
    let outcome = charter_pack_gitignore::apply(root).unwrap();
    assert!(!outcome.changed);
    assert!(outcome.warnings[0].contains("add !.kittify/charter-packs/pack.yaml by hand"));
    assert!(root.join(".gitignore").is_dir());
}

#[cfg(unix)]
#[test]
fn track_charter_pack_yaml_fails_closed_on_a_symlink() {
    let directory = project();
    let root = directory.path();
    std::fs::write(root.join("elsewhere"), ".kittify/charter-packs/**\n").unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), root.join(".gitignore")).unwrap();
    assert!(charter_pack_gitignore::detect(root).is_err());
    assert!(charter_pack_gitignore::apply(root).unwrap().warnings[0].contains("symlink"));
    assert_eq!(
        std::fs::read_to_string(root.join("elsewhere")).unwrap(),
        ".kittify/charter-packs/**\n"
    );
}
