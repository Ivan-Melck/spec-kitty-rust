//! Verified Mission-write rollback protocol tests
//! (`mission_write.capture_rollback_point` / `rollback_events_log` /
//! `rollback_status_artifacts`).
//!
//! These cover the guarantees the generic byte snapshot does **not** provide:
//! the capture needs the registered hold, a rollback only cuts whole rows that
//! this operation appended, committed rows are preserved, and every ambiguity
//! (torn tail, unreadable `HEAD`, vanished/shrunk log) refuses with the files
//! left byte-identical.

mod mutation_support;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sk_kernel::locks::LockWait;
use sk_status::SNAPSHOT_FILENAME;
use sk_status::mutation::{
    AppendOptions, HeadCommittedProbe, MutationError, RollbackRefusal, acquire_mission_lock,
    append_rows_atomic, appended_event_ids, capture_rollback_point, events_path,
    rollback_events_log, rollback_status_artifacts,
};

use mutation_support::{TempMission, read_text, temp_mission};

struct Probe {
    ids: BTreeSet<String>,
    fail: bool,
}

impl Probe {
    fn returning(ids: &[&str]) -> Self {
        Self {
            ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            ids: BTreeSet::new(),
            fail: true,
        }
    }
}

impl HeadCommittedProbe for Probe {
    fn committed_event_ids(&self, _events_path: &Path) -> Result<BTreeSet<String>, String> {
        if self.fail {
            Err("git rev-parse failed".to_owned())
        } else {
            Ok(self.ids.clone())
        }
    }
}

fn row(event_id: &str, to_lane: &str) -> Value {
    json!({
        "event_id": event_id,
        "mission_slug": mutation_support::MISSION_SLUG,
        "mission_id": mutation_support::MISSION_ID,
        "wp_id": "WP01",
        "from_lane": "genesis",
        "to_lane": to_lane,
        "at": "2026-11-02T09:00:00Z",
        "actor": "cli",
        "force": false,
        "execution_mode": "worktree",
        "reason": null,
        "review_ref": null,
        "evidence": null,
        "policy_metadata": null,
    })
}

fn append(mission: &TempMission, ids: &[(&str, &str)]) {
    let rows: Vec<Value> = ids.iter().map(|(id, lane)| row(id, lane)).collect();
    append_rows_atomic(
        &events_path(&mission.feature_dir),
        &rows,
        &AppendOptions::default(),
    )
    .expect("append rows");
}

fn lock(mission: &TempMission) -> sk_status::mutation::MissionLock {
    acquire_mission_lock(
        &mission.feature_dir,
        Some(&mission.repo_root),
        LockWait::Unbounded,
    )
    .expect("acquire mission lock")
}

fn snapshot(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

#[test]
fn a_verified_rollback_cuts_only_the_appended_rows_and_restores_status_json() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let status_path = mission.feature_dir.join(SNAPSHOT_FILENAME);
    fs::write(&status_path, b"{\"pre\": true}\n").unwrap();
    let prefix = snapshot(&events_path(&mission.feature_dir)).unwrap();

    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");
    assert_eq!(point.pre_event_size(), prefix.len() as u64);
    assert!(point.events_existed());

    append(
        &mission,
        &[
            ("01K9M0R3W0000000000000000B", "claimed"),
            ("01K9M0R3W0000000000000000C", "in_progress"),
        ],
    );
    let appended = appended_event_ids(&point)
        .expect("tail read")
        .expect("whole rows");
    assert_eq!(
        appended,
        vec![
            "01K9M0R3W0000000000000000B".to_owned(),
            "01K9M0R3W0000000000000000C".to_owned()
        ]
    );
    fs::write(&status_path, b"{\"post\": true}\n").unwrap();

    let outcome =
        rollback_status_artifacts(&lock, &point, Some(&appended), Some(&Probe::returning(&[])))
            .expect("rollback");
    assert!(outcome.rolled_back, "{}", outcome.message());
    assert_eq!(outcome.refusal, None);
    assert_eq!(
        snapshot(&events_path(&mission.feature_dir)).unwrap(),
        prefix,
        "the captured prefix is byte-identical"
    );
    assert_eq!(
        snapshot(&status_path).unwrap(),
        b"{\"pre\": true}\n",
        "status.json is restored from the capture"
    );
}

#[test]
fn a_foreign_tail_row_refuses_and_leaves_every_file_unchanged() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");
    append(&mission, &[("01K9M0R3W0000000000000000B", "claimed")]);
    // A different writer's row lands after ours (simulated: the protocol must
    // refuse even when the caller asserts only its own ids).
    append(&mission, &[("01K9M0R3W0000000000000000F", "blocked")]);
    let before = snapshot(&events_path(&mission.feature_dir)).unwrap();

    let outcome = rollback_events_log(
        &lock,
        &point,
        Some(&["01K9M0R3W0000000000000000B".to_owned()]),
        Some(&Probe::returning(&[])),
    )
    .expect("rollback");
    assert!(!outcome.rolled_back);
    assert_eq!(outcome.refusal, Some(RollbackRefusal::TailNotOwned));
    assert!(outcome.message().starts_with("STATUS_ROLLBACK_REFUSED: "));
    assert_eq!(
        snapshot(&events_path(&mission.feature_dir)).unwrap(),
        before
    );
}

#[test]
fn a_torn_tail_refuses_rather_than_partially_cutting() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");
    let log = events_path(&mission.feature_dir);
    let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
    std::io::Write::write_all(&mut file, b"{\"event_id\": \"torn\"").unwrap();
    drop(file);
    let before = snapshot(&log).unwrap();

    let outcome =
        rollback_events_log(&lock, &point, None, Some(&Probe::returning(&[]))).expect("rollback");
    assert_eq!(outcome.refusal, Some(RollbackRefusal::TailUnparseable));
    assert_eq!(snapshot(&log).unwrap(), before);
}

#[test]
fn a_tail_row_committed_at_head_refuses() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");
    append(&mission, &[("01K9M0R3W0000000000000000B", "claimed")]);
    let before = snapshot(&events_path(&mission.feature_dir)).unwrap();

    let outcome = rollback_events_log(
        &lock,
        &point,
        None,
        Some(&Probe::returning(&["01K9M0R3W0000000000000000B"])),
    )
    .expect("rollback");
    assert_eq!(outcome.refusal, Some(RollbackRefusal::TailAlreadyCommitted));
    assert!(!outcome.refusal.unwrap().may_have_mutated());
    assert_eq!(
        snapshot(&events_path(&mission.feature_dir)).unwrap(),
        before
    );
}

#[test]
fn an_absent_or_failing_committed_probe_refuses_a_non_empty_tail() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");
    append(&mission, &[("01K9M0R3W0000000000000000B", "claimed")]);

    let absent = rollback_events_log(&lock, &point, None, None).expect("rollback");
    assert_eq!(absent.refusal, Some(RollbackRefusal::HeadUnreadable));

    let failing =
        rollback_events_log(&lock, &point, None, Some(&Probe::failing())).expect("rollback");
    assert_eq!(failing.refusal, Some(RollbackRefusal::HeadUnreadable));
    assert!(read_text(&events_path(&mission.feature_dir)).contains("01K9M0R3W0000000000000000B"));
}

#[test]
fn a_shrunk_or_vanished_log_refuses() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let log = events_path(&mission.feature_dir);
    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");

    fs::write(&log, b"").unwrap();
    let shrunk =
        rollback_events_log(&lock, &point, None, Some(&Probe::returning(&[]))).expect("rollback");
    assert_eq!(shrunk.refusal, Some(RollbackRefusal::LogShrank));

    fs::remove_file(&log).unwrap();
    let vanished =
        rollback_events_log(&lock, &point, None, Some(&Probe::returning(&[]))).expect("rollback");
    assert_eq!(vanished.refusal, Some(RollbackRefusal::LogVanished));
}

#[test]
fn a_capture_requires_the_matching_mission_hold() {
    let first = temp_mission();
    let second_mission = temp_mission();
    let lock = lock(&first);
    capture_rollback_point(&lock, &first.feature_dir).expect("the held Mission is capturable");
    let error = capture_rollback_point(&lock, &second_mission.feature_dir)
        .expect_err("a different Mission's directory is not covered by this hold");
    assert!(matches!(error, MutationError::RollbackRequiresHold { .. }));
}

#[test]
fn a_reacquired_hold_still_refuses_a_foreign_row() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let point = {
        let lock = lock(&mission);
        capture_rollback_point(&lock, &mission.feature_dir).expect("capture")
    };
    append(&mission, &[("01K9M0R3W0000000000000000B", "claimed")]);
    append(&mission, &[("01K9M0R3W0000000000000000F", "blocked")]);

    let lock = lock(&mission);
    let outcome = rollback_events_log(
        &lock,
        &point,
        Some(&["01K9M0R3W0000000000000000B".to_owned()]),
        Some(&Probe::returning(&[])),
    )
    .expect("rollback");
    assert_eq!(outcome.refusal, Some(RollbackRefusal::TailNotOwned));
}

#[test]
fn an_absent_log_at_capture_is_removed_and_a_missing_status_json_stays_absent() {
    let mission = temp_mission();
    let log = events_path(&mission.feature_dir);
    let status_path = mission.feature_dir.join(SNAPSHOT_FILENAME);
    assert!(!log.exists() && !status_path.exists());

    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");
    assert!(!point.events_existed());
    append(&mission, &[("01K9M0R3W0000000000000000B", "planned")]);
    fs::write(&status_path, b"{}\n").unwrap();

    let outcome = rollback_status_artifacts(&lock, &point, None, Some(&Probe::returning(&[])))
        .expect("rollback");
    assert!(outcome.rolled_back, "{}", outcome.message());
    assert!(
        !log.exists(),
        "a log that did not exist at capture is removed"
    );
    assert!(
        !status_path.exists(),
        "status.json is removed, not invented"
    );
}

#[test]
fn appended_event_ids_reports_a_torn_tail_as_unknown() {
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");
    let log = events_path(&mission.feature_dir);
    let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
    std::io::Write::write_all(&mut file, b"{\"event_id\": \"X\"}").unwrap();
    drop(file);
    assert_eq!(appended_event_ids(&point).expect("read tail"), None);
}

#[test]
fn a_shrunken_log_wins_over_tail_ownership_and_appended_ids_is_unknown() {
    // N13: the baseline's `appended_event_ids` seeks past EOF and parses an
    // empty tail (``[]``); native answers "unknown" (``None``). No caller can
    // observe the difference: the rollback refuses LOG_SHRANK before it ever
    // compares the tail's ownership, so the sentinel cannot be read as "no
    // appended rows" by a future caller either.
    let mission = temp_mission();
    append(&mission, &[("01K9M0R3W0000000000000000A", "planned")]);
    let log = events_path(&mission.feature_dir);
    let lock = lock(&mission);
    let point = capture_rollback_point(&lock, &mission.feature_dir).expect("capture");

    fs::write(&log, b"").unwrap();
    assert_eq!(appended_event_ids(&point).expect("read tail"), None);

    let expected = vec!["01K9M0R3W0000000000000000B".to_owned()];
    let shrunk = rollback_events_log(&lock, &point, Some(&expected), Some(&Probe::returning(&[])))
        .expect("rollback");
    assert_eq!(shrunk.refusal, Some(RollbackRefusal::LogShrank));
}
