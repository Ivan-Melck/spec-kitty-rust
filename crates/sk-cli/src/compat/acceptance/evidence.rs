//! The Source implementation-evidence and review-independence axis, ported for
//! `accept` (#6715).
//!
//! This is a port of `specify_cli/consolidation/wp_evidence.py`: the one
//! authority for "may this approved work package land as reviewed?". It is a
//! separate axis from the approval bound — the bound refuses content a lane
//! gained *after* its approval; this axis refuses an approval with **no owned
//! content behind it**, or one recorded by the implementer itself.
//!
//! Per approved (or `done`), non-canceled work package, Source's first match
//! wins:
//!
//! 1. `EVIDENCE_WINDOW_UNCOMPUTABLE` — `lanes.json` records no planning commit
//!    (or the work package sits in no lane).
//! 2. `IMPLEMENTATION_UNREADABLE` — the window's tree diff cannot be read, or a
//!    dependency window cannot be diffed (fail closed).
//! 3. `IMPLEMENTATION_MISSING` — no non-bookkeeping changed path of the window
//!    matches the work package's `owned_files`, and no no-implementation
//!    attestation is in force for it.
//! 4. `REVIEW_NOT_INDEPENDENT` — the approving actor and the implementer of
//!    record are not provably different identities.
//!
//! Everything is read through a canonical owner; nothing here re-implements a
//! parser or a Git process:
//!
//! * Git — [`sk_git::GitRepository`] (`diff_name_status`, `resolve_commit`,
//!   `merge_base`, `commit_parents`, `is_ancestor`, [`sk_git::BranchName`] for
//!   the "is this ref truly absent?" probe).
//! * status — the raw event rows the caller already read, plus
//!   [`sk_status::latest_implementer_actor`] and
//!   [`sk_status::actor_identities_independent`] (the one segment-wise identity
//!   comparison, operator ruling R1).
//! * ownership — [`sk_status::owned_file_covers`] for Source's `fnmatch` half
//!   and the three-line directory-prefix half of `wp_evidence.matches_owned`.
//! * lanes — [`sk_workspace::LanesManifest`] and
//!   [`sk_workspace::lane_branch_name`].
//!
//! # Known duplication (reported, not hidden)
//!
//! The bookkeeping exclusion list below is Source's
//! `lanes.for_review_gate.status_only_excluded_prefixes`. The Native `move-task`
//! gate in `compat::status` carries the same pinned table privately. One of the
//! two should own it (see the task report's owner request); this module cannot
//! edit `compat/status.rs` and `sk-status` is outside this lane's scope.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use sk_git::{BranchName, GitRepository, Oid, StartPoint};
use sk_status::{
    GENERIC_IMPLEMENTATION_ACTORS, actor_full_identity, actor_identities_independent, actor_key,
    is_migration_actor, latest_implementer_actor, owned_file_covers,
};
use sk_workspace::{LanesManifest, lane_branch_name};

/// The neutral evidence code of an approval with no owned content behind it.
pub(super) const IMPLEMENTATION_MISSING: &str = "IMPLEMENTATION_MISSING";
/// The neutral evidence code of a self- (or unprovable-) reviewed approval.
pub(super) const REVIEW_NOT_INDEPENDENT: &str = "REVIEW_NOT_INDEPENDENT";
/// The neutral evidence code of an unreadable evidence window.
pub(super) const IMPLEMENTATION_UNREADABLE: &str = "IMPLEMENTATION_UNREADABLE";
/// The neutral evidence code of a window that cannot be computed at all.
pub(super) const EVIDENCE_WINDOW_UNCOMPUTABLE: &str = "EVIDENCE_WINDOW_UNCOMPUTABLE";

const _NEUTRAL_CODES: [&str; 4] = [
    IMPLEMENTATION_MISSING,
    REVIEW_NOT_INDEPENDENT,
    IMPLEMENTATION_UNREADABLE,
    EVIDENCE_WINDOW_UNCOMPUTABLE,
];

const SINGLE_BRANCH_IMPLEMENTATION_MISSING: &str = "SINGLE_BRANCH_IMPLEMENTATION_MISSING";
const SINGLE_BRANCH_REVIEW_NOT_INDEPENDENT: &str = "SINGLE_BRANCH_REVIEW_NOT_INDEPENDENT";
const SINGLE_BRANCH_EVIDENCE_UNREADABLE: &str = "SINGLE_BRANCH_EVIDENCE_UNREADABLE";
const SINGLE_BRANCH_IMPLEMENTATION_UNREADABLE: &str = "SINGLE_BRANCH_IMPLEMENTATION_UNREADABLE";
const SINGLE_BRANCH_EVIDENCE_WINDOW_UNCOMPUTABLE: &str =
    "SINGLE_BRANCH_EVIDENCE_WINDOW_UNCOMPUTABLE";

/// The CLI flag that records a no-implementation attestation (Source
/// `wp_evidence.ATTEST_NO_IMPLEMENTATION_FLAG`).
pub(super) const ATTEST_NO_IMPLEMENTATION_FLAG: &str = "--attest-no-implementation";
/// The `--attest-no-implementation` companion flag (Source
/// `canceled_attestation.ATTEST_REASON_FLAG`).
pub(super) const ATTEST_REASON_FLAG: &str = "--attest-reason";
/// `policy_metadata` key every operator attestation writes.
pub(super) const ATTESTATION_KEY: &str = "attestation";
/// `policy_metadata[ATTESTATION_KEY]` of a no-implementation attestation: it
/// lifts `IMPLEMENTATION_MISSING` only and is never read as a review.
pub(super) const NO_IMPLEMENTATION: &str = "no_implementation";

/// Lanes a work package may sit in without voiding its attestation.
const ATTESTATION_KEEPING_LANES: [&str; 2] = ["approved", "done"];

/// The membership lanes the axis judges.
const APPROVED_MEMBERSHIP_LANES: [&str; 2] = ["approved", "done"];

const CANNOT_VERIFY: &str = "so implementation evidence cannot be verified.";
const NOT_INDEPENDENT_DETAIL: &str = "was approved without a confirmable independent review (the implementer of record and the approving actor are the same identity, or one of them is not recorded).";

/// The verdict of the axis for one Mission (`wp_evidence.EvidenceReport`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct EvidenceReport {
    /// The refusal text, exactly Source's shape. `None` means the axis passed.
    pub refusal: Option<String>,
    /// Work packages whose `IMPLEMENTATION_MISSING` an attestation lifted.
    pub lifted_wp_ids: BTreeSet<String>,
    /// Work package id → its first (neutral) code; absent means it passed.
    pub verdicts: BTreeMap<String, String>,
}

impl EvidenceReport {
    /// Every distinct code the refusal names, first-seen first.
    pub fn codes(&self) -> Vec<String> {
        let Some(refusal) = &self.refusal else {
            return Vec::new();
        };
        let mut seen: Vec<String> = Vec::new();
        for line in refusal.lines() {
            for code in all_codes() {
                if line.starts_with(&format!("{code}: "))
                    && !seen.iter().any(|known| known.as_str() == code)
                {
                    seen.push(code.to_owned());
                }
            }
        }
        seen
    }
}

/// Every evidence code the axis can name, longest first (Source `_ALL_CODES`).
fn all_codes() -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = vec![
        IMPLEMENTATION_MISSING,
        REVIEW_NOT_INDEPENDENT,
        IMPLEMENTATION_UNREADABLE,
        EVIDENCE_WINDOW_UNCOMPUTABLE,
        SINGLE_BRANCH_IMPLEMENTATION_MISSING,
        SINGLE_BRANCH_REVIEW_NOT_INDEPENDENT,
        SINGLE_BRANCH_EVIDENCE_UNREADABLE,
        SINGLE_BRANCH_IMPLEMENTATION_UNREADABLE,
        SINGLE_BRANCH_EVIDENCE_WINDOW_UNCOMPUTABLE,
    ];
    codes.sort_by_key(|code| std::cmp::Reverse(code.len()));
    codes
}

/// The neutral code for `code`: a `SINGLE_BRANCH_*` alias maps onto it.
fn canonical_code(code: &str) -> &str {
    match code {
        SINGLE_BRANCH_IMPLEMENTATION_MISSING => IMPLEMENTATION_MISSING,
        SINGLE_BRANCH_REVIEW_NOT_INDEPENDENT => REVIEW_NOT_INDEPENDENT,
        SINGLE_BRANCH_EVIDENCE_UNREADABLE | SINGLE_BRANCH_IMPLEMENTATION_UNREADABLE => {
            IMPLEMENTATION_UNREADABLE
        }
        SINGLE_BRANCH_EVIDENCE_WINDOW_UNCOMPUTABLE => EVIDENCE_WINDOW_UNCOMPUTABLE,
        other => other,
    }
}

/// Everything the axis reads, resolved once by the caller.
pub(super) struct EvidenceInputs<'a> {
    /// The repository the windows are diffed in.
    pub repo_root: &'a Path,
    /// The Mission's PRIMARY anchor directory (where `tasks/*.md` live).
    pub tasks_dir: &'a Path,
    /// `lanes.json`.
    pub manifest: &'a LanesManifest,
    /// Work package id → reduced lane (`approved`/`done`/…).
    pub lanes: &'a BTreeMap<String, String>,
    /// The status rows in append order, or `None` when the log cannot be read.
    /// `None` yields no attestation, so `IMPLEMENTATION_MISSING` stands.
    pub events: Option<&'a [Value]>,
    /// The STORED topology (`meta.json topologies`/`topology`, else `lanes.json`).
    pub is_single_branch: bool,
    /// Canceled work package ids (never judged).
    pub canceled: &'a BTreeSet<String>,
    /// A caller-verified PR landing commit (#6780).
    pub landed_commit: Option<&'a str>,
}

/// Source `status_only_excluded_prefixes`: the path prefixes that never count
/// as implementation work (the status log, `meta.json`, `tasks/`, `lanes.json`,
/// the matrix, the decision ledger, traces and `.kittify/`).
fn status_only_excluded_prefixes(slug: &str) -> Vec<String> {
    [
        format!("kitty-specs/{slug}/status."),
        format!("kitty-specs/{slug}/issue-matrix."),
        format!("kitty-specs/{slug}/meta.json"),
        format!("kitty-specs/{slug}/tasks/"),
        format!("kitty-specs/{slug}/lanes.json"),
        format!("kitty-specs/{slug}/acceptance-matrix.json"),
        format!("kitty-specs/{slug}/analysis-report.md"),
        format!("kitty-specs/{slug}/decisions/"),
        format!("kitty-specs/{slug}/decisions.events.jsonl"),
        format!("kitty-specs/{slug}/traces/"),
        ".kittify/".to_owned(),
    ]
    .into_iter()
    .collect()
}

/// True when `path` is not Mission bookkeeping (Source `_is_evidence_path`).
fn is_evidence_path(path: &str, slug: &str) -> bool {
    let normalized = path.replace('\\', "/");
    !status_only_excluded_prefixes(slug)
        .iter()
        .any(|prefix| normalized.starts_with(prefix.as_str()))
}

/// Normalized ownership globs (`wp_evidence.owned_patterns`).
pub(super) fn owned_patterns(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .filter(|pattern| !pattern.trim().is_empty())
        .map(|pattern| {
            pattern
                .replace('\\', "/")
                .strip_prefix("./")
                .map(str::to_owned)
                .unwrap_or_else(|| pattern.replace('\\', "/"))
        })
        .collect()
}

/// Source `matches_owned`: the canonical glob matcher plus the directory-prefix
/// rule.
fn matches_owned(path: &str, patterns: &[String]) -> bool {
    let normalized = path.replace('\\', "/");
    let normalized = normalized
        .strip_prefix("./")
        .unwrap_or(&normalized)
        .to_owned();
    patterns.iter().any(|pattern| {
        if owned_file_covers(std::slice::from_ref(pattern), &normalized) {
            return true;
        }
        let prefix = pattern
            .replace("/**", "")
            .replace("/*", "")
            .trim_end_matches('/')
            .to_owned();
        !prefix.is_empty() && normalized.starts_with(&format!("{prefix}/"))
    })
}

/// True when some changed path matches the work package's ownership patterns.
fn has_owned_change(paths: &BTreeSet<String>, patterns: &[String]) -> bool {
    !patterns.is_empty() && paths.iter().any(|path| matches_owned(path, patterns))
}

/// The no-implementation attestations in force, read in APPEND order.
///
/// An attestation counts only while the work package has stayed in
/// `{approved, done}` since it was recorded; any later non-migration event
/// outside that set voids it. A migration actor neither records nor voids one.
fn in_force_attestations(events: &[Value]) -> BTreeSet<String> {
    let mut in_force: BTreeSet<String> = BTreeSet::new();
    for row in events {
        if row
            .get("actor")
            .and_then(value_actor_text)
            .is_some_and(|actor| is_migration_actor(Some(actor.as_str())))
        {
            continue;
        }
        let Some(wp_id) = row.get("wp_id").and_then(Value::as_str) else {
            continue;
        };
        let attested = row
            .get("policy_metadata")
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get(ATTESTATION_KEY))
            .and_then(Value::as_str)
            .is_some_and(|value| value == NO_IMPLEMENTATION);
        if attested {
            in_force.insert(wp_id.to_owned());
            continue;
        }
        let to_lane = row
            .get("to_lane")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !ATTESTATION_KEEPING_LANES.contains(&to_lane) {
            in_force.remove(wp_id);
        }
    }
    in_force
}

/// One row's actor as text (a compact string, or a structured object's `tool`).
fn value_actor_text(actor: &Value) -> Option<String> {
    match actor {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => object
            .get("tool")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

/// Source `preflight._latest_actor_for_transition(feature_dir, wp, "approved")`.
///
/// The latest transition INTO `approved`, in append order, skipping operator
/// attestations (a forced self-transition is not a review) and migration seeds;
/// a generic placeholder actor leaves the actor of record unknown rather than
/// letting an older reviewer stand in.
fn latest_approver(events: &[Value], wp_id: &str) -> Option<String> {
    let mut latest: Option<String> = None;
    for row in events {
        if row.get("wp_id").and_then(Value::as_str) != Some(wp_id) {
            continue;
        }
        if row.get("to_lane").and_then(Value::as_str) != Some("approved") {
            continue;
        }
        if row
            .get("policy_metadata")
            .and_then(Value::as_object)
            .is_some_and(|metadata| metadata.contains_key(ATTESTATION_KEY))
        {
            continue;
        }
        let Some(actor) = row.get("actor") else {
            continue;
        };
        if value_actor_text(actor).is_some_and(|text| is_migration_actor(Some(text.as_str()))) {
            continue;
        }
        if actor_key(actor).is_some_and(|key| GENERIC_IMPLEMENTATION_ACTORS.contains(&key.as_str()))
        {
            latest = None;
            continue;
        }
        latest = actor_full_identity(actor);
    }
    latest
}

/// Source `wp_evidence._independent` for one work package.
fn is_independent(events: &[Value], wp_id: &str) -> bool {
    let implementer = latest_implementer_actor(events, wp_id);
    let reviewer = latest_approver(events, wp_id);
    actor_identities_independent(implementer.as_deref(), reviewer.as_deref())
}

/// One Mission's evaluation state (`wp_evidence._Scan`).
struct Scan {
    owned: BTreeMap<String, Vec<String>>,
    lines: Vec<(String, String)>,
    lifted: BTreeSet<String>,
    verdicts: BTreeMap<String, String>,
}

impl Scan {
    fn add(&mut self, wp_id: &str, line: String) {
        let code = line.split(": ").next().unwrap_or_default().to_owned();
        self.verdicts
            .entry(wp_id.to_owned())
            .or_insert_with(|| canonical_code(&code).to_owned());
        self.lines.push((wp_id.to_owned(), line));
    }

    fn mark(&mut self, wp_ids: &[String], code: &str) {
        for wp_id in wp_ids {
            self.verdicts
                .entry(wp_id.clone())
                .or_insert_with(|| canonical_code(code).to_owned());
        }
    }
}

/// The evidence window `(planning_commit, tip)` for a lane (`lane_evidence_window`).
fn lane_evidence_window(
    manifest: &LanesManifest,
    lane_id: &str,
    planning: bool,
) -> Option<(String, String)> {
    let base = manifest
        .planning_commit_sha
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    if planning {
        return Some((base, manifest.target_branch.clone()));
    }
    let tip = lane_override_tip(manifest, lane_id)?;
    Some((base, tip))
}

/// The lane branch name for `lane_id` (`lane_created_branch`).
fn lane_override_tip(manifest: &LanesManifest, lane_id: &str) -> Option<String> {
    lane_branch_name(
        manifest.mission_slug.as_str(),
        lane_id,
        manifest.target_branch.as_str(),
    )
    .ok()
}

/// A resolved commit, or the opaque probe failure text Source prints.
fn resolve(git: &GitRepository, revision: &str) -> Result<Oid, String> {
    match Oid::parse(revision) {
        Ok(oid) => git
            .resolve_commit(&StartPoint::Oid(oid))
            .map_err(|error| format!("{revision}: {error}")),
        Err(oid_error) => match BranchName::parse(revision) {
            Ok(branch) => git
                .resolve_commit(&StartPoint::Ref(branch.as_ref_name()))
                .map_err(|error| format!("{revision}: {error}")),
            Err(_) => Err(format!("{revision}: {oid_error}")),
        },
    }
}

/// The non-bookkeeping paths whose content differs across `window`.
fn evidence_paths(
    git: &GitRepository,
    window: &(String, String),
    slug: &str,
) -> Result<BTreeSet<String>, String> {
    let (base, tip) = window;
    let base = resolve(git, base)?;
    let tip = resolve(git, tip)?;
    let entries = git
        .diff_name_status(&base, &tip)
        .map_err(|error| error.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| entry.path.display_lossy())
        .filter(|path| is_evidence_path(path, slug))
        .collect())
}

/// True only when the local branch `branch` is **truly** absent.
///
/// Source accepts only `show-ref --verify` exit 1 as "absent"; a ref that exists
/// but cannot be read is not absent. A parse/probe failure is therefore a
/// failure, never an absent ref.
fn branch_ref_absent(git: &GitRepository, branch: &str) -> Result<bool, String> {
    let parsed = BranchName::parse(branch).map_err(|error| format!("{branch}: {error}"))?;
    git.ref_exists(&parsed.as_ref_name())
        .map(|exists| !exists)
        .map_err(|error| format!("{branch}: {error}"))
}

/// The window actually judged, plus what it stands in for (#6780).
struct LandedWindow {
    window: (String, String),
    absent_ref: Option<String>,
    landed_commit: Option<String>,
}

/// `wp_evidence._landed_window`: stand a verified landing commit in for a
/// pruned lane branch.
fn landed_window(
    git: &GitRepository,
    window: (String, String),
    landed_commit: Option<&str>,
) -> Result<LandedWindow, String> {
    let (base, tip) = window;
    let Some(landed_commit) = landed_commit else {
        return Ok(LandedWindow {
            window: (base, tip),
            absent_ref: None,
            landed_commit: None,
        });
    };
    if !branch_ref_absent(git, &tip)? {
        return Ok(LandedWindow {
            window: (base, tip),
            absent_ref: None,
            landed_commit: None,
        });
    }
    let base_oid = resolve(git, &base)?;
    let landed = resolve(git, landed_commit)?;
    if !git
        .is_ancestor(&base_oid, &landed)
        .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "the verified landing commit {landed_commit} does not contain the recorded planning commit {base} (the window {base}..{landed_commit} would not describe this Mission's work)"
        ));
    }
    let parents = git
        .commit_parents(&landed)
        .map_err(|error| error.to_string())?;
    if parents.len() < 2 {
        return Err(format!(
            "the verified landing commit {landed_commit} is not a merge commit (a squash or fast-forward landing), so the pull request's own change cannot be separated from unrelated work on the target; restore the lane branch from `refs/spec-kitty/lane-tip/{tip}` or the pull request's head, then re-run"
        ));
    }
    let mut merged_side = false;
    for parent in &parents[1..] {
        if git
            .is_ancestor(&base_oid, parent)
            .map_err(|error| error.to_string())?
        {
            merged_side = true;
            break;
        }
    }
    if !merged_side {
        return Err(format!(
            "the recorded planning commit {base} is not on the merged-in side of the landing commit {landed_commit}, so that merge did not land this Mission; restore the lane branch from `refs/spec-kitty/lane-tip/{tip}` or the pull request's head, then re-run"
        ));
    }
    Ok(LandedWindow {
        window: (parents[0].to_string(), landed_commit.to_owned()),
        absent_ref: Some(tip),
        landed_commit: Some(landed_commit.to_owned()),
    })
}

/// `wp_evidence._dependency_evidence_windows`: `(merge-base(dependency tip, lane
/// tip), lane tip)` for each dependency lane whose branch exists.
fn dependency_windows(
    git: &GitRepository,
    manifest: &LanesManifest,
    dependency_ids: &[String],
    effective_tip: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut windows = Vec::new();
    let tip = resolve(git, effective_tip)?;
    for dependency_id in dependency_ids {
        let Some(dependency) = manifest
            .lanes
            .iter()
            .find(|lane| lane.lane_id == *dependency_id)
        else {
            continue;
        };
        if dependency.is_planning() {
            continue;
        }
        let Some(branch) = lane_override_tip(manifest, &dependency.lane_id) else {
            continue;
        };
        let absent = branch_ref_absent(git, &branch)?;
        if absent {
            continue;
        }
        let dependency_tip = resolve(git, &branch)?;
        let base = git
            .merge_base(&dependency_tip, &tip)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("git merge-base {branch} {effective_tip} could not be read"))?;
        windows.push((base.to_string(), effective_tip.to_owned()));
    }
    Ok(windows)
}

/// Source `widened_lane_paths`: add the dependency windows' paths when some
/// judged work package shows no owned change in its own planning window.
fn widened_paths(
    git: &GitRepository,
    manifest: &LanesManifest,
    dependency_ids: &[String],
    paths: &BTreeSet<String>,
    pattern_sets: &[&Vec<String>],
    slug: &str,
    effective_tip: &str,
) -> Result<BTreeSet<String>, String> {
    let needs_widening = !dependency_ids.is_empty()
        && !pattern_sets
            .iter()
            .all(|patterns| has_owned_change(paths, patterns));
    if !needs_widening {
        return Ok(paths.clone());
    }
    let mut widened = paths.clone();
    for window in dependency_windows(git, manifest, dependency_ids, effective_tip)? {
        widened.extend(evidence_paths(git, &window, slug)?);
    }
    Ok(widened)
}

/// Verdicts 3 and 4 for one work package whose window was read.
fn judge(
    scan: &mut Scan,
    attested: &BTreeSet<String>,
    events: Option<&[Value]>,
    wp_id: &str,
    paths: &BTreeSet<String>,
    missing_line: &dyn Fn(&str) -> String,
    not_independent_code: &str,
) {
    let owned = scan.owned.get(wp_id).cloned().unwrap_or_default();
    if !has_owned_change(paths, &owned) {
        if !attested.contains(wp_id) {
            scan.add(wp_id, missing_line(wp_id));
            return;
        }
        scan.lifted.insert(wp_id.to_owned());
    }
    let independent = events.is_some_and(|rows| is_independent(rows, wp_id));
    if !independent {
        scan.add(
            wp_id,
            format!("{not_independent_code}: {wp_id} {NOT_INDEPENDENT_DETAIL}"),
        );
    }
}

/// Source `_lanes_refusal_text`: a header, a line per work package, one recovery
/// line per code named.
fn lanes_refusal_text(slug: &str, lines: &[String]) -> String {
    let mut codes: Vec<&str> = Vec::new();
    for line in lines {
        for code in all_codes() {
            if line.starts_with(&format!("{code}: ")) {
                let canonical = canonical_code(code);
                if !codes.contains(&canonical) {
                    codes.push(canonical);
                }
            }
        }
    }
    let mut body: Vec<String> = lines.to_vec();
    for code in _NEUTRAL_CODES {
        if codes.contains(&code) {
            body.push(recovery_line(code).to_owned());
        }
    }
    format!(
        "Mission {slug}: work packages cannot be landed as reviewed:\n{}",
        body.join("\n")
    )
}

/// Source `_RECOVERY`: the one recovery line per refusal code.
fn recovery_line(code: &str) -> &'static str {
    match code {
        IMPLEMENTATION_MISSING => concat!(
            "Recovery for IMPLEMENTATION_MISSING: commit the work package's change to its owned_files (on its lane branch, or on the ",
            "target branch for a planning-artifact work package) and have it reviewed and approved again. A work package that owes no ",
            "implementation is recorded with `--attest-no-implementation <WP> --attest-reason \"<why>\"` on `spec-kitty accept` or ",
            "`spec-kitty consolidate`."
        ),
        REVIEW_NOT_INDEPENDENT => concat!(
            "Recovery for REVIEW_NOT_INDEPENDENT: have a reviewer who is not the implementer review and approve the work package ",
            "(`spec-kitty agent action review <WP> --agent <reviewer>`), then re-run. No attestation lifts this."
        ),
        IMPLEMENTATION_UNREADABLE => concat!(
            "Recovery for IMPLEMENTATION_UNREADABLE: restore or fetch the branch the window ends at (and the recorded planning commit) so it is readable again, ",
            "or, for a Mission that landed through a pull request merge commit (not a squash), re-run `spec-kitty accept --mode pr --merge-commit <sha>` ",
            "with the verified PR landing commit so the evidence is judged against the landed tree, then re-run."
        ),
        _ => {
            "Recovery for EVIDENCE_WINDOW_UNCOMPUTABLE: run `spec-kitty agent mission finalize-tasks` to record the planning commit, then re-run."
        }
    }
}

/// Source `_single_branch_refusal_text`.
fn single_branch_refusal_text(slug: &str, lines: &[String]) -> String {
    let mut body: Vec<String> = lines.to_vec();
    body.push(
        "Recovery: implement the work package (commit a real change in its owned_files on the mission branch) and have a reviewer who is not the implementer approve it, then re-run consolidate."
            .to_owned(),
    );
    format!(
        "Mission {slug}: single_branch work packages cannot be landed as reviewed:\n{}",
        body.join("\n")
    )
}

/// The approved (or done), non-canceled work package ids, sorted.
fn approved_ids(lanes: &BTreeMap<String, String>, canceled: &BTreeSet<String>) -> Vec<String> {
    let mut approved: Vec<String> = lanes
        .iter()
        .filter(|(wp_id, lane)| {
            !canceled.contains(wp_id.as_str()) && APPROVED_MEMBERSHIP_LANES.contains(&lane.as_str())
        })
        .map(|(wp_id, _)| wp_id.clone())
        .collect();
    approved.sort();
    approved
}

/// Source `_owned_patterns_by_wp`: each wanted work package's authored globs.
fn owned_patterns_by_wp(tasks_dir: &Path, wanted: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut resolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(tasks_dir) else {
        // An unreadable `tasks/` leaves every work package unresolved, so the
        // axis refuses (fail closed) instead of passing vacuously.
        return resolved;
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "md")
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("readme.md"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let Ok(package) = sk_mission::WorkPackage::from_file(&path) else {
            // An unreadable work-package file stays unresolved; the caller then
            // reads "implementation cannot be proven" and refuses.
            continue;
        };
        let wp_id = package.id().as_str().to_owned();
        if !wanted.contains(&wp_id) {
            continue;
        }
        let patterns = package
            .frontmatter_field("owned_files")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        resolved.insert(wp_id, owned_patterns(&patterns));
    }
    resolved
}

/// Evaluate the axis for one Mission (Source `evaluate_evidence`).
pub(super) fn evaluate_evidence(inputs: &EvidenceInputs<'_>) -> EvidenceReport {
    let approved = approved_ids(inputs.lanes, inputs.canceled);
    if approved.is_empty() {
        return EvidenceReport::default();
    }
    let slug = inputs.manifest.mission_slug.as_str();
    let mut scan = Scan {
        owned: owned_patterns_by_wp(inputs.tasks_dir, &approved),
        lines: Vec::new(),
        lifted: BTreeSet::new(),
        verdicts: BTreeMap::new(),
    };
    let attested = inputs.events.map(in_force_attestations).unwrap_or_default();
    let git = match GitRepository::open(inputs.repo_root) {
        Ok(git) => git,
        Err(error) => {
            for wp_id in &approved {
                scan.add(
                    wp_id,
                    format!(
                        "{IMPLEMENTATION_UNREADABLE}: {wp_id}'s evidence window cannot be read: the repository could not be opened ({error}), {CANNOT_VERIFY}"
                    ),
                );
            }
            let lines = ordered_lines(&scan);
            return EvidenceReport {
                refusal: Some(lanes_refusal_text(slug, &lines)),
                lifted_wp_ids: scan.lifted,
                verdicts: scan.verdicts,
            };
        }
    };
    let refusal = if inputs.is_single_branch {
        single_branch_scan(inputs, &git, &mut scan, &approved, &attested)
    } else {
        lanes_scan(inputs, &git, &mut scan, &approved, &attested)
    };
    EvidenceReport {
        refusal,
        lifted_wp_ids: scan.lifted,
        verdicts: scan.verdicts,
    }
}

/// The refusal lines ordered by work package id (Source's sort by `wp_id`).
fn ordered_lines(scan: &Scan) -> Vec<String> {
    let mut ordered: Vec<(String, String)> = scan.lines.clone();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    ordered.into_iter().map(|(_, line)| line).collect()
}

/// Source `_single_branch_scan`.
fn single_branch_scan(
    inputs: &EvidenceInputs<'_>,
    git: &GitRepository,
    scan: &mut Scan,
    approved: &[String],
    attested: &BTreeSet<String>,
) -> Option<String> {
    let slug = inputs.manifest.mission_slug.as_str();
    let Some(base) = inputs
        .manifest
        .planning_commit_sha
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        let line = format!(
            "{SINGLE_BRANCH_EVIDENCE_WINDOW_UNCOMPUTABLE}: the implementation-evidence window (planning_commit..mission_branch) cannot be computed for this single_branch mission (no recorded planning commit), {CANNOT_VERIFY} Recovery: run `spec-kitty agent mission finalize-tasks` to record the planning commit, then re-run."
        );
        scan.mark(approved, SINGLE_BRANCH_EVIDENCE_WINDOW_UNCOMPUTABLE);
        return Some(single_branch_refusal_text(slug, &[line]));
    };
    let tip = if inputs.manifest.mission_branch.trim().is_empty() {
        inputs.manifest.target_branch.clone()
    } else {
        inputs.manifest.mission_branch.clone()
    };
    let window = (base.to_owned(), tip.clone());
    let paths = match evidence_paths(git, &window, slug) {
        Ok(paths) => paths,
        Err(_) => {
            let line = format!(
                "{SINGLE_BRANCH_EVIDENCE_UNREADABLE}: the implementation-evidence window {base}..{tip} could not be diffed, {CANNOT_VERIFY}"
            );
            scan.mark(approved, SINGLE_BRANCH_EVIDENCE_UNREADABLE);
            return Some(single_branch_refusal_text(slug, &[line]));
        }
    };
    let missing = |wp_id: &str| {
        format!(
            "{SINGLE_BRANCH_IMPLEMENTATION_MISSING}: {wp_id} was approved with no implementation commit touching its owned_files in the mission branch window {base}..{tip}."
        )
    };
    for wp_id in approved {
        judge(
            scan,
            attested,
            inputs.events,
            wp_id,
            &paths,
            &missing,
            SINGLE_BRANCH_REVIEW_NOT_INDEPENDENT,
        );
    }
    if scan.lines.is_empty() {
        return None;
    }
    Some(single_branch_refusal_text(slug, &ordered_lines(scan)))
}

/// Source `_lanes_scan`.
fn lanes_scan(
    inputs: &EvidenceInputs<'_>,
    git: &GitRepository,
    scan: &mut Scan,
    approved: &[String],
    attested: &BTreeSet<String>,
) -> Option<String> {
    let slug = inputs.manifest.mission_slug.as_str();
    for lane in &inputs.manifest.lanes {
        let lane_approved: Vec<String> = lane
            .wp_ids
            .iter()
            .map(|wp| wp.as_str().to_owned())
            .filter(|wp_id| approved.contains(wp_id))
            .collect();
        if lane_approved.is_empty() {
            continue;
        }
        scan_lane(inputs, git, scan, attested, lane, &lane_approved);
    }
    let laned: BTreeSet<String> = inputs
        .manifest
        .lanes
        .iter()
        .flat_map(|lane| lane.wp_ids.iter().map(|wp| wp.as_str().to_owned()))
        .collect();
    for wp_id in approved {
        if !laned.contains(wp_id) {
            scan.add(
                wp_id,
                format!(
                    "{EVIDENCE_WINDOW_UNCOMPUTABLE}: {wp_id} is approved but in no lane of lanes.json, {CANNOT_VERIFY}"
                ),
            );
        }
    }
    if scan.lines.is_empty() {
        return None;
    }
    Some(lanes_refusal_text(slug, &ordered_lines(scan)))
}

/// Source `_scan_lane`: one lane's window, landed stand-in, dependency widening
/// and the per-work-package verdicts.
fn scan_lane(
    inputs: &EvidenceInputs<'_>,
    git: &GitRepository,
    scan: &mut Scan,
    attested: &BTreeSet<String>,
    lane: &sk_workspace::ExecutionLane,
    lane_approved: &[String],
) {
    let slug = inputs.manifest.mission_slug.as_str();
    let window = lane_evidence_window(inputs.manifest, &lane.lane_id, lane.is_planning());
    let Some(window) = window else {
        for wp_id in lane_approved {
            scan.add(
                wp_id,
                format!(
                    "{EVIDENCE_WINDOW_UNCOMPUTABLE}: {wp_id} has no evidence window ({}: lanes.json records no planning commit), {CANNOT_VERIFY}",
                    lane.lane_id
                ),
            );
        }
        return;
    };
    let judged = match landed_window(git, window.clone(), inputs.landed_commit) {
        Ok(judged) => judged,
        Err(detail) => {
            for wp_id in lane_approved {
                scan.add(
                    wp_id,
                    format!(
                        "{IMPLEMENTATION_UNREADABLE}: {wp_id}'s evidence window {}..{} ({}) cannot be read: {detail}, {CANNOT_VERIFY}",
                        window.0, window.1, lane.lane_id
                    ),
                );
            }
            return;
        }
    };
    let (base, tip) = judged.window.clone();
    let paths = match evidence_paths(git, &judged.window, slug) {
        Ok(paths) => paths,
        Err(detail) => {
            let cause = match (&judged.absent_ref, &judged.landed_commit) {
                (Some(absent), Some(landed)) => format!(
                    "the lane branch {absent} no longer exists, so the verified landing commit {landed} stands in for it; the window could not be diffed ({detail})"
                ),
                (Some(absent), None) => format!(
                    "the window tip {absent} no longer exists (a pruned lane branch?), so the window cannot be read; for a Mission that landed through a pull request merge commit (not a squash), pass `--merge-commit <sha>` with the verified PR landing commit on `spec-kitty accept --mode pr` to judge this work package against the landed tree; otherwise restore or fetch the branch"
                ),
                _ => format!("the window could not be diffed ({detail})"),
            };
            for wp_id in lane_approved {
                scan.add(
                    wp_id,
                    format!(
                        "{IMPLEMENTATION_UNREADABLE}: {wp_id}'s evidence window {base}..{tip} ({}) cannot be read: {cause}, {CANNOT_VERIFY}",
                        lane.lane_id
                    ),
                );
            }
            return;
        }
    };
    let missing = |wp_id: &str| {
        let mut line = format!(
            "{IMPLEMENTATION_MISSING}: {wp_id} was approved with no change to its owned_files in its evidence window {base}..{tip} ({}).",
            lane.lane_id
        );
        if let (true, Some(absent)) = (judged.landed_commit.is_some(), &judged.absent_ref) {
            line.push_str(&format!(
                " The lane branch {absent} was pruned, so the verified landing commit stands in for it."
            ));
        }
        line
    };
    let empty: Vec<String> = Vec::new();
    let owned_by_wp: Vec<&Vec<String>> = lane_approved
        .iter()
        .map(|wp_id| scan.owned.get(wp_id).unwrap_or(&empty))
        .collect();
    let dependency_ids: Vec<String> = lane.depends_on_lanes.clone();
    let widened = match widened_paths(
        git,
        inputs.manifest,
        &dependency_ids,
        &paths,
        &owned_by_wp,
        slug,
        &judged.window.1,
    ) {
        Ok(widened) => widened,
        Err(detail) => {
            for wp_id in lane_approved {
                scan.add(
                    wp_id,
                    format!(
                        "{IMPLEMENTATION_UNREADABLE}: {wp_id}'s window against its dependency lanes ({}) could not be diffed ({detail}), {CANNOT_VERIFY}",
                        lane.lane_id
                    ),
                );
            }
            return;
        }
    };
    for wp_id in lane_approved {
        judge(
            scan,
            attested,
            inputs.events,
            wp_id,
            &widened,
            &missing,
            REVIEW_NOT_INDEPENDENT,
        );
    }
}
