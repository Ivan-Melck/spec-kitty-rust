use sk_mission::{MissionError, WorkPackageGraph, list_missions};
use std::fs;

#[cfg(unix)]
#[test]
fn non_utf8_mission_names_are_refused_not_silently_lost() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    let mission = temp
        .path()
        .join("kitty-specs")
        .join(std::ffi::OsString::from_vec(b"bad\xffname".to_vec()));
    fs::create_dir_all(&mission).unwrap();
    fs::write(
        mission.join("meta.json"),
        r#"{"mission_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#,
    )
    .unwrap();
    assert!(matches!(
        list_missions(temp.path()),
        Err(MissionError::NonUtf8FileName { .. })
    ));
}

#[cfg(unix)]
#[test]
fn an_in_root_specs_symlink_is_refused_before_resolution() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("real-specs")).unwrap();
    std::os::unix::fs::symlink("real-specs", temp.path().join("kitty-specs")).unwrap();
    assert!(matches!(
        list_missions(temp.path()),
        Err(MissionError::SymlinkedEntry { .. })
    ));
}

#[test]
fn only_filenames_claiming_a_wp_id_enter_the_graph() {
    let temp = tempfile::tempdir().unwrap();
    for (file, id) in [
        ("WP01-normal.md", "WP01"),
        ("WP1-note.md", "WP02"),
        ("WP-notes.md", "WP03"),
    ] {
        fs::write(
            temp.path().join(file),
            format!("---\nwork_package_id: {id}\ntitle: Test\n---\nBody\n"),
        )
        .unwrap();
    }
    let graph = WorkPackageGraph::from_tasks_dir(temp.path()).unwrap();
    assert_eq!(
        graph.ids().map(ToString::to_string).collect::<Vec<_>>(),
        ["WP01"]
    );
}
