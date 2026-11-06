//! `review`, `accept`, `consolidate`, and the removed `merge` alias.
//!
//! These are the adapters that turn a real user invocation into the owned domain
//! calls:
//!
//! * [`review`] resolves the mission, its recorded baseline and its canonical
//!   status snapshot, builds the Git-derived dead-code evidence, and asks
//!   `sk-mission` to run the Source gates and write
//!   `mission-review-report.md`;
//! * [`accept`] gathers the canonical status snapshot, the event-sourced review
//!   verdicts, the implementer identity of record, each work package's declared
//!   `owned_files`, and the real per-lane changed paths, then asks `sk-status` to
//!   judge acceptance and writes/commits `acceptance-matrix.json`;
//! * [`consolidate`] hands the canonical lane readiness to `sk-workspace`'s
//!   consolidation engine, which lands real merge commits, records the baseline
//!   and performs the conditional teardown;
//! * [`merge`] is the Source rename stub: the same message and the same refusal.
//!
//! Everything reads the canonical sources (event log, `lanes.json`, `meta.json`,
//! Git trees). Nothing here synthesizes a verdict, an object id, or a readiness
//! signal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::Value;
use sk_git::{BranchName, GitPath, GitRepository, Oid};
use sk_mission::{Mission, ReviewInputs};
use sk_status::acceptance::ReviewEvidence;
use sk_workspace::consolidation::{
    ConsolidationReport, ConsolidationRequest, MergeStrategy, consolidate_mission,
};

use super::status;
use super::support::{self, CompatError, emit_json, path_string};

/// The Source `merge` removal message, verbatim.
pub const MERGE_RENAMED_MESSAGE: &str = "The `merge` command has been renamed to `spec-kitty consolidate`. Re-run with `spec-kitty consolidate` (same flags).";

/// `spec-kitty review` options.
#[derive(Debug, Args)]
pub struct ReviewArgs {
    /// Mission handle (id, mid8, or slug).
    #[arg(long)]
    pub mission: Option<String>,
    /// Review mode: `lightweight` or `post-merge` (auto-detected when omitted).
    #[arg(long, value_name = "MODE")]
    pub mode: Option<String>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `spec-kitty accept` options.
#[derive(Debug, Args)]
pub struct AcceptArgs {
    /// Mission handle (id, mid8, or slug).
    #[arg(long)]
    pub mission: Option<String>,
    /// Acceptance mode: `local` (the default) or `pr`.
    #[arg(long, value_name = "MODE")]
    pub mode: Option<String>,
    /// Record this PR landing commit as the mission's review baseline.
    #[arg(long = "merge-commit", value_name = "SHA")]
    pub merge_commit: Option<String>,
    /// The branch the PR merged into (used with `--merge-commit`).
    #[arg(long = "target-branch", value_name = "BRANCH")]
    pub target_branch: Option<String>,
    /// Attest that the supplied commit's first parent is the pre-landing target tip.
    #[arg(long = "attest-first-landing-commit")]
    pub attest_first_landing_commit: bool,
    /// Origin freshness policy: `enforce`, `warn`, or `off`.
    #[arg(long = "origin-check", value_name = "MODE")]
    pub origin_check: Option<String>,
    /// Skip strict metadata validation and downgrade the advisory checks to
    /// warnings (Source's `strict_metadata=False`).
    #[arg(long)]
    pub lenient: bool,
    /// Diagnose acceptance blockers without writing metadata or matrix artifacts
    /// (Source `--diagnose`: one read-only collection, negative invariants are
    /// not executed).
    #[arg(long)]
    pub diagnose: bool,
    /// Report acceptance readiness without writing metadata or creating commits
    /// (Source `--no-commit`). The acceptance matrix is still resolved, because a
    /// matrix left `pending` can never pass the gate.
    #[arg(long = "no-commit", alias = "no_commit")]
    pub no_commit: bool,
    /// Name to record as the acceptance actor (defaults to `$USER`, then
    /// `$USERNAME`, then `system`).
    #[arg(long)]
    pub actor: Option<String>,
    /// Operator attestation (repeatable, one work-package id each): the named
    /// approved work package owes no implementation (for example its deliverable
    /// already exists). Lifts `IMPLEMENTATION_MISSING` for that work package
    /// only; never lifts `REVIEW_NOT_INDEPENDENT` and is never read as a review.
    /// Requires `--attest-reason`; recorded durably in the status event log
    /// before the readiness checks, and refused (exit 2, nothing recorded) in a
    /// read-only mode.
    #[arg(long = "attest-no-implementation", value_name = "WP")]
    pub attest_no_implementation: Vec<String>,
    /// Why no implementation is owed, recorded with
    /// `--attest-no-implementation` (required with it).
    #[arg(long = "attest-reason", value_name = "TEXT")]
    pub attest_reason: Option<String>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `spec-kitty consolidate` options.
#[derive(Debug, Args)]
pub struct ConsolidateArgs {
    /// Branch-integration strategy: merge (Source also supports squash/rebase,
    /// which this engine does not implement yet).
    #[arg(long, value_name = "STRATEGY")]
    pub strategy: Option<String>,
    /// Delete lane branches after a landed merge.
    #[arg(long = "delete-branch", overrides_with = "keep_branch")]
    pub delete_branch: bool,
    /// Keep lane branches after a landed merge.
    #[arg(long = "keep-branch", overrides_with = "delete_branch")]
    pub keep_branch: bool,
    /// Remove lane worktrees after a landed merge.
    #[arg(long = "remove-worktree", overrides_with = "keep_worktree")]
    pub remove_worktree: bool,
    /// Keep lane worktrees after a landed merge.
    #[arg(long = "keep-worktree", overrides_with = "remove_worktree")]
    pub keep_worktree: bool,
    /// Target branch for the branch-integration step (defaults to the mission's).
    #[arg(long = "target", value_name = "BRANCH")]
    pub target: Option<String>,
    /// Origin freshness check before anything lands: `enforce` (the default)
    /// refuses when the remote's status evidence or an approved lane branch is
    /// ahead of this clone, or the remote cannot be reached; `warn` reports the
    /// same findings and continues; `off` contacts nothing. Overrides
    /// `SPEC_KITTY_ORIGIN_CHECK`.
    #[arg(long = "origin-check", value_name = "MODE")]
    pub origin_check: Option<String>,
    /// Show what would be done without executing.
    #[arg(long)]
    pub dry_run: bool,
    /// Emit deterministic JSON.
    #[arg(long)]
    pub json: bool,
    /// Mission handle when merging from the target branch.
    #[arg(long)]
    pub mission: Option<String>,
    /// Resume an interrupted run.
    #[arg(long)]
    pub resume: bool,
    /// Abort a run that provably did not move the target.
    #[arg(long)]
    pub abort: bool,
    /// Complete a mission whose `lanes.json` is genuinely absent.
    #[arg(long = "skip-lanes", alias = "no-lanes")]
    pub skip_lanes: bool,
}

/// `spec-kitty merge-driver-*` options: git supplies `%O %A %B` as three
/// sibling temporary files.
#[derive(Debug, Args)]
pub struct MergeDriverArgs {
    /// base, ours, theirs, in git's `%O %A %B` order.
    #[arg(value_name = "PATH", num_args = 3)]
    pub paths: Vec<PathBuf>,
}

/// Run one registered merge-driver body exactly as git invokes it.
///
/// The parent root CLI wires the seven names:
///
/// ```text
/// Command::MergeDriverMeta(args) =>
///     Ok(lifecycle::run_merge_driver("merge-driver-meta", &args))
/// ```
///
/// `%A` (ours) is written on success; a body's `MergeDriverError` is the exact
/// stderr text and exit 1, which git treats as an unresolved conflict.
pub fn run_merge_driver(name: &str, args: &MergeDriverArgs) -> u8 {
    let [base, ours, theirs] = args.paths.as_slice() else {
        eprintln!("merge driver {name} needs exactly %O %A %B (three paths)");
        return 2;
    };
    match sk_workspace::merge_drivers::run_driver_paths(name, base, ours, theirs) {
        Ok(outcome) => {
            if let Some(notice) = outcome.notice {
                println!("{notice}");
            }
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

/// `spec-kitty merge` options (the removed alias; every former flag is accepted
/// and refused with the rename message).
#[derive(Debug, Args)]
pub struct MergeArgs {
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
    /// Every former `merge` argument, refused verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub(crate) fn require_mission(handle: Option<&str>, command: &str) -> Result<String, CompatError> {
    let value = handle.map(str::trim).unwrap_or_default();
    if value.is_empty() {
        return Err(CompatError::usage(format!(
            "--mission is required for {command}"
        )));
    }
    Ok(value.to_owned())
}

/// Read the canonical status rows for a mission (absent log → no rows).
pub(crate) fn status_rows(mission: &Mission) -> Result<Vec<Value>, CompatError> {
    let path = support::events_path(mission);
    match sk_status::read_raw_events(&path) {
        Ok(rows) => Ok(rows),
        Err(sk_status::SnapshotError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(CompatError::domain(format!(
            "could not read {}: {error}",
            path.display()
        ))),
    }
}

/// Reduce the canonical event rows into the work-package lanes.
pub(crate) fn work_package_lanes(rows: &[Value]) -> Result<Vec<(String, String)>, CompatError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let snapshot = sk_status::reduce_rows(rows).map_err(|error| {
        CompatError::domain(format!("could not reduce the status log: {error}"))
    })?;
    let mut lanes: Vec<(String, String)> = snapshot
        .work_packages
        .iter()
        .map(|(wp_id, state)| {
            (
                wp_id.clone(),
                state.lane_string().unwrap_or("genesis").to_owned(),
            )
        })
        .collect();
    lanes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(lanes)
}

/// Latest event-sourced review verdict per work package.
pub(crate) fn review_evidence(rows: &[Value]) -> BTreeMap<String, ReviewEvidence> {
    let mut latest: BTreeMap<String, ReviewEvidence> = BTreeMap::new();
    for row in rows {
        let Some(wp_id) = row.get("wp_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(result) = row.get("review_result").and_then(Value::as_object) else {
            continue;
        };
        let (Some(reviewer), Some(verdict)) = (
            result.get("reviewer").and_then(Value::as_str),
            result.get("verdict").and_then(Value::as_str),
        ) else {
            continue;
        };
        let reference = result
            .get("reference")
            .and_then(Value::as_str)
            .unwrap_or_default();
        latest.insert(
            wp_id.to_owned(),
            ReviewEvidence {
                wp_id: wp_id.to_owned(),
                reviewer: reviewer.to_owned(),
                verdict: verdict.to_owned(),
                reference: reference.to_owned(),
            },
        );
    }
    latest
}

/// Read the mission's lane manifest, if it has one.
pub(crate) fn lane_manifest(
    mission: &Mission,
) -> Result<Option<sk_workspace::LanesManifest>, CompatError> {
    let store = sk_workspace::LanesStore::new(mission.spec_dir());
    store
        .read()
        .map_err(|error| CompatError::domain(format!("could not read lanes.json: {error}")))
}

/// Resolve the repository-relative path of a checkout path.
fn relative(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .and_then(|value| value.to_str())
        .map(|value| value.replace('\\', "/"))
}

/// `spec-kitty review`.
///
/// Returns the exit code (0 pass/pass_with_notes, 1 fail).
pub fn review(root: &Path, args: &ReviewArgs) -> Result<(Value, u8), CompatError> {
    let handle = require_mission(args.mission.as_deref(), "review")?;
    let mission = support::resolve_mission(root, Some(&handle))?;
    let rows = status_rows(&mission)?;
    let lanes = work_package_lanes(&rows)?;

    let baseline = mission
        .meta_field("baseline_merge_commit")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    // A rejected review artifact on a terminal work package is a blocking
    // consistency conflict (the event-sourced verdict wins over any artifact).
    let evidence = review_evidence(&rows);
    let rejected: Vec<(String, String, String)> = lanes
        .iter()
        .filter(|(_, lane)| lane == "done")
        .filter_map(|(wp_id, lane)| {
            let review = evidence.get(wp_id)?;
            (review.verdict != "approved").then(|| {
                (
                    wp_id.clone(),
                    lane.clone(),
                    format!("review-cycle of {wp_id}: {}", review.reference),
                )
            })
        })
        .collect();

    let dead_code = match &baseline {
        Some(baseline) => dead_code_evidence(root, baseline).map_err(|error| {
            CompatError::domain(format!("could not derive dead-code evidence: {error}"))
        })?,
        None => None,
    };

    let inputs = ReviewInputs {
        repo_root: root.to_path_buf(),
        feature_dir: mission.spec_dir().to_path_buf(),
        mission_slug: mission.slug().as_str().to_owned(),
        mission_id: Some(mission.id().as_str().to_owned()),
        friendly_name: mission.friendly_name().map(str::to_owned),
        baseline_merge_commit: baseline,
        acceptance_mode: mission
            .meta_field("acceptance_mode")
            .and_then(Value::as_str)
            .map(str::to_owned),
        pr_merge_evidence: mission
            .meta_field("pr_merge_evidence")
            .and_then(Value::as_str)
            .map(str::to_owned),
        work_packages: lanes,
        rejected_review_artifacts: rejected,
        dead_code,
    };
    let report = sk_mission::review_mission(&inputs, args.mode.as_deref()).map_err(|error| {
        let code = match &error {
            sk_mission::MissionReviewError::ModeMismatch => "MISSION_REVIEW_MODE_MISMATCH",
            sk_mission::MissionReviewError::InvalidMode(_) => "MISSION_REVIEW_MODE_INVALID",
            _ => "MISSION_REVIEW_FAILED",
        };
        CompatError::coded(code, error.to_string())
    })?;

    let payload = report.to_json();
    if args.json {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "\nReviewing mission: {} ({})",
            inputs
                .friendly_name
                .as_deref()
                .unwrap_or(&inputs.mission_slug),
            inputs.mission_slug
        );
        println!("Mode: {}", report.mode.as_str());
        for gate in &report.gates {
            println!("  {:<18} {}", gate.name, gate.result);
        }
        println!(
            "\nVerdict: {} ({} finding(s))",
            report.verdict,
            report.findings.len()
        );
        println!("Report written: {}", path_string(&report.report_path));
    }
    let code = if report.verdict == "fail" { 1 } else { 0 };
    Ok((payload, code))
}

/// The Git-derived dead-code evidence.
///
/// `None` only when the baseline commit cannot be resolved at all (the gate then
/// reports `undeterminable`, never a green scan).
fn dead_code_evidence(
    root: &Path,
    baseline: &str,
) -> Result<Option<sk_mission::DeadCodeEvidence>, String> {
    let repository = GitRepository::open(root).map_err(|error| error.to_string())?;
    let base = Oid::parse(baseline)
        .map_err(|error| format!("baseline {baseline:?} is not a commit id: {error}"))?;
    let Some(head_text) = repository
        .head_commit()
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let head = Oid::parse(&head_text).map_err(|error| error.to_string())?;
    let changed = status::changed_paths_for_lifecycle(&repository, &base, &head, "")?;
    let supported: Vec<String> = changed
        .iter()
        .filter(|path| is_supported_python_path(path))
        .cloned()
        .collect();
    let mut added_symbols: Vec<(String, String)> = Vec::new();
    for path in &supported {
        let git_path = match GitPath::from_git_bytes(path.as_bytes()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        // The Source basis is the `+` lines of `git diff --unified=0 -M`; the
        // whole-blob comparison this replaced lost a declaration moved verbatim
        // inside a file. The adapter's typed unified-diff read is that basis, so
        // an addition is exact.
        let added = repository
            .diff_added_lines(&base, &head, &git_path)
            .map_err(|error| format!("could not read the added lines of {path}: {error}"))?;
        for line in added {
            if let Some(symbol) = public_declaration_symbol(line.trim_end()) {
                added_symbols.push((symbol, path.clone()));
            }
        }
    }
    let corpus = python_corpus(root, &supported)?;
    Ok(Some(sk_mission::DeadCodeEvidence {
        changed_paths: changed,
        supported_paths: supported,
        added_symbols,
        corpus,
        discovery_error: None,
    }))
}
/// A changed path the Python-only scan supports (Source
/// `path.startswith("src/") or not _is_test_only_path(path)`).
fn is_supported_python_path(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    if !lowered.ends_with(".py") {
        return false;
    }
    path.starts_with("src/") || !sk_mission::is_test_only_path(path)
}

/// Public `def`/`class` declarations added between `before` and `after`.
///
/// Kept as the unit-level expression of the declaration rule; the evidence path
/// now reads git's own unified diff (`diff_added_lines`), which is Source's
/// basis and handles a declaration moved verbatim inside a file correctly.
#[cfg(test)]
fn added_public_symbols(before: &str, after: &str) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut symbols = Vec::new();
    let before_lines: BTreeSet<&str> = before.lines().map(str::trim_end).collect();
    for line in after.lines() {
        let trimmed = line.trim_end();
        if before_lines.contains(trimmed) {
            continue;
        }
        if let Some(symbol) = public_declaration_symbol(trimmed) {
            symbols.push(symbol);
        }
    }
    symbols
}

/// `^\s*(def|class)\s+([A-Za-z]\w*)\s*[\(:]`, minus a leading underscore.
fn public_declaration_symbol(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("def ")
        .or_else(|| trimmed.strip_prefix("class "))?;
    let name: String = rest
        .chars()
        .take_while(|value| value.is_alphanumeric() || *value == '_')
        .collect();
    let mut chars = name.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() || first == '_' {
        return None;
    }
    let after = &rest[name.len()..];
    let opener = after.trim_start().chars().next()?;
    if opener != '(' && opener != ':' {
        return None;
    }
    Some(name)
}

/// The Python corpus the scan searches, deterministically.
fn python_corpus(root: &Path, changed: &[String]) -> Result<Vec<(String, String)>, String> {
    let search_root = if !changed.is_empty() && changed.iter().all(|path| path.starts_with("src/"))
    {
        root.join("src")
    } else {
        root.to_path_buf()
    };
    let mut files = Vec::new();
    collect_python_files(&search_root, &mut files)?;
    files.sort();
    let mut corpus = Vec::new();
    for path in files {
        let relative = relative(root, &path)
            .ok_or_else(|| format!("{} is outside the repository root", path.display()))?;
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => corpus.push((relative, text)),
                Err(_) => return Err(format!("{relative} is not valid UTF-8")),
            },
            Err(error) => return Err(format!("could not read {relative}: {error}")),
        }
    }
    Ok(corpus)
}

const EXCLUDED_CORPUS_PARTS: &[&str] = &[
    ".git",
    ".nox",
    ".pytest_cache",
    ".tox",
    ".venv",
    ".worktrees",
    "node_modules",
    "venv",
];

fn collect_python_files(directory: &Path, into: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if EXCLUDED_CORPUS_PARTS.contains(&name.as_ref()) {
                continue;
            }
            collect_python_files(&path, into)?;
        } else if path.extension().is_some_and(|value| value == "py") {
            into.push(path);
        }
    }
    Ok(())
}

/// `spec-kitty accept`.
///
/// The Source-compatible pipeline (merge-commit verification -> origin gate ->
/// fact collection -> policy judgement -> matrix execution -> acceptance record)
/// lives in [`super::acceptance`]. This entry stays the callable lifecycle
/// surface the dispatcher and the existing callers use; it holds no second
/// acceptance policy.
pub fn accept(root: &Path, args: &AcceptArgs) -> Result<(Value, u8), CompatError> {
    super::acceptance::accept(root, args)
}

/// Source `resolve_acceptance_actor`.
pub(crate) fn resolve_acceptance_actor(actor: Option<&str>) -> String {
    let candidate = actor
        .map(str::to_owned)
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_else(|| "system".to_owned());
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        "system".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The Source acceptance mode actually recorded (`local` / `pr` / `checklist`).
pub(crate) fn resolved_mode(args: &AcceptArgs, mission: &Mission) -> Result<String, CompatError> {
    match args.mode.as_deref().map(str::trim) {
        Some(mode) if !mode.is_empty() && mode != "auto" => Ok(mode.to_lowercase()),
        _ => Ok(match args.merge_commit {
            Some(_) => "pr".to_owned(),
            None => mission
                .meta_field("acceptance_mode")
                .and_then(Value::as_str)
                .filter(|mode| matches!(*mode, "pr" | "local" | "checklist"))
                .unwrap_or("local")
                .to_owned(),
        }),
    }
}

/// Commit exactly `paths` on the current branch with `message` (the acceptance
/// bookkeeping seam: the matrix is excluded because it is committed by the
/// finalize step itself).
pub(crate) fn commit_acceptance_paths(
    root: &Path,
    mission: &Mission,
    paths: &[PathBuf],
    message: &str,
    allow_empty: bool,
) -> Result<(), CompatError> {
    let repository = GitRepository::open(root)
        .map_err(|error| CompatError::domain(format!("could not open the repository: {error}")))?;
    let Some(branch_text) = repository
        .current_branch()
        .map_err(|error| CompatError::domain(format!("could not read HEAD: {error}")))?
    else {
        return Err(CompatError::coded(
            "ACCEPTANCE_NOT_RECORDED",
            "the acceptance bookkeeping needs a branch checkout",
        ));
    };
    let branch = BranchName::parse(&branch_text)
        .map_err(|error| CompatError::coded("DESTINATION_REF_INVALID", error.to_string()))?;
    let mut git_paths: Vec<GitPath> = Vec::new();
    for path in paths {
        let Some(rel) = relative(root, path) else {
            continue;
        };
        git_paths.push(
            GitPath::from_git_bytes(rel.as_bytes()).map_err(|error| {
                CompatError::coded("ACCEPTANCE_PATH_INVALID", error.to_string())
            })?,
        );
    }
    if git_paths.is_empty() {
        return Ok(());
    }
    let request = sk_git::SafeCommitRequest::new(branch, message, git_paths);
    let plan = match repository.plan_safe_commit(&request) {
        Ok(plan) => plan,
        // The artifact is already at the judged bytes on HEAD: nothing to
        // publish. Callers that require a new commit (the acceptance metadata)
        // keep the refusal; the matrix finalize tolerates it.
        Err(sk_git::GitError::SafeCommitEmptyChangeset) if allow_empty => return Ok(()),
        Err(sk_git::GitError::SafeCommitEmptyChangeset) => {
            return Err(CompatError::coded(
                "ACCEPTANCE_NOT_RECORDED",
                "the acceptance commit is empty",
            ));
        }
        Err(error) => {
            return Err(CompatError::domain(format!(
                "could not plan the acceptance commit: {error}"
            )));
        }
    };
    repository
        .apply_safe_commit(&plan)
        .map(|_outcome| ())
        .map_err(|error| {
            CompatError::domain(format!("could not apply the acceptance commit: {error}"))
        })?;
    let _ = mission;
    Ok(())
}

/// The branch whose status evidence the gate compares (Source's placement seam
/// `write_target(STATUS_STATE)`): the mission's integration branch when the
/// mission names one, else the branch this checkout stands on.
pub(crate) fn evidence_branch(
    mission: &Mission,
    repository: &GitRepository,
) -> Result<Option<BranchName>, CompatError> {
    for field in ["mission_branch", "target_branch"] {
        if let Some(name) = mission
            .meta_field(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let branch = BranchName::parse(name).map_err(|error| {
                CompatError::domain(format!("{field} {name:?} is not a branch: {error}"))
            })?;
            if repository
                .read_branch_tip(&branch)
                .map_err(|error| CompatError::domain(format!("could not read {name}: {error}")))?
                .is_some()
            {
                return Ok(Some(branch));
            }
        }
    }
    let Some(name) = repository
        .current_branch()
        .map_err(|error| CompatError::domain(format!("could not read HEAD: {error}")))?
    else {
        return Ok(None);
    };
    BranchName::parse(&name)
        .map(Some)
        .map_err(|error| CompatError::domain(format!("HEAD is not a branch: {error}")))
}

/// The code-lane branches the gate covers (Source `approved_lane_branches`):
/// every non-planning lane that is not fully canceled. An unreadable lane
/// manifest or status log over-selects (fewer lanes are skipped), never
/// under-selects, exactly as Source documents.
pub(crate) fn origin_lane_branches(
    manifest: Option<&sk_workspace::LanesManifest>,
    lanes: &[(String, String)],
) -> Vec<String> {
    let Some(manifest) = manifest else {
        return Vec::new();
    };
    let canceled: Vec<&str> = lanes
        .iter()
        .filter(|(_, lane)| lane == "canceled")
        .map(|(wp_id, _)| wp_id.as_str())
        .collect();
    let mut branches = Vec::new();
    for lane in &manifest.lanes {
        if lane.is_planning() {
            continue;
        }
        if !lane.wp_ids.is_empty()
            && lane
                .wp_ids
                .iter()
                .all(|wp_id| canceled.iter().any(|canceled| *canceled == wp_id.as_str()))
        {
            continue;
        }
        if let Ok(branch) = sk_workspace::naming::lane_branch_name(
            &manifest.mission_slug,
            &lane.lane_id,
            &manifest.target_branch,
        ) && !branches.iter().any(|existing| existing == &branch)
        {
            branches.push(branch);
        }
    }
    branches
}

/// Resolve the origin-freshness policy and gate the Mission's evidence branch
/// (and its code lanes) with SOURCE'S OWN observation: a bounded `ls-remote`
/// plus the bounded `fetch` of the listed tips, then the tracking-ref compare.
///
/// This is Source's `run_origin_gate` policy path: `off` contacts nothing and
/// returns the one not-checked warning; `warn` turns every refusal into a
/// warning naming the resolved setting; `enforce` refuses with Source's codes,
/// headline and operator text. Nothing is written except tracking refs.
pub(crate) fn origin_gate(
    root: &Path,
    mission: &Mission,
    flag: Option<&str>,
    manifest: Option<&sk_workspace::LanesManifest>,
    lanes: &[(String, String)],
) -> Result<Vec<String>, CompatError> {
    let environment = std::env::var(sk_git::ORIGIN_CHECK_ENV).ok();
    let setting = sk_git::resolve_origin_check_mode(flag, environment.as_deref())
        .map_err(CompatError::domain)?;
    if setting.mode == sk_git::OriginCheckMode::Off {
        return Ok(vec![sk_git::origin_not_checked_warning(&setting)]);
    }
    let repository = match GitRepository::open(root) {
        Ok(repository) => repository,
        // A repository that genuinely is not a checkout has no origin to be
        // stale against; that is absence, not a stale verdict.
        Err(sk_git::GitError::NotARepository { .. } | sk_git::GitError::BareRepository { .. }) => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(CompatError::coded(
                "REPOSITORY_REQUIRED",
                format!(
                    "{} could not be opened as a Git checkout: {error}",
                    root.display()
                ),
            ));
        }
    };
    let Some(branch) = evidence_branch(mission, &repository)? else {
        return Ok(Vec::new());
    };
    let lane_branches = origin_lane_branches(manifest, lanes);
    let mut wanted: Vec<&str> = vec![branch.as_str()];
    for lane in &lane_branches {
        if !wanted.contains(&lane.as_str()) {
            wanted.push(lane.as_str());
        }
    }
    let verdicts = sk_git::refresh_branch_facts(&repository, &wanted).map_err(|error| {
        CompatError::coded(
            "ORIGIN_FRESHNESS_FAILED",
            format!("the origin freshness check could not be completed: {error}"),
        )
    })?;
    let evidence = verdicts.first();
    let lanes = verdicts.get(1..).unwrap_or_default();
    sk_git::enforce_merge_gate(evidence, lanes, &setting, false, None).map_err(|refused| {
        CompatError::coded(
            refused.error_code,
            format!(
                "{}\nerror_codes: {}",
                refused.message,
                refused.error_codes.join(",")
            ),
        )
    })
}

pub(crate) fn accept_origin_gate(
    root: &Path,
    mission: &Mission,
    args: &AcceptArgs,
) -> Result<Vec<String>, CompatError> {
    // Source's `off` "contacts nothing and accepts stale evidence", so the gate
    // returns before it reads any evidence. Keeping that order matters: an
    // explicit opt-out must not be the reason a corrupt work-package-only
    // surface fails an acceptance run that never judges work packages.
    if args
        .origin_check
        .as_deref()
        .map(str::trim)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("off"))
    {
        return Ok(Vec::new());
    }
    let manifest = lane_manifest(mission)?;
    let rows = status_rows(mission)?;
    let lanes = work_package_lanes(&rows)?;
    origin_gate(
        root,
        mission,
        args.origin_check.as_deref(),
        manifest.as_ref(),
        &lanes,
    )
}

/// Validate `--merge-commit` BEFORE any acceptance write (Source `_verify_merge_commit`).
pub(crate) fn verify_accept_merge_commit(
    root: &Path,
    mission: &Mission,
    args: &AcceptArgs,
) -> Result<Option<sk_git::PrMergeEvidence>, CompatError> {
    let Some(merge_commit) = args.merge_commit.as_deref() else {
        if args.attest_first_landing_commit {
            return Err(CompatError::coded(
                "MERGE_COMMIT_REQUIRED",
                "--attest-first-landing-commit is only valid with --merge-commit".to_owned(),
            ));
        }
        if args.target_branch.is_some() {
            return Err(CompatError::coded(
                "MERGE_COMMIT_REQUIRED",
                "--target-branch is only valid with --merge-commit".to_owned(),
            ));
        }
        return Ok(None);
    };
    let mode = args.mode.as_deref().unwrap_or("local");
    if mode != "pr" {
        return Err(CompatError::coded(
            "MERGE_COMMIT_REQUIRES_PR_MODE",
            format!(
                "--merge-commit is only valid with --mode pr (resolved mode: {mode}). A {mode}                  acceptance records its baseline_merge_commit through `spec-kitty consolidate`,                  not here."
            ),
        ));
    }
    let target_ref = match args.target_branch.as_deref().map(str::trim) {
        Some(supplied) if !supplied.is_empty() => supplied.to_owned(),
        Some(_) => {
            return Err(CompatError::coded(
                "TARGET_BRANCH_EMPTY",
                "the supplied target branch is empty".to_owned(),
            ));
        }
        None => mission
            .meta()
            .get("target_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| "main".to_owned()),
    };
    if target_ref.starts_with('-') {
        return Err(CompatError::coded(
            "TARGET_BRANCH_INVALID",
            format!("{target_ref:?} is not a branch name (it looks like a git option)"),
        ));
    }
    let repository = GitRepository::open(root).map_err(|error| {
        CompatError::coded(
            "REPOSITORY_REQUIRED",
            format!(
                "{} could not be opened as a Git checkout: {error}",
                root.display()
            ),
        )
    })?;
    sk_git::verify_pr_merge_evidence(
        &repository,
        mission.slug().as_str(),
        merge_commit,
        &target_ref,
        args.attest_first_landing_commit,
    )
    .map(Some)
    .map_err(|error| {
        CompatError::coded(
            "PR_MERGE_EVIDENCE_REFUSED",
            format!(
                "Cannot record PR merge for {}: {error}",
                mission.slug().as_str()
            ),
        )
    })
}

/// `spec-kitty consolidate`.
pub fn consolidate(root: &Path, args: &ConsolidateArgs) -> Result<(Value, u8), CompatError> {
    let handle = require_mission(args.mission.as_deref(), "consolidate")?;
    let project = sk_workspace::ProjectWorkspace::open(root)
        .map_err(|error| CompatError::domain(format!("could not open the project: {error}")))?;
    let workspace = project
        .mission(&handle)
        .map_err(|error| CompatError::coded("MISSION_NOT_FOUND", error.to_string()))?;

    let strategy = match args.strategy.as_deref() {
        Some(value) => Some(MergeStrategy::parse(value).map_err(map_consolidation_error)?),
        None => None,
    };
    let request = ConsolidationRequest {
        target_branch: args.target.clone(),
        strategy,
        delete_branch: if args.delete_branch {
            Some(true)
        } else if args.keep_branch {
            Some(false)
        } else {
            None
        },
        remove_worktree: if args.remove_worktree {
            Some(true)
        } else if args.keep_worktree {
            Some(false)
        } else {
            None
        },
        dry_run: args.dry_run,
        resume: args.resume,
        abort: args.abort,
        skip_lanes: args.skip_lanes,
    };

    let rows = status_rows(workspace.mission())?;
    let lanes = work_package_lanes(&rows)?;
    // Source runs the origin gate BEFORE the status directory is resolved and
    // before any branch moves (`consolidation/executor.py::_enforce_origin_gate`),
    // so a refusal leaves the target, the lanes and the status surface untouched.
    // `--abort` and `--dry-run` are read-only planning paths: Source's abort
    // dispatch returns before the gate, and a dry run must not need a remote.
    if !args.abort && !args.dry_run {
        let manifest = lane_manifest(workspace.mission())?;
        let warnings = origin_gate(
            root,
            workspace.mission(),
            args.origin_check.as_deref(),
            manifest.as_ref(),
            &lanes,
        )?;
        for warning in &warnings {
            eprintln!("{warning}");
        }
    }
    let report =
        consolidate_mission(&workspace, &lanes, &request).map_err(map_consolidation_error)?;
    render_consolidation(&report, args.json)?;
    Ok((report.to_json(), 0))
}

fn map_consolidation_error(error: sk_workspace::ConsolidationError) -> CompatError {
    match &error {
        sk_workspace::ConsolidationError::Refused { code, message } => {
            CompatError::coded(static_code(code), message.clone())
        }
        sk_workspace::ConsolidationError::Workspace(inner) => match inner.as_ref() {
            sk_workspace::WorkspaceError::LaneNotFound { wp } => CompatError::coded(
                "LANE_NOT_FOUND",
                format!("work package {wp} is not assigned to any execution lane"),
            ),
            other => CompatError::domain(other.to_string()),
        },
        other => CompatError::domain(other.to_string()),
    }
}

/// `CompatError::coded` needs a `&'static str`; keep the closed set explicit so
/// an unknown code is still a stable, documented one.
fn static_code(code: &str) -> &'static str {
    match code {
        "MERGE_STRATEGY_UNSUPPORTED" => "MERGE_STRATEGY_UNSUPPORTED",
        "MERGE_STRATEGY_INVALID" => "MERGE_STRATEGY_INVALID",
        "DESTINATION_REF_INVALID" => "DESTINATION_REF_INVALID",
        "MISSION_ALREADY_CONSOLIDATED" => "MISSION_ALREADY_CONSOLIDATED",
        "MISSION_NOT_MERGE_READY" => "MISSION_NOT_MERGE_READY",
        "MISSING_LANES" => "MISSING_LANES",
        "CONSOLIDATION_CHECKOUT_REQUIRED" => "CONSOLIDATION_CHECKOUT_REQUIRED",
        "CONSOLIDATION_DIRTY_CHECKOUT" => "CONSOLIDATION_DIRTY_CHECKOUT",
        "CONSOLIDATION_TARGET_MISSING" => "CONSOLIDATION_TARGET_MISSING",
        "CONSOLIDATION_BASELINE_INVALID" => "CONSOLIDATION_BASELINE_INVALID",
        "CONSOLIDATION_RESUME_UNLANDED_LANES" => "CONSOLIDATION_RESUME_UNLANDED_LANES",
        "CONSOLIDATION_MERGE_CONFLICT" => "CONSOLIDATION_MERGE_CONFLICT",
        "CONSOLIDATION_MERGE_UNCONFIRMED" => "CONSOLIDATION_MERGE_UNCONFIRMED",
        "CONSOLIDATION_FAILED_WITH_JOURNAL_FAULT" => "CONSOLIDATION_FAILED_WITH_JOURNAL_FAULT",
        "CONSOLIDATION_JOURNAL_CORRUPT" => "CONSOLIDATION_JOURNAL_CORRUPT",
        "CONSOLIDATION_ALREADY_LANDED" => "CONSOLIDATION_ALREADY_LANDED",
        "NOTHING_TO_ABORT" => "NOTHING_TO_ABORT",
        _ => "CONSOLIDATION_REFUSED",
    }
}

fn render_consolidation(
    report: &ConsolidationReport,
    json_output: bool,
) -> Result<(), CompatError> {
    if json_output {
        emit_json(&report.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
        return Ok(());
    }
    if report.already_consolidated {
        println!(
            "Mission {} is already consolidated at {} on {}",
            report.mission_slug, report.baseline_commit, report.target_branch
        );
        return Ok(());
    }
    if report.dry_run {
        println!(
            "Dry run: {} lane(s) would land on {} (strategy {})",
            report.lanes.len(),
            report.target_branch,
            report.strategy
        );
    } else {
        println!(
            "Consolidated {} onto {} (strategy {})",
            report.mission_slug, report.target_branch, report.strategy
        );
    }
    for lane in &report.lanes {
        match &lane.reason {
            Some(reason) => println!("  {:<12} {:<18} {}", lane.lane_id, lane.outcome, reason),
            None => println!("  {:<12} {:<18}", lane.lane_id, lane.outcome),
        }
    }
    for action in &report.teardown {
        match &action.reason {
            Some(reason) => println!(
                "  teardown {:<18} {} ({reason})",
                action.action, action.target
            ),
            None => println!("  teardown {:<18} {}", action.action, action.target),
        }
    }
    println!("Baseline recorded: {}", report.baseline_commit);
    if let Some(fault) = &report.journal_fault {
        println!("Journal fault (the landing stands): {fault}");
    }
    Ok(())
}

/// `spec-kitty merge` — the Source rename stub.
pub fn merge(args: &MergeArgs) -> Result<u8, CompatError> {
    // Every former flag is accepted (clap swallows it) and refused identically:
    // the Source stub never inspects them.
    let _ = (args.json, args.args.len());
    Err(CompatError::coded("MERGE_RENAMED", MERGE_RENAMED_MESSAGE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_declarations_are_detected_and_private_ones_are_not() {
        assert_eq!(
            public_declaration_symbol("def helper(a, b):"),
            Some("helper".to_owned())
        );
        assert_eq!(
            public_declaration_symbol("    class Widget:"),
            Some("Widget".to_owned())
        );
        assert_eq!(public_declaration_symbol("def _private():"), None);
        assert_eq!(public_declaration_symbol("def 9lives():"), None);
        assert_eq!(public_declaration_symbol("# def commented():"), None);
    }

    #[test]
    fn an_added_symbol_is_only_reported_when_it_is_new() {
        let before = "def existing():\n    pass\n";
        let after = "def existing():\n    pass\n\ndef added():\n    pass\n";
        assert_eq!(
            added_public_symbols(before, after),
            vec!["added".to_owned()]
        );
        assert!(added_public_symbols(after, after).is_empty());
    }

    #[test]
    fn only_python_source_paths_are_supported() {
        assert!(is_supported_python_path("src/demo/x.py"));
        assert!(is_supported_python_path("scripts/x.py"));
        assert!(!is_supported_python_path("tests/test_x.py"));
        assert!(!is_supported_python_path("src/demo/x.rs"));
        assert!(!sk_mission::is_test_only_path("src/attestation.py"));
    }

    #[test]
    fn the_merge_stub_is_the_source_message() {
        let args = MergeArgs {
            json: false,
            args: vec!["--resume".to_owned()],
        };
        let error = merge(&args).unwrap_err();
        assert_eq!(error.message(), MERGE_RENAMED_MESSAGE);
    }
}
