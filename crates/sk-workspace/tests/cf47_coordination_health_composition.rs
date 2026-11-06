//! CF47: the composed read-only coordination-health service.
//!
//! Every fixture is a real `tempfile` repository with real commits, branches and
//! linked worktrees. The assertions name observed strings/values the pinned
//! reference emits (see the lane report's Source receipts) — no OID, path, lane or
//! marker value is fabricated.
//!
//! Coverage, one assertion group per formerly-unavailable check plus the
//! caught-vs-escaped phases the reference keeps apart:
//!
//! * lane sparse-checkout drift: drift, healthy row, and the whole-check ordering;
//! * stranded coordination `done`: live error, pruned-worktree error, stale marker
//!   (no finding), `null` marker (skipped), unparseable marker (warning);
//! * the marker sweep's escaped phase (a non-object top-level `state.json`);
//! * the committed-DONE reader: derived, absent ref, unsupported ref spelling, and
//!   an undecodable log (the reference's `errors="replace"` → decode-refusal path);
//! * the expected-divergence exemption (coord-only history leaves no verdict);
//! * the whole run is read-only (byte footprint unchanged).
//!
//! Not claimed: `--fix` (refused by the adapter), the reference's substring-of-
//! porcelain registration observation when `git worktree list` fails, unbounded
//! pattern/event-log reads, and platform/performance behaviour. Those are recorded
//! as debt in the report.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sk_workspace::coherence::{CoordDoneReadError, coord_incoherent_done_wps};
use sk_workspace::coordination_health::{
    COORDINATION_BRANCH_DIVERGED_VS_TARGET, COORDINATION_BRANCH_STALE_VS_TARGET,
    COORDINATION_RECONCILE_MARKER_UNPARSEABLE, COORDINATION_STRANDED_COORD_REVERT,
    COORDINATION_STRANDED_COORD_REVERT_STUCK, COORDINATION_WORKTREE_DIRTY, CoordinationFinding,
    CoordinationHealthError, CoordinationHealthReport, CoordinationHealthRequest, Severity,
    TRACKED_WORKTREES_CONTENT, coordination_health,
};
use sk_workspace::workspace::ProjectWorkspace;

use support::{Fixture, commit_all, git, init_repo, write};

const SLUG: &str = "cf47-mission";
const MISSION_ID: &str = "01CF47AAATWWFXS3K5ZJ9E5001";
const MID8: &str = "01CF47AA";
const COORD_BRANCH: &str = "kitty/mission-cf47-mission-01CF47AA";
const COORD_DIR: &str = "cf47-mission-01CF47AA-coord";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

#[test]
fn the_workspace_test_binary_runs_inside_the_repository() {
    assert!(repo_root().join("Cargo.toml").is_file());
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn event(wp_id: &str, from_lane: &str, to_lane: &str, at: &str, event_id: &str) -> Value {
    json!({
        "actor": "cf47",
        "at": at,
        "event_id": event_id,
        "evidence": null,
        "execution_mode": "worktree",
        "force": true,
        "from_lane": from_lane,
        "mission_slug": SLUG,
        "reason": "cf47 fixture",
        "review_ref": null,
        "to_lane": to_lane,
        "wp_id": wp_id,
    })
}

fn render(events: &[Value]) -> String {
    let mut text = String::new();
    for row in events {
        text.push_str(&serde_json::to_string(row).expect("serialize event"));
        text.push('\n');
    }
    text
}

fn mission_dir(root: &Path) -> PathBuf {
    root.join("kitty-specs").join(SLUG)
}

fn log_path(root: &Path) -> PathBuf {
    mission_dir(root).join("status.events.jsonl")
}

/// A repository with one coordinated Mission and a real coordination worktree.
fn coordinated_repo() -> (Fixture, PathBuf) {
    let fixture = init_repo();
    let root = fixture.root();
    write(&root.join(".gitignore"), ".worktrees/\n");
    write(
        &mission_dir(root).join("meta.json"),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "topology": "coord",
                "target_branch": "main",
                "coordination_branch": COORD_BRANCH,
                "mission_type": "software-dev",
            }))
            .expect("meta JSON")
        ),
    );
    commit_all(root, "mission");

    git(root, &["branch", COORD_BRANCH]);
    fs::create_dir_all(fixture.path(".worktrees")).expect("worktrees dir");
    let worktree = fixture.path(".worktrees").join(COORD_DIR);
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
    (fixture, worktree)
}

fn write_state(root: &Path, payload: &str) {
    let directory = root
        .join(".kittify")
        .join("runtime")
        .join("merge")
        .join(MISSION_ID);
    fs::create_dir_all(&directory).expect("merge runtime dir");
    fs::write(directory.join("state.json"), payload).expect("state.json");
}

fn marker_payload(
    coord_ref: &str,
    captured_sha: &str,
    coord_worktree: &Path,
    stranded: &[&str],
    strand_shas: Option<&[&str]>,
) -> String {
    let mut marker = json!({
        "coord_ref": coord_ref,
        "captured_sha": captured_sha,
        "coord_worktree": coord_worktree.display().to_string(),
        "stranded_wp_ids": stranded,
    });
    if let Some(shas) = strand_shas {
        marker
            .as_object_mut()
            .expect("marker object")
            .insert("strand_shas".to_owned(), json!(shas));
    }
    format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "mission_id": MISSION_ID,
            "mission_slug": SLUG,
            "target_branch": "main",
            "wp_order": ["WP01"],
            "pending_coord_reconcile": marker,
        }))
        .expect("state JSON")
    )
}

/// Commit a `done` status log on `COORD_BRANCH`, from the branch's own worktree
/// (a branch checked out in a worktree cannot be force-moved).
fn commit_done_log(root: &Path, worktree: &Path) -> String {
    let events = [
        event(
            "WP01",
            "planned",
            "claimed",
            "2026-11-06T00:00:01+00:00",
            "01A",
        ),
        event(
            "WP01",
            "claimed",
            "done",
            "2026-11-06T00:00:02+00:00",
            "01B",
        ),
    ];
    let log = worktree
        .join("kitty-specs")
        .join(SLUG)
        .join("status.events.jsonl");
    fs::create_dir_all(log.parent().expect("log parent")).expect("log dir");
    write(&log, &render(&events));
    commit_all(worktree, "coordination status log");
    git(root, &["rev-parse", "refs/heads/main"])
}

fn diagnose(root: &Path, request: &CoordinationHealthRequest) -> CoordinationHealthReport {
    let workspace = ProjectWorkspace::open(root).expect("fixture repository");
    coordination_health(&workspace, request).expect("diagnosis answers")
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
        .unwrap_or_else(|| panic!("no {code} finding: {:?}", codes(report)))
}

/// Every file under the fixture, byte for byte.
fn footprint(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("read directory")
            .map(|entry| entry.expect("directory entry"))
            .collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((path.display().to_string(), bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// A lane worktree of this Mission with the given sparse-checkout body.
fn lane_worktree(fixture: &Fixture, lane: &str, sparse_body: &str) -> PathBuf {
    let root = fixture.root();
    let branch = format!("{SLUG}-lane-{lane}");
    git(root, &["branch", &branch]);
    let lane_dir = fixture
        .path(".worktrees")
        .join(format!("{SLUG}-lane-{lane}"));
    git(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            lane_dir.to_str().expect("utf-8 lane path"),
            &branch,
        ],
    );
    let sparse = git(
        &lane_dir,
        &["rev-parse", "--git-path", "info/sparse-checkout"],
    );
    let sparse_path = PathBuf::from(&sparse);
    let sparse_path = if sparse_path.is_absolute() {
        sparse_path
    } else {
        lane_dir.join(sparse_path)
    };
    fs::create_dir_all(sparse_path.parent().expect("sparse parent")).expect("sparse dir");
    fs::write(&sparse_path, sparse_body).expect("sparse body");
    lane_dir
}

// ---------------------------------------------------------------------------
// lane sparse-checkout drift (formerly an unavailable check)
// ---------------------------------------------------------------------------

#[test]
fn lane_sparse_drift_is_reported_with_the_reference_text_and_payload() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    let lane = lane_worktree(&fixture, "a", "/*\n# a comment\n\n");
    // A healthy sibling lane must not produce a row of its own.
    lane_worktree(
        &fixture,
        "b",
        &format!(
            "/*\n!kitty-specs/{SLUG}-{MID8}/status.events.jsonl\n!kitty-specs/{SLUG}-{MID8}/status.json\n"
        ),
    );

    let report = diagnose(root, &CoordinationHealthRequest::new());
    assert!(report.is_complete(), "every check ran");
    let drift = finding(&report, "LANE_SPARSE_CHECKOUT_DRIFT");
    assert_eq!(drift.severity(), Severity::Warning);
    assert_eq!(
        drift.message(),
        format!(
            "Lane worktree {} sparse-checkout is missing 2 expected pattern(s): ['!kitty-specs/{SLUG}-{MID8}/status.events.jsonl', '!kitty-specs/{SLUG}-{MID8}/status.json'].",
            lane.display()
        )
    );
    assert_eq!(
        drift.next_step(),
        Some("Run `spec-kitty doctor workspaces --fix` to restore.")
    );
    assert_eq!(
        drift.extra()["missing_patterns"],
        json!([
            format!("!kitty-specs/{SLUG}-{MID8}/status.events.jsonl"),
            format!("!kitty-specs/{SLUG}-{MID8}/status.json"),
        ])
    );

    // A clean Mission reports the reference's single `ok` row instead.
    let (clean_fixture, _) = coordinated_repo();
    lane_worktree(
        &clean_fixture,
        "a",
        &format!(
            "/*\n!kitty-specs/{SLUG}-{MID8}/status.events.jsonl\n!kitty-specs/{SLUG}-{MID8}/status.json\n"
        ),
    );
    let clean = diagnose(clean_fixture.root(), &CoordinationHealthRequest::new());
    assert!(
        !codes(&clean).contains(&Some("LANE_SPARSE_CHECKOUT_DRIFT")),
        "{:?}",
        codes(&clean)
    );
    assert!(
        clean.findings().iter().any(|finding| finding.message()
            == "All lane worktrees carry the expected sparse-checkout policy."),
        "{:?}",
        codes(&clean)
    );
    let _ = worktree;
}

#[test]
fn a_missing_sparse_policy_file_is_its_own_reference_finding() {
    let (fixture, _) = coordinated_repo();
    let root = fixture.root();
    let lane = lane_worktree(&fixture, "a", "");
    // Remove the pattern file the scan would read.
    let sparse = git(&lane, &["rev-parse", "--git-path", "info/sparse-checkout"]);
    let sparse_path = PathBuf::from(&sparse);
    let sparse_path = if sparse_path.is_absolute() {
        sparse_path
    } else {
        lane.join(sparse_path)
    };
    fs::remove_file(&sparse_path).expect("remove pattern file");

    let report = diagnose(root, &CoordinationHealthRequest::new());
    let drift = finding(&report, "LANE_SPARSE_CHECKOUT_DRIFT");
    assert_eq!(
        drift.message(),
        format!(
            "Lane worktree {} is missing the sparse-checkout policy that excludes status files.",
            lane.display()
        )
    );
    assert_eq!(
        drift.next_step(),
        Some("Run `spec-kitty doctor workspaces --fix` to restore.")
    );
}

// ---------------------------------------------------------------------------
// stranded coordination `done` (formerly an unavailable check)
// ---------------------------------------------------------------------------

#[test]
fn a_live_strand_is_an_error_with_the_reference_payload() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    let captured = commit_done_log(root, &worktree);
    write_state(
        root,
        &marker_payload(COORD_BRANCH, &captured, &worktree, &["WP01"], Some(&[])),
    );

    let report = diagnose(root, &CoordinationHealthRequest::new());
    assert!(report.is_complete());
    let stranded = finding(&report, COORDINATION_STRANDED_COORD_REVERT);
    assert_eq!(stranded.severity(), Severity::Error);
    assert_eq!(
        stranded.message(),
        format!(
            "Coordination ref '{COORD_BRANCH}' for mission '{SLUG}' still reduces WP(s) ['WP01'] to `done` after a merge rollback (expected `approved`)."
        )
    );
    assert_eq!(
        stranded.next_step(),
        Some(
            "Run `spec-kitty doctor coordination --fix` to revert the stranded coordination `done` commit(s) and clear the reconcile marker."
        )
    );
    assert_eq!(stranded.extra()["mission_id"], json!(MISSION_ID));
    assert_eq!(stranded.extra()["mission_slug"], json!(SLUG));
    assert_eq!(stranded.extra()["coord_ref"], json!(COORD_BRANCH));
    assert_eq!(stranded.extra()["captured_sha"], json!(captured));
    assert_eq!(
        stranded.extra()["coord_worktree"],
        json!(worktree.display().to_string())
    );
    assert_eq!(stranded.extra()["candidate_wps"], json!(["WP01"]));
    assert_eq!(stranded.extra()["stranded_wp_ids"], json!(["WP01"]));
    assert_eq!(stranded.extra()["strand_shas"], json!([]));
    assert_eq!(report.exit_code(), Some(1));
}

#[test]
fn a_live_strand_whose_worktree_is_gone_is_stuck_and_stays_an_error() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    let captured = commit_done_log(root, &worktree);
    let missing = root.join(".worktrees").join("gone-coord");
    write_state(
        root,
        &marker_payload(COORD_BRANCH, &captured, &missing, &["WP01"], None),
    );

    let report = diagnose(root, &CoordinationHealthRequest::new());
    let stuck = finding(&report, COORDINATION_STRANDED_COORD_REVERT_STUCK);
    assert_eq!(stuck.severity(), Severity::Error);
    assert_eq!(
        stuck.message(),
        format!(
            "Coordination ref '{COORD_BRANCH}' for mission '{SLUG}' still strands WP(s) ['WP01'] at `done`, but its coordination worktree '{}' no longer exists — `--fix` cannot revert it.",
            missing.display()
        )
    );
    assert!(
        stuck
            .next_step()
            .unwrap_or_default()
            .starts_with("The coordination worktree recorded for this strand no longer exists"),
        "{:?}",
        stuck.next_step()
    );
    // A marker with no recorded strand SHAs reads as `null`, exactly as the
    // reference's `marker_strand_shas` does.
    assert_eq!(stuck.extra()["strand_shas"], Value::Null);
    assert_eq!(report.exit_code(), Some(1));
}

#[test]
fn a_stale_marker_derives_no_finding_and_a_null_marker_is_skipped() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    let captured = commit_done_log(root, &worktree);
    // `WP02` is not in the committed log's write-set, so there is no strand.
    write_state(
        root,
        &marker_payload(COORD_BRANCH, &captured, &worktree, &["WP02"], None),
    );
    let report = diagnose(root, &CoordinationHealthRequest::new());
    assert!(
        !codes(&report)
            .iter()
            .any(|code| code.is_some_and(|code| code.starts_with("COORDINATION_STRANDED"))),
        "{:?}",
        codes(&report)
    );
    assert!(report.is_complete());

    // A `null` marker is not a marker at all.
    write_state(
        root,
        &format!(
            "{}\n",
            json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "target_branch": "main",
                "wp_order": ["WP01"],
                "pending_coord_reconcile": Value::Null,
            })
        ),
    );
    let after_null = diagnose(root, &CoordinationHealthRequest::new());
    assert!(after_null.is_complete());
    assert!(
        !codes(&after_null)
            .iter()
            .any(|code| code.is_some_and(|code| code.starts_with("COORDINATION_"))),
        "{:?}",
        codes(&after_null)
    );
}

#[test]
fn an_unparseable_marker_is_a_warning_that_names_only_the_mission() {
    let (fixture, _) = coordinated_repo();
    let root = fixture.root();
    write_state(
        root,
        &format!(
            "{}\n",
            json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "target_branch": "main",
                "wp_order": ["WP01"],
                "pending_coord_reconcile": {},
            })
        ),
    );

    let report = diagnose(root, &CoordinationHealthRequest::new());
    let unparseable = finding(&report, COORDINATION_RECONCILE_MARKER_UNPARSEABLE);
    assert_eq!(unparseable.severity(), Severity::Warning);
    assert_eq!(
        unparseable.message(),
        format!(
            "Mission '{SLUG}' carries a `pending_coord_reconcile` marker that could not be parsed into repair inputs."
        )
    );
    assert_eq!(
        unparseable.extra().keys().cloned().collect::<Vec<_>>(),
        vec!["mission_id".to_owned(), "mission_slug".to_owned()]
    );
    assert_eq!(unparseable.extra()["mission_id"], json!(MISSION_ID));
    assert_eq!(unparseable.extra()["mission_slug"], json!(SLUG));
    assert_eq!(report.exit_code(), Some(0));
}

// ---------------------------------------------------------------------------
// the sweep's caught vs escaped phases
// ---------------------------------------------------------------------------

#[test]
fn a_malformed_state_file_is_skipped_but_a_non_object_top_level_escapes() {
    let (fixture, _) = coordinated_repo();
    let root = fixture.root();

    // Caught: invalid JSON (`json.JSONDecodeError`) — the reference skips it.
    write_state(root, "{not json");
    let report = diagnose(root, &CoordinationHealthRequest::new());
    assert!(report.is_complete());
    assert!(
        !codes(&report)
            .iter()
            .any(|code| code.is_some_and(|code| code.starts_with("COORDINATION_RECONCILE"))),
        "{:?}",
        codes(&report)
    );

    // Escaped: a JSON array raises `AttributeError` on `data.items()` and aborts
    // the reference's sweep. It must not be read as "no marker"/healthy.
    write_state(root, "[1, 2]\n");
    let workspace = ProjectWorkspace::open(root).expect("fixture repository");
    let error = coordination_health(&workspace, &CoordinationHealthRequest::new())
        .expect_err("an escaped top level is not a report");
    match error {
        CoordinationHealthError::MarkerSweep(inner) => {
            assert!(
                inner.to_string().contains("must be a JSON object"),
                "{inner}"
            );
        }
        other => panic!("unexpected error: {other}"),
    }
}

// ---------------------------------------------------------------------------
// committed-DONE reader (the consolidated coord authority)
// ---------------------------------------------------------------------------

#[test]
fn the_committed_done_reader_keeps_the_reference_caught_and_escaped_phases() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    commit_done_log(root, &worktree);
    let feature_dir = mission_dir(root);
    let candidates = vec!["WP01".to_owned(), "WP02".to_owned()];

    // Derived from the committed ref, in candidate order.
    assert_eq!(
        coord_incoherent_done_wps(COORD_BRANCH, &candidates, root, &feature_dir)
            .expect("readable ref"),
        vec!["WP01".to_owned()]
    );
    // No candidates short-circuits before any Git read.
    assert!(
        coord_incoherent_done_wps(COORD_BRANCH, &[], Path::new("/nope"), &feature_dir)
            .expect("no read")
            .is_empty()
    );
    // Caught arm: an absent branch is "no events", exactly like a non-zero
    // `git show`.
    assert!(
        coord_incoherent_done_wps(
            "kitty/mission-absent-01CF47AA",
            &candidates,
            root,
            &feature_dir
        )
        .expect("absent ref reads as no events")
        .is_empty()
    );
    // Source breadth: `git show <any revision>` — a relative revision is read, and
    // its absence of a log is the caught arm (no events), not a refusal.
    assert!(
        coord_incoherent_done_wps("HEAD~1", &candidates, root, &feature_dir)
            .expect("a Source-valid revision is read")
            .is_empty()
    );
    // An operand that can never be a Git revision argument is the typed refusal.
    assert!(matches!(
        coord_incoherent_done_wps("--output=/tmp/x", &candidates, root, &feature_dir),
        Err(CoordDoneReadError::UnresolvableCoordRef { .. })
    ));
    // Not a repository at all: Source's `git show` fails, so "no events".
    assert!(
        coord_incoherent_done_wps(COORD_BRANCH, &candidates, Path::new("/nope"), &feature_dir)
            .expect("unreadable repository reads as no events")
            .is_empty()
    );

    // Escaped arm: undecodable bytes become U+FFFD (the reference's
    // `errors="replace"`), which leaves invalid JSON, so the read is refused —
    // never "no events".
    let log = worktree
        .join("kitty-specs")
        .join(SLUG)
        .join("status.events.jsonl");
    fs::write(&log, b"{\"wp_id\": \xff}\n").expect("non-UTF-8 log");
    commit_all(&worktree, "undecodable status log");
    assert!(matches!(
        coord_incoherent_done_wps(COORD_BRANCH, &candidates, root, &feature_dir),
        Err(CoordDoneReadError::MalformedLog { .. })
    ));
}

// ---------------------------------------------------------------------------
// expected divergence, whole-run order, read-only
// ---------------------------------------------------------------------------

#[test]
fn an_expected_coordination_divergence_leaves_no_divergence_verdict() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    // A coordination-only commit past the target: the status log is COORD-partition
    // for this Mission, so the canonical exemption is expected.
    write(
        &log_path(&worktree),
        &render(&[event(
            "WP01",
            "planned",
            "claimed",
            "2026-11-06T00:00:01+00:00",
            "01A",
        )]),
    );
    commit_all(&worktree, "coordination-only commit");
    // The target moves on independently, so the pair genuinely diverges.
    write(&root.join("target.txt"), "target\n");
    commit_all(root, "target side");

    let report = diagnose(root, &CoordinationHealthRequest::new().with_staleness());
    assert!(
        !codes(&report).contains(&Some(COORDINATION_BRANCH_DIVERGED_VS_TARGET)),
        "the expected-divergence owner suppresses the verdict: {:?}",
        codes(&report)
    );
    assert!(!codes(&report).contains(&Some(COORDINATION_BRANCH_STALE_VS_TARGET)));
    assert_eq!(report.exit_code(), Some(0));
}

#[test]
fn the_check_order_matches_the_reference_and_the_run_writes_nothing() {
    let (fixture, worktree) = coordinated_repo();
    let root = fixture.root();
    // A tracked `.worktrees/` path (repo-level error), a dirty coordination
    // worktree, a lane sparse drift, and a coordination branch behind the target.
    write(&fixture.path(".worktrees").join("stray.txt"), "scratch\n");
    git(root, &["add", "-f", ".worktrees/stray.txt"]);
    commit_all(root, "track scratch");
    write(&worktree.join("scratch.txt"), "in flight\n");
    lane_worktree(&fixture, "a", "/*\n");
    write(&root.join("target.txt"), "target\n");
    commit_all(root, "target advances");

    let before = footprint(root);
    let report = diagnose(root, &CoordinationHealthRequest::new().with_staleness());
    let after = footprint(root);
    assert_eq!(before, after, "the diagnosis must write nothing");

    assert_eq!(
        codes(&report),
        vec![
            None,
            Some(TRACKED_WORKTREES_CONTENT),
            Some(COORDINATION_WORKTREE_DIRTY),
            Some("LANE_SPARSE_CHECKOUT_DRIFT"),
            Some(COORDINATION_BRANCH_STALE_VS_TARGET),
        ],
        "{:?}",
        report.findings()
    );
    assert_eq!(report.exit_code(), Some(1));
    assert!(report.is_complete());
}

#[test]
fn a_mission_without_a_coordination_branch_contributes_nothing() {
    let fixture = init_repo();
    let root = fixture.root();
    write(
        &mission_dir(root).join("meta.json"),
        &format!(
            "{}\n",
            json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "target_branch": "main",
                "mission_type": "software-dev",
            })
        ),
    );
    commit_all(root, "flat mission");

    let report = diagnose(root, &CoordinationHealthRequest::new().with_staleness());
    assert_eq!(codes(&report), vec![None, None], "{:?}", codes(&report));
    assert!(report.is_complete());
    assert_eq!(report.exit_code(), Some(0));
}
