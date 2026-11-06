//! End-to-end lifecycle through the **actual** `spec-kitty` binary.
//!
//! The dispatch wiring is the parent's, so this test proves the surface a user
//! invokes: `spec-kitty accept`, `spec-kitty review`, `spec-kitty consolidate`
//! (Source default strategy) and the removed `spec-kitty merge`, each running
//! against a real temporary repository with its own isolated home.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "demo";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn run(root: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .args(args)
        .output()
        .expect("run spec-kitty")
}

fn json_of(output: &Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    serde_json::from_str(text.trim()).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON document ({error}):\n{text}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
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
        git(&root, &home, &["add", "README.md"]);
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
        write(&mission.join("spec.md"), "# Demo spec\n");
        write(&mission.join("tasks.md"), "- [x] T001 land the work\n");
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
        let mut approve = transition(
            "01J00000000000000000000005",
            "WP01",
            "in_review",
            "approved",
            "codex:gpt-5:reviewer",
            "2026-11-02T10:05:00Z",
        );
        approve["review_result"] = json!({
            "reviewer": "codex:gpt-5:reviewer",
            "verdict": "approved",
            "reference": "review-cycle-1:WP01:approved",
        });
        let events = [
            transition(
                "01J00000000000000000000001",
                "WP01",
                "planned",
                "claimed",
                "claude:opus:implementer",
                "2026-11-02T10:01:00Z",
            ),
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
            approve,
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

    fn meta(&self) -> Value {
        serde_json::from_str(
            &fs::read_to_string(self.root.join("kitty-specs").join(SLUG).join("meta.json"))
                .unwrap(),
        )
        .unwrap()
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

#[test]
fn the_binary_runs_the_whole_lifecycle_end_to_end() {
    let fixture = Fixture::new();

    // review before consolidation: lightweight, dead-code baseline missing.
    let output = run(
        &fixture.root,
        &fixture.home,
        &["review", "--mission", SLUG, "--json"],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "pre-consolidation review fails"
    );
    let payload = json_of(&output);
    assert_eq!(payload["mode"], "lightweight");
    assert_eq!(payload["verdict"], "fail");

    // accept: real gates + a committed acceptance matrix.
    let output = run(
        &fixture.root,
        &fixture.home,
        &["accept", "--mission", SLUG, "--json"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "accept failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = json_of(&output);
    assert_eq!(payload["ok"], json!(true));
    assert_eq!(payload["verdict"], "pass");
    assert!(
        fixture
            .root
            .join("kitty-specs")
            .join(SLUG)
            .join("acceptance-matrix.json")
            .is_file()
    );

    // consolidate with no --strategy: the Source default (squash).
    let before = fixture.head();
    let output = run(
        &fixture.root,
        &fixture.home,
        &["consolidate", "--mission", SLUG, "--json"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "consolidate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = json_of(&output);
    assert_eq!(payload["strategy"], "squash");
    let baseline = payload["baseline_commit"].as_str().expect("baseline");
    // Source's review baseline is the PRE-landing target. The report's landed
    // commit is recorded separately as merged_commit, never a zero-width window.
    assert_eq!(fixture.meta()["baseline_merge_commit"], json!(before));
    assert_eq!(fixture.meta()["merged_commit"], json!(baseline));
    assert!(fixture.root.join("src/a.txt").is_file());
    let parents = git(
        &fixture.root,
        &fixture.home,
        &["rev-list", "--parents", "-1", baseline],
    );
    assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
    assert_eq!(parents.split_whitespace().last().unwrap(), before);

    // post-merge review: auto-detected mode, gates green.
    let output = run(
        &fixture.root,
        &fixture.home,
        &["review", "--mission", SLUG, "--json"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "post-merge review failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = json_of(&output);
    assert_eq!(payload["mode"], "post-merge");
    assert_eq!(payload["gates_recorded"][0]["result"], "pass");
    assert_ne!(payload["verdict"], "fail");

    // `merge` is the Source rename stub: exit 1 with the exact message.
    let output = run(&fixture.root, &fixture.home, &["merge"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("has been renamed to `spec-kitty consolidate`"),
        "{stderr}"
    );

    // A second consolidate refuses; --resume verifies the landed state.
    let output = run(
        &fixture.root,
        &fixture.home,
        &["consolidate", "--mission", SLUG, "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    let output = run(
        &fixture.root,
        &fixture.home,
        &["consolidate", "--mission", SLUG, "--resume", "--json"],
    );
    assert_eq!(output.status.code(), Some(0));
    let payload = json_of(&output);
    assert_eq!(payload["already_consolidated"], json!(true));
}

#[test]
fn the_binary_refuses_an_unknown_strategy_and_keeps_the_target_untouched() {
    let fixture = Fixture::new();
    let before = fixture.head();
    let output = run(
        &fixture.root,
        &fixture.home,
        &[
            "consolidate",
            "--mission",
            SLUG,
            "--strategy",
            "octopus",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let payload = json_of(&output);
    assert_eq!(payload["error_code"], "MERGE_STRATEGY_INVALID");
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|text| text.contains("unknown merge strategy")),
        "{payload}"
    );
    assert_eq!(fixture.head(), before);
}
