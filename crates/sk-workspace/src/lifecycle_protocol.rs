//! The post-landing lifecycle of a consolidation run: the Source phases that wrap
//! the landing itself.
//!
//! Source order (pinned `consolidation/executor.py`):
//!
//! 1. land the mission branch on the target (`squash`/`merge`/`rebase`);
//! 2. reconciliation guard — a merge/rebase landing must leave every approved
//!    lane commit reachable from the target; a squash landing reports the
//!    attribution instead (`lanes/consolidation.py`);
//! 3. mission-number bake + `meta.json` projection (`merged_commit`,
//!    `merged_at`, `baseline_merge_commit`, `status_phase`) — `phase_advance.py`
//!    / `phase_bookkeeping.py`;
//! 4. record each merged work package `approved -> done` in the canonical status
//!    log, one bookkeeping commit per transition plus the batch commit
//!    (`done_bookkeeping.py`);
//! 5. the one-directory coordination alias fold (`bookkeeping_projection.py`);
//! 6. teardown (lane worktrees/branches, integration branch);
//! 7. the retrospective capture commit (`post_merge/retrospective_terminus.py`).
//!
//! Any failure in step 2-7 after the landing is the **rollback door**: every
//! branch the run moved is restored to its pre-consolidation commit with a CAS
//! against the observed tip, the recorded baseline is cleared, and the run
//! reports what it restored. Nothing is torn down before the gate that can
//! refuse, and a branch whose observed tip is not the one this run left is
//! reported `NOT restored` instead of being overwritten.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sk_git::{BranchName, GitError, GitPath, GitRepository, Oid, SafeCommitRequest, StartPoint};
use sk_mission::Mission;
use sk_status::mutation_emit::{ActorValue, TransitionRequest, emit_status_transition};

use crate::retrospective::{
    RetrospectiveError, RetrospectiveRequest, generate_retrospective, retrospective_exists,
    write_retrospective_record,
};

/// A lifecycle failure after a landing.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error(transparent)]
    /// A Git adapter failure.
    Git(#[from] GitError),
    #[error(transparent)]
    /// A Mission metadata failure.
    Mission(#[from] sk_mission::MissionError),
    #[error(transparent)]
    /// The canonical status writer refused or failed.
    Status(#[from] sk_status::MutationError),
    #[error(transparent)]
    /// The retrospective capture failed.
    Retrospective(#[from] RetrospectiveError),
    #[error("{0}")]
    /// A typed lifecycle refusal (nothing was changed when it is returned).
    Refused(String),
}

/// One merged work package the done recording covers.
#[derive(Clone, Debug)]
pub struct MergedWorkPackage {
    /// The work package id.
    pub wp_id: String,
    /// The lane branch tip this work package landed from, when it has one.
    pub lane_head: Option<Oid>,
    /// The reviewer of record (the latest approval's reviewer).
    pub reviewer: String,
}

/// The outcome of the done recording.
#[derive(Clone, Debug, Default)]
pub struct DoneRecording {
    /// The work packages this run recorded `done`.
    pub recorded: Vec<String>,
    /// The per-transition bookkeeping commits.
    pub transition_commits: Vec<Oid>,
    /// The batch commit that carries the `meta.json` projection.
    pub metadata_commit: Option<Oid>,
}

/// The Source consolidation metadata projection.
#[derive(Clone, Debug)]
pub struct ConsolidationMetadata<'a> {
    /// The pre-landing target tip.
    pub baseline: &'a str,
    /// The landing commit on the target.
    pub merged_commit: &'a str,
    /// The merge timestamp.
    pub merged_at: &'a str,
    /// The assigned Mission number.
    pub mission_number: i64,
}

/// Record every merged work package `approved -> done` through the canonical
/// status writer, committing each transition and then the batch commit that
/// carries the `meta.json` projection.
///
/// Source (`done_bookkeeping._mark_wp_merged_done`): actor `{role: system,
/// tool: merge}`, `reason = "Merged <WP> into <target>"`, evidence
/// `review = {reference: "lane-approved:<WP>", reviewer, verdict: "approved"}`,
/// `policy_metadata = {merge_phase: "lane_integrated", target_branch, lane_head}`.
pub fn record_done_transitions(
    mission: &Mission,
    repository: &GitRepository,
    target: &BranchName,
    packages: &[MergedWorkPackage],
    metadata: &ConsolidationMetadata<'_>,
) -> Result<DoneRecording, LifecycleError> {
    let feature_dir = mission.spec_dir().to_path_buf();
    let slug = mission.slug().as_str().to_owned();
    let mut recording = DoneRecording::default();

    for package in packages {
        // Source retries inspect the CURRENT canonical lane, not whether any
        // historical done event exists (a reopened WP must be recorded again).
        let log_path = feature_dir.join("status.events.jsonl");
        let rows = match sk_status::read_raw_events(&log_path) {
            Ok(rows) => rows,
            Err(sk_status::SnapshotError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                Vec::new()
            }
            Err(error) => {
                return Err(LifecycleError::Refused(format!(
                    "could not read current WP state from {}: {error}",
                    log_path.display()
                )));
            }
        };
        let snapshot = sk_status::reduce_rows(&rows).map_err(|error| {
            LifecycleError::Refused(format!("could not reduce current WP state: {error}"))
        })?;
        if snapshot
            .work_packages
            .get(&package.wp_id)
            .is_some_and(|state| state.lane_string() == Some("done"))
        {
            continue;
        }
        let mut evidence = json!({
            "review": {
                "reference": format!("lane-approved:{}", package.wp_id),
                "reviewer": if package.reviewer.is_empty() { "unknown" } else { package.reviewer.as_str() },
                "verdict": "approved",
            }
        });
        if let Some(head) = &package.lane_head {
            evidence["lane_head"] = json!(head.to_string());
        }
        let mut policy_metadata = json!({
            "merge_phase": "lane_integrated",
            "target_branch": target.as_str(),
        });
        if let Some(head) = &package.lane_head {
            policy_metadata["lane_head"] = json!(head.to_string());
        }

        let mut request = TransitionRequest::new(
            &feature_dir,
            slug.clone(),
            package.wp_id.clone(),
            "done",
            ActorValue::Structured {
                role: Some("system".to_owned()),
                profile: None,
                tool: Some("merge".to_owned()),
                model: None,
            },
        );
        request.mission_id = Some(mission.id().as_str().to_owned());
        request.reason = Some(format!("Merged {} into {}", package.wp_id, target.as_str()));
        request.evidence = Some(evidence);
        request.workspace_context = Some(format!("merge:{}", repository.root().display()));
        request.repo_root = Some(repository.root().to_path_buf());
        request.execution_mode = "worktree".to_owned();
        request.policy_metadata = Some(policy_metadata);
        request.default_workspace_context = false;

        emit_status_transition(&request)?;
        recording.recorded.push(package.wp_id.clone());
        if let Some(commit) = commit_mission_paths(
            repository,
            target,
            &feature_dir,
            &format!("chore(spec-kitty): status transition {}", package.wp_id),
        )? {
            recording.transition_commits.push(commit);
        }
    }

    sk_mission::record_consolidation_metadata(
        &feature_dir,
        metadata.baseline,
        metadata.merged_commit,
        metadata.merged_at,
        metadata.mission_number,
    )?;
    recording.metadata_commit = commit_mission_paths(
        repository,
        target,
        &feature_dir,
        &format!("chore({slug}): record done transitions for merged WPs"),
    )?;
    Ok(recording)
}

/// Capture (or heal) the retrospective record and commit it.
///
/// Source `post_merge/retrospective_terminus.py`: capture is idempotent (an
/// existing record is not regenerated), the commit is fail-open but always
/// performed for a dirty artifact, and the message is
/// `chore(<slug>): capture mission retrospective`.
pub fn capture_retrospective(
    mission: &Mission,
    repository: &GitRepository,
    target: &BranchName,
    invoked_at: &str,
) -> Result<Option<Oid>, LifecycleError> {
    let feature_dir = mission.spec_dir().to_path_buf();
    let slug = mission.slug().as_str().to_owned();
    let mut record = None;
    if !retrospective_exists(&feature_dir) {
        // Source's facilitator resolves the CONFIGURED policy first
        // (`retrospective.policy.resolve_policy`): charter.yaml authority, then
        // charter.md frontmatter, then .kittify/config.yaml, with the built-in
        // defaults as the last resort. A disabled policy is a no-op — no record,
        // no capture event — and the record's `policy_source` map is the resolved
        // one, never a blanket `<default>`.
        let resolved =
            crate::retrospective_policy::resolve(repository.root()).map_err(|error| {
                LifecycleError::Refused(format!(
                    "the retrospective policy could not be resolved: {error}"
                ))
            })?;
        if !resolved.policy.enabled {
            return Ok(None);
        }
        let request = RetrospectiveRequest {
            repo_root: repository.root(),
            feature_dir: &feature_dir,
            provenance_kind: "runtime_post_completion",
            invoked_at,
            policy_source: resolved.source_map,
        };
        let generated = generate_retrospective(&request)?;
        write_retrospective_record(&generated, &feature_dir)?;
        record = Some(generated);
    }
    // Source appends one `RetrospectiveCaptured` lifecycle row to the durable
    // log, so the capture is auditable even when the record is later edited. The
    // row is the FLAT schema-5 shape (not a lifecycle envelope), which is why it
    // goes through the flat emit door.
    if let Some(record) = &record
        && feature_dir.join("status.events.jsonl").is_file()
    {
        let request = sk_status::mutation_emit::RetrospectiveCapturedRequest {
            feature_dir: feature_dir.clone(),
            mission_id: record.mission_id.clone(),
            mission_slug: record.mission_slug.clone(),
            provenance_kind: record.provenance_kind.clone(),
            findings_status: record.findings_status.to_owned(),
            generator_version: record.generator_version.clone(),
            evidence_ref_count: record.evidence_refs.len(),
            proposal_count: 0,
            policy_source: record.policy_source.clone(),
            record_path: format!(
                "kitty-specs/{slug}/{}",
                crate::retrospective::RETROSPECTIVE_FILENAME
            ),
            at: Some(invoked_at.to_owned()),
            event_id: None,
            repo_root: Some(repository.root().to_path_buf()),
            lock_wait: None,
        };
        sk_status::mutation_emit::emit_retrospective_captured(&request).map_err(|error| {
            LifecycleError::Refused(format!(
                "the retrospective was captured but its lifecycle event could not be \
                     recorded: {error}"
            ))
        })?;
    }
    let artifacts = [
        feature_dir.join(crate::retrospective::RETROSPECTIVE_FILENAME),
        feature_dir.join("status.events.jsonl"),
        // The lifecycle append materializes the derived snapshot; committing it
        // with the capture keeps the landed tree clean (Source leaves none of its
        // own writes uncommitted).
        feature_dir.join("status.json"),
    ];
    let paths: Vec<GitPath> = artifacts
        .iter()
        .filter(|path| path.exists())
        .filter_map(|path| {
            let relative = path
                .strip_prefix(repository.root())
                .ok()
                .map(|value| value.to_string_lossy().replace('\\', "/"))?;
            GitPath::from_git_bytes(relative.as_bytes()).ok()
        })
        .collect();
    commit_paths(
        repository,
        target,
        &paths,
        &format!("chore({slug}): capture mission retrospective"),
    )
}

/// Commit exactly `paths` on `target` (the caller already proved they exist).
pub fn commit_paths(
    repository: &GitRepository,
    target: &BranchName,
    paths: &[GitPath],
    message: &str,
) -> Result<Option<Oid>, LifecycleError> {
    if paths.is_empty() {
        return Ok(None);
    }
    let request = SafeCommitRequest::new(target.clone(), message, paths.to_vec());
    let plan = match repository.plan_safe_commit(&request) {
        Ok(plan) => plan,
        Err(GitError::SafeCommitEmptyChangeset) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match repository.apply_safe_commit(&plan) {
        Ok(outcome) => Ok(Some(outcome.commit().clone())),
        Err(GitError::SafeCommitEmptyChangeset) => Ok(None),
        Err(GitError::SafeCommitLanded { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// `true` when `path`, relative to a Mission directory, is a coordination-kind
/// file (the native port of Source `coordination.coherence.is_coordination_kind_file`).
///
/// Source's kind table classifies the status pair, the decision log, the two gate
/// matrices, tracer files under `traces/` and `tasks/<wp>/review-cycle-*.md` as
/// COORD-partition (not PRIMARY). Planning and identity files are not.
pub fn is_coordination_kind_file(relative: &str) -> bool {
    let normalized = relative.trim_start_matches("./").replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or("");
    if matches!(
        name,
        "status.events.jsonl" | "status.json" | "decisions.events.jsonl" | "decisions.index.json"
    ) {
        return true;
    }
    if matches!(
        name,
        "acceptance-matrix.json" | "issue-matrix.json" | "issue-matrix.md"
    ) {
        return true;
    }
    if normalized.starts_with("traces/") && name.ends_with(".md") {
        return true;
    }
    if let Some(rest) = normalized.strip_prefix("tasks/")
        && let Some((_, tail)) = rest.split_once('/')
        && tail.starts_with("review-cycle-")
        && tail.ends_with(".md")
    {
        return true;
    }
    false
}

/// The one-directory coordination alias fold: remove the composed
/// `kitty-specs/<slug>-<mid8>` coordination directory from the target checkout
/// after proving every file it holds is already represented by the primary
/// Mission directory.
///
/// Proofs (Source `bookkeeping_projection`):
/// * `status.events.jsonl` — every `event_id` of the composed log is in the
///   primary log (the union the project just wrote);
/// * `status.json` — derived from that log, rematerialised into the primary dir;
/// * every other coordination-kind file — the primary directory holds identical
///   bytes at the same relative path, now or at `pre_mutation_tip`.
///
/// A symbolic link is never proven. Nothing is deleted when a proof fails.
pub struct AliasFold {
    /// The composed coordination directory.
    pub directory: PathBuf,
    /// The primary Mission directory.
    pub primary_directory: PathBuf,
    /// The coordination-kind files the fold may remove.
    pub files: Vec<PathBuf>,
}

/// A refusal from the alias fold. Nothing has been deleted when it is returned.
#[derive(Debug, thiserror::Error)]
pub enum AliasFoldRefusal {
    #[error(
        "the composed coordination directory {composed} was not removed, because its status log holds event(s) the primary Mission directory's event log lacks: {missing}. Nothing was deleted."
    )]
    /// The composed log holds an event the primary log lacks.
    StatusEventsNotPreserved {
        /// The composed coordination directory.
        composed: String,
        /// The event ids the primary log lacks.
        missing: String,
    },
    #[error(
        "the composed coordination directory {composed} was not removed, because {count} coordination file(s) in it are not also held, with the same content, by the primary Mission directory {primary}: {relpaths}. Nothing was deleted."
    )]
    /// A coordination-kind file is not proven redundant with the primary directory.
    FileNotPreserved {
        /// The composed coordination directory.
        composed: String,
        /// The primary Mission directory.
        primary: String,
        /// How many files are unproven.
        count: usize,
        /// The unproven repo-relative paths, comma separated.
        relpaths: String,
    },
}

/// Enumerate the coordination-kind files of the composed coordination directory.
pub fn coordination_alias_files(
    primary_root: &Path,
    composed_directory: &Path,
    primary_directory: &Path,
) -> Result<Option<AliasFold>, LifecycleError> {
    if composed_directory == primary_directory {
        return Ok(None);
    }
    let repository = GitRepository::open(primary_root)?;
    let Ok(relative) = composed_directory.strip_prefix(primary_root) else {
        return Ok(None);
    };
    let prefix = format!("{}/", relative.to_string_lossy().replace('\\', "/"));
    let mut files = Vec::new();
    for entry in repository.status()? {
        if entry.is_untracked() {
            continue;
        }
        let rendered = entry.path().display_lossy();
        if !rendered.starts_with(&prefix) {
            continue;
        }
        let tail = &rendered[prefix.len()..];
        if !is_coordination_kind_file(tail) {
            continue;
        }
        let path = primary_root.join(&rendered);
        if path.is_file() || path.is_symlink() {
            files.push(path);
        }
    }
    files.sort();
    if files.is_empty() {
        return Ok(None);
    }
    Ok(Some(AliasFold {
        directory: composed_directory.to_path_buf(),
        primary_directory: primary_directory.to_path_buf(),
        files,
    }))
}

fn event_ids(path: &Path) -> Result<BTreeSet<String>, LifecycleError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        LifecycleError::Refused(format!(
            "could not read {} while proving the coordination fold: {error}",
            path.display()
        ))
    })?;
    let mut ids = BTreeSet::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(line).map_err(|error| {
            LifecycleError::Refused(format!(
                "{} holds a line that is not an event with an event_id ({error}); its events cannot be proven present",
                path.display()
            ))
        })?;
        let Some(id) = row.get("event_id").and_then(Value::as_str) else {
            return Err(LifecycleError::Refused(format!(
                "{} holds a line without a string event_id; its events cannot be proven present",
                path.display()
            )));
        };
        ids.insert(id.to_owned());
    }
    Ok(ids)
}

/// Prove every file of `fold` redundant, then unlink them and the directories the
/// unlinks emptied (never a recursive delete).
pub fn fold_coordination_alias(
    repository: &GitRepository,
    fold: &AliasFold,
    pre_mutation_tip: Option<&Oid>,
) -> Result<Vec<PathBuf>, LifecycleError> {
    let alias_log = fold.directory.join("status.events.jsonl");
    let primary_log = fold.primary_directory.join("status.events.jsonl");
    if alias_log.is_file() && !alias_log.is_symlink() {
        let composed = event_ids(&alias_log)?;
        let primary = event_ids(&primary_log)?;
        let missing: Vec<String> = composed.difference(&primary).cloned().collect();
        if !missing.is_empty() {
            return Err(LifecycleError::Refused(
                AliasFoldRefusal::StatusEventsNotPreserved {
                    composed: fold.directory.display().to_string(),
                    missing: missing.join(", "),
                }
                .to_string(),
            ));
        }
    }
    let mut unproven: Vec<String> = Vec::new();
    for path in &fold.files {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if name == "status.json" || (name == "status.events.jsonl" && !path.is_symlink()) {
            continue;
        }
        if path.is_symlink() {
            unproven.push(
                path.strip_prefix(&fold.directory)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
            continue;
        }
        let relative = path
            .strip_prefix(&fold.directory)
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let content = std::fs::read(path).map_err(|error| {
            LifecycleError::Refused(format!("could not read {}: {error}", path.display()))
        })?;
        let primary = fold.primary_directory.join(&relative);
        if primary.is_file()
            && !primary.is_symlink()
            && std::fs::read(&primary).ok().as_deref() == Some(content.as_slice())
        {
            continue;
        }
        let mut proven_by_tip = false;
        if let Some(tip) = pre_mutation_tip {
            let spec_relative = fold
                .primary_directory
                .strip_prefix(repository.root())
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let full = format!("{spec_relative}/{relative}");
            if let Ok(git_path) = GitPath::from_git_bytes(full.as_bytes())
                && let Ok(Some(blob)) =
                    repository.read_blob_at(&StartPoint::Oid(tip.clone()), &git_path)
                && blob == content
            {
                proven_by_tip = true;
            }
        }
        if !proven_by_tip {
            unproven.push(relative);
        }
    }
    if !unproven.is_empty() {
        unproven.sort();
        return Err(LifecycleError::Refused(
            AliasFoldRefusal::FileNotPreserved {
                composed: fold.directory.display().to_string(),
                primary: fold.primary_directory.display().to_string(),
                count: unproven.len(),
                relpaths: unproven.join(", "),
            }
            .to_string(),
        ));
    }
    let mut removed = Vec::new();
    for path in &fold.files {
        std::fs::remove_file(path).map_err(|error| {
            LifecycleError::Refused(format!("could not remove {}: {error}", path.display()))
        })?;
        removed.push(path.clone());
        let mut directory = path.parent().map(Path::to_path_buf);
        while let Some(candidate) = directory {
            if fold
                .directory
                .parent()
                .is_some_and(|parent| candidate.as_path() == parent)
            {
                break;
            }
            if candidate == fold.directory || candidate.starts_with(&fold.directory) {
                match std::fs::remove_dir(&candidate) {
                    Ok(()) => {}
                    Err(_) => break,
                }
                directory = candidate.parent().map(Path::to_path_buf);
            } else {
                break;
            }
        }
    }
    Ok(removed)
}

/// A branch the rollback door inspected.
#[derive(Clone, Debug)]
pub struct RollbackEntry {
    /// The branch name.
    pub branch: String,
    /// `unchanged`, `restored`, `absent` or `NOT restored`.
    pub outcome: &'static str,
    /// The tip observed before the restore.
    pub observed: Option<String>,
    /// The pre-consolidation commit the branch was restored to.
    pub restored_to: Option<String>,
    /// Why a branch was not restored.
    pub detail: Option<String>,
}

/// The rollback door's report.
#[derive(Clone, Debug)]
pub struct RollbackReport {
    /// One entry per branch the rollback considered.
    pub entries: Vec<RollbackEntry>,
}

impl RollbackReport {
    /// How many branches this rollback restored.
    pub fn restored(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.outcome == "restored")
            .count()
    }
    /// How many branches are `NOT restored`.
    pub fn failed(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.outcome == "NOT restored")
            .count()
    }
    /// The report as JSON.
    pub fn to_json(&self) -> Value {
        json!({
            "entries": self.entries.iter().map(|entry| json!({
                "branch": entry.branch,
                "outcome": entry.outcome,
                "observed": entry.observed,
                "restored_to": entry.restored_to,
                "detail": entry.detail,
            })).collect::<Vec<_>>(),
            "restored": self.restored(),
            "failed": self.failed(),
        })
    }
}

/// Restore every branch the run moved to its pre-consolidation commit.
///
/// Each restore is a CAS ref transaction against the tip this run observed: a
/// branch another process advanced since is reported `NOT restored` and left
/// alone, never overwritten. A branch already at its pre-run commit is
/// `unchanged`.
pub fn rollback_to_pre_run(
    repository: &GitRepository,
    target: &BranchName,
    observed_target: Option<&Oid>,
    pre_target: &Oid,
    mission_branch: Option<(&BranchName, &Oid)>,
    lane_branches: &[(BranchName, Oid)],
) -> RollbackReport {
    let mut entries = Vec::new();
    let mut plan_entries: Vec<(BranchName, Oid, Oid)> = Vec::new();
    let consider = |branch: &BranchName,
                    expected_landed: Option<&Oid>,
                    pre: &Oid,
                    entries: &mut Vec<RollbackEntry>,
                    plan_entries: &mut Vec<(BranchName, Oid, Oid)>| {
        let observed = repository.read_branch_tip(branch).ok().flatten();
        let observed_text = observed.as_ref().map(ToString::to_string);
        match &observed {
            Some(tip) if tip == pre => entries.push(RollbackEntry {
                branch: branch.as_str().to_owned(),
                outcome: "unchanged",
                observed: observed_text,
                restored_to: None,
                detail: None,
            }),
            Some(tip) => {
                if expected_landed.is_some_and(|landed| landed != tip) {
                    entries.push(RollbackEntry {
                        branch: branch.as_str().to_owned(),
                        outcome: "NOT restored",
                        observed: observed_text,
                        restored_to: Some(pre.to_string()),
                        detail: Some(
                            "the branch holds a commit this run did not leave; it is never overwritten"
                                .to_owned(),
                        ),
                    });
                    return;
                }
                plan_entries.push((branch.clone(), tip.clone(), pre.clone()));
                entries.push(RollbackEntry {
                    branch: branch.as_str().to_owned(),
                    outcome: "restored",
                    observed: observed_text,
                    restored_to: Some(pre.to_string()),
                    detail: None,
                });
            }
            None => entries.push(RollbackEntry {
                branch: branch.as_str().to_owned(),
                outcome: "absent",
                observed: None,
                restored_to: None,
                detail: Some("the branch does not resolve".to_owned()),
            }),
        }
    };
    for (branch, pre) in lane_branches {
        consider(branch, None, pre, &mut entries, &mut plan_entries);
    }
    if let Some((branch, pre)) = mission_branch {
        consider(branch, None, pre, &mut entries, &mut plan_entries);
    }
    consider(
        target,
        observed_target,
        pre_target,
        &mut entries,
        &mut plan_entries,
    );

    for (branch, observed, pre) in plan_entries {
        let report = repository.restore_branch_ref(&branch, &observed, &pre);
        if let Err(error) = report
            && let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.branch == branch.as_str() && entry.outcome == "restored")
        {
            entry.outcome = "NOT restored";
            entry.detail = Some(error.to_string());
        }
    }
    RollbackReport { entries }
}

/// A PRIMARY-partition planning artifact (Source `is_primary_artifact_kind` on
/// the mission-directory kind table), excluding the driver-covered paths whose
/// reconciliation belongs to a merge driver.
pub fn is_primary_planning_path(relative: &str) -> bool {
    let normalized = relative.trim_start_matches("./").replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or("");
    if normalized == "decisions/index.json" {
        return false;
    }
    if matches!(
        name,
        "spec.md"
            | "plan.md"
            | "tasks.md"
            | "research.md"
            | "data-model.md"
            | "quickstart.md"
            | "analysis-report.md"
            | "mission-review-report.md"
            | "workflow-failures-log.md"
            | "retrospective.yaml"
    ) {
        return true;
    }
    if normalized.starts_with("tasks/") && name.ends_with(".md") {
        return true;
    }
    if (normalized.starts_with("checklists/") || normalized.starts_with("contracts/"))
        && (name.ends_with(".md") || normalized.starts_with("contracts/"))
    {
        return true;
    }
    false
}

/// The PRIMARY planning paths on which the **target** is newer than the mission
/// branch (Source `planning_recency.target_newer_primary_artifacts`).
///
/// Case 1: the target advanced the path since the merge base while the mission
/// branch did not (the lane copy is base-or-ancestor).
/// Case 3: both advanced it and the target's last commit has a strictly later —
/// or equal — committer date.
pub fn target_newer_planning_paths(
    repository: &GitRepository,
    target_tip: &Oid,
    source_tip: &Oid,
) -> Result<Vec<GitPath>, LifecycleError> {
    let Some(base) = repository.merge_base(target_tip, source_tip)? else {
        return Ok(Vec::new());
    };
    let target_changed = changed_paths(repository, &base, target_tip)?;
    let source_changed = changed_paths(repository, &base, source_tip)?;
    let mut result = Vec::new();
    for path in target_changed {
        let relative = path.display_lossy();
        if !is_primary_planning_path(&relative) {
            continue;
        }
        if !source_changed.contains(&path) {
            result.push(path);
            continue;
        }
        let target_time = repository.committer_timestamp(target_tip, &path)?;
        let source_time = repository.committer_timestamp(source_tip, &path)?;
        let target_wins = match (target_time, source_time) {
            (None, None) => false,
            (None, Some(_)) => false,
            (Some(_), None) => true,
            (Some(left), Some(right)) => left >= right,
        };
        if target_wins {
            result.push(path);
        }
    }
    Ok(result)
}

fn changed_paths(
    repository: &GitRepository,
    base: &Oid,
    tip: &Oid,
) -> Result<BTreeSet<GitPath>, LifecycleError> {
    let statuses = repository.diff_name_status(base, tip)?;
    Ok(statuses.into_iter().map(|entry| entry.path).collect())
}

/// Restore the target's bytes for every target-newer planning path in `worktree`
/// and stage them, so the squash commit keeps the target's refinement
/// (Source `_preserve_target_newer_planning_artifacts`, squash only).
pub fn preserve_target_newer_planning(
    repository: &GitRepository,
    worktree: &GitRepository,
    target_tip: &Oid,
    source_tip: &Oid,
) -> Result<Vec<String>, LifecycleError> {
    let paths = target_newer_planning_paths(repository, target_tip, source_tip)?;
    let mut restored = Vec::new();
    for path in paths {
        let Some(bytes) = repository.read_blob_at(&StartPoint::Oid(target_tip.clone()), &path)?
        else {
            continue;
        };
        let destination = worktree.root().join(path.display_lossy());
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                LifecycleError::Refused(format!(
                    "could not restore target-newer planning artifact {}: {error}",
                    path.display_lossy()
                ))
            })?;
        }
        std::fs::write(&destination, &bytes).map_err(|error| {
            LifecycleError::Refused(format!(
                "could not restore target-newer planning artifact {}: {error}",
                path.display_lossy()
            ))
        })?;
        worktree.stage_path(&path)?;
        restored.push(path.display_lossy());
    }
    Ok(restored)
}

/// A repository-relative path under the Mission directory that the run changed.
pub fn mission_relative_paths(
    repository: &GitRepository,
    feature_dir: &Path,
) -> Result<Vec<GitPath>, LifecycleError> {
    let Some(prefix) = feature_dir
        .strip_prefix(repository.root())
        .ok()
        .map(|value| value.to_string_lossy().replace('\\', "/"))
    else {
        return Ok(Vec::new());
    };
    let prefix = format!("{prefix}/");
    let mut paths = Vec::new();
    for entry in repository.status()? {
        if entry.is_untracked() {
            continue;
        }
        let rendered = entry.path().display_lossy();
        if rendered.starts_with(&prefix) {
            paths.push(entry.path().clone());
        }
    }
    Ok(paths)
}

/// Commit the Mission-directory paths the run changed, if any.
pub fn commit_mission_paths(
    repository: &GitRepository,
    target: &BranchName,
    feature_dir: &Path,
    message: &str,
) -> Result<Option<Oid>, LifecycleError> {
    let paths = mission_relative_paths(repository, feature_dir)?;
    if paths.is_empty() {
        return Ok(None);
    }
    let request = SafeCommitRequest::new(target.clone(), message, paths);
    let plan = match repository.plan_safe_commit(&request) {
        Ok(plan) => plan,
        Err(GitError::SafeCommitEmptyChangeset) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match repository.apply_safe_commit(&plan) {
        Ok(outcome) => Ok(Some(outcome.commit().clone())),
        Err(GitError::SafeCommitEmptyChangeset) => Ok(None),
        Err(GitError::SafeCommitLanded { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The next Mission number for `slug`: its recorded number when it already has a
/// positive one, otherwise one past the highest number recorded by any Mission in
/// `kitty-specs/` (Source's first-landing assignment).
pub fn assign_mission_number(primary_root: &Path, slug: &str, recorded: Option<i64>) -> i64 {
    if let Some(number) = recorded.filter(|value| *value > 0) {
        return number;
    }
    let mut highest = 0i64;
    let directory = primary_root.join("kitty-specs");
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let path = entry.path().join("meta.json");
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            match value.get("mission_number") {
                Some(Value::Number(number)) => {
                    if let Some(number) = number.as_i64() {
                        highest = highest.max(number);
                    }
                }
                Some(Value::String(text)) => {
                    if let Ok(number) = text.parse::<i64>() {
                        highest = highest.max(number);
                    }
                }
                _ => {}
            }
        }
    }
    let _ = slug;
    highest + 1
}

/// The reviewer of record per work package, from the canonical status rows.
///
/// The latest `review_result` for the WP names its reviewer; a WP whose approval
/// carries no reviewer falls back to the runtime agent the snapshot recorded, and
/// finally to `unknown` (Source `done_bookkeeping`).
pub fn reviewers_of_record(rows: &[Value]) -> BTreeMap<String, String> {
    let mut reviewers: BTreeMap<String, String> = BTreeMap::new();
    for row in rows {
        let Some(wp_id) = row.get("wp_id").and_then(Value::as_str) else {
            continue;
        };
        let result = row.get("review_result").or_else(|| {
            row.get("payload")
                .and_then(Value::as_object)
                .and_then(|payload| payload.get("review_result"))
        });
        let Some(result) = result.and_then(Value::as_object) else {
            continue;
        };
        let Some(reviewer) = result.get("reviewer").and_then(Value::as_str) else {
            continue;
        };
        if reviewer.trim().is_empty() {
            continue;
        }
        reviewers.insert(wp_id.to_owned(), reviewer.trim().to_owned());
    }
    for row in rows {
        let Some(wp_id) = row.get("wp_id").and_then(Value::as_str) else {
            continue;
        };
        if reviewers.contains_key(wp_id) {
            continue;
        }
        if let Some(agent) = row.get("agent").and_then(Value::as_str)
            && !agent.trim().is_empty()
        {
            reviewers.insert(wp_id.to_owned(), agent.trim().to_owned());
        }
    }
    reviewers
}
