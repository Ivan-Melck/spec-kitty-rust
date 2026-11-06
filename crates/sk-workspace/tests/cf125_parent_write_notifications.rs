//! Parent functional composition smoke; not Source differential certification.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sk_kernel::source_writer::{WriteObserverError, observe_writes};
use sk_workspace::{BakeUndoOutcome, LanesManifest, LanesStore, ObservationSource};

type Observations = Arc<Mutex<Vec<(PathBuf, Vec<u8>)>>>;

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("meta.json"),
        b"{\"extension\":\"kept\"}\n",
    )
    .unwrap();
    directory
}

fn manifest() -> LanesManifest {
    serde_json::from_value(json!({
        "version": 1, "mission_slug": "001-example", "mission_branch": "topic",
        "target_branch": "main", "lanes": [], "computed_at": "2026-01-01T00:00:00Z",
        "computed_from": "authored test fixture"
    }))
    .unwrap()
}

#[test]
fn direct_metadata_writer_notifies_its_exact_bytes_once_and_noops_stay_noops() {
    let directory = fixture();
    let observations: Observations = Arc::default();
    let sink = Arc::clone(&observations);
    let token = observe_writes(move |path: &Path, bytes: &[u8]| {
        assert_eq!(fs::read(path).unwrap(), bytes, "already published");
        sink.lock().unwrap().push((path.to_owned(), bytes.to_vec()));
        Ok(())
    });
    let path = sk_mission::record_mission_number(directory.path(), 7).unwrap();
    sk_mission::record_mission_number(directory.path(), 7).unwrap();
    sk_kernel::stop_observing_writes(&token).unwrap();
    let observations = observations.lock().unwrap();
    assert_eq!(
        observations.len(),
        1,
        "Native durable no-op contract retained"
    );
    assert_eq!(observations[0], (path.clone(), fs::read(path).unwrap()));
}

#[test]
fn bake_capture_comes_from_inside_the_real_metadata_writer() {
    let directory = fixture();
    let observations: Observations = Arc::default();
    let sink = Arc::clone(&observations);
    let token = observe_writes(move |path: &Path, bytes: &[u8]| {
        sink.lock().unwrap().push((path.to_owned(), bytes.to_vec()));
        Ok(())
    });
    let capture = sk_workspace::bake_mission_number(directory.path(), 8).unwrap();
    sk_kernel::stop_observing_writes(&token).unwrap();
    assert_eq!(capture.source, ObservationSource::Observer);
    assert_eq!(capture.written, fs::read(capture.path).unwrap());
    assert_eq!(
        observations.lock().unwrap().len(),
        1,
        "no boundary double-notify"
    );
}

#[test]
fn metadata_observer_error_retains_cause_and_published_state() {
    let directory = fixture();
    let token = observe_writes(|_: &Path, _: &[u8]| {
        Err(WriteObserverError::os(std::io::Error::other(
            "actual observer failure",
        )))
    });
    let error = sk_workspace::bake_mission_number(directory.path(), 9).unwrap_err();
    sk_kernel::stop_observing_writes(&token).unwrap();
    assert!(error.published_before_error());
    match error {
        sk_workspace::WriteLedgerError::Mission(sk_mission::MissionError::WriteObserver {
            source,
            ..
        }) => {
            assert!(source.is_os_error());
            assert_eq!(source.message(), "actual observer failure");
        }
        other => panic!("wrong typed cause: {other}"),
    }
    let value: Value =
        serde_json::from_slice(&fs::read(directory.path().join("meta.json")).unwrap()).unwrap();
    assert_eq!(value["mission_number"], 9);
    assert_eq!(value["extension"], "kept");
}

#[test]
fn a_callback_foreign_edit_is_never_misattributed_as_the_bakes_payload() {
    let directory = fixture();
    let token = observe_writes(|path: &Path, bytes: &[u8]| {
        let text = std::str::from_utf8(bytes).unwrap();
        let foreign = text.replace("\"mission_number\": 7", "\"mission_number\": 8");
        assert_ne!(foreign.as_bytes(), bytes);
        assert_eq!(
            foreign.len(),
            bytes.len(),
            "same length cannot prove ownership"
        );
        fs::write(path, foreign).unwrap();
        Ok(())
    });
    let capture = sk_workspace::bake_mission_number(directory.path(), 7).unwrap();
    sk_kernel::stop_observing_writes(&token).unwrap();
    let own: Value = serde_json::from_slice(&capture.written).unwrap();
    assert_eq!(own["mission_number"], 7);
    assert_eq!(capture.source, ObservationSource::Observer);
    assert_eq!(
        sk_workspace::undo_meta_bake(&capture).unwrap(),
        BakeUndoOutcome::ChangedByAnotherWriter
    );
    let foreign: Value = serde_json::from_slice(&fs::read(capture.path).unwrap()).unwrap();
    assert_eq!(foreign["mission_number"], 8);
}

#[test]
fn both_lanes_manifest_writers_notify_from_the_owning_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let store = LanesStore::with_lock_path(directory.path(), directory.path().join("test.lock"));
    let observations: Observations = Arc::default();
    let sink = Arc::clone(&observations);
    let token = observe_writes(move |path: &Path, bytes: &[u8]| {
        assert_eq!(fs::read(path).unwrap(), bytes);
        sink.lock().unwrap().push((path.to_owned(), bytes.to_vec()));
        Ok(())
    });
    store.write(&manifest()).unwrap();
    store
        .update(|current| {
            let mut updated = current.clone();
            updated.computed_from = "updated authored fixture".to_owned();
            Ok(updated)
        })
        .unwrap();
    sk_kernel::stop_observing_writes(&token).unwrap();
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(
        observations[1],
        (store.path(), fs::read(store.path()).unwrap())
    );
}

#[test]
fn lanes_observer_failure_is_typed_and_does_not_claim_no_publication() {
    let directory = tempfile::tempdir().unwrap();
    let store = LanesStore::with_lock_path(directory.path(), directory.path().join("test.lock"));
    let token = observe_writes(|_: &Path, _: &[u8]| {
        Err(WriteObserverError::os(std::io::Error::other(
            "lanes observer refused",
        )))
    });
    let error = store.write(&manifest()).unwrap_err();
    sk_kernel::stop_observing_writes(&token).unwrap();
    assert!(matches!(
        error,
        sk_workspace::WorkspaceError::WriteObserver { .. }
    ));
    assert_eq!(store.read().unwrap(), Some(manifest()));
}
