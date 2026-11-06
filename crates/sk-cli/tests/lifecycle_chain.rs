//! End-to-end lifecycle chain through the CLI adapters, on a real repository.
//!
//! `create`/`tasks`/`implement` are already wired to the real binary; this test
//! drives the remaining vertical through the same adapters the dispatcher calls:
//! `accept` (gate evaluation + real `acceptance-matrix.json` commit), `review`
//! (pre- and post-consolidation), `consolidate` (real merge commits, recorded
//! baseline, teardown), and the `merge` rename refusal.
//!
//! Everything happens in an isolated temporary repository with its own home; no
//! probe touches this checkout, a remote, or the operator's state.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::lifecycle::{AcceptArgs, ConsolidateArgs, MergeArgs, ReviewArgs};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "demo";

/// `true` when the git command exits 0 (used for reachability probes).
fn git_ok(dir: &Path, home: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .expect("run git")
        .status
        .success()
}

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
        "git {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git_ref_exists(root: &Path, home: &Path, reference: &str) -> bool {
    Command::new("git")
        .current_dir(root)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["rev-parse", "--verify", reference])
        .output()
        .expect("run git")
        .status
        .success()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn transition(event_id: &str, wp_id: &str, from: &str, to: &str, actor: &str, at: &str) -> Value {
    json!({
        "actor": actor,
        "at": at,
        "event_id": event_id,
        "execution_mode": "worktree",
        "force": false,
        "from_lane": from,
        "mission_slug": SLUG,
        "to_lane": to,
        "wp_id": wp_id,
    })
}

fn approval(event_id: &str, wp_id: &str, reviewer: &str, at: &str) -> Value {
    let mut row = transition(event_id, wp_id, "in_review", "approved", reviewer, at);
    row["review_result"] = json!({
        "reviewer": reviewer,
        "verdict": "approved",
        "reference": format!("review-cycle-1:{wp_id}:approved"),
    });
    row["review_ref"] = json!(format!("review-cycle-1:{wp_id}"));
    row
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).expect("repo");
        fs::create_dir_all(&home).expect("home");
        git(&root, &home, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        write(&root.join(".gitignore"), ".worktrees/\n");
        git(&root, &home, &["add", "README.md", ".gitignore"]);
        git(&root, &home, &["commit", "-q", "-m", "initial"]);

        let mission = root.join("kitty-specs").join(SLUG);
        write(
            &mission.join("meta.json"),
            &serde_json::to_string_pretty(&json!({
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
            }))
            .unwrap(),
        );
        // The scaffolded acceptance matrix (`finalize-tasks` shape): one
        // pending `code_review` row the accept gate derives from the recorded
        // review verdict, and no negative invariants to execute.
        write(
            &mission.join("acceptance-matrix.json"),
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&json!({
                    "mission_slug": SLUG,
                    "mission_number": 7,
                    "mission_type": "software-dev",
                    "overall_verdict": "pending",
                    "criteria": [{
                        "criterion_id": "AC-001",
                        "description": "Every work package carries an approved review",
                        "proof_type": "code_review",
                        "evidence": null,
                        "pass_fail": "pending",
                        "verified_by": null,
                        "verified_at": null,
                        "notes": null
                    }],
                    "negative_invariants": []
                }))
                .unwrap()
            ),
        );
        write(&mission.join("spec.md"), "# Demo spec\n");
        write(&mission.join("plan.md"), "# Demo plan\n");
        write(&mission.join("tasks.md"), "- [x] T001 land the lane work\n");
        write(
            &mission.join("tasks/WP01-a.md"),
            "---\nwork_package_id: WP01\ntitle: A\nowned_files:\n  - src/a.txt\n---\n\n# WP01\n",
        );
        write(
            &mission.join("lanes.json"),
            &serde_json::to_string_pretty(&json!({
                "version": 1,
                "mission_slug": SLUG,
                "mission_id": MISSION_ID,
                "mission_branch": format!("kitty/mission-{SLUG}"),
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
            .unwrap(),
        );
        // The canonical event log: implement, review (independent actor), approve, done.
        let mut claim = transition(
            "01J00000000000000000000001",
            "WP01",
            "planned",
            "claimed",
            "claude:opus:implementer",
            "2026-11-02T10:01:00Z",
        );
        claim["policy_metadata"] = json!({ "agent": "claude:opus:implementer" });
        let events = [
            claim,
            transition(
                "01J00000000000000000000002",
                "WP01",
                "claimed",
                "in_progress",
                "claude:opus:implementer",
                "2026-11-02T10:02:00Z",
            ),
            transition(
                "01J00000000000000000000003",
                "WP01",
                "in_progress",
                "for_review",
                "claude:opus:implementer",
                "2026-11-02T10:03:00Z",
            ),
            transition(
                "01J00000000000000000000004",
                "WP01",
                "for_review",
                "in_review",
                "codex:gpt-5:reviewer",
                "2026-11-02T10:04:00Z",
            ),
            approval(
                "01J00000000000000000000005",
                "WP01",
                "codex:gpt-5:reviewer",
                "2026-11-02T10:05:00Z",
            ),
            transition(
                "01J00000000000000000000006",
                "WP01",
                "approved",
                "done",
                "codex:gpt-5:reviewer",
                "2026-11-02T10:06:00Z",
            ),
        ];
        let log: String = events
            .iter()
            .map(|row| serde_json::to_string(row).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        write(&mission.join("status.events.jsonl"), &format!("{log}\n"));
        git(&root, &home, &["add", "kitty-specs"]);
        git(&root, &home, &["commit", "-q", "-m", "mission planning"]);

        // The lane branch and its worktree hold the real implementation.
        let worktree = root.join(".worktrees").join(format!("{SLUG}-lane-a"));
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            &root,
            &home,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("kitty/mission-{SLUG}-lane-a"),
                worktree.to_str().unwrap(),
                "main",
            ],
        );
        write(&worktree.join("src/a.txt"), "implemented\n");
        git(&worktree, &home, &["add", "src/a.txt"]);
        git(&worktree, &home, &["commit", "-q", "-m", "WP01 work"]);

        Self {
            _temp: temp,
            root,
            home,
        }
    }

    fn head(&self) -> String {
        git(&self.root, &self.home, &["rev-parse", "HEAD"])
    }

    fn mission_file(&self, name: &str) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG).join(name)
    }

    fn meta(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.mission_file("meta.json")).unwrap()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["worktree", "prune"])
            .output();
    }
}

fn consolidate_args() -> ConsolidateArgs {
    ConsolidateArgs {
        strategy: None,
        delete_branch: false,
        keep_branch: false,
        remove_worktree: false,
        keep_worktree: false,
        target: None,
        origin_check: None,
        dry_run: false,
        json: false,
        mission: Some(SLUG.to_owned()),
        resume: false,
        abort: false,
        skip_lanes: false,
    }
}

fn review_args(mode: Option<&str>) -> ReviewArgs {
    ReviewArgs {
        mission: Some(SLUG.to_owned()),
        mode: mode.map(str::to_owned),
        json: false,
    }
}

fn acceptance_ok(payload: &Value) -> bool {
    payload["ok"].as_bool().unwrap_or(false)
}

#[test]
fn the_whole_accept_review_consolidate_chain_runs_on_real_state() {
    let fixture = Fixture::new();

    // 1. `review` before consolidation: lightweight (no baseline) and the
    //    Source hard failure for a modern mission without a recorded anchor.
    let (payload, code) =
        sk_cli::compat::lifecycle::review(&fixture.root, &review_args(None)).expect("review runs");
    assert_eq!(code, 1, "pre-consolidation review fails: {payload}");
    assert_eq!(payload["mode"], "lightweight");
    assert_eq!(payload["verdict"], "fail");
    assert!(fixture.mission_file("mission-review-report.md").is_file());
    let report = fs::read_to_string(fixture.mission_file("mission-review-report.md")).unwrap();
    assert!(
        report.starts_with("---\nverdict: fail\nmode: lightweight"),
        "{report}"
    );
    assert!(report.contains("dead_code_baseline_missing"), "{report}");

    // The review artifact is mission bookkeeping: land it before accepting, as
    // the workflow does (Source's dirty gate keeps an untracked one).
    git(
        &fixture.root,
        &fixture.home,
        &["add", "kitty-specs/demo/mission-review-report.md"],
    );
    git(
        &fixture.root,
        &fixture.home,
        &["commit", "-q", "-m", "record review"],
    );

    // 2. `accept`: real gates over the canonical snapshot + event-sourced review.
    let (payload, code) = sk_cli::compat::lifecycle::accept(
        &fixture.root,
        &AcceptArgs {
            mission: Some(SLUG.to_owned()),
            mode: None,
            merge_commit: None,
            target_branch: None,
            attest_first_landing_commit: false,
            origin_check: None,
            lenient: false,
            diagnose: false,
            no_commit: false,
            actor: Some("operator".to_owned()),
            // CF20: Source defaults for the new attestation flags.
            attest_no_implementation: Vec::new(),
            attest_reason: None,
            json: false,
        },
    )
    .expect("accept runs");
    assert_eq!(code, 0, "acceptance should pass: {payload}");
    assert!(acceptance_ok(&payload), "{payload}");
    // The Source summary carries the matrix verdict (the old Native report key).
    assert_eq!(payload["matrix_verdict"], "pass");
    let matrix: Value = serde_json::from_str(
        &fs::read_to_string(fixture.mission_file("acceptance-matrix.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(matrix["overall_verdict"], "pass");
    // The matrix is committed, so no tracked path is left modified. (Untracked
    // files — the lane worktree, the review report — do not block a merge.)
    let porcelain = git(
        &fixture.root,
        &fixture.home,
        &["status", "--porcelain", "--untracked-files=no"],
    );
    assert!(
        porcelain.is_empty(),
        "acceptance must leave no tracked modification, found: {porcelain}"
    );

    // 3. `consolidate` with the Source default (squash): ONE single-parent commit
    //    on the target, then a second run with `--strategy merge` is refused as
    //    already consolidated.
    let before = fixture.head();
    let (payload, code) =
        sk_cli::compat::lifecycle::consolidate(&fixture.root, &consolidate_args())
            .expect("consolidate runs with the Source default strategy");
    assert_eq!(code, 0, "{payload}");
    assert_eq!(payload["strategy"], "squash", "{payload}");
    assert!(fixture.root.join("src/a.txt").is_file());
    let meta = fixture.meta();
    let baseline = payload["baseline_commit"]
        .as_str()
        .expect("baseline")
        .to_owned();
    // `baseline_merge_commit` is the PRE-landing target tip; `merged_commit` is the
    // landing commit the payload reports as `baseline_commit`.
    assert_eq!(meta["baseline_merge_commit"], json!(before));
    assert_eq!(meta["merged_commit"], json!(baseline));
    assert_eq!(meta["mission_id"], json!(MISSION_ID));
    // The squash landing is a single-parent commit whose parent is the pre-run tip.
    let parents = git(
        &fixture.root,
        &fixture.home,
        &["rev-list", "--parents", "-1", &baseline],
    );
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "a squash landing has exactly one parent: {parents}"
    );
    assert_eq!(parents.split_whitespace().last().unwrap(), before);
    // The landing commit sits under the bookkeeping commits: it is an ancestor of
    // the tip, and the tip is not its ancestor.
    assert!(
        git_ok(
            &fixture.root,
            &fixture.home,
            &["merge-base", "--is-ancestor", &baseline, &fixture.head()]
        ),
        "the landing commit is on the target history"
    );
    assert!(
        !git_ok(
            &fixture.root,
            &fixture.home,
            &["merge-base", "--is-ancestor", &fixture.head(), &baseline]
        ),
        "the bookkeeping commits follow the landing"
    );
    assert!(
        payload["teardown"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["action"] == "branch_deleted")
    );
    assert!(
        !git_ref_exists(
            &fixture.root,
            &fixture.home,
            "refs/heads/kitty/mission-demo-lane-a"
        ),
        "the landed lane branch must be deleted"
    );
    assert!(
        !git_ref_exists(
            &fixture.root,
            &fixture.home,
            "refs/heads/kitty/mission-demo"
        ),
        "the landed integration branch must be deleted"
    );

    // 4. `review` after consolidation: auto-detected post-merge mode, gate 1
    //    green, dead-code scan applicable-or-not on the real change set.
    let (payload, code) = sk_cli::compat::lifecycle::review(&fixture.root, &review_args(None))
        .expect("post-merge review runs");
    assert_eq!(payload["mode"], "post-merge");
    assert_ne!(payload["verdict"], "fail", "{payload}");
    assert_eq!(code, 0, "{payload}");
    let gates = payload["gates_recorded"].as_array().expect("gates");
    assert_eq!(gates[0]["result"], "pass", "gate 1: {payload}");
    assert!(
        matches!(gates[1]["result"].as_str(), Some("pass" | "skip")),
        "gate 2: {payload}"
    );

    // 5. `merge` is the Source rename stub.
    let error = sk_cli::compat::lifecycle::merge(&MergeArgs {
        json: false,
        args: vec!["--resume".to_owned()],
    })
    .expect_err("merge is removed");
    assert_eq!(
        error.message(),
        "The `merge` command has been renamed to `spec-kitty consolidate`. Re-run with `spec-kitty consolidate` (same flags)."
    );

    // 6. A second consolidate is refused, and `--resume` verifies.
    let error = sk_cli::compat::lifecycle::consolidate(&fixture.root, &consolidate_args())
        .expect_err("already consolidated");
    assert!(
        error
            .message()
            .contains("already records baseline_merge_commit"),
        "{}",
        error.message()
    );
    let (payload, _) = sk_cli::compat::lifecycle::consolidate(
        &fixture.root,
        &ConsolidateArgs {
            resume: true,
            ..consolidate_args()
        },
    )
    .expect("resume verifies");
    assert_eq!(payload["already_consolidated"], json!(true));
    assert_eq!(
        fixture.head(),
        git(&fixture.root, &fixture.home, &["rev-parse", "HEAD"])
    );
}
