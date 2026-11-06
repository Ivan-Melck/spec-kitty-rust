//! Exact raw edges and uncaught exists failures must not become empty policy.

use std::fs;
use std::path::Path;

use sk_charter::ArtifactKind;
use sk_doctrine::{org_charter_raw_documents, read_org_required_tokens};

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    write(
        temporary.path(),
        ".kittify/config.yaml",
        "charter_packs:\n  org:\n    packs:\n      - name: a\n        local_path: a\n      - name: b\n        local_path: b\n",
    );
    write(
        temporary.path(),
        "a/org-charter.yaml",
        "required_directives: [A]\n",
    );
    write(
        temporary.path(),
        "b/org-charter.yaml",
        "required_directives: [B]\n",
    );
    temporary
}

#[test]
fn an_exact_non_scalar_extends_is_a_missing_base_not_no_edge() {
    let temporary = fixture();
    write(
        temporary.path(),
        "a/org-charter.yaml",
        "extends: \"\\ud800\"\nrequired_directives: [A]\n",
    );
    let error = org_charter_raw_documents(temporary.path(), true)
        .expect_err("Source refuses a dangling exact base");
    assert!(
        matches!(error, sk_doctrine::DoctrineError::OrgCharterExtends { .. }),
        "{error:?}"
    );
}

#[test]
fn an_inherited_exact_missing_base_falls_back_to_declared_alone() {
    let temporary = fixture();
    write(
        temporary.path(),
        "a/org-charter.yaml",
        "extends: b\nrequired_directives: [A]\n",
    );
    write(
        temporary.path(),
        "b/org-charter.yaml",
        "extends: \"\\ud800\"\nrequired_directives: [B]\n",
    );
    let documents = org_charter_raw_documents(temporary.path(), false).unwrap();
    let members: Vec<&str> = documents
        .iter()
        .filter(|document| document.declared() == "a")
        .map(|document| document.member())
        .collect();
    assert_eq!(members, ["a"]);
}

#[cfg(unix)]
#[test]
fn exists_permission_errors_escape_the_lenient_read_catch() {
    use std::os::unix::fs::PermissionsExt;
    struct RestorePermissions(std::path::PathBuf, fs::Permissions);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }
    let temporary = fixture();
    let directory = temporary.path().join("a");
    let _restore = RestorePermissions(
        directory.clone(),
        fs::metadata(&directory).unwrap().permissions(),
    );
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o0)).unwrap();
    let error = fs::metadata(directory.join("org-charter.yaml"))
        .expect_err("fixture requires an unprivileged process");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    let error = read_org_required_tokens(temporary.path(), ArtifactKind::Directive, false)
        .expect_err("Source exists error escapes even leniently");
    assert!(
        matches!(error, sk_doctrine::DoctrineError::Io { .. }),
        "{error:?}"
    );
}
