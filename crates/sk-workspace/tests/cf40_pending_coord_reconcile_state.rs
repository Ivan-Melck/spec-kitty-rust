//! CF40: the canonical persisted merge-state reader and its marker enumeration.
//!
//! Every fixture is a real `tempfile` directory holding real
//! `.kittify/runtime/merge/<mission_id>/state.json` files. Nothing here touches this
//! checkout, and every assertion is about observed bytes on disk: which records are
//! yielded, in which order, what survived the known-field filter, and that a read
//! creates, rewrites or repairs nothing.
//!
//! Not claimed here: Source's `AttributeError` crash on a top-level JSON array (this
//! reader classifies that shape as the undecodable record the docstring promises to
//! skip) and a non-UTF-8 *directory* name ordering matrix are recorded as remaining
//! debt in the lane report, not exercised as parity.

use std::fs;
use std::path::{Path, PathBuf};

use sk_kernel::python_json::PythonJsonValue;
use sk_workspace::consolidation::state::{
    ConsolidationState, STATE_FILENAME, STRAND_SHAS_KEY, UNSETTLED_ALL,
    iter_pending_coord_reconcile_markers, load_state, load_state_file, marker_strand_shas,
    merge_runtime_dir, parse_reconcile_marker, state_path,
};

/// A marker with every repair input populated, plus the given strand SHAs.
fn marker(stranded: &[&str], strand_shas: Option<&[&str]>) -> serde_json::Value {
    let mut marker = serde_json::json!({
        "coord_ref": "kitty/mission-cf40-mission-01KNXQS9",
        "captured_sha": "0123456789abcdef0123456789abcdef01234567",
        "coord_worktree": "/tmp/cf40-coord-worktree",
        "stranded_wp_ids": stranded,
        "revert_error": "conflict",
        "detected_at": "2026-11-05T00:00:00+00:00",
    });
    if let (Some(shas), Some(object)) = (strand_shas, marker.as_object_mut()) {
        object.insert(
            STRAND_SHAS_KEY.to_owned(),
            serde_json::Value::Array(
                shas.iter()
                    .map(|sha| serde_json::Value::String((*sha).to_owned()))
                    .collect(),
            ),
        );
    }
    marker
}

/// A complete record with `mission_id`/`mission_slug`/`target_branch`/`wp_order`.
fn record(
    mission_id: &str,
    mission_slug: &str,
    marker: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut record = serde_json::json!({
        "mission_id": mission_id,
        "mission_slug": mission_slug,
        "target_branch": "main",
        "wp_order": ["WP01", "WP02"],
    });
    if let (Some(marker), Some(object)) = (marker, record.as_object_mut()) {
        object.insert("pending_coord_reconcile".to_owned(), marker);
    }
    record
}

fn write_record(root: &Path, mission_id: &str, record: &serde_json::Value) -> PathBuf {
    let path = state_path(root, mission_id);
    fs::create_dir_all(path.parent().expect("state parent")).expect("create mission runtime dir");
    fs::write(
        &path,
        serde_json::to_string_pretty(record).expect("serialize"),
    )
    .expect("write state");
    path
}

fn write_raw(root: &Path, mission_id: &str, bytes: &[u8]) -> PathBuf {
    let path = state_path(root, mission_id);
    fs::create_dir_all(path.parent().expect("state parent")).expect("create mission runtime dir");
    fs::write(&path, bytes).expect("write state");
    path
}

fn as_text(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::String(text) => text.to_scalar_string().expect("scalar text"),
        other => panic!("expected a string value, got {other:?}"),
    }
}

fn tree_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("read directory")
            .map(|entry| entry.expect("directory entry"))
            .collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("path under root")
                .to_string_lossy()
                .replace('\\', "/");
            if entry.metadata().expect("metadata").is_dir() {
                out.push((format!("{relative}/"), Vec::new()));
                walk(root, &path, out);
            } else {
                out.push((relative, fs::read(&path).expect("read file")));
            }
        }
    }
    let mut snapshot = Vec::new();
    walk(root, root, &mut snapshot);
    snapshot
}

#[test]
fn cf40_enumerates_every_marked_state_in_python_directory_order() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    // Created out of order on purpose: the yield order must be directory order.
    write_record(
        root,
        "020-empty-marker",
        &record("02", "020-empty-marker", Some(serde_json::json!({}))),
    );
    write_record(
        root,
        "010-full-marker",
        &record("01", "010-full-marker", Some(marker(&["WP01"], None))),
    );
    write_record(root, "030-coherent", &record("03", "030-coherent", None));
    write_record(
        root,
        "040-null-marker",
        &record("04", "040-null-marker", Some(serde_json::Value::Null)),
    );

    let states = iter_pending_coord_reconcile_markers(root);
    let slugs: Vec<String> = states
        .iter()
        .map(|state| as_text(state.mission_slug().expect("mission slug")))
        .collect();
    // The empty mapping IS a marker (Source: non-None), the null marker is not.
    assert_eq!(slugs, vec!["010-full-marker", "020-empty-marker"]);

    // The empty mapping is yielded but is not a parseable strand.
    assert_eq!(
        states[1].pending_coord_reconcile(),
        Some(&PythonJsonValue::ExactObject(Default::default()))
    );
    assert!(states[1].parse_pending_coord_reconcile().is_none());
    assert!(states[1].marker_strand_shas().is_none());

    let parsed = states[0]
        .parse_pending_coord_reconcile()
        .expect("parseable marker");
    assert_eq!(parsed.coord_ref(), "kitty/mission-cf40-mission-01KNXQS9");
    assert_eq!(
        parsed.stranded_wp_ids_as_strings().expect("scalar ids"),
        vec!["WP01"]
    );
    assert_eq!(parsed.strand_shas(), None);
}

#[test]
fn cf40_skips_unreadable_and_corrupt_states_without_aborting_the_sweep() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    write_raw(root, "010-corrupt-json", b"{not json");
    write_raw(root, "020-top-level-array", b"[1, 2, 3]");
    write_raw(
        root,
        "030-missing-required",
        br#"{"mission_id": "03", "mission_slug": "030"}"#,
    );
    write_raw(root, "040-non-utf8", &[0xff, 0xfe, 0x00, 0x7b]);
    let directory_state = state_path(root, "050-state-is-a-directory");
    fs::create_dir_all(&directory_state).expect("state path as directory");
    write_record(
        root,
        "060-good",
        &record("06", "060-good", Some(marker(&["WP02"], None))),
    );

    let states = iter_pending_coord_reconcile_markers(root);
    assert_eq!(
        states.len(),
        1,
        "only the decodable marked record is yielded"
    );
    assert_eq!(as_text(states[0].mission_id().expect("mission id")), "06");

    // The single-mission reader (Source's fail-closed path) still reports each cause.
    for mission_id in [
        "010-corrupt-json",
        "020-top-level-array",
        "030-missing-required",
        "040-non-utf8",
        "050-state-is-a-directory",
    ] {
        let error = load_state(root, mission_id).expect_err("undecodable record must refuse");
        assert!(
            error
                .path()
                .ends_with(Path::new(mission_id).join(STATE_FILENAME)),
            "error names the offending record: {}",
            error.path().display()
        );
        assert!(!error.reason().is_empty(), "error carries a reason");
    }
}

#[test]
fn cf40_absent_files_and_directories_are_not_errors_and_create_nothing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    // No .kittify runtime at all: enumeration is empty, not an error, and nothing appears.
    let before = tree_snapshot(root);
    assert!(iter_pending_coord_reconcile_markers(root).is_empty());
    assert_eq!(
        load_state(root, "anything").expect("absent is not an error"),
        None
    );
    assert_eq!(
        tree_snapshot(root),
        before,
        "a read must not create the runtime tree"
    );

    // A mission directory without a state file is absent too.
    fs::create_dir_all(merge_runtime_dir(root).join("empty-mission")).expect("dir");
    let before = tree_snapshot(root);
    assert_eq!(load_state(root, "empty-mission").expect("absent"), None);
    assert!(iter_pending_coord_reconcile_markers(root).is_empty());
    assert_eq!(tree_snapshot(root), before);
}

#[test]
fn cf40_keeps_known_fields_and_applies_the_source_fail_closed_projections() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    let raw = serde_json::json!({
        "mission_id": "01",
        "mission_slug": "cf40-mission",
        "target_branch": "main",
        "wp_order": ["WP01", "WP02"],
        "unknown_field": "dropped",
        "pre_mutation_refs": {"main": "abc"},
        "post_mutation_refs": {"main": 5},
        "dependency_ref_pins": {},
        "dependency_refs": ["refs/heads/dep", 7],
        "snapshot_lane_branches": ["lane-a"],
        "unsettled_refs": "everything",
        "advance_intents": {"main": ["base"]},
        "completed_wps": ["WP01"],
        "strategy": "squash",
        "pending_coord_reconcile": {"strand_shas": []},
    });
    let path = write_record(root, "010-projections", &raw);
    let state = load_state_file(&path).expect("readable").expect("present");

    // Unknown keys are dropped by the known-field filter.
    assert!(state.field("unknown_field").is_none());
    // Authored values that Source does not coerce survive verbatim.
    assert_eq!(
        as_text(state.field("strategy").expect("strategy")),
        "squash"
    );
    assert_eq!(as_text(state.mission_id().expect("mission id")), "01");
    // A valid str->str map survives; a mixed one collapses to {}.
    assert_eq!(
        state.field("pre_mutation_refs"),
        Some(&PythonJsonValue::ExactObject({
            let mut map = sk_kernel::python_json::PythonJsonDict::new();
            map.insert(
                sk_kernel::PythonString::from_scalars("main"),
                PythonJsonValue::String(sk_kernel::PythonString::from_scalars("abc")),
            );
            map
        }))
    );
    assert_eq!(
        state.field("post_mutation_refs"),
        Some(&PythonJsonValue::ExactObject(Default::default()))
    );
    // A malformed list collapses to []; a valid one survives.
    assert_eq!(
        state.field("dependency_refs"),
        Some(&PythonJsonValue::Array(Vec::new()))
    );
    assert_eq!(
        state.field("snapshot_lane_branches"),
        Some(&PythonJsonValue::Array(vec![PythonJsonValue::String(
            sk_kernel::PythonString::from_scalars("lane-a")
        )]))
    );
    // A malformed unsettled set fails closed to the every-branch sentinel.
    assert_eq!(
        state.field("unsettled_refs"),
        Some(&PythonJsonValue::Array(vec![PythonJsonValue::String(
            sk_kernel::PythonString::from_scalars(UNSETTLED_ALL)
        )]))
    );
    // A one-SHA intent chain is no proof at all.
    assert_eq!(
        state.field("advance_intents"),
        Some(&PythonJsonValue::ExactObject(Default::default()))
    );
    // An explicitly recorded empty strand list is recorded-but-empty, not absent.
    assert_eq!(state.marker_strand_shas(), Some(Vec::new()));
}

#[test]
fn cf40_required_fields_are_enforced_and_the_record_is_complete() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    let path = write_record(root, "010-complete", &record("01", "cf40-mission", None));
    let state = load_state_file(&path).expect("readable").expect("present");

    // Every known field is materialized, in Source's declaration order.
    for name in sk_workspace::consolidation::state::KNOWN_FIELDS {
        assert!(state.field(name).is_some(), "missing known field {name}");
    }
    // Dataclass defaults: absent `strategy` is "merge", absent marker is None.
    assert_eq!(as_text(state.field("strategy").expect("strategy")), "merge");
    assert_eq!(state.pending_coord_reconcile(), None);

    // A record missing any required field is an undecodable record (Source TypeError).
    for missing in ["mission_id", "mission_slug", "target_branch", "wp_order"] {
        let mut value = record("01", "cf40-mission", None);
        value.as_object_mut().expect("object").remove(missing);
        let path = write_record(root, &format!("020-missing-{missing}"), &value);
        let error = load_state_file(&path).expect_err("missing required field refuses");
        assert!(
            error.reason().contains(missing),
            "reason names the missing field {missing}: {}",
            error.reason()
        );
    }
}

#[test]
fn cf40_marker_strand_shas_and_repair_inputs_follow_the_source_projection() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();

    // Legacy marker (no strand_shas) reads as "nothing recorded".
    let legacy = write_record(
        root,
        "010-legacy",
        &record("01", "cf40-mission", Some(marker(&["WP01"], None))),
    );
    let state = load_state_file(&legacy)
        .expect("readable")
        .expect("present");
    assert_eq!(state.marker_strand_shas(), None);
    let parsed = state.parse_pending_coord_reconcile().expect("parseable");
    assert_eq!(
        parsed.stranded_wp_ids_as_strings().expect("scalar"),
        vec!["WP01"]
    );
    assert_eq!(parsed.coord_worktree(), "/tmp/cf40-coord-worktree");

    // Recorded SHAs survive; a malformed value reads as nothing recorded.
    let recorded = write_record(
        root,
        "020-recorded",
        &record(
            "02",
            "cf40-mission",
            Some(marker(&["WP01"], Some(&["abc", "def"]))),
        ),
    );
    let state = load_state_file(&recorded)
        .expect("readable")
        .expect("present");
    let shas: Vec<String> = state
        .marker_strand_shas()
        .expect("recorded")
        .iter()
        .map(|sha| sha.to_scalar_string().expect("scalar"))
        .collect();
    assert_eq!(shas, vec!["abc", "def"]);

    let mut malformed = marker(&["WP01"], None);
    malformed
        .as_object_mut()
        .expect("object")
        .insert(STRAND_SHAS_KEY.to_owned(), serde_json::json!(5));
    assert_eq!(
        marker_strand_shas(&PythonJsonValue::from_serde(&malformed)),
        None
    );

    // An empty write-set is not a strand; missing repair inputs are not a marker.
    let empty_strand = write_record(
        root,
        "030-empty-strand",
        &record("03", "cf40-mission", Some(marker(&[], None))),
    );
    assert!(
        load_state_file(&empty_strand)
            .expect("readable")
            .expect("present")
            .parse_pending_coord_reconcile()
            .is_none()
    );
    for key in ["coord_ref", "captured_sha", "coord_worktree"] {
        let mut broken = marker(&["WP01"], None);
        broken.as_object_mut().expect("object").remove(key);
        let path = write_record(
            root,
            &format!("040-no-{key}"),
            &record("04", "cf40-mission", Some(broken)),
        );
        let state = load_state_file(&path).expect("readable").expect("present");
        assert!(
            state.parse_pending_coord_reconcile().is_none(),
            "a marker without {key} is unparseable"
        );
        // The raw marker is still enumerated: the consumer decides it is unparseable.
        assert!(state.pending_coord_reconcile().is_some());
    }

    // `str(w)` projection: a JSON string keeps its text, a non-string uses value repr.
    let mut mixed = marker(&["WP01"], None);
    mixed.as_object_mut().expect("object").insert(
        "stranded_wp_ids".to_owned(),
        serde_json::json!(["WP01", 5, null]),
    );
    let parsed = parse_reconcile_marker(&PythonJsonValue::from_serde(&mixed)).expect("parseable");
    assert_eq!(
        parsed.stranded_wp_ids_as_strings().expect("scalar"),
        vec!["WP01", "5", "None"]
    );
}

#[test]
fn cf40_foreign_missions_keep_their_own_identity_and_the_read_is_read_only() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    write_record(
        root,
        "010-foreign",
        &record("foreign-id", "010-foreign", Some(marker(&["WP09"], None))),
    );
    write_record(
        root,
        "020-own",
        &record("own-id", "020-own", Some(marker(&["WP01"], None))),
    );

    let before = tree_snapshot(root);
    let states = iter_pending_coord_reconcile_markers(root);
    assert_eq!(states.len(), 2);
    assert_eq!(as_text(states[0].mission_id().expect("id")), "foreign-id");
    assert_eq!(
        as_text(states[0].mission_slug().expect("slug")),
        "010-foreign"
    );
    assert_eq!(
        states[0]
            .parse_pending_coord_reconcile()
            .expect("parseable")
            .stranded_wp_ids_as_strings()
            .expect("scalar"),
        vec!["WP09"]
    );
    assert_eq!(as_text(states[1].mission_id().expect("id")), "own-id");
    assert_eq!(
        states[1]
            .parse_pending_coord_reconcile()
            .expect("parseable")
            .stranded_wp_ids_as_strings()
            .expect("scalar"),
        vec!["WP01"]
    );

    // Read-only proof: identical tree, no lock file, no marker rewrite.
    assert_eq!(tree_snapshot(root), before, "enumeration rewrote the tree");
    assert!(
        !merge_runtime_dir(root)
            .join("010-foreign")
            .join("lock")
            .exists()
    );
}

#[test]
fn cf40_state_path_is_the_one_runtime_shape() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    assert_eq!(
        state_path(root, "060-mission-id"),
        root.join(".kittify")
            .join("runtime")
            .join("merge")
            .join("060-mission-id")
            .join(STATE_FILENAME)
    );
    assert_eq!(
        merge_runtime_dir(root),
        root.join(".kittify").join("runtime").join("merge")
    );
    // The decoded record type is reachable and constructible from the canonical value model.
    let value = PythonJsonValue::from_serde(&record("01", "cf40-mission", None));
    let state = ConsolidationState::from_json(root, &value).expect("decodes");
    assert_eq!(as_text(state.target_branch().expect("target")), "main");
}
