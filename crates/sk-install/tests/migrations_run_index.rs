//! `4.0.0rc5_heal_run_index_paths` acceptance tests.
//!
//! The project is always a temporary directory; the index bytes are asserted
//! exactly, including Python's sorted keys, two-space indentation and
//! ASCII-only string escaping.

use std::path::Path;

use serde_json::json;
use sk_install::migrations::run_index;

fn write_index(root: &Path, value: serde_json::Value) {
    let path = run_index::feature_runs_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn read_index(root: &Path) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(run_index::feature_runs_path(root)).expect("index"),
    )
    .expect("json")
}

#[test]
fn absolute_paths_inside_and_outside_are_healed_to_portable_tokens() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let inside = root.join(".kittify/runtime/runs/01AAA");
    let outside = Path::new("/elsewhere/copied/.kittify/runtime/runs/01BBB");
    write_index(
        root,
        json!({
            "inside": {"run_id": "01AAA", "run_dir": inside.to_string_lossy()},
            "outside": {"run_id": "01BBB", "run_dir": outside.to_string_lossy()},
            "relative": {"run_id": "01CCC", "run_dir": ".kittify/runtime/runs/01CCC"},
            "no-id": {"run_dir": outside.to_string_lossy()},
            "scalar": "ignored"
        }),
    );

    assert!(run_index::detect(root).expect("detect"));
    let outcome = run_index::apply(root).expect("apply");
    assert!(outcome.changed);
    assert_eq!(outcome.changes.len(), 3);

    let index = read_index(root);
    assert_eq!(index["inside"]["run_dir"], ".kittify/runtime/runs/01AAA");
    assert_eq!(index["outside"]["run_dir"], ".kittify/runtime/runs/01BBB");
    assert_eq!(index["relative"]["run_dir"], ".kittify/runtime/runs/01CCC");
    assert_eq!(index["no-id"]["run_dir"], ".kittify/runtime/runs/01BBB");
    assert_eq!(index["scalar"], "ignored");
    assert!(!run_index::detect(root).expect("detect after"));
    let again = run_index::apply(root).expect("second apply");
    assert!(!again.changed);
    assert!(again.changes.is_empty());
}

#[test]
fn an_in_tree_absolute_path_preserves_its_subpath() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let nested = root.join(".kittify/runtime/runs/01AAA/extra");
    write_index(
        root,
        json!({"m": {"run_id": "01AAA", "run_dir": nested.to_string_lossy()}}),
    );
    run_index::apply(root).expect("apply");
    assert_eq!(
        read_index(root)["m"]["run_dir"],
        ".kittify/runtime/runs/01AAA/extra"
    );
}

#[test]
fn writeback_is_python_shaped_and_ascii_escaped() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let outside = Path::new("/tmp/é/01ZZZ");
    write_index(
        root,
        json!({"m": {"run_dir": outside.to_string_lossy(), "run_id": "01ZZZ", "label": "é"}}),
    );
    run_index::apply(root).expect("apply");
    let raw = std::fs::read_to_string(run_index::feature_runs_path(root)).unwrap();
    assert!(raw.contains("\\u00e9"), "{raw}");
    assert!(!raw.ends_with('\n'), "{raw:?}");
    assert!(raw.starts_with("{\n  \"m\": {"), "{raw}");
}

#[test]
fn missing_and_corrupt_indexes_are_no_ops_and_create_nothing() {
    let missing = tempfile::tempdir().expect("temp");
    std::fs::create_dir_all(missing.path().join(".kittify")).unwrap();
    assert!(!run_index::detect(missing.path()).expect("detect"));
    assert!(!run_index::apply(missing.path()).expect("apply").changed);
    assert!(!missing.path().join(".kittify/runtime").exists());

    let corrupt = tempfile::tempdir().expect("temp");
    std::fs::create_dir_all(corrupt.path().join(".kittify/runtime")).unwrap();
    std::fs::write(run_index::feature_runs_path(corrupt.path()), "{not json").unwrap();
    assert!(!run_index::detect(corrupt.path()).expect("detect"));
    assert!(!run_index::apply(corrupt.path()).expect("apply").changed);
}

#[test]
fn a_non_object_root_is_refused_not_silently_replaced() {
    let directory = tempfile::tempdir().expect("temp");
    let path = run_index::feature_runs_path(directory.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[]").unwrap();
    assert!(run_index::detect(directory.path()).is_err());
    assert!(run_index::apply(directory.path()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
}

#[test]
fn an_undecodable_index_is_refused_not_read_as_empty() {
    let directory = tempfile::tempdir().expect("temp");
    let path = run_index::feature_runs_path(directory.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
    assert!(run_index::detect(directory.path()).is_err());
    assert!(run_index::apply(directory.path()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), vec![0xff, 0xfe, 0x00]);
}

#[test]
fn preview_reports_without_writing() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let before = "{\"m\":{\"run_dir\":\"/tmp/01AAA\"}}";
    std::fs::create_dir_all(root.join(".kittify/runtime")).unwrap();
    std::fs::write(run_index::feature_runs_path(root), before).unwrap();
    let preview = run_index::preview(root).expect("preview");
    assert_eq!(preview.len(), 1);
    assert_eq!(
        std::fs::read_to_string(run_index::feature_runs_path(root)).unwrap(),
        before
    );
}
