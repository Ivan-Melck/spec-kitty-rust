//! CF17 Workspace smoke: the ONE canonical decision fold and the real
//! PRIMARY-vs-COORDINATION fork diagnosis, against real Git fixtures.
//!
//! These are small, meaningful functional checks — not a differential campaign.
//! They exercise: the folded full entry (all fields, not just status), the
//! reopen rule, stream fork/prefix/single-home classification on
//! `status.events.jsonl` AND `decisions.events.jsonl`, the ledger-home verdict
//! (PRIMARY / coordination-only / both), the worktree-or-ref read (a
//! coordination surface with no worktree), Source's never-auto-merge reconcile
//! text, and read-only behaviour (no ref, file or worktree mutation).
//!
//! Deliberately deferred to a later campaign: Source line-by-line diffs of this
//! report, huge logs, concurrent writers, remote-only refs, non-UTF-8 surfaces,
//! Windows/macOS path spellings, materialized-worktree parity and performance.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_workspace::decision_fork::{
    DECISION_LOG_STREAM, LedgerState, STATUS_STREAM, StreamState, detect_decision_forks,
    ledger_is_coordination_only,
};
use sk_workspace::{MissionWorkspace, ProjectWorkspace};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "001-x";
const MID8: &str = "01KN2371";
const COORD_BRANCH: &str = "kitty/mission-001-x";

struct Fixture {
    root: tempfile::TempDir,
    home: tempfile::TempDir,
}

fn git(root: &Path, home: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "CF17 Fixture")
        .env("GIT_AUTHOR_EMAIL", "cf17@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF17 Fixture")
        .env("GIT_COMMITTER_EMAIL", "cf17@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("home");
        git(
            root.path(),
            home.path(),
            &["init", "--quiet", "--initial-branch=main"],
        );
        write(&root.path().join("README.md"), "fixture\n");
        git(root.path(), home.path(), &["add", "README.md"]);
        git(
            root.path(),
            home.path(),
            &["commit", "--quiet", "-m", "fixture"],
        );
        Self { root, home }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn mission_root(&self) -> PathBuf {
        self.path().join("kitty-specs").join(SLUG)
    }

    fn meta(&self, topology: &str) {
        write(
            &self.mission_root().join("meta.json"),
            &format!(
                "{{\"mission_id\":\"{MISSION_ID}\",\"mission_slug\":\"{SLUG}\",\"mid8\":\"{MID8}\",\
                 \"topology\":\"{topology}\",\"target_branch\":\"main\",\
                 \"coordination_branch\":\"{COORD_BRANCH}\"}}"
            ),
        );
    }

    fn commit(&self, message: &str) {
        git(self.path(), self.home.path(), &["add", "-A"]);
        git(
            self.path(),
            self.home.path(),
            &["commit", "--quiet", "-m", message],
        );
    }

    fn open(&self) -> MissionWorkspace {
        ProjectWorkspace::open(self.path())
            .expect("workspace")
            .mission(SLUG)
            .expect("mission")
    }

    /// `git for-each-ref` plus the bytes of every mission file: the read-only
    /// control's before/after snapshot.
    fn snapshot(&self) -> (String, BTreeMap<PathBuf, Vec<u8>>) {
        let refs = git(
            self.path(),
            self.home.path(),
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        let mut files = BTreeMap::new();
        fn walk(base: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let Ok(metadata) = fs::symlink_metadata(path) else {
                return;
            };
            if metadata.is_dir() {
                for entry in fs::read_dir(path).expect("read_dir") {
                    walk(base, &entry.expect("entry").path(), files);
                }
            } else if metadata.is_file() {
                files.insert(
                    path.strip_prefix(base).expect("inside base").to_path_buf(),
                    fs::read(path).expect("read"),
                );
            }
        }
        walk(self.path(), self.path(), &mut files);
        (String::from_utf8_lossy(&refs.stdout).into_owned(), files)
    }
}

fn opened(event_id: &str, decision_id: &str) -> String {
    format!(
        r#"{{"event_id":"{event_id}","event_type":"DecisionPointOpened","payload":{{"decision_point_id":"{decision_id}","origin_flow":"specify","step_id":"s","input_key":"k","question":"q","options":[],"state_entered_at":"2026-01-01T00:00:00+00:00","actor_id":"claude","mission_id":"{MISSION_ID}","mission_slug":"{SLUG}"}}}}"#
    )
}

fn resolved(event_id: &str, decision_id: &str) -> String {
    format!(
        r#"{{"event_id":"{event_id}","event_type":"DecisionPointResolved","payload":{{"decision_point_id":"{decision_id}","terminal_outcome":"resolved","state_entered_at":"2026-01-02T00:00:00+00:00","resolved_by":"operator","final_answer":"yes"}}}}"#
    )
}

fn decision_log_row(event_id: &str, decision_id: &str) -> String {
    format!(
        r#"{{"event_id":"{event_id}","event_type":"DecisionInputRequested","payload":{{"decision_id":"{decision_id}"}}}}"#
    )
}

fn index_doc(entry_ids: &[&str]) -> String {
    let entries: Vec<String> = entry_ids
        .iter()
        .map(|id| {
            format!(
                r#"{{"decision_id":"{id}","origin_flow":"specify","step_id":"s","slot_key":null,"input_key":"k","question":"q","options":[],"status":"open","final_answer":null,"rationale":null,"other_answer":false,"summary_json":null,"created_at":"2026-01-01T00:00:00Z","resolved_at":null,"resolved_by":null,"opened_by":"claude","mission_id":"{MISSION_ID}","mission_slug":"{SLUG}"}}"#
            )
        })
        .collect();
    format!(
        "{{\"version\":1,\"mission_id\":\"{MISSION_ID}\",\"entries\":[{}]}}\n",
        entries.join(",")
    )
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn rendered(values: &[sk_kernel::PythonString]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.to_scalar_string().expect("scalar id"))
        .collect()
}

#[test]
fn a_lanes_mission_reports_single_home_and_the_primary_ledger() {
    let fixture = Fixture::new();
    fixture.meta("lanes");
    write(
        &fixture.mission_root().join(STATUS_STREAM),
        &format!("{}\n", opened("e1", "d1")),
    );
    write(
        &fixture.mission_root().join("decisions/index.json"),
        &index_doc(&["d1"]),
    );
    fixture.commit("mission");

    let mission = fixture.open();
    let report = detect_decision_forks(&mission).expect("fork report");

    assert_eq!(report.mission_slug, SLUG);
    assert_eq!(report.topology.as_deref(), Some("lanes"));
    assert!(!report.forked);
    assert_eq!(report.streams.len(), 2);
    assert_eq!(report.streams[0].stream, STATUS_STREAM);
    assert_eq!(report.streams[0].state, StreamState::SingleHome);
    assert!(report.streams[0].coordination.is_none());
    assert_eq!(report.streams[1].stream, DECISION_LOG_STREAM);
    assert_eq!(report.streams[1].state, StreamState::Absent);
    assert_eq!(report.ledger.state, LedgerState::Primary);
    assert!(!ledger_is_coordination_only(&report.ledger));
    assert!(report.reconcile_steps.is_empty(), "{report:?}");
    // No coordination surface means no derived worktree directory is created.
    assert!(!fixture.path().join(".worktrees").exists());
}

#[test]
fn a_coordination_mission_with_no_worktree_is_read_from_the_ref_and_forked() {
    let fixture = Fixture::new();
    fixture.meta("coord");
    write(
        &fixture.mission_root().join(STATUS_STREAM),
        &format!("{}\n", opened("e1", "d1")),
    );
    write(
        &fixture.mission_root().join("decisions/index.json"),
        &index_doc(&["d1"]),
    );
    fixture.commit("primary base");

    let mission = fixture.open();
    let coord_subdir = mission.coord_mission_dir_name();
    let coord_mission_dir = fixture.path().join("kitty-specs").join(&coord_subdir);

    // The coordination branch from the same base, carrying a diverging stream
    // and the only ledger content (Source's pre-fix shape).
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "-b", COORD_BRANCH],
    );
    write(
        &coord_mission_dir.join(STATUS_STREAM),
        &format!("{}\n{}\n", opened("e1", "d1"), resolved("e2", "d2")),
    );
    write(
        &coord_mission_dir.join("decisions/index.json"),
        &index_doc(&["d1", "d2"]),
    );
    git(fixture.path(), fixture.home.path(), &["add", "-A"]);
    git(
        fixture.path(),
        fixture.home.path(),
        &["commit", "--quiet", "-m", "coord"],
    );
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "main"],
    );

    // PRIMARY diverges on both streams.
    write(
        &fixture.mission_root().join(STATUS_STREAM),
        &format!("{}\n{}\n", opened("e1", "d1"), resolved("e3", "d3")),
    );
    write(
        &fixture.mission_root().join(DECISION_LOG_STREAM),
        &format!("{}\n", decision_log_row("x1", "d9")),
    );
    fixture.commit("primary fork");

    let before = fixture.snapshot();
    let report = detect_decision_forks(&mission).expect("fork report");
    let after = fixture.snapshot();
    assert_eq!(before, after, "the fork detector must be read-only");
    assert!(
        !fixture.path().join(".worktrees").exists(),
        "no worktree may be materialized by a read"
    );

    assert!(report.forked);
    let status = &report.streams[0];
    assert_eq!(status.stream, STATUS_STREAM);
    assert_eq!(status.state, StreamState::Forked);
    let primary = status.primary.as_ref().expect("primary log");
    assert_eq!(primary.source.as_str(), "worktree");
    assert_eq!(primary.event_ids.len(), 2);
    let coordination = status.coordination.as_ref().expect("coord log");
    assert_eq!(
        coordination.source.as_str(),
        "ref",
        "no worktree exists, so the coordination side must be read from the ref"
    );
    assert_eq!(coordination.reference.as_deref(), Some(COORD_BRANCH));
    assert_eq!(coordination.event_ids.len(), 2);
    assert_eq!(rendered(&status.decisions_only_on_primary), ids(&["d3"]));
    assert_eq!(
        rendered(&status.decisions_only_on_coordination),
        ids(&["d2"])
    );
    assert_eq!(rendered(&status.decisions_on_both), ids(&["d1"]));

    // The decisions stream exists only on PRIMARY: single home, not a fork.
    assert_eq!(report.streams[1].state, StreamState::SingleHome);
    assert_eq!(
        report.streams[1]
            .coordination
            .as_ref()
            .map(|log| log.event_ids.len()),
        Some(0)
    );

    // The ledger is on BOTH surfaces with coordination-only residue.
    assert_eq!(report.ledger.state, LedgerState::Both);
    assert_eq!(
        rendered(&report.ledger.entries_only_on_coordination),
        ids(&["d2"])
    );
    assert!(ledger_is_coordination_only(&report.ledger));

    // Never auto-merged (C-003), and the ledger remedy is Source's.
    let steps = report.reconcile_steps.join("\n");
    assert!(
        steps.contains("is forked: decisions only on PRIMARY = [d3]"),
        "{steps}"
    );
    assert!(steps.contains("never auto-merged (C-003)"), "{steps}");
    assert!(steps.contains("--mission 001-x --repair"), "{steps}");
}

#[test]
fn a_coordination_stream_that_is_a_prefix_is_not_a_fork() {
    let fixture = Fixture::new();
    fixture.meta("coord");
    write(
        &fixture.mission_root().join(STATUS_STREAM),
        &format!("{}\n{}\n", opened("e1", "d1"), resolved("e2", "d2")),
    );
    fixture.commit("primary");

    let mission = fixture.open();
    let coord_mission_dir = fixture
        .path()
        .join("kitty-specs")
        .join(mission.coord_mission_dir_name());
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "-b", COORD_BRANCH],
    );
    write(
        &coord_mission_dir.join(STATUS_STREAM),
        &format!("{}\n", opened("e1", "d1")),
    );
    git(fixture.path(), fixture.home.path(), &["add", "-A"]);
    git(
        fixture.path(),
        fixture.home.path(),
        &["commit", "--quiet", "-m", "coord prefix"],
    );
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "main"],
    );

    let report = detect_decision_forks(&mission).expect("fork report");
    assert_eq!(report.streams[0].state, StreamState::Prefix);
    assert!(!report.forked);
    assert_eq!(report.ledger.state, LedgerState::Absent);
    assert!(!ledger_is_coordination_only(&report.ledger));
    assert!(report.reconcile_steps.is_empty(), "{report:?}");
}

#[test]
fn a_coordination_only_ledger_is_reported_as_needing_the_repair_copy() {
    let fixture = Fixture::new();
    fixture.meta("coord");
    fixture.commit("primary without a ledger");

    let mission = fixture.open();
    let coord_mission_dir = fixture
        .path()
        .join("kitty-specs")
        .join(mission.coord_mission_dir_name());
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "-b", COORD_BRANCH],
    );
    write(
        &coord_mission_dir.join("decisions/index.json"),
        &index_doc(&["d1"]),
    );
    write(
        &coord_mission_dir.join("decisions/DM-d1.md"),
        "# Decision Moment `d1`\n",
    );
    git(fixture.path(), fixture.home.path(), &["add", "-A"]);
    git(
        fixture.path(),
        fixture.home.path(),
        &["commit", "--quiet", "-m", "coord ledger"],
    );
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "main"],
    );

    let report = detect_decision_forks(&mission).expect("fork report");
    assert_eq!(report.ledger.state, LedgerState::CoordinationOnly);
    assert!(ledger_is_coordination_only(&report.ledger));
    assert_eq!(
        rendered(&report.ledger.entries_only_on_coordination),
        ids(&["d1"])
    );
    assert_eq!(
        rendered(&report.ledger.dm_files_only_on_coordination),
        ids(&["DM-d1.md"])
    );
    assert!(!report.forked, "an empty-stream Mission is not forked");
    assert_eq!(report.streams[0].state, StreamState::Absent);
    let steps = report.reconcile_steps.join("\n");
    assert!(steps.contains("only on the coordination branch"), "{steps}");
    assert!(steps.contains("kitty-specs/001-x/decisions/"), "{steps}");
}
