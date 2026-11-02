use sk_status::snapshot::{
    MAX_DIARY_BYTES, ReplayInput, SnapshotError, materialize, read_raw_events,
};

#[test]
fn oversized_diary_is_refused_before_reading_the_payload() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("status.events.jsonl");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(MAX_DIARY_BYTES + 1)
        .unwrap();
    assert!(matches!(
        read_raw_events(&path),
        Err(SnapshotError::InputRead { .. })
    ));
}

#[test]
fn malformed_existing_snapshot_remains_legacy_but_nonregular_input_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("status.json");
    let diary = include_bytes!("fixtures/replay/cases/dedupe-first-wins/status.events.jsonl");
    std::fs::write(temp.path().join("status.events.jsonl"), diary).unwrap();
    for content in ["not json", "[]", "null"] {
        std::fs::write(&path, content).unwrap();
        let input = ReplayInput::read_dir(temp.path()).unwrap();
        assert!(input.existing_snapshot.is_some(), "{content}");
        let snapshot = materialize(&input).unwrap();
        assert_eq!(snapshot.schema_version, None, "{content}");
        assert!(!snapshot.work_packages.is_empty());
        for state in snapshot.work_packages.values() {
            assert!(state.get("implementer_of_record").is_none(), "{content}");
        }
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(
        ReplayInput::read_dir(temp.path()),
        Err(SnapshotError::InputRead { .. })
    ));
}

#[test]
fn absent_and_typeless_metadata_do_not_invent_a_mission_type() {
    for meta in [None, Some(serde_json::json!({"mission_slug": "m"}))] {
        let snapshot = materialize(&ReplayInput {
            meta,
            ..ReplayInput::default()
        })
        .unwrap();
        assert_eq!(snapshot.to_json_value()["mission_type"], "");
    }
}

#[test]
fn universal_newlines_and_read_error_paths_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("status.events.jsonl");
    for separator in ["\n", "\r\n", "\r"] {
        std::fs::write(&path, format!("{{}}{separator}{{}}{separator}")).unwrap();
        assert_eq!(read_raw_events(&path).unwrap().len(), 2);
        std::fs::write(&path, format!("{{}}{separator}broken{separator}")).unwrap();
        assert!(matches!(
            read_raw_events(&path),
            Err(SnapshotError::InvalidJson { line: 2, .. })
        ));
    }
    std::fs::write(&path, [0xff]).unwrap();
    let error = read_raw_events(&path).unwrap_err();
    assert!(matches!(error, SnapshotError::InputRead { .. }));
    assert!(error.to_string().contains(path.to_str().unwrap()));
    std::fs::write(&path, "{\"number\":1e400}\n").unwrap();
    assert!(matches!(
        read_raw_events(&path),
        Err(SnapshotError::InvalidJson { .. })
    ));
}

#[cfg(unix)]
#[test]
fn replay_file_readers_cannot_block_on_fifos() {
    for name in ["status.events.jsonl", "meta.json", "status.json"] {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(temp.path().join(name))
                .status()
                .unwrap()
                .success()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(ReplayInput::read_dir(temp.path()));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("replay must not block opening special files");
        assert!(
            matches!(result, Err(SnapshotError::InputRead { .. })),
            "{name}"
        );
    }
}
