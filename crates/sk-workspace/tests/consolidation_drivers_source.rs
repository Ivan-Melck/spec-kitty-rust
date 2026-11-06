//! Native-only consolidation control for the registered bookkeeping drivers.
//!
//! ## Evidence-history correction (forward-dated, never backdated)
//!
//! This file was originally titled and documented as "Source-first proof" of
//! Source parity for the squash landing. **That claim was false and is corrected
//! here, not erased.** The file contains NO Source execution at all: every case
//! builds a Native fixture and calls the Native consolidation engine. It is a
//! Native behaviour control — useful, but it cannot establish Source parity.
//!
//! The original bytes and mtime are preserved at
//! `/tmp/sk-evidence-contH2/preserved/consolidation_drivers_source.NATIVE_ONLY_CONTROL.original.rs`
//! (sha256 `d2142ad1e046c1d23d15e912c9bfd0acdcc477690932b064c0b67a7ebc0762e9`,
//! mtime 2026-11-04 19:00:18Z). The file keeps its name so that nothing is
//! silently renamed away from the earlier report's reference.
//!
//! The real paired Source-then-Native evidence lives in
//! `crates/sk-cli/tests/merge_driver_binary_source.rs` (seven driver subprocesses
//! through the pinned Source CLI first, then the actual Native binary) and in
//! `crates/sk-workspace/tests/consolidation_protocol_source.rs` (complete
//! squash/merge/rebase landings with recovery, Source first).
//!
//! What follows therefore binds NATIVE behaviour only: the squash landing runs the
//! real `git merge --squash` protocol with the registered bookkeeping drivers, and
//! the drivers reconcile `meta.json` / `status.events.jsonl` the way the pinned
//! Source does.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_workspace::ProjectWorkspace;
use sk_workspace::consolidation::{ConsolidationRequest, MergeStrategy, consolidate_mission};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "demo";
const MISSION_BRANCH: &str = "kitty/mission-demo";

fn git(dir: &Path, home: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn meta(extra: Value) -> String {
    let mut value = json!({
        "created_at": "2026-11-02T10:00:00Z",
        "flattened": false,
        "friendly_name": "Demo",
        "mission_id": MISSION_ID,
        "mission_number": 7,
        "mission_slug": SLUG,
        "mission_type": "software-dev",
        "slug": SLUG,
        "target_branch": "main",
        "topology": "lanes",
    });
    if let Value::Object(extra) = extra {
        for (key, entry) in extra {
            value_set(&mut value, &key, entry);
        }
    }
    serde_json::to_string_pretty(&value).expect("meta json")
}

fn value_set(target: &mut Value, key: &str, value: Value) {
    if let Value::Object(map) = target {
        map.insert(key.to_owned(), value);
    }
}

fn event(event_id: &str, at: &str, from: &str, to: &str) -> String {
    // A lane-transition row the CANONICAL reducer accepts: `force` is a boolean
    // and `execution_mode` is present. The reader is strict on purpose, so the
    // control fixture carries every required field rather than a shorthand.
    serde_json::to_string(&json!({
        "event_id": event_id,
        "at": at,
        "wp_id": "WP01",
        "from_lane": from,
        "to_lane": to,
        "actor": "claude",
        "force": false,
        "execution_mode": "worktree",
    }))
    .expect("event json")
}

fn lanes_json() -> String {
    serde_json::to_string_pretty(&json!({
        "version": 1,
        "mission_slug": SLUG,
        "mission_id": MISSION_ID,
        "mission_branch": MISSION_BRANCH,
        "target_branch": "main",
        "lanes": [{
            "lane_id": "lane-a",
            "wp_ids": ["WP01"],
            "write_scope": ["src/a.txt"],
            "predicted_surfaces": [],
            "depends_on_lanes": [],
            "parallel_group": 0
        }],
        "computed_at": "2026-11-02T10:00:00Z",
        "computed_from": "test",
        "planning_artifact_wps": [],
        "planning_commit_sha": null
    }))
    .expect("lanes json")
}

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    mission_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).expect("repo dir");
        fs::create_dir_all(&home).expect("home dir");
        git(&root, &home, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        git(&root, &home, &["add", "README.md"]);
        git(&root, &home, &["commit", "-q", "-m", "initial"]);

        let mission_dir = root.join("kitty-specs").join(SLUG);
        write(
            &mission_dir.join("meta.json"),
            &meta(json!({"planning": "base"})),
        );
        write(
            &mission_dir.join("status.events.jsonl"),
            &format!("{}\n", event("e1", "1", "planned", "in_progress")),
        );
        write(&mission_dir.join("lanes.json"), &lanes_json());
        git(&root, &home, &["add", "kitty-specs"]);
        git(&root, &home, &["commit", "-q", "-m", "mission base"]);

        // The mission branch carries a divergent meta.json and a second event.
        git(&root, &home, &["checkout", "-q", "-b", MISSION_BRANCH]);
        write(
            &mission_dir.join("meta.json"),
            &meta(json!({
                "planning": "mission",
                "acceptance_history": [{"accepted_at": "1", "accepted_by": "mission"}],
            })),
        );
        write(
            &mission_dir.join("status.events.jsonl"),
            &format!(
                "{}\n{}\n",
                event("e1", "1", "planned", "in_progress"),
                event("e2", "2", "in_progress", "for_review")
            ),
        );
        write(&root.join("src/a.txt"), "lane a\n");
        git(&root, &home, &["add", "kitty-specs", "src/a.txt"]);
        git(&root, &home, &["commit", "-q", "-m", "lane a work"]);

        // The lane branch the engine consolidates into the mission branch.
        git(
            &root,
            &home,
            &["branch", "kitty/mission-demo-lane-a", "HEAD"],
        );

        // The target diverges on the same bookkeeping files.
        git(&root, &home, &["checkout", "-q", "main"]);
        write(
            &mission_dir.join("meta.json"),
            &meta(json!({
                "planning": "base",
                "accepted_at": "2026-11-02T12:00:00Z",
                "accepted_by": "reviewer",
                "status": "merged",
                "merged_at": "2026-11-02T12:05:00Z",
                "acceptance_history": [{"accepted_at": "2", "accepted_by": "target"}],
            })),
        );
        write(
            &mission_dir.join("status.events.jsonl"),
            &format!(
                "{}\n{}\n",
                event("e1", "1", "planned", "in_progress"),
                event("e3", "3", "for_review", "approved")
            ),
        );
        git(&root, &home, &["add", "kitty-specs"]);
        git(
            &root,
            &home,
            &["commit", "-q", "-m", "target accept bookkeeping"],
        );

        Self {
            temp,
            root,
            home,
            mission_dir,
        }
    }

    fn workspace(&self) -> sk_workspace::MissionWorkspace {
        let project = ProjectWorkspace::open(&self.root).expect("open project");
        project.mission(SLUG).expect("resolve mission")
    }

    fn done_lanes(&self) -> Vec<(String, String)> {
        vec![("WP01".to_owned(), "done".to_owned())]
    }

    fn read_landed(&self, path: &str) -> String {
        git(&self.root, &self.home, &["show", &format!("main:{path}")])
    }
}

#[test]
fn the_squash_landing_runs_the_real_merge_and_the_meta_driver_reconciles_two_way() {
    let fixture = Fixture::new();
    let report = consolidate_mission(
        &fixture.workspace(),
        &fixture.done_lanes(),
        &ConsolidationRequest {
            strategy: Some(MergeStrategy::Squash),
            ..ConsolidationRequest::default()
        },
    )
    .expect("squash landing succeeds");
    assert_eq!(report.strategy, "squash");

    // The LANDING commit is a squash commit: `report.baseline_commit` is the
    // commit this run landed (the bookkeeping commits come after it), so read its
    // own object rather than the current tip.
    let landed = report.baseline_commit.to_string();
    let parents = git(
        &fixture.root,
        &fixture.home,
        &["rev-list", "--parents", "-n", "1", &landed],
    );
    assert_eq!(
        parents.split(' ').count(),
        2,
        "a squash commit has exactly one parent: {parents}"
    );
    let parent = parents.split(' ').nth(1).expect("parent id").to_owned();
    assert_eq!(
        Some(parent),
        report.pre_target_tip.as_ref().map(ToString::to_string),
        "the squash commit's parent is the target tip observed before the run"
    );

    // The landed meta.json is the driver's two-way reconciliation: the target's
    // accepted-newer provenance survives, the mission's planning intent survives,
    // and acceptance_history is the deduplicated union.
    let landed_meta: Value =
        serde_json::from_str(&fixture.read_landed("kitty-specs/demo/meta.json"))
            .expect("landed meta is JSON");
    assert_eq!(landed_meta["accepted_at"], json!("2026-11-02T12:00:00Z"));
    assert_eq!(landed_meta["accepted_by"], json!("reviewer"));
    assert_eq!(landed_meta["status"], json!("merged"));
    // `merged_at` is the landing RUN's clock (the consolidation metadata
    // projection writes it), so it is bound to a real instant rather than to the
    // fixture's pre-landing placeholder.
    let merged_at = landed_meta["merged_at"]
        .as_str()
        .expect("merged_at is a string");
    sk_kernel::clock::parse_iso(&merged_at.replace('Z', "+00:00"))
        .unwrap_or_else(|error| panic!("merged_at {merged_at:?} is ISO-8601 ({error})"));
    assert_eq!(
        landed_meta["merged_commit"],
        json!(report.baseline_commit.to_string())
    );
    assert_eq!(landed_meta["planning"], json!("mission"));
    assert_eq!(landed_meta["mission_number"], json!(7));
    let history = landed_meta["acceptance_history"]
        .as_array()
        .expect("history is an array");
    assert_eq!(
        history.len(),
        2,
        "both acceptance records survive: {history:?}"
    );
    assert_eq!(history[0]["accepted_by"], json!("mission"));
    assert_eq!(history[1]["accepted_by"], json!("target"));

    // The landed status log is the event-log union, sorted by `at`.
    let landed_log = fixture.read_landed("kitty-specs/demo/status.events.jsonl");
    let rows: Vec<Value> = landed_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event row"))
        .collect();
    let ids: Vec<Value> = rows.iter().map(|row| row["event_id"].clone()).collect();
    assert_eq!(
        ids[..3].to_vec(),
        vec![json!("e1"), json!("e2"), json!("e3")],
        "{landed_log}"
    );
    // The bookkeeping projection then records the real `approved -> done`
    // transition for the merged work package (one per merged WP).
    let done: Vec<&Value> = rows
        .iter()
        .filter(|row| row["to_lane"] == json!("done"))
        .collect();
    assert_eq!(done.len(), 1, "{landed_log}");
    assert_eq!(done[0]["wp_id"], json!("WP01"));
    assert_eq!(done[0]["from_lane"], json!("approved"));
    assert_eq!(done[0]["actor"]["tool"], json!("merge"));

    // The ephemeral `.git/info/attributes` activation is torn down; the driver
    // definitions persist (they are inert without a mapping).
    let attributes =
        fs::read_to_string(fixture.root.join(".git/info/attributes")).unwrap_or_default();
    assert!(
        !attributes.contains("merge=spec-kitty-"),
        "the activation must not outlive the landing: {attributes}"
    );
    let driver = git(
        &fixture.root,
        &fixture.home,
        &["config", "--local", "merge.spec-kitty-meta.driver"],
    );
    assert_eq!(driver, "spec-kitty merge-driver-meta %O %A %B");
    let name = git(
        &fixture.root,
        &fixture.home,
        &["config", "--local", "merge.spec-kitty-meta.name"],
    );
    assert_eq!(name, "Spec Kitty mission meta field merge");

    // The operator checkout is the target and is clean of tracked changes.
    assert_eq!(
        git(
            &fixture.root,
            &fixture.home,
            &["rev-parse", "--abbrev-ref", "HEAD"],
        ),
        "main"
    );
    let status = git(&fixture.root, &fixture.home, &["status", "--porcelain"]);
    assert!(
        status
            .lines()
            .all(|line| !line.starts_with(" M") && !line.starts_with("M ")),
        "the landing leaves no tracked modification: {status}"
    );

    // The mission directory is committed on the target.
    assert!(fixture.mission_dir.exists());
    assert!(fixture.temp.path().exists());
}

#[test]
fn a_concurrent_cancel_fork_makes_the_event_log_driver_refuse_and_the_target_stand() {
    let fixture = Fixture::new();
    // The lane branch independently cancels WP01 while the target advanced it to
    // `approved`: a union fold would bury one of them (#5967), so the driver
    // must refuse and the squash must not land.
    git(
        &fixture.root,
        &fixture.home,
        &["checkout", "-q", "kitty/mission-demo-lane-a"],
    );
    write(
        &fixture.mission_dir.join("status.events.jsonl"),
        &format!(
            "{}\n{}\n{}\n",
            event("e1", "1", "planned", "in_progress"),
            event("e2", "2", "in_progress", "for_review"),
            event("e4", "4", "for_review", "canceled")
        ),
    );
    git(
        &fixture.root,
        &fixture.home,
        &["commit", "-q", "-am", "lane cancels WP01"],
    );
    git(&fixture.root, &fixture.home, &["checkout", "-q", "main"]);
    let before = git(&fixture.root, &fixture.home, &["rev-parse", "main"]);
    let error = consolidate_mission(
        &fixture.workspace(),
        &fixture.done_lanes(),
        &ConsolidationRequest {
            strategy: Some(MergeStrategy::Squash),
            ..ConsolidationRequest::default()
        },
    )
    .expect_err("a buried cancel must fail the squash");
    let rendered = error.to_string();
    assert!(
        rendered.contains("CONSOLIDATION_MERGE_DRIVER_FAILED")
            || rendered.contains("concurrent transitions"),
        "{rendered}"
    );
    assert_eq!(
        git(&fixture.root, &fixture.home, &["rev-parse", "main"]),
        before,
        "the target keeps its exact pre-run tip"
    );
    // The activation is torn down even on the refusal path.
    let attributes =
        fs::read_to_string(fixture.root.join(".git/info/attributes")).unwrap_or_default();
    assert!(!attributes.contains("merge=spec-kitty-"), "{attributes}");
}
