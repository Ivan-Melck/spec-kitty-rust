//! CF35: read-only coordination health against real Git repositories.
//!
//! Every fixture is a real `tempfile` repository with a real index, real refs
//! and — where the case needs one — a real `git worktree add` linked worktree
//! (its own git-dir binding plus the shared common-dir). Nothing is mocked and
//! no OID, timestamp, event or readiness value is fabricated.
//!
//! Not claimed here: the two reference checks whose canonical owner does not
//! exist yet (lane sparse-checkout drift, stranded-`done` re-verification) are
//! asserted as `unavailable`, never as a verdict, and the whole report is
//! therefore asserted to be incomplete.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sk_workspace::coordination_health::{
    COORD_BRANCH_ABSENT_HINT, COORDINATION_BRANCH_STALE_VS_TARGET, COORDINATION_META_INCOMPLETE,
    COORDINATION_WORKTREE_BRANCH_MISMATCH, COORDINATION_WORKTREE_DIRTY,
    COORDINATION_WORKTREE_MISSING, COORDINATION_WORKTREE_NEVER_CREATED,
    COORDINATION_WORKTREE_STALE, CoordinationFinding, CoordinationHealthError,
    CoordinationHealthReport, CoordinationHealthRequest, Severity, TRACKED_WORKTREES_CONTENT,
    coordination_health,
};
use sk_workspace::workspace::ProjectWorkspace;

use support::{Fixture, git, init_repo, write};

const MISSION_ID: &str = "01KNXQS9ATWWFXS3K5ZJ9E5001";
const MID8: &str = "01KNXQS9";
const SLUG: &str = "cf35-mission";
const COORD_BRANCH: &str = "kitty/mission-cf35-mission-01KNXQS9";

fn coord_dir() -> String {
    format!("{SLUG}-{MID8}-coord")
}

fn write_meta(root: &Path) {
    write(
        &root.join("kitty-specs").join(SLUG).join("meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "topology": "coord",
                "target_branch": "main",
                "coordination_branch": COORD_BRANCH,
                "mission_type": "software-dev",
            })
        ),
    );
}

/// A committed fixture with one Mission that declares a coordination branch.
///
/// `.worktrees/` is gitignored, exactly as a real Spec Kitty repository
/// ignores its execution scratch space — otherwise `git add -A` would track
/// the linked worktree as a gitlink and the run would (correctly) report
/// tracked `.worktrees/` content.
fn coordinated_repo() -> (Fixture, PathBuf) {
    let fixture = init_repo();
    write(&fixture.path(".gitignore"), ".worktrees/\nremote.git/\n");
    write_meta(fixture.root());
    commit_all(fixture.root(), "mission manifest");
    let worktree = fixture.path(".worktrees").join(coord_dir());
    (fixture, worktree)
}

fn commit_all(root: &Path, message: &str) {
    support::git(root, &["add", "-A"]);
    support::git(root, &["commit", "-m", message]);
}

fn add_coord_branch(root: &Path) -> String {
    git(root, &["branch", COORD_BRANCH]);
    git(root, &["rev-parse", COORD_BRANCH])
}

fn add_coord_worktree(root: &Path, worktree: &Path) -> String {
    fs::create_dir_all(worktree.parent().expect("worktrees parent")).expect("worktrees dir");
    git(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().expect("utf-8 fixture path"),
            COORD_BRANCH,
        ],
    );
    git(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
}

fn findings(root: &Path, request: &CoordinationHealthRequest) -> CoordinationHealthReport {
    let workspace = ProjectWorkspace::open(root).expect("a real Git repository");
    coordination_health(&workspace, request).expect("the diagnosis answers")
}

fn codes(report: &CoordinationHealthReport) -> Vec<Option<&'static str>> {
    report
        .findings()
        .iter()
        .map(CoordinationFinding::code)
        .collect()
}

fn finding<'a>(report: &'a CoordinationHealthReport, code: &str) -> &'a CoordinationFinding {
    report
        .findings()
        .iter()
        .find(|finding| finding.code() == Some(code))
        .unwrap_or_else(|| panic!("no {code} finding in {:?}", codes(report)))
}

/// A stable content+ref snapshot proving the diagnosis wrote nothing.
fn footprint(root: &Path) -> (BTreeMap<String, Vec<u8>>, String, Vec<u8>) {
    let mut files = BTreeMap::new();
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if path.is_dir() {
                walk(root, &path, files);
            } else if let Ok(bytes) = fs::read(&path) {
                files.insert(relative, bytes);
            }
        }
    }
    walk(root, root, &mut files);
    let refs = git(root, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let index = fs::read(root.join(".git").join("index")).unwrap_or_default();
    (files, refs, index)
}

#[test]
fn a_healthy_materialized_coordination_worktree_reports_exactly_the_reference_findings() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);

    // The linked worktree really is bound to this repository: its own git-dir
    // differs from the shared common-dir.
    let git_dir = git(&worktree, &["rev-parse", "--absolute-git-dir"]);
    let common_dir = git(&worktree, &["rev-parse", "--git-common-dir"]);
    assert!(
        git_dir.contains("worktrees"),
        "a linked worktree has its own git-dir: {git_dir}"
    );
    assert!(
        Path::new(&common_dir).ends_with(".git") || common_dir == ".git",
        "the linked worktree shares the repository common-dir: {common_dir}"
    );

    let before = footprint(root);
    let report = findings(root, &CoordinationHealthRequest::new());
    let after = footprint(root);
    assert_eq!(before, after, "the read-only diagnosis must write nothing");

    // Reference order: Git version, tracked-.worktrees hygiene, stranded strands
    // (none here), then the per-mission worktree block and the lane sparse policy.
    let messages: Vec<String> = report
        .findings()
        .iter()
        .map(|finding| finding.message().to_owned())
        .collect();
    assert_eq!(messages.len(), 4, "findings: {messages:?}");
    assert!(
        messages[0].starts_with("git ")
            && messages[0].contains("satisfies the >= 2.25 requirement."),
        "{}",
        messages[0]
    );
    assert_eq!(messages[1], "No tracked content under .worktrees/.");
    assert_eq!(
        messages[2],
        format!("Coordination worktree {} is healthy.", worktree.display())
    );
    assert_eq!(
        messages[3],
        "All lane worktrees carry the expected sparse-checkout policy."
    );
    assert!(codes(&report).iter().all(Option::is_none));

    // Every requested check is composed, so this is a complete verdict.
    assert!(report.unavailable().is_empty());
    assert_eq!(report.exit_code(), Some(0));
}

#[test]
fn the_diagnosis_is_cwd_invariant_from_inside_the_coordination_worktree() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);

    let from_root = findings(root, &CoordinationHealthRequest::new());
    let from_worktree = findings(&worktree, &CoordinationHealthRequest::new());
    assert_eq!(
        from_root.findings(),
        from_worktree.findings(),
        "the primary checkout's specs/ and refs are the authority from any worktree"
    );
}

#[test]
fn a_declared_branch_without_a_worktree_is_reported_as_missing_not_never_created() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);

    let report = findings(root, &CoordinationHealthRequest::new());
    let missing = finding(&report, COORDINATION_WORKTREE_MISSING);
    assert_eq!(missing.severity(), Severity::Warning);
    assert_eq!(
        missing.message(),
        format!(
            "Coordination worktree {} is missing for mission '{SLUG}' (the branch '{COORD_BRANCH}' exists).",
            worktree.display()
        )
    );
    // A LOCAL head exists, so the `--fix` command really can materialize it.
    let next_step = missing.next_step().expect("a remedy");
    assert!(
        next_step.starts_with(&format!(
            "Run: `spec-kitty doctor coordination --mission {SLUG} --fix`"
        )),
        "{next_step}"
    );
    assert!(next_step.contains("worktree add"), "{next_step}");
    assert_eq!(
        missing.extra()["recovery_args"],
        serde_json::json!([
            "git",
            "-C",
            root.display().to_string(),
            "worktree",
            "add",
            worktree.display().to_string(),
            COORD_BRANCH,
        ])
    );
    assert_eq!(missing.extra()["mission_slug"], serde_json::json!(SLUG));
    assert_eq!(missing.extra()["mid8"], serde_json::json!(MID8));
    assert_eq!(
        missing.extra()["coord_branch"],
        serde_json::json!(COORD_BRANCH)
    );
    assert!(!report.has_error(), "a missing worktree is a warning");
}

#[test]
fn a_remote_only_coordination_branch_leads_with_fetch_branch_fix() {
    let fixture = init_repo();
    let root = fixture.root();
    write_meta(root);
    commit_all(root, "mission manifest");

    // A real remote whose branch the checkout never fetched as a local head:
    // create it in a bare remote and delete the local branch again.
    let remote = fixture.path("remote.git");
    git(
        root,
        &[
            "init",
            "--quiet",
            "--bare",
            remote.to_str().expect("utf-8 fixture path"),
        ],
    );
    git(
        root,
        &["remote", "add", "origin", remote.to_str().expect("utf-8")],
    );
    git(root, &["branch", COORD_BRANCH]);
    git(root, &["push", "--quiet", "origin", COORD_BRANCH]);
    git(root, &["fetch", "--quiet", "origin"]);
    git(root, &["branch", "-D", COORD_BRANCH]);

    let report = findings(root, &CoordinationHealthRequest::new());
    let missing = finding(&report, COORDINATION_WORKTREE_MISSING);
    let next_step = missing.next_step().expect("a remedy");
    assert!(
        next_step.starts_with("Run in order: `git -C "),
        "{next_step}"
    );
    assert!(next_step.contains("fetch"), "{next_step}");
    assert!(
        next_step.contains(&format!("branch {COORD_BRANCH} origin/{COORD_BRANCH}")),
        "{next_step}"
    );
    assert!(
        next_step.contains(&format!(
            "spec-kitty doctor coordination --mission {SLUG} --fix"
        )),
        "{next_step}"
    );
}

#[test]
fn an_absent_coordination_branch_is_never_created_and_points_at_the_flatten_remedy() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();

    let report = findings(root, &CoordinationHealthRequest::new());
    let never = finding(&report, COORDINATION_WORKTREE_NEVER_CREATED);
    assert_eq!(never.severity(), Severity::Warning);
    assert_eq!(
        never.message(),
        format!(
            "Coordination worktree {} is missing for mission '{SLUG}' and the declared coordination branch '{COORD_BRANCH}' does not exist in git (never created or deleted).",
            worktree.display()
        )
    );
    assert_eq!(never.next_step(), Some(COORD_BRANCH_ABSENT_HINT));
    // The reference attaches the manifest path so its --fix can find the Mission.
    assert_eq!(
        never.extra()["meta_path"],
        serde_json::json!(
            root.join("kitty-specs")
                .join(SLUG)
                .join("meta.json")
                .display()
                .to_string()
        )
    );
}

#[test]
fn a_coordination_worktree_on_another_branch_is_a_mismatch_warning() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    git(root, &["branch", "some-other-branch"]);
    // Materialize the worktree path from the other branch, then declare the
    // coordination branch: the checkout is real, just not on the declared ref.
    fs::create_dir_all(worktree.parent().expect("parent")).expect("worktrees dir");
    git(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().expect("utf-8"),
            "some-other-branch",
        ],
    );

    let report = findings(root, &CoordinationHealthRequest::new());
    let mismatch = finding(&report, COORDINATION_WORKTREE_BRANCH_MISMATCH);
    assert_eq!(mismatch.severity(), Severity::Warning);
    assert_eq!(
        mismatch.message(),
        format!(
            "Coordination worktree {} is on 'refs/heads/some-other-branch', expected '{COORD_BRANCH}'.",
            worktree.display()
        )
    );
    assert!(
        mismatch
            .next_step()
            .unwrap_or_default()
            .contains(&format!("checkout {COORD_BRANCH}")),
        "{:?}",
        mismatch.next_step()
    );
    // A mismatch does not stop the other checks from running (the lane sparse
    // policy is reported after the worktree block, exactly as the reference does).
    assert!(
        codes(&report).contains(&Some(COORDINATION_WORKTREE_BRANCH_MISMATCH)),
        "{:?}",
        codes(&report)
    );
    assert_eq!(
        codes(&report).last().copied(),
        Some(None),
        "the trailing lane sparse finding is an `ok` row: {:?}",
        codes(&report)
    );
}

#[test]
fn a_detached_coordination_worktree_reports_the_reference_detached_marker() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    let tip = add_coord_branch(root);
    fs::create_dir_all(worktree.parent().expect("parent")).expect("worktrees dir");
    git(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            worktree.to_str().expect("utf-8"),
            &tip,
        ],
    );

    let report = findings(root, &CoordinationHealthRequest::new());
    let mismatch = finding(&report, COORDINATION_WORKTREE_BRANCH_MISMATCH);
    assert!(
        mismatch.message().contains("is on '<detached>'"),
        "{}",
        mismatch.message()
    );
}

#[test]
fn an_uncommitted_change_in_the_coordination_worktree_is_reported_dirty() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);
    write(&worktree.join("scratch.txt"), "in flight\n");

    let report = findings(root, &CoordinationHealthRequest::new());
    let dirty = finding(&report, COORDINATION_WORKTREE_DIRTY);
    assert_eq!(dirty.severity(), Severity::Warning);
    assert_eq!(
        dirty.message(),
        format!(
            "Coordination worktree {} has uncommitted changes.",
            worktree.display()
        )
    );
    assert!(!report.has_error(), "dirty worktree is not an error");
}

#[test]
fn a_coordination_worktree_behind_its_own_branch_tip_is_stale_but_not_diverged() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    let old_tip = add_coord_branch(root);
    // The worktree is checked out on its own local branch at the old commit, so
    // moving the DECLARED coordination branch forward leaves its HEAD (and its
    // index) untouched: a clean worktree that is a genuine fast-forward
    // candidate. Advancing the branch a worktree already has checked out would
    // instead move that checkout's HEAD and show up as `dirty`, which is what
    // real Git — and therefore the reference — reports too.
    git(root, &["branch", "coord-behind", &old_tip]);
    fs::create_dir_all(worktree.parent().expect("parent")).expect("worktrees dir");
    git(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().expect("utf-8"),
            "coord-behind",
        ],
    );

    write(&root.join("later.txt"), "later\n");
    commit_all(root, "later");
    let new_tip = git(root, &["rev-parse", "HEAD"]);
    git(
        root,
        &[
            "update-ref",
            &format!("refs/heads/{COORD_BRANCH}"),
            &new_tip,
        ],
    );
    assert_ne!(old_tip, new_tip);

    let report = findings(root, &CoordinationHealthRequest::new());
    let stale = finding(&report, COORDINATION_WORKTREE_STALE);
    assert_eq!(
        stale.message(),
        format!(
            "Coordination worktree {} is behind the coord branch '{COORD_BRANCH}' tip (fast-forward available).",
            worktree.display()
        )
    );
    assert!(
        stale
            .next_step()
            .unwrap_or_default()
            .contains("spec-kitty doctor workspaces --fix"),
        "{:?}",
        stale.next_step()
    );
    // The branch-mismatch check is independent in the reference, so it fires
    // for the same worktree; staleness is an additional warning, never a
    // replacement for it.
    assert!(codes(&report).contains(&Some(COORDINATION_WORKTREE_BRANCH_MISMATCH)));
    assert!(!codes(&report).contains(&Some(COORDINATION_WORKTREE_DIRTY)));
    assert!(!report.has_error());
}

#[test]
fn tracked_content_under_worktrees_is_a_blocking_error() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);
    // Force-add scratch content under .worktrees/ (what the #1772 guard exists
    // for), then commit it for real.
    write(
        &fixture.path(".worktrees").join("stray").join("junk.txt"),
        "scratch\n",
    );
    git(root, &["add", "-f", ".worktrees/stray"]);
    git(root, &["commit", "--quiet", "-m", "track scratch content"]);

    let report = findings(root, &CoordinationHealthRequest::new());
    let tracked = finding(&report, TRACKED_WORKTREES_CONTENT);
    assert_eq!(tracked.severity(), Severity::Error);
    assert!(report.has_error());
    assert_eq!(tracked.extra()["tracked_count"], serde_json::json!(1));
    assert_eq!(tracked.extra()["truncated"], serde_json::json!(false));
    let preview = tracked.extra()["tracked"]
        .as_array()
        .expect("tracked preview");
    assert_eq!(preview.len(), 1);
    assert!(
        preview[0].as_str().expect("path text").contains("junk.txt"),
        "{preview:?}"
    );
    assert!(
        tracked
            .next_step()
            .unwrap_or_default()
            .starts_with("Remove it from version control: `git rm -r --cached .worktrees/`"),
        "{:?}",
        tracked.next_step()
    );
}

#[test]
fn a_mission_without_a_declared_coordination_branch_contributes_no_findings() {
    let fixture = init_repo();
    let root = fixture.root();
    write(
        &root.join("kitty-specs").join("plain").join("meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "mission_id": MISSION_ID,
                "mission_slug": "plain",
                "topology": "lanes",
                "target_branch": "main",
            })
        ),
    );
    commit_all(root, "plain mission");

    let report = findings(root, &CoordinationHealthRequest::new());
    assert_eq!(report.findings().len(), 2, "{:?}", codes(&report));
    assert!(codes(&report).iter().all(Option::is_none));
}

#[test]
fn incomplete_coordination_identity_is_a_warning_not_a_skip() {
    let fixture = init_repo();
    let root = fixture.root();
    write(
        &root.join("kitty-specs").join("half").join("meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "coordination_branch": COORD_BRANCH,
                "mission_slug": "half",
            })
        ),
    );
    commit_all(root, "half mission");

    let report = findings(root, &CoordinationHealthRequest::new());
    let incomplete = finding(&report, COORDINATION_META_INCOMPLETE);
    assert_eq!(incomplete.severity(), Severity::Warning);
    assert!(
        incomplete
            .message()
            .contains("coord worktree health cannot be verified"),
        "{}",
        incomplete.message()
    );
    assert_eq!(
        incomplete.next_step(),
        Some("Run `spec-kitty doctor identity --json` for details.")
    );
}

#[test]
fn staleness_reports_only_the_decidable_arms() {
    // Coord strictly behind target: a fast-forward candidate.
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);
    write(&root.join("target-moves.txt"), "target\n");
    commit_all(root, "target advances");

    let report = findings(root, &CoordinationHealthRequest::new().with_staleness());
    let stale = finding(&report, COORDINATION_BRANCH_STALE_VS_TARGET);
    assert_eq!(
        stale.message(),
        format!(
            "Coordination branch '{COORD_BRANCH}' is behind target branch 'main' (fast-forward available)."
        )
    );
    assert_eq!(
        stale.next_step(),
        Some("Run `spec-kitty doctor coordination --fix` to fast-forward it.")
    );
    // --check-staleness stays a reporting toggle; nothing is left uncomposed.
    assert!(report.unavailable().is_empty());
    assert_eq!(report.exit_code(), Some(0));
}

#[test]
fn a_coordination_branch_ahead_of_target_is_expected_and_leaves_no_finding() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);
    // A coordination-only commit past the target.
    write(&worktree.join("coord-only.txt"), "coord\n");
    commit_all(&worktree, "coordination commit");

    let report = findings(root, &CoordinationHealthRequest::new().with_staleness());
    assert!(
        !codes(&report).contains(&Some(COORDINATION_BRANCH_STALE_VS_TARGET)),
        "a coordination branch ahead of target is by design: {:?}",
        codes(&report)
    );
    assert!(
        !codes(&report).contains(&Some(
            sk_workspace::coordination_health::COORDINATION_BRANCH_DIVERGED_VS_TARGET
        )),
        "no invented divergence verdict: {:?}",
        codes(&report)
    );
}

#[test]
fn a_diverged_pair_is_classified_through_the_expected_divergence_owner() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);
    // Diverge for real: one commit on the coordination branch, one on main.
    // The coordination commit writes a NON-coordination path, so the canonical
    // exemption refuses it and the reference reports genuine divergence.
    write(&worktree.join("coord.txt"), "coord\n");
    commit_all(&worktree, "coord side");
    write(&root.join("main.txt"), "main\n");
    commit_all(root, "main side");

    let report = findings(root, &CoordinationHealthRequest::new().with_staleness());
    let diverged = finding(
        &report,
        sk_workspace::coordination_health::COORDINATION_BRANCH_DIVERGED_VS_TARGET,
    );
    assert_eq!(
        diverged.message(),
        format!(
            "Coordination branch '{COORD_BRANCH}' has diverged from target branch 'main' and cannot be fast-forwarded automatically."
        )
    );
    assert!(
        !codes(&report).contains(&Some(COORDINATION_BRANCH_STALE_VS_TARGET)),
        "a diverged pair is not a fast-forward candidate: {:?}",
        codes(&report)
    );
    assert_eq!(report.exit_code(), Some(0), "a divergence is a warning");
}

#[test]
fn the_mission_handle_scopes_the_per_mission_checks() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);
    // A second coordinated Mission with a missing worktree: it must NOT appear
    // when the handle scopes the run to the first Mission.
    write(
        &root.join("kitty-specs").join("other").join("meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5002",
                "mission_slug": "other",
                "topology": "coord",
                "target_branch": "main",
                "coordination_branch": "kitty/mission-other-01KNXQS9",
            })
        ),
    );
    git(root, &["branch", "kitty/mission-other-01KNXQS9"]);
    commit_all(root, "second mission");

    let scoped = findings(root, &CoordinationHealthRequest::new().with_mission(SLUG));
    assert_eq!(scoped.findings().len(), 4, "{:?}", codes(&scoped));
    assert!(
        !codes(&scoped).contains(&Some(COORDINATION_WORKTREE_MISSING)),
        "{:?}",
        codes(&scoped)
    );

    let unscoped = findings(root, &CoordinationHealthRequest::new());
    assert!(
        codes(&unscoped).contains(&Some(COORDINATION_WORKTREE_MISSING)),
        "the unscoped sweep sees both Missions: {:?}",
        codes(&unscoped)
    );
}

#[test]
fn an_unknown_mission_handle_fails_closed() {
    let (fixture, _worktree) = coordinated_repo();
    let root = fixture.root();
    let workspace = ProjectWorkspace::open(root).expect("repository");
    let error = coordination_health(
        &workspace,
        &CoordinationHealthRequest::new().with_mission("no-such-mission"),
    )
    .expect_err("an unknown handle is an error");
    match error {
        CoordinationHealthError::Mission(sk_mission::MissionError::MissionNotFound {
            selector,
        }) => {
            assert_eq!(selector, "no-such-mission");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_corrupt_manifest_is_skipped_the_way_the_reference_skips_it() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    add_coord_worktree(root, &worktree);
    write(
        &root.join("kitty-specs").join("broken").join("meta.json"),
        "{[not json",
    );

    let report = findings(root, &CoordinationHealthRequest::new());
    // Git version + tracked-worktrees + the one healthy Mission's worktree row and
    // its lane sparse `ok` row; the corrupt Mission contributes nothing and does
    // not abort the sweep.
    assert_eq!(report.findings().len(), 4, "{:?}", codes(&report));
}

#[test]
fn an_unreadable_coordination_worktree_directory_is_not_a_false_healthy_verdict() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    add_coord_branch(root);
    // A plain directory (not a registered worktree) at the canonical path: Git
    // resolves its HEAD by walking up to the primary checkout, exactly like the
    // reference's `git -C <worktree> symbolic-ref HEAD`.
    fs::create_dir_all(&worktree).expect("husk");
    let report = findings(root, &CoordinationHealthRequest::new());
    let mismatch = finding(&report, COORDINATION_WORKTREE_BRANCH_MISMATCH);
    assert!(
        mismatch.message().contains("is on 'refs/heads/main'"),
        "{}",
        mismatch.message()
    );
}
