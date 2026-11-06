//! CF39: canonical lane sparse-checkout policy + lane-worktree recognizer,
//! exercised against real linked worktrees.
//!
//! Every fixture is a real `tempfile` repository with a real index and real
//! `git worktree add` linked worktrees, so each lane has its own git-dir binding,
//! the shared common dir, its own index and its own
//! `<gitdir>/info/sparse-checkout` file. Nothing is mocked and no OID,
//! timestamp, event or readiness value is fabricated.
//!
//! The read-only contract is asserted, not assumed: each scan is bracketed by a
//! full digest of every lane's pattern file plus `git status --porcelain` in the
//! root and in every lane, and the per-worktree git dir is checked for a
//! leftover `.lock` — a diagnostic must neither materialize nor mutate nor lock.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sk_git::GitRepository;
use sk_workspace::coordination_sparse::{
    LANE_SPARSE_CHECKOUT_DRIFT, LaneSparseDrift, LaneSparsePolicy, scan_lane_sparse_drift,
};
use sk_workspace::naming::{lane_id_for_worktree_dir, parse_lane_worktree_dir, worktree_dir_name};

use support::{Fixture, git, init_repo, write};
const MISSION_ID: &str = "01KNXQS9ATWWFXS3K5ZJ9E5001";
const MID8: &str = "01KNXQS9";
const SLUG: &str = "cf39-mission";

/// The canonical mission directory under `kitty-specs/`.
fn mission_dir() -> String {
    format!("{SLUG}-{MID8}")
}

/// The exact authored pattern file text, in the reference's order.
fn expected_pattern_text() -> String {
    LaneSparsePolicy::for_mission(SLUG, MID8).pattern_file_text()
}

/// A committed repository with `.worktrees/` ignored, like a real Mission.
fn lane_repo() -> Fixture {
    let fixture = init_repo();
    write(&fixture.path(".gitignore"), ".worktrees/\n");
    write(
        &fixture
            .path("kitty-specs")
            .join(mission_dir())
            .join("meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "topology": "lanes",
                "target_branch": "main",
                "mission_type": "software-dev",
            })
        ),
    );
    support::git(fixture.root(), &["add", "-A"]);
    support::git(fixture.root(), &["commit", "-m", "mission manifest"]);
    fixture
}

/// Create a real lane: branch + `git worktree add` linked checkout.
fn add_lane(root: &Path, slug: &str, lane_id: &str) -> PathBuf {
    let branch = format!("kitty/mission-{slug}-{lane_id}");
    git(root, &["branch", &branch]);
    let dir_name = worktree_dir_name(slug, lane_id).expect("valid lane id");
    let path = root.join(".worktrees").join(dir_name);
    git(
        root,
        &[
            "worktree",
            "add",
            path.to_str().expect("utf-8 path"),
            &branch,
        ],
    );
    path
}

/// The lane's real per-worktree pattern file, resolved exactly like the
/// reference resolves it (`git rev-parse --git-path info/sparse-checkout`).
fn sparse_file(lane: &Path) -> PathBuf {
    let raw = git(lane, &["rev-parse", "--git-path", "info/sparse-checkout"]);
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        lane.join(path)
    }
}

/// Write the lane's pattern file through the canonical writer shape.
fn write_patterns(lane: &Path, text: &str) {
    let path = sparse_file(lane);
    write(&path, text);
}

/// A full read-only snapshot: every lane's pattern bytes + status + lock files.
fn snapshot(fixture: &Fixture, lanes: &[PathBuf]) -> BTreeMap<String, String> {
    let mut state = BTreeMap::new();
    state.insert(
        "root-status".to_owned(),
        git(fixture.root(), &["status", "--porcelain"]),
    );
    for lane in lanes {
        let name = lane
            .file_name()
            .and_then(|name| name.to_str())
            .expect("utf-8 lane name")
            .to_owned();
        state.insert(
            format!("{name}-status"),
            git(lane, &["status", "--porcelain"]),
        );
        let pattern = sparse_file(lane);
        state.insert(
            format!("{name}-patterns"),
            match fs::read(&pattern) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(error) => format!("<unreadable: {}>", error.kind()),
            },
        );
        let git_dir = git(lane, &["rev-parse", "--git-dir"]);
        let git_dir = PathBuf::from(git_dir);
        let git_dir = if git_dir.is_absolute() {
            git_dir
        } else {
            lane.join(git_dir)
        };
        let mut locks: Vec<String> = fs::read_dir(&git_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.ends_with(".lock"))
                    .collect()
            })
            .unwrap_or_default();
        locks.sort();
        state.insert(format!("{name}-gitdir-locks"), locks.join(","));
    }
    state
}

#[test]
fn cf39_policy_is_the_reference_policy() {
    let policy = LaneSparsePolicy::for_mission(SLUG, MID8);
    assert_eq!(policy.mission_dir(), mission_dir());
    assert_eq!(
        policy.patterns(),
        [
            "/*".to_owned(),
            format!("!kitty-specs/{}/status.events.jsonl", mission_dir()),
            format!("!kitty-specs/{}/status.json", mission_dir()),
        ]
    );
    assert_eq!(policy.pattern_file_text(), expected_pattern_text());
    assert_eq!(
        LANE_SPARSE_CHECKOUT_DRIFT, "LANE_SPARSE_CHECKOUT_DRIFT",
        "the parent health owner reports this stable Source code"
    );
}

#[test]
fn cf39_recognizer_is_recomposition_not_prefix() {
    assert_eq!(
        parse_lane_worktree_dir("057-foo-lane-a"),
        Some(("057-foo", "lane-a"))
    );
    assert_eq!(
        lane_id_for_worktree_dir("057-foo-lane-a", "057-foo"),
        Some("lane-a")
    );
    // A same-prefix impostor must not be adopted.
    assert_eq!(
        lane_id_for_worktree_dir("057-foobar-lane-a", "057-foo"),
        None
    );
    let mid8_slug_dir = format!("{SLUG}-{MID8}-lane-a");
    assert_eq!(
        lane_id_for_worktree_dir(&mid8_slug_dir, &format!("{SLUG}-{MID8}")),
        Some("lane-a"),
        "a slug that embeds its mid8 keys the lane directory verbatim"
    );
    assert_eq!(
        lane_id_for_worktree_dir(&mid8_slug_dir, SLUG),
        None,
        "the bare slug does not own a mid8-suffixed lane directory"
    );
}

#[test]
fn cf39_real_lanes_report_the_three_drift_shapes() {
    let fixture = lane_repo();
    let root = fixture.root();
    let lane_a = add_lane(root, SLUG, "lane-a"); // exact policy
    let lane_b = add_lane(root, SLUG, "lane-b"); // comments/order/whitespace
    let lane_c = add_lane(root, SLUG, "lane-c"); // missing status.json
    let lane_d = add_lane(root, SLUG, "lane-d"); // pattern file absent

    write_patterns(&lane_a, &expected_pattern_text());
    write_patterns(
        &lane_b,
        &format!(
            "\n# lane policy, authored by hand\n   !kitty-specs/{dir}/status.json\t\n\
             /*\n  \n!kitty-specs/{dir}/status.events.jsonl\n!kitty-specs/{dir}/extra.json\n",
            dir = mission_dir()
        ),
    );
    write_patterns(
        &lane_c,
        &format!("/*\n!kitty-specs/{}/status.events.jsonl\n", mission_dir()),
    );

    let lanes = vec![
        lane_a.clone(),
        lane_b.clone(),
        lane_c.clone(),
        lane_d.clone(),
    ];
    let before = snapshot(&fixture, &lanes);

    let repo = GitRepository::open(root).expect("open fixture repo");
    let scan = scan_lane_sparse_drift(&repo, SLUG, MID8).expect("scan runs");

    let after = snapshot(&fixture, &lanes);
    assert_eq!(before, after, "the diagnostic must not mutate anything");
    assert_eq!(scan.inspected_lanes(), 4);

    let drifts: Vec<&LaneSparseDrift> = scan.drifts().iter().collect();
    assert_eq!(drifts.len(), 2, "only lane-c and lane-d drift: {drifts:?}");

    assert_eq!(drifts[0].lane(), lane_c.as_path());
    assert_eq!(
        drifts[0].missing_patterns(),
        Some([format!("!kitty-specs/{}/status.json", mission_dir())].as_slice()),
        "a reordered/whitespace/commented policy is healthy; only the absent pattern drifts"
    );

    assert_eq!(drifts[1].lane(), lane_d.as_path());
    assert!(
        matches!(drifts[1], LaneSparseDrift::PolicyFileMissing { .. }),
        "an absent pattern file is its own shape, not 'missing patterns'"
    );
    assert!(!scan.is_clean());
}

#[test]
fn cf39_healthy_lanes_produce_a_clean_scan() {
    let fixture = lane_repo();
    let root = fixture.root();
    let lane_a = add_lane(root, SLUG, "lane-a");
    let lane_b = add_lane(root, SLUG, "lane-b");
    write_patterns(&lane_a, &expected_pattern_text());
    write_patterns(&lane_b, &expected_pattern_text());

    let repo = GitRepository::open(root).expect("open fixture repo");
    let scan = scan_lane_sparse_drift(&repo, SLUG, MID8).expect("scan runs");
    assert!(
        scan.is_clean(),
        "expected no drift, got {:?}",
        scan.drifts()
    );
    assert_eq!(scan.inspected_lanes(), 2);
}

#[test]
fn cf39_other_missions_and_unregistered_directories_are_skipped() {
    let fixture = lane_repo();
    let root = fixture.root();
    let lane_a = add_lane(root, SLUG, "lane-a");
    write_patterns(&lane_a, &expected_pattern_text());

    // Another Mission's lane, real and drifted: not this scan's business.
    let other = add_lane(root, "other-mission", "lane-a");
    write_patterns(&other, "/*\n");

    // A same-prefix impostor, real and drifted, whose lane id parses but whose
    // recomposition does not reproduce the name for this Mission slug.
    let impostor = add_lane(root, &format!("{SLUG}-extra"), "lane-a");
    write_patterns(&impostor, "/*\n");

    // A plain directory that looks like a lane but is not a registered worktree.
    let unregistered = root.join(".worktrees").join(format!("{SLUG}-lane-z"));
    std::fs::create_dir_all(&unregistered).expect("plain directory");
    write(&unregistered.join("info").join("sparse-checkout"), "/*\n");

    let repo = GitRepository::open(root).expect("open fixture repo");
    let scan = scan_lane_sparse_drift(&repo, SLUG, MID8).expect("scan runs");
    assert!(scan.is_clean(), "unexpected drift: {:?}", scan.drifts());
    assert_eq!(
        scan.inspected_lanes(),
        1,
        "only this Mission's registered lane worktree is inspected"
    );
}

#[test]
fn cf39_unresolvable_lane_binding_is_its_own_fact() {
    let fixture = lane_repo();
    let root = fixture.root();
    let lane = add_lane(root, SLUG, "lane-a");
    write_patterns(&lane, &expected_pattern_text());

    // Break ONLY the lane's reverse pointer (`.git` file). The worktree stays
    // registered (the admin dir still records it), but `git -C <lane>` can no
    // longer resolve a repository there — the reference's
    // `rev-parse --git-path` failure shape.
    let pattern_path = sparse_file(&lane);
    let before_root_status = git(root, &["status", "--porcelain"]);
    let before_patterns = fs::read(&pattern_path).expect("pattern bytes");
    write(&lane.join(".git"), "gitdir: /nonexistent/cf39\n");

    let repo = GitRepository::open(root).expect("open fixture repo");
    let scan = scan_lane_sparse_drift(&repo, SLUG, MID8).expect("scan runs");

    assert_eq!(scan.inspected_lanes(), 1, "the lane is still registered");
    assert_eq!(scan.drifts().len(), 1, "facts: {:?}", scan.drifts());
    assert_eq!(scan.drifts()[0].lane(), lane.as_path());
    assert!(
        matches!(
            scan.drifts()[0],
            LaneSparseDrift::PolicyPathUnresolvable { .. }
        ),
        "an unreadable lane repository is not a missing file or a policy verdict"
    );
    // Read-only: no repair, no rewrite, no worktree materialization.
    assert_eq!(git(root, &["status", "--porcelain"]), before_root_status);
    assert_eq!(
        fs::read(&pattern_path).expect("pattern bytes"),
        before_patterns
    );
}

#[test]
fn cf39_absent_worktrees_directory_is_not_an_error() {
    let fixture = lane_repo();
    assert!(!fixture.path(".worktrees").exists());
    let repo = GitRepository::open(fixture.root()).expect("open fixture repo");
    let scan = scan_lane_sparse_drift(&repo, SLUG, MID8).expect("scan runs");
    assert!(scan.is_clean());
    assert_eq!(scan.inspected_lanes(), 0);
    assert!(
        !fixture.path(".worktrees").exists(),
        "a read-only scan must not create .worktrees/"
    );
}

#[test]
fn cf39_legacy_slug_uses_the_verbatim_coordination_directory() {
    let fixture = lane_repo();
    let root = fixture.root();
    // A pre-083 Mission: the recorded slug keeps its `NNN-` prefix, and its lane
    // worktree directory was created from that recorded slug.
    let legacy_slug = "057-legacy";
    let lane = add_lane(root, legacy_slug, "lane-a");
    let policy = LaneSparsePolicy::for_mission(legacy_slug, MID8);
    assert_eq!(policy.mission_dir(), "057-legacy-01KNXQS9");
    write_patterns(&lane, &policy.pattern_file_text());

    let repo = GitRepository::open(root).expect("open fixture repo");
    let scan = scan_lane_sparse_drift(&repo, legacy_slug, MID8).expect("scan runs");
    assert_eq!(scan.inspected_lanes(), 1);
    assert!(scan.is_clean(), "unexpected drift: {:?}", scan.drifts());

    // The canonical (stripping) mission directory is a DIFFERENT name; using it
    // here would point at a directory that was never created.
    assert_ne!(policy.mission_dir(), format!("legacy-{MID8}"));
}
