#![cfg(unix)]
use sk_doctrine::resolve_project_governance;
use std::{fs, path::Path, process::Command};
fn root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(root.path())
            .status()
            .unwrap()
            .success()
    );
    fs::create_dir_all(root.path().join(".kittify/charter")).unwrap();
    root
}
fn charter(root: &Path) -> std::path::PathBuf {
    root.join(".kittify/charter/charter.yaml")
}
#[test]
fn external_symlink_is_source_governance_not_empty_defaults() {
    let root = root();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("external.yaml");
    fs::write(
        &file,
        "governance:\n  charter:\n    available_tools: [source-external-tool]\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&file, charter(root.path())).unwrap();
    let result = resolve_project_governance(root.path()).unwrap();
    assert!(
        result.tools.iter().any(|s| s == "source-external-tool"),
        "Source follows this valid symlink rather than substituting an absent charter"
    );
}
#[test]
fn directory_read_error_is_not_a_healthy_absent_charter() {
    let root = root();
    fs::create_dir(charter(root.path())).unwrap();
    assert!(resolve_project_governance(root.path()).is_err());
}
#[test]
fn unignored_stat_error_is_not_a_healthy_absent_charter() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let path = root.path().join(".kittify/charter");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
    let result = resolve_project_governance(root.path());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
}
