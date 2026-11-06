//! Source Path.write_text publication, not durable atomic replacement. No paired claim.
use sk_charter::bundle_restamp::{RestampLockFailure, restamp_bundle_hash};
use std::{fs, path::Path, time::Duration};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".kittify/charter");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("charter.yaml"), b"catalog: {}\n").unwrap();
    fs::write(dir.join("synthesis-manifest.yaml"), format!(
        "created_at: '{}'\nrun_id: '{}'\nadapter_id: fixture\nadapter_version: '1'\nsynthesizer_version: 4.0.0\nmanifest_hash: '{}'\nartifacts: []\nbundle_content_hash: sha256:stale\n",
        sk_kernel::clock::now_utc_iso(), temp.path().file_name().unwrap().to_str().unwrap(), "0".repeat(64)
    )).unwrap();
    temp
}
fn run(
    root: &Path,
) -> Result<sk_charter::bundle_restamp::RestampOutcome, sk_charter::bundle_restamp::RestampError> {
    restamp_bundle_hash(root, |root| {
        sk_kernel::FileLock::acquire(
            &root.join(".kittify/runtime/locks/charter-synthesis.lock"),
            Duration::from_secs(1),
        )
        .map_err(|e| RestampLockFailure::Other {
            detail: e.to_string(),
        })
    })
}
#[test]
#[cfg(unix)]
fn restamp_preserves_inode_and_updates_every_hardlink_like_source_write_text() {
    use std::os::unix::fs::MetadataExt;
    let temp = fixture();
    let root = temp.path();
    let manifest = root.join(".kittify/charter/synthesis-manifest.yaml");
    let alias = root.join("manifest-hardlink.yaml");
    fs::hard_link(&manifest, &alias).unwrap();
    let before = fs::metadata(&manifest).unwrap().ino();
    assert!(run(root).unwrap().is_restamped());
    assert_eq!(
        fs::metadata(&manifest).unwrap().ino(),
        before,
        "Source truncates the existing inode"
    );
    assert_eq!(
        fs::read(&alias).unwrap(),
        fs::read(&manifest).unwrap(),
        "Source updates all hardlink associations"
    );
}
#[test]
#[cfg(unix)]
fn restamp_follows_manifest_symlink_instead_of_replacing_it() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let temp = fixture();
    let root = temp.path();
    let manifest = root.join(".kittify/charter/synthesis-manifest.yaml");
    let target = root.join("authored-manifest.yaml");
    fs::rename(&manifest, &target).unwrap();
    symlink(&target, &manifest).unwrap();
    let link_inode = fs::symlink_metadata(&manifest).unwrap().ino();
    let target_inode = fs::metadata(&target).unwrap().ino();
    let before = fs::read(&target).unwrap();
    assert!(run(root).unwrap().is_restamped());
    assert!(fs::symlink_metadata(&manifest).unwrap().is_symlink());
    assert_eq!(fs::symlink_metadata(&manifest).unwrap().ino(), link_inode);
    assert_eq!(fs::metadata(&target).unwrap().ino(), target_inode);
    assert_ne!(fs::read(&target).unwrap(), before);
    assert_eq!(fs::read(&target).unwrap(), fs::read(&manifest).unwrap());
}
#[test]
#[cfg(unix)]
fn source_readonly_manifest_write_refuses_without_atomic_permission_bypass() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let temp = fixture();
    let root = temp.path();
    let manifest = root.join(".kittify/charter/synthesis-manifest.yaml");
    let before = fs::read(&manifest).unwrap();
    let inode = fs::metadata(&manifest).unwrap().ino();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o444)).unwrap();
    let cannot_write = fs::OpenOptions::new().write(true).open(&manifest).is_err();
    assert!(
        cannot_write,
        "requires unprivileged process for Source EACCES control"
    );
    let outcome = run(root);
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        outcome.is_err(),
        "Source write_text fails before writing a read-only destination: {outcome:?}"
    );
    assert_eq!(fs::read(&manifest).unwrap(), before);
    assert_eq!(fs::metadata(&manifest).unwrap().ino(), inode);
}
