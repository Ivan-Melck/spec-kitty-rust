//! CF40: the committed candidate-DONE reducer against real Git refs.
//!
//! Every fixture is a real `tempfile` repository: real commits, real branch refs and
//! a real `kitty-specs/<mission>/status.events.jsonl` committed on each ref. The
//! assertions name the observed refs/tips or compare the reducer's answer with the
//! log's own reduction — no OID, path or lane is fabricated.
//!
//! Not claimed here: Source's `git show <any-revision>` breadth (a tag, a raw OID or
//! `HEAD` as the ref), the strict per-event `StoreError` validation of
//! `read_events_from_text`, and indefinite event-log sizes are recorded as remaining
//! debt in the lane report; the exercised contract is the documented one — the
//! coordination branch the reconcile marker records.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sk_workspace::coherence::{CoordDoneReadError, coord_incoherent_done_wps};
use sk_workspace::consolidation::state::{iter_pending_coord_reconcile_markers, state_path};

use support::{Fixture, commit_all, git, init_repo, write};

const SLUG: &str = "cf40-mission";
const COORD_BRANCH: &str = "kitty/mission-cf40-mission-01KNXQS9";

fn event(wp_id: &str, from_lane: &str, to_lane: &str, at: &str, event_id: &str) -> Value {
    json!({
        "actor": "claude",
        "at": at,
        "event_id": event_id,
        "evidence": null,
        "execution_mode": "worktree",
        "force": false,
        "from_lane": from_lane,
        "mission_slug": SLUG,
        "reason": null,
        "review_ref": null,
        "to_lane": to_lane,
        "wp_id": wp_id,
    })
}

fn forced_event(wp_id: &str, from_lane: &str, to_lane: &str, at: &str, event_id: &str) -> Value {
    let mut row = event(wp_id, from_lane, to_lane, at, event_id);
    row.as_object_mut()
        .expect("object")
        .insert("force".to_owned(), Value::Bool(true));
    row
}

fn render(events: &[Value]) -> String {
    let mut text = String::new();
    for row in events {
        text.push_str(&serde_json::to_string(row).expect("serialize event"));
        text.push('\n');
    }
    text
}

fn log_path(root: &Path, mission_dir_name: &str) -> PathBuf {
    root.join("kitty-specs")
        .join(mission_dir_name)
        .join("status.events.jsonl")
}

fn feature_dir(root: &Path, mission_dir_name: &str) -> PathBuf {
    root.join("kitty-specs").join(mission_dir_name)
}

/// Write the log, commit it, and mint `branch` at that commit.
fn seed_branch(root: &Path, mission_dir_name: &str, events: &[Value], branch: &str) -> String {
    write(&log_path(root, mission_dir_name), &render(events));
    commit_all(root, "seed status log");
    git(root, &["branch", branch]);
    git(root, &["rev-parse", &format!("refs/heads/{branch}")])
}

/// Append a log commit and move `branch` to it.
fn advance_branch(root: &Path, mission_dir_name: &str, events: &[Value], branch: &str) -> String {
    write(&log_path(root, mission_dir_name), &render(events));
    commit_all(root, "advance status log");
    git(root, &["branch", "-f", branch]);
    git(root, &["rev-parse", &format!("refs/heads/{branch}")])
}

fn wps(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn derive(
    fixture: &Fixture,
    branch: &str,
    candidates: &[&str],
    mission_dir_name: &str,
) -> Vec<String> {
    coord_incoherent_done_wps(
        branch,
        &wps(candidates),
        fixture.root(),
        &feature_dir(fixture.root(), mission_dir_name),
    )
    .expect("committed ref is readable")
}

/// Recursive worktree snapshot excluding `.git` (tracked by refs/index separately).
fn worktree_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("read directory")
            .map(|entry| entry.expect("directory entry"))
            .collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            if entry.file_name() == ".git" {
                continue;
            }
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
fn cf40_derives_only_the_stranded_done_wp() {
    let fixture = init_repo();
    let root = fixture.root();
    seed_branch(
        root,
        SLUG,
        &[
            event(
                "WP-A",
                "in_review",
                "approved",
                "2026-11-05T10:00:00+00:00",
                "01A00",
            ),
            event(
                "WP-A",
                "approved",
                "done",
                "2026-11-05T10:05:00+00:00",
                "01A01",
            ),
            event(
                "WP-B",
                "in_review",
                "approved",
                "2026-11-05T10:00:00+00:00",
                "01B00",
            ),
        ],
        COORD_BRANCH,
    );

    let result = derive(&fixture, COORD_BRANCH, &["WP-A", "WP-B"], SLUG);
    assert_eq!(result, vec!["WP-A"]);
    // Anti-fakeability: neither a hardcoded id nor a candidate passthrough.
    assert_ne!(result, vec!["WP01"]);
    assert_ne!(result, vec!["WP-A", "WP-B"]);
}

#[test]
fn cf40_excludes_a_pre_existing_done_wp_outside_the_write_set() {
    let fixture = init_repo();
    let root = fixture.root();
    seed_branch(
        root,
        SLUG,
        &[
            // A genuinely-done WP from a prior merge, outside this write-set.
            event(
                "WP-C",
                "approved",
                "done",
                "2026-11-05T09:00:00+00:00",
                "01C01",
            ),
            event(
                "WP-A",
                "approved",
                "done",
                "2026-11-05T10:05:00+00:00",
                "01A01",
            ),
            event(
                "WP-B",
                "in_review",
                "approved",
                "2026-11-05T10:00:00+00:00",
                "01B00",
            ),
        ],
        COORD_BRANCH,
    );

    // WP-C really is done on the ref, so its exclusion below is by write-set.
    assert_eq!(
        derive(&fixture, COORD_BRANCH, &["WP-C"], SLUG),
        vec!["WP-C"]
    );
    assert_eq!(
        derive(&fixture, COORD_BRANCH, &["WP-A", "WP-B"], SLUG),
        vec!["WP-A"]
    );
}

#[test]
fn cf40_preserves_candidate_order_and_duplicates() {
    let fixture = init_repo();
    let root = fixture.root();
    seed_branch(
        root,
        SLUG,
        &[
            event(
                "WP-A",
                "approved",
                "done",
                "2026-11-05T10:05:00+00:00",
                "01A01",
            ),
            event(
                "WP-B",
                "approved",
                "done",
                "2026-11-05T10:06:00+00:00",
                "01B01",
            ),
            event(
                "WP-D",
                "in_review",
                "approved",
                "2026-11-05T10:00:00+00:00",
                "01D00",
            ),
        ],
        COORD_BRANCH,
    );

    assert_eq!(
        derive(
            &fixture,
            COORD_BRANCH,
            &["WP-B", "WP-D", "WP-A", "WP-A"],
            SLUG
        ),
        vec!["WP-B", "WP-A", "WP-A"]
    );
}

#[test]
fn cf40_reopening_after_done_is_not_stranded_on_the_newer_ref() {
    let fixture = init_repo();
    let root = fixture.root();
    seed_branch(
        root,
        SLUG,
        &[event(
            "WP-A",
            "approved",
            "done",
            "2026-11-05T10:05:00+00:00",
            "01A01",
        )],
        "cf40-before-reopen",
    );
    // A reviewer's reopen leaves `done` for a pre-approval lane, so nothing lifts it.
    advance_branch(
        root,
        SLUG,
        &[
            event(
                "WP-A",
                "approved",
                "done",
                "2026-11-05T10:05:00+00:00",
                "01A01",
            ),
            forced_event(
                "WP-A",
                "done",
                "in_progress",
                "2026-11-05T11:00:00+00:00",
                "01A02",
            ),
        ],
        "cf40-after-reopen",
    );

    // Two real commits, two real refs: the strand is only on the older one.
    assert_eq!(
        derive(&fixture, "cf40-before-reopen", &["WP-A"], SLUG),
        vec!["WP-A"]
    );
    assert_eq!(
        derive(&fixture, "cf40-after-reopen", &["WP-A"], SLUG),
        Vec::<String>::new()
    );
}

#[test]
fn cf40_reapproval_does_not_clear_the_strand_matching_the_diary_fold() {
    // The canonical diary fold lifts a departure FROM `approved` to just after its
    // arrival, so a forced `done -> approved` reopen re-orders the `done` departure
    // after the new approval: the WP still reduces to `done`. An isolated pinned
    // reference observation (`specify_cli.status.reduce`, 2026-11-06) reports the
    // same lane for the same two rows, so this is behaviour parity, not a guess.
    let fixture = init_repo();
    let root = fixture.root();
    seed_branch(
        root,
        SLUG,
        &[
            event(
                "WP-A",
                "approved",
                "done",
                "2026-11-05T10:05:00+00:00",
                "01A01",
            ),
            forced_event(
                "WP-A",
                "done",
                "approved",
                "2026-11-05T11:00:00+00:00",
                "01A02",
            ),
        ],
        "cf40-reapproved",
    );
    assert_eq!(
        derive(&fixture, "cf40-reapproved", &["WP-A"], SLUG),
        vec!["WP-A"]
    );
}

#[test]
fn cf40_accepts_a_short_branch_or_a_fully_qualified_ref() {
    let fixture = init_repo();
    let root = fixture.root();
    seed_branch(
        root,
        SLUG,
        &[event(
            "WP-A",
            "approved",
            "done",
            "2026-11-05T10:05:00+00:00",
            "01A01",
        )],
        COORD_BRANCH,
    );

    let short = derive(&fixture, COORD_BRANCH, &["WP-A"], SLUG);
    let qualified = derive(
        &fixture,
        &format!("refs/heads/{COORD_BRANCH}"),
        &["WP-A"],
        SLUG,
    );
    assert_eq!(short, vec!["WP-A"]);
    assert_eq!(qualified, short);

    // A branch that does not exist is "no events", never an assumed strand.
    assert_eq!(
        derive(&fixture, "no-such-branch", &["WP-A"], SLUG),
        Vec::<String>::new()
    );
    // The same for a resolvable branch whose tree carries no status log.
    git(
        root,
        &[
            "rm",
            "-q",
            &format!("kitty-specs/{SLUG}/status.events.jsonl"),
        ],
    );
    git(root, &["commit", "-q", "-m", "remove the status log"]);
    git(root, &["branch", "cf40-no-log"]);
    assert_eq!(
        derive(&fixture, "cf40-no-log", &["WP-A"], SLUG),
        Vec::<String>::new()
    );
}

#[test]
fn cf40_empty_candidates_short_circuit_before_any_git_read() {
    // Deliberately NOT a repository and NOT a mission directory: reading would refuse.
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    let result =
        coord_incoherent_done_wps("no-such-branch", &[], root, &root.join("kitty-specs/none"))
            .expect("empty candidates never touch Git");
    assert!(result.is_empty());

    // Source shells `git show <anything>`, so a revision spelling like `HEAD` is
    // now read at the reference's own breadth instead of being refused. This
    // fixture has no coordination log at `HEAD`, which is the reference's
    // caught arm: "no events".
    let fixture = init_repo();
    let reached = coord_incoherent_done_wps(
        "HEAD",
        &wps(&["WP-A"]),
        fixture.root(),
        &feature_dir(fixture.root(), SLUG),
    )
    .expect("a Source-valid revision is read, not refused");
    assert!(reached.is_empty(), "{reached:?}");

    // An argument that is not admissible as a revision at all is the remaining
    // typed refusal (it can never be handed to git).
    let error = coord_incoherent_done_wps(
        "--output=/tmp/x",
        &wps(&["WP-A"]),
        fixture.root(),
        &feature_dir(fixture.root(), SLUG),
    )
    .expect_err("an option-shaped operand is refused, not guessed");
    assert!(matches!(
        error,
        CoordDoneReadError::UnresolvableCoordRef { .. }
    ));
}

#[test]
fn cf40_undecodable_committed_logs_refuse_with_their_own_cause() {
    let fixture = init_repo();
    let root = fixture.root();
    let first = seed_branch(
        root,
        SLUG,
        &[event(
            "WP-A",
            "approved",
            "done",
            "2026-11-05T10:05:00+00:00",
            "01A01",
        )],
        "cf40-ok",
    );
    assert!(!first.is_empty());

    // Malformed JSON line.
    write(&log_path(root, SLUG), "{not json\n");
    commit_all(root, "malformed json");
    git(root, &["branch", "cf40-bad-json"]);
    let error = coord_incoherent_done_wps(
        "cf40-bad-json",
        &wps(&["WP-A"]),
        root,
        &feature_dir(root, SLUG),
    )
    .expect_err("malformed log refuses");
    match error {
        CoordDoneReadError::MalformedLog { line, .. } => assert_eq!(line, 1),
        other => panic!("expected a malformed-log refusal, got {other:?}"),
    }

    // A JSON row that is not an object.
    write(&log_path(root, SLUG), "[1, 2, 3]\n");
    commit_all(root, "non-object row");
    git(root, &["branch", "cf40-non-object"]);
    let error = coord_incoherent_done_wps(
        "cf40-non-object",
        &wps(&["WP-A"]),
        root,
        &feature_dir(root, SLUG),
    )
    .expect_err("non-object row refuses");
    assert!(matches!(
        error,
        CoordDoneReadError::MalformedLog { line: 1, .. }
    ));

    // Bytes that are not UTF-8. Source reads with `errors="replace"`, so these
    // become U+FFFD and the refusal comes from the JSON decode — the same phase,
    // never a fail-closed "unreadable" of its own.
    write(&log_path(root, SLUG), "unused\n");
    fs::write(log_path(root, SLUG), [0x7b, 0xff, 0xfe, 0x7d, 0x0a]).expect("write raw bytes");
    commit_all(root, "non-utf8 log");
    git(root, &["branch", "cf40-bad-bytes"]);
    let error = coord_incoherent_done_wps(
        "cf40-bad-bytes",
        &wps(&["WP-A"]),
        root,
        &feature_dir(root, SLUG),
    )
    .expect_err("non-UTF-8 log refuses");
    assert!(matches!(
        error,
        CoordDoneReadError::MalformedLog { line: 1, .. }
    ));

    // A row the diary decoder does not recognise.
    write(&log_path(root, SLUG), "{\"kind\": \"weird\"}\n");
    commit_all(root, "unknown row kind");
    git(root, &["branch", "cf40-unknown-kind"]);
    let error = coord_incoherent_done_wps(
        "cf40-unknown-kind",
        &wps(&["WP-A"]),
        root,
        &feature_dir(root, SLUG),
    )
    .expect_err("unknown row kind refuses");
    assert!(matches!(error, CoordDoneReadError::UndecodableDiary { .. }));
}

#[test]
fn cf40_foreign_mission_log_does_not_leak_into_this_derivation() {
    let fixture = init_repo();
    let root = fixture.root();
    write(
        &log_path(root, "cf40-foreign"),
        &render(&[event(
            "WP-A",
            "approved",
            "done",
            "2026-11-05T10:05:00+00:00",
            "01A01",
        )]),
    );
    write(
        &log_path(root, SLUG),
        &render(&[event(
            "WP-A",
            "in_review",
            "approved",
            "2026-11-05T10:00:00+00:00",
            "01A00",
        )]),
    );
    commit_all(root, "two mission logs");
    git(root, &["branch", "cf40-two-missions"]);

    // The same ref read through another Mission's directory is a different tree path.
    assert_eq!(
        derive(&fixture, "cf40-two-missions", &["WP-A"], SLUG),
        Vec::<String>::new()
    );
    assert_eq!(
        derive(&fixture, "cf40-two-missions", &["WP-A"], "cf40-foreign"),
        vec!["WP-A"]
    );
}

#[test]
fn cf40_reducer_changes_no_ref_index_or_file() {
    let fixture = init_repo();
    let root = fixture.root();
    seed_branch(
        root,
        SLUG,
        &[event(
            "WP-A",
            "approved",
            "done",
            "2026-11-05T10:05:00+00:00",
            "01A01",
        )],
        COORD_BRANCH,
    );

    let head_before = git(root, &["rev-parse", "HEAD"]);
    let refs_before = git(root, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let index_before = fs::read(root.join(".git/index")).expect("index bytes");
    let worktree_before = worktree_snapshot(root);
    let worktrees_before = root.join(".git/worktrees").exists();

    assert_eq!(
        derive(&fixture, COORD_BRANCH, &["WP-A"], SLUG),
        vec!["WP-A"]
    );

    assert_eq!(git(root, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        git(root, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        refs_before
    );
    assert_eq!(
        fs::read(root.join(".git/index")).expect("index bytes"),
        index_before
    );
    assert_eq!(worktree_snapshot(root), worktree_before);
    assert_eq!(root.join(".git/worktrees").exists(), worktrees_before);
}

#[test]
fn cf40_a_stale_marker_is_enumerated_but_derives_no_strand() {
    let fixture = init_repo();
    let root = fixture.root();
    // The committed ref is coherent (only `approved`), yet a marker names WP-A.
    seed_branch(
        root,
        SLUG,
        &[event(
            "WP-A",
            "in_review",
            "approved",
            "2026-11-05T10:00:00+00:00",
            "01A00",
        )],
        COORD_BRANCH,
    );
    let marker_path = state_path(root, "060-marker");
    fs::create_dir_all(marker_path.parent().expect("state parent")).expect("runtime dir");
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&json!({
            "mission_id": "060-marker",
            "mission_slug": SLUG,
            "target_branch": "main",
            "wp_order": ["WP-A"],
            "pending_coord_reconcile": {
                "coord_ref": COORD_BRANCH,
                "captured_sha": git(root, &["rev-parse", "HEAD"]),
                "coord_worktree": root.display().to_string(),
                "stranded_wp_ids": ["WP-A"],
            },
        }))
        .expect("serialize"),
    )
    .expect("write state");

    // The enumerator yields the marker (it does not re-verify; the consumer does).
    let states = iter_pending_coord_reconcile_markers(root);
    assert_eq!(states.len(), 1);
    let parsed = states[0]
        .parse_pending_coord_reconcile()
        .expect("parseable marker");
    assert_eq!(
        parsed.stranded_wp_ids_as_strings().expect("scalar"),
        vec!["WP-A"]
    );

    // Re-deriving from the COMMITTED ref proves the marker stale: no strand.
    let stranded = coord_incoherent_done_wps(
        parsed.coord_ref(),
        &wps(&["WP-A"]),
        root,
        &feature_dir(root, SLUG),
    )
    .expect("readable ref");
    assert!(
        stranded.is_empty(),
        "a coherent ref has no strand to repair"
    );
}
