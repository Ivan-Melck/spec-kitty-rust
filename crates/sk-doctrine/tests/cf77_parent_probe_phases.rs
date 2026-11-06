use sk_doctrine::{
    IdentityIndex, LoadDeniedOptions, governance_charter_selection, ledger_may_hold_deny,
    load_denied,
};
use std::{
    fs,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};
fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn mtime(path: &Path, time: std::time::SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
}
#[test]
fn warm_pointed_probe_uses_same_stat_snapshot() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let path = root.join("charter.yaml");
    write(
        &root.join(".kittify/config.yaml"),
        "charter: charter.yaml\n",
    );
    write(&path, "activated_directives: []\n");
    assert!(!ledger_may_hold_deny(root).unwrap());
    let metadata = fs::metadata(&path).unwrap();
    write(&path, "activated_directives: [ \n");
    mtime(&path, metadata.modified().unwrap());
    assert_eq!(fs::metadata(&path).unwrap().len(), metadata.len());
    assert!(
        !ledger_may_hold_deny(root)
            .expect("Source reuses cached pointed-file snapshot when key stat is unchanged")
    );
}
#[test]
fn distinct_before_epoch_mtimes_are_distinct_probe_keys() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join(".kittify/config.yaml");
    write(&path, "activated_directives: []  \n");
    mtime(&path, UNIX_EPOCH - Duration::from_secs(1));
    assert!(!ledger_may_hold_deny(t.path()).unwrap());
    let len = fs::metadata(&path).unwrap().len();
    write(&path, "deactivated_directives: []\n");
    mtime(&path, UNIX_EPOCH - Duration::from_secs(2));
    assert_eq!(fs::metadata(&path).unwrap().len(), len);
    assert!(
        ledger_may_hold_deny(t.path()).unwrap(),
        "different signed mtimes cannot both collapse to epoch-zero"
    );
}
#[test]
fn activation_directory_is_source_absent_ledger_not_read_failure() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join(".kittify/charter/activation.yaml")).unwrap();
    assert!(ledger_may_hold_deny(t.path()).unwrap());
    assert!(
        load_denied(t.path(), &LoadDeniedOptions::default())
            .unwrap()
            .is_empty()
    );
}
#[test]
fn config_deny_builds_its_own_source_index_not_callers_snapshot() {
    let t = tempfile::tempdir().unwrap();
    write(
        &t.path().join(".kittify/charter/charter.yaml"),
        "governance:\n  charter:\n    selected_directives: [010-specification-fidelity-requirement]\n",
    );
    write(
        &t.path().join(".kittify/charter/activation.yaml"),
        "deactivated_directives: [DIRECTIVE_010]\n",
    );
    let empty = t.path().join("empty-built-in");
    fs::create_dir_all(&empty).unwrap();
    let earlier_index = IdentityIndex::from_roots(&empty, &[], None);
    let cfg = governance_charter_selection(t.path(), Some(&earlier_index)).unwrap();
    assert!(
        cfg.selected_directives.is_empty(),
        "Source load_denied builds an index from repo roots at its own phase"
    );
}

#[test]
fn config_directory_still_fails_fresh_wrapper_validation() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join(".kittify/config.yaml")).unwrap();
    assert!(load_denied(t.path(), &LoadDeniedOptions::default()).is_err());
}
