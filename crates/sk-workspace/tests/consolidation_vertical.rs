//! Real Git behavior for the consolidation vertical.
//!
//! Every test builds an isolated temporary repository (never this checkout) with
//! a real mission directory, a real `lanes.json`, real lane branches and real
//! lane worktrees, then runs the consolidation engine and asserts the actual
//! repository state: refs, commit ancestry, the recorded baseline, the status of
//! the lane refs and worktrees, and the coordination journal.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_workspace::consolidation::{
    ConsolidationRequest, MergeStrategy, consolidate_mission, resolve_effective_strategy,
};
use sk_workspace::{LanesStore, ProjectWorkspace};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "demo";

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

fn git_status(dir: &Path, home: &Path, args: &[&str]) -> bool {
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

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn meta_json(topology: &str, baseline: Option<&str>, retain: bool) -> String {
    let mut meta = json!({
        "created_at": "2026-11-02T10:00:00Z",
        "flattened": false,
        "friendly_name": "Demo",
        "mission_id": MISSION_ID,
        "mission_number": 7,
        "mission_slug": SLUG,
        "mission_type": "software-dev",
        "slug": SLUG,
        "target_branch": "main",
        "topology": topology,
        "retain_branches": retain,
        "retain_worktrees": retain,
    });
    if let Some(baseline) = baseline {
        meta["baseline_merge_commit"] = json!(baseline);
    }
    serde_json::to_string_pretty(&meta).expect("meta json")
}

/// A canonical `planned -> in_progress -> for_review -> approved` history for
/// both work packages, in one append-only log.
fn status_log() -> String {
    let mut rows = String::new();
    let mut index = 0u32;
    for wp in ["WP01", "WP02"] {
        for (from, to) in [
            ("planned", "in_progress"),
            ("in_progress", "for_review"),
            ("for_review", "approved"),
        ] {
            index += 1;
            rows.push_str(
                &serde_json::to_string(&json!({
                    "event_id": format!("01KN2371WNZX28ME4MN97B67P{index:02}"),
                    "at": format!("2026-11-02T10:0{index}:00+00:00"),
                    "wp_id": wp,
                    "from_lane": from,
                    "to_lane": to,
                    "actor": "claude",
                    "force": false,
                    "execution_mode": "worktree",
                }))
                .expect("event json"),
            );
            rows.push('\n');
        }
    }
    rows
}

fn lanes_json() -> Value {
    json!({
        "version": 1,
        "mission_slug": SLUG,
        "mission_id": MISSION_ID,
        "mission_branch": format!("kitty/mission-{SLUG}"),
        "target_branch": "main",
        "lanes": [
            {
                "lane_id": "lane-a",
                "wp_ids": ["WP01"],
                "write_scope": ["src/a.txt"],
                "predicted_surfaces": [],
                "depends_on_lanes": [],
                "parallel_group": 0
            },
            {
                "lane_id": "lane-b",
                "wp_ids": ["WP02"],
                "write_scope": ["src/b.txt"],
                "predicted_surfaces": [],
                "depends_on_lanes": [],
                "parallel_group": 0
            }
        ],
        "computed_at": "2026-11-02T10:00:00Z",
        "computed_from": "test",
        "planning_artifact_wps": [],
        "planning_commit_sha": null
    })
}

/// A repository with a `main` target, a mission directory, a lane manifest, and
/// two lane branches + worktrees each holding one commit.
struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
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
            &meta_json("lanes", None, false),
        );
        write(&mission_dir.join("spec.md"), "# Demo spec\n");
        write(
            &mission_dir.join("tasks.md"),
            "- [x] T001 one\n- [x] T002 two\n",
        );
        write(
            &mission_dir.join("tasks/WP01-a.md"),
            "---\nwork_package_id: WP01\n---\n",
        );
        write(
            &mission_dir.join("tasks/WP02-b.md"),
            "---\nwork_package_id: WP02\n---\n",
        );
        // The canonical lane history each work package needs before a `done`
        // transition is legal: the reducer has to see `approved` as the current
        // lane, not `genesis`. These are the same row shapes the status writer
        // emits (`force` boolean, `execution_mode`, structured actor).
        write(&mission_dir.join("status.events.jsonl"), &status_log());
        let store = LanesStore::new(&mission_dir);
        let manifest: sk_workspace::LanesManifest =
            serde_json::from_value(lanes_json()).expect("manifest fixture");
        store.write(&manifest).expect("write lanes.json");
        git(&root, &home, &["add", "kitty-specs"]);
        git(&root, &home, &["commit", "-q", "-m", "mission planning"]);

        for (lane, branch, path, contents) in [
            (
                "lane-a",
                format!("kitty/mission-{SLUG}-lane-a"),
                "src/a.txt",
                "a\n",
            ),
            (
                "lane-b",
                format!("kitty/mission-{SLUG}-lane-b"),
                "src/b.txt",
                "b\n",
            ),
        ] {
            let worktree = root.join(".worktrees").join(format!("{SLUG}-{lane}"));
            fs::create_dir_all(worktree.parent().unwrap()).unwrap();
            git(
                &root,
                &home,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &branch,
                    worktree.to_str().unwrap(),
                    "main",
                ],
            );
            write(&worktree.join(path), contents);
            git(&worktree, &home, &["add", path]);
            git(
                &worktree,
                &home,
                &["commit", "-q", "-m", &format!("{lane} work")],
            );
        }

        Self { temp, root, home }
    }

    fn workspace(&self) -> sk_workspace::MissionWorkspace {
        ProjectWorkspace::open(&self.root)
            .expect("open project")
            .mission(SLUG)
            .expect("mission workspace")
    }

    fn head(&self) -> String {
        git(&self.root, &self.home, &["rev-parse", "HEAD"])
    }

    fn meta(&self) -> Value {
        let text = fs::read_to_string(self.root.join("kitty-specs").join(SLUG).join("meta.json"))
            .expect("read meta");
        serde_json::from_str(&text).expect("meta json")
    }

    fn branch_exists(&self, branch: &str) -> bool {
        git_status(
            &self.root,
            &self.home,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        )
    }

    fn done_lanes(&self) -> Vec<(String, String)> {
        vec![
            ("WP01".to_owned(), "done".to_owned()),
            ("WP02".to_owned(), "done".to_owned()),
        ]
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Remove worktree registrations before the temp directory disappears so
        // no stale registration can leak into another repository.
        let _ = Command::new("git")
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["worktree", "prune"])
            .output();
        let _ = &self.temp;
    }
}

fn merge_request() -> ConsolidationRequest {
    ConsolidationRequest {
        strategy: Some(MergeStrategy::Merge),
        ..ConsolidationRequest::default()
    }
}

fn default_request() -> ConsolidationRequest {
    ConsolidationRequest::default()
}

#[test]
fn the_source_default_squash_lands_one_single_parent_commit() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let before = fixture.head();

    let report = consolidate_mission(&workspace, &fixture.done_lanes(), &default_request())
        .expect("the Source default (squash) consolidates");

    assert!(!report.already_consolidated);
    assert_eq!(report.strategy, "squash", "the Source default strategy");
    let head = fixture.head();
    // The landed commit is `report.baseline_commit`; the bookkeeping commits come
    // after it, and `meta.baseline_merge_commit` records the PRE-landing tip.
    let landed = report.baseline_commit.to_string();
    assert!(
        git_status(
            &fixture.root,
            &fixture.home,
            &["merge-base", "--is-ancestor", &landed, &head]
        ),
        "the landing commit is on the target's history"
    );
    assert!(
        !git_status(
            &fixture.root,
            &fixture.home,
            &["merge-base", "--is-ancestor", &head, &landed]
        ),
        "the bookkeeping commits follow the landing, not the other way round"
    );
    assert_eq!(
        report.pre_target_tip.as_ref().map(ToString::to_string),
        Some(before.clone())
    );
    let meta: serde_json::Value = serde_json::from_str(&git(
        &fixture.root,
        &fixture.home,
        &["show", &format!("main:kitty-specs/{SLUG}/meta.json")],
    ))
    .expect("landed meta");
    assert_eq!(
        meta["baseline_merge_commit"],
        json!(before),
        "meta records the pre-landing target tip"
    );
    assert_eq!(meta["merged_commit"], json!(landed));
    // `metadata_commit` is the batch commit that carried the meta.json projection;
    // the retrospective commit lands on top of it.
    assert!(
        report.metadata_commit.is_some(),
        "the bookkeeping batch commit is reported"
    );
    assert_ne!(
        report.metadata_commit.as_ref().map(ToString::to_string),
        Some(head.clone()),
        "the retrospective commit is the tip, not the batch commit"
    );

    // ONE single-parent commit carries the whole mission tree.
    let parents = git(
        &fixture.root,
        &fixture.home,
        &[
            "rev-list",
            "--parents",
            "-1",
            &report.baseline_commit.to_string(),
        ],
    );
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "a squash landing has exactly one parent: {parents}"
    );
    assert_eq!(parents.split_whitespace().last().unwrap(), before);
    assert!(fixture.root.join("src/a.txt").is_file());
    assert!(fixture.root.join("src/b.txt").is_file());
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/a.txt")).unwrap(),
        "a\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/b.txt")).unwrap(),
        "b\n"
    );
    // Both lanes were integrated into the integration branch first. Source's
    // lane merge is `git merge --no-edit -m ...` with NO `--no-ff`, so a lane
    // that already descends from the integration tip FAST-FORWARDS instead of
    // producing a merge commit (Source-first causal evidence:
    // `/tmp/sk-h4-restart-evidence/h4r-causal2-seed{0,1}.*.log` — the first lane
    // fast-forwards, later divergent lanes merge). Both outcomes are Source
    // behaviour; what must hold is that every lane was integrated and carries
    // the commit that represents it.
    assert!(report.integration_created);
    let outcomes: Vec<&str> = report
        .lanes
        .iter()
        .map(|lane| lane.outcome.as_str())
        .collect();
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(*outcome, "merged" | "fast_forwarded")),
        "every lane is integrated (merged or fast-forwarded): {outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == "merged")
            .count(),
        1,
        "in this fixture the FIRST lane fast-forwards and the second diverges, so exactly \
         one lane merge commit exists (Source's behaviour): {outcomes:?}"
    );
    assert!(report.lanes.iter().all(|lane| lane.merge_commit.is_some()));

    // The consolidation projection is recorded, other metadata is preserved.
    let meta = fixture.meta();
    assert_eq!(
        meta["baseline_merge_commit"],
        json!(before),
        "baseline_merge_commit is the PRE-landing target tip"
    );
    assert_eq!(
        meta["merged_commit"],
        json!(report.baseline_commit.to_string()),
        "merged_commit is the landing commit"
    );
    assert_eq!(meta["mission_id"], json!(MISSION_ID));

    // Teardown: lane branches/worktrees gone, integration branch gone.
    assert!(
        !fixture.branch_exists(&format!("kitty/mission-{SLUG}-lane-a")),
        "teardown: {:?}",
        report.teardown
    );
    assert!(!fixture.branch_exists(&format!("kitty/mission-{SLUG}-lane-b")));
    assert!(!fixture.branch_exists(&format!("kitty/mission-{SLUG}")));
    assert!(
        !fixture
            .root
            .join(".worktrees")
            .join(format!("{SLUG}-lane-a"))
            .exists()
    );
    assert!(
        !fixture
            .root
            .join(".worktrees")
            .join(format!("{SLUG}-integration"))
            .exists()
    );
}

#[test]
fn the_merge_strategy_lands_one_real_merge_commit_on_the_target() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let before = fixture.head();
    let report = consolidate_mission(&workspace, &fixture.done_lanes(), &merge_request())
        .expect("merge strategy consolidates");
    assert_eq!(report.strategy, "merge");
    let parents = git(
        &fixture.root,
        &fixture.home,
        &[
            "rev-list",
            "--parents",
            "-1",
            &report.baseline_commit.to_string(),
        ],
    );
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "the mission→target step is one merge commit: {parents}"
    );
    assert!(
        parents.split_whitespace().any(|parent| parent == before),
        "the merge commit keeps the pre-run target tip as its first parent: {parents}"
    );
    assert!(fixture.root.join("src/a.txt").is_file());
    assert!(fixture.root.join("src/b.txt").is_file());
}

#[test]
fn a_second_run_refuses_unless_it_is_a_resume() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    consolidate_mission(&workspace, &fixture.done_lanes(), &merge_request()).expect("first run");

    // A second CLI invocation re-reads the mission, so the recorded baseline is
    // visible to the guard.
    let error = consolidate_mission(
        &fixture.workspace(),
        &fixture.done_lanes(),
        &merge_request(),
    )
    .expect_err("a recorded baseline refuses a fresh run");
    assert!(
        error.to_string().contains("MISSION_ALREADY_CONSOLIDATED"),
        "{error}"
    );

    let resumed = consolidate_mission(
        &fixture.workspace(),
        &fixture.done_lanes(),
        &ConsolidationRequest {
            resume: true,
            ..merge_request()
        },
    )
    .expect("resume verifies the landed state");
    assert!(resumed.already_consolidated);
}

#[test]
fn retention_policy_keeps_branches_and_worktrees() {
    let fixture = Fixture::new();
    let mission_dir = fixture.root.join("kitty-specs").join(SLUG);
    write(
        &mission_dir.join("meta.json"),
        &meta_json("lanes", None, true),
    );
    git(&fixture.root, &fixture.home, &["add", "kitty-specs"]);
    git(
        &fixture.root,
        &fixture.home,
        &["commit", "-q", "-m", "retain policy"],
    );
    let workspace = fixture.workspace();

    let _report = consolidate_mission(&workspace, &fixture.done_lanes(), &merge_request())
        .expect("consolidation succeeds");
    assert!(fixture.branch_exists(&format!("kitty/mission-{SLUG}-lane-a")));
    assert!(fixture.branch_exists(&format!("kitty/mission-{SLUG}")));
    assert!(
        fixture
            .root
            .join(".worktrees")
            .join(format!("{SLUG}-lane-a"))
            .exists()
    );
}

#[test]
fn an_explicit_keep_flag_beats_the_policy() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let report = consolidate_mission(
        &workspace,
        &fixture.done_lanes(),
        &ConsolidationRequest {
            delete_branch: Some(false),
            remove_worktree: Some(false),
            ..merge_request()
        },
    )
    .expect("consolidation succeeds");
    // Only the temporary integration worktree is torn down; the lane branch and
    // worktree are kept because the operator asked for that.
    assert!(
        report
            .teardown
            .iter()
            .all(|action| action.action.starts_with("integration_"))
    );
    assert!(fixture.branch_exists(&format!("kitty/mission-{SLUG}-lane-a")));
    assert!(
        fixture
            .root
            .join(".worktrees")
            .join(format!("{SLUG}-lane-a"))
            .exists()
    );
}

#[test]
fn rebase_is_refused_when_it_would_drop_approved_commits() {
    // Source's reconciliation gate refuses a rebase that rewrites approved lane
    // commits (their ids are not reachable from the target), and the rollback door
    // restores every branch this run moved. The old expectation — "rebase lands" —
    // predated that gate and was wrong about Source.
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let before = git(&fixture.root, &fixture.home, &["rev-parse", "main"]);
    let error = consolidate_mission(
        &workspace,
        &fixture.done_lanes(),
        &ConsolidationRequest {
            strategy: Some(MergeStrategy::Rebase),
            ..ConsolidationRequest::default()
        },
    )
    .expect_err("a rebase that drops approved work is refused");
    let message = error.to_string();
    assert!(
        message.contains("NOT reachable from the target"),
        "the refusal names the dropped approved work: {message}"
    );
    assert_eq!(
        git(&fixture.root, &fixture.home, &["rev-parse", "main"]),
        before,
        "the rollback restores the pre-run target tip"
    );
    let tree = git(
        &fixture.root,
        &fixture.home,
        &["ls-tree", "-r", "--name-only", "main"],
    );
    assert!(!tree.contains("src/a.txt"), "{tree}");
    assert_eq!(
        git(
            &fixture.root,
            &fixture.home,
            &["rev-parse", "--abbrev-ref", "HEAD"]
        ),
        "main",
        "the operator checkout is resynced after the rollback"
    );
    let meta: serde_json::Value = serde_json::from_str(&git(
        &fixture.root,
        &fixture.home,
        &["show", &format!("main:kitty-specs/{SLUG}/meta.json")],
    ))
    .expect("meta");
    assert!(
        meta["baseline_merge_commit"].is_null(),
        "a rolled-back run records no baseline: {meta}"
    );
}

#[test]
fn the_config_strategy_is_used_when_no_flag_is_given() {
    let fixture = Fixture::new();
    write(
        &fixture.root.join(".kittify/config.yaml"),
        "merge:\n  strategy: merge\n",
    );
    git(&fixture.root, &fixture.home, &["add", ".kittify"]);
    git(
        &fixture.root,
        &fixture.home,
        &["commit", "-q", "-m", "merge config"],
    );
    let workspace = fixture.workspace();
    let report = consolidate_mission(&workspace, &fixture.done_lanes(), &default_request())
        .expect("config strategy consolidates");
    assert_eq!(report.strategy, "merge");
}

#[test]
fn an_invalid_configured_strategy_is_refused_not_defaulted() {
    let fixture = Fixture::new();
    write(
        &fixture.root.join(".kittify/config.yaml"),
        "merge:\n  strategy: octopus\n",
    );
    git(&fixture.root, &fixture.home, &["add", ".kittify"]);
    git(
        &fixture.root,
        &fixture.home,
        &["commit", "-q", "-m", "bad merge config"],
    );
    let workspace = fixture.workspace();
    let before = fixture.head();
    let error = consolidate_mission(&workspace, &fixture.done_lanes(), &default_request())
        .expect_err("an invalid strategy is refused");
    assert!(
        error.to_string().contains("MERGE_STRATEGY_INVALID"),
        "{error}"
    );
    assert_eq!(fixture.head(), before);
}

#[test]
fn a_resume_with_a_contradicting_strategy_is_a_flip_refusal() {
    assert_eq!(
        resolve_effective_strategy(
            Some(MergeStrategy::Merge),
            Some(MergeStrategy::Squash),
            None,
            false
        )
        .unwrap(),
        MergeStrategy::Merge
    );
    let error = resolve_effective_strategy(
        Some(MergeStrategy::Merge),
        Some(MergeStrategy::Squash),
        None,
        true,
    )
    .expect_err("a strategy flip across a resume is refused");
    assert!(error.to_string().contains("MERGE_STRATEGY_FLIP"), "{error}");
    // Resume without a flag honours the persisted strategy.
    assert_eq!(
        resolve_effective_strategy(None, Some(MergeStrategy::Squash), None, true).unwrap(),
        MergeStrategy::Squash
    );
    // Fresh without a flag/config uses the Source default.
    assert_eq!(
        resolve_effective_strategy(None, Some(MergeStrategy::Merge), None, false).unwrap(),
        MergeStrategy::Squash
    );
}

#[test]
fn a_non_terminal_work_package_refuses_before_any_mutation() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let before = fixture.head();
    let error = consolidate_mission(
        &workspace,
        &[("WP01".to_owned(), "for_review".to_owned())],
        &merge_request(),
    )
    .expect_err("not merge ready");
    assert!(
        error.to_string().contains("MISSION_NOT_MERGE_READY"),
        "{error}"
    );
    assert_eq!(fixture.head(), before);
}

#[test]
fn a_dirty_checkout_refuses_and_leaves_the_target_alone() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    write(&fixture.root.join("README.md"), "# Demo changed\n");
    let before = fixture.head();
    let error = consolidate_mission(&workspace, &fixture.done_lanes(), &merge_request())
        .expect_err("dirty checkout");
    assert!(
        error.to_string().contains("CONSOLIDATION_DIRTY_CHECKOUT"),
        "{error}"
    );
    assert_eq!(fixture.head(), before);
}

#[test]
fn a_wrong_checkout_refuses_before_merging() {
    let fixture = Fixture::new();
    git(
        &fixture.root,
        &fixture.home,
        &["checkout", "-q", "-b", "elsewhere"],
    );
    let workspace = fixture.workspace();
    let error = consolidate_mission(&workspace, &fixture.done_lanes(), &merge_request())
        .expect_err("wrong checkout");
    assert!(
        error
            .to_string()
            .contains("CONSOLIDATION_CHECKOUT_REQUIRED"),
        "{error}"
    );
}

#[test]
fn a_conflicting_lane_refuses_and_keeps_earlier_landings() {
    let fixture = Fixture::new();
    // lane-b rewrites the file lane-a also owns, with different content, so the
    // second merge conflicts.
    let worktree = fixture
        .root
        .join(".worktrees")
        .join(format!("{SLUG}-lane-b"));
    write(&worktree.join("src/a.txt"), "lane-b wins\n");
    git(&worktree, &fixture.home, &["add", "src/a.txt"]);
    git(
        &worktree,
        &fixture.home,
        &["commit", "-q", "-m", "lane-b touches a"],
    );

    let workspace = fixture.workspace();
    let before = fixture.head();
    let error = consolidate_mission(&workspace, &fixture.done_lanes(), &merge_request())
        .expect_err("conflict");
    let text = error.to_string();
    assert!(text.contains("CONSOLIDATION_MERGE_CONFLICT"), "{text}");
    assert!(text.contains("src/a.txt"), "{text}");
    assert!(text.contains("target branch was not touched"), "{text}");

    // The conflict happened in the temporary integration worktree: the target
    // was never touched, no merge state remains, and no baseline was recorded.
    assert!(!fixture.root.join(".git").join("MERGE_HEAD").exists());
    assert_eq!(fixture.head(), before);
    assert!(!fixture.root.join("src/a.txt").is_file());
    assert_eq!(fixture.meta().get("baseline_merge_commit"), None);
}

#[test]
fn dry_run_plans_without_touching_anything() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let before = fixture.head();
    let report = consolidate_mission(
        &workspace,
        &fixture.done_lanes(),
        &ConsolidationRequest {
            dry_run: true,
            ..merge_request()
        },
    )
    .expect("dry run");
    assert!(report.dry_run);
    assert_eq!(fixture.head(), before);
    assert_eq!(report.lanes.len(), 2);
    assert!(report.lanes.iter().all(|lane| lane.outcome == "planned"));
    assert_eq!(fixture.meta().get("baseline_merge_commit"), None);
    assert!(fixture.branch_exists(&format!("kitty/mission-{SLUG}-lane-a")));
}

#[test]
fn abort_before_any_landing_clears_the_run_and_abort_after_landing_refuses() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    // A journaled run that never moved the target: record the intent, then abort.
    let common_dir = git(
        &fixture.root,
        &fixture.home,
        &["rev-parse", "--git-common-dir"],
    );
    let common = if Path::new(&common_dir).is_absolute() {
        PathBuf::from(common_dir)
    } else {
        fixture.root.join(common_dir)
    };
    let journal = sk_workspace::CoordJournal::new(&common, SLUG);
    let tip = fixture.head();
    let oid = sk_git::Oid::parse(&tip).unwrap();
    journal
        .begin(sk_workspace::CoordRecord::intent(
            sk_workspace::CoordRecordKind::Integration,
            SLUG,
            "consolidate_merge",
            Some("main"),
            Some(&fixture.root),
            Some(&oid),
        ))
        .expect("journal intent");

    let report = consolidate_mission(
        &workspace,
        &fixture.done_lanes(),
        &ConsolidationRequest {
            abort: true,
            ..merge_request()
        },
    )
    .expect("abort clears a run that did not land");
    assert!(!report.already_consolidated);

    // A landed run is never rewound.
    consolidate_mission(&workspace, &fixture.done_lanes(), &merge_request()).expect("land");
    journal
        .begin(sk_workspace::CoordRecord::intent(
            sk_workspace::CoordRecordKind::Integration,
            SLUG,
            "consolidate_merge",
            Some("main"),
            Some(&fixture.root),
            Some(&oid),
        ))
        .expect("journal intent");
    let error = consolidate_mission(
        &workspace,
        &fixture.done_lanes(),
        &ConsolidationRequest {
            abort: true,
            ..merge_request()
        },
    )
    .expect_err("abort never rewinds a landed target");
    assert!(
        error.to_string().contains("CONSOLIDATION_ALREADY_LANDED"),
        "{error}"
    );
}
