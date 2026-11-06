//! PR-landing verification for `accept --merge-commit` (Source
//! `specify_cli/consolidation/baseline.py::verify_pr_merge_evidence`).
//!
//! A supplied landing commit becomes the mission's recorded baseline, which
//! anchors `review --mode post-merge`'s dead-code scan. A wrong anchor silently
//! skips part of the gate, so verification is strict and every fact is a
//! read-only git observation:
//!
//! 1. the value is a hexadecimal SHA (never a ref, never an option-looking
//!    string) and resolves to a real commit in this repository;
//! 2. `kitty-specs/<slug>/meta.json` exists AT the commit — the commit really
//!    carries this mission;
//! 3. the commit has a first parent (it is not a root commit);
//! 4. `kitty-specs/<slug>/meta.json` does NOT exist at that parent — the commit
//!    introduced the mission corpus (this also rejects handing over the mission
//!    branch head, whose parent already contains the mission);
//! 5. `kitty-specs/<slug>/` does not exist at the parent AT ALL (the
//!    staged-corpus shape would otherwise anchor past already-landed work);
//! 6. the commit is an ancestor of the target branch — it actually landed.
//!
//! Checks 1–6 prove landing and first introduction, never that the anchor is the
//! PRE-LANDING TARGET TIP. Post-landing history cannot distinguish a merge
//! performed on the target from an internal merge that was fast-forwarded
//! (identical graphs), nor a corpus-first stack landing from one whose
//! implementation commits preceded the corpus. Both shapes are refused unless
//! the operator supplies the explicit `--attest-first-landing-commit`
//! attestation, and the recorded [`PrMergeEvidence::anchor_evidence`] names the
//! attestation class rather than claiming a git proof.

use crate::error::GitError;
use crate::oid::Oid;
use crate::path::GitPath;
use crate::refname::StartPoint;
use crate::repository::GitRepository;

/// The anchor rests on the operator's attestation for a two-parent landing.
pub const ANCHOR_EVIDENCE_MERGE_COMMIT_PARENT_ATTESTED: &str = "merge-commit-parent-attested";
/// The anchor rests on the operator's attestation for a single-parent landing.
pub const ANCHOR_EVIDENCE_CORPUS_PARENT_ATTESTED: &str = "corpus-parent-attested";

/// A verified PR landing commit and the baseline it anchors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrMergeEvidence {
    /// The landing commit (`pr_merge_commit`).
    pub pr_merge_commit: Oid,
    /// Its first parent, the recorded `baseline_merge_commit` anchor.
    pub baseline_merge_commit: Oid,
    /// Which operator attestation the anchor rests on.
    pub anchor_evidence: &'static str,
}

impl PrMergeEvidence {
    /// Whether the landing commit had two parents (a merge shape).
    pub fn is_merge_landing(&self) -> bool {
        self.anchor_evidence == ANCHOR_EVIDENCE_MERGE_COMMIT_PARENT_ATTESTED
    }
}

/// Why a supplied landing commit was refused. Every message names the observed
/// facts; none of them is a generic "invalid input".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrMergeEvidenceError {
    /// The operator-facing refusal text.
    pub message: String,
    /// The git error behind the refusal, when one was observed.
    pub source: Option<String>,
}

impl PrMergeEvidenceError {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }
}

impl std::fmt::Display for PrMergeEvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PrMergeEvidenceError {}

/// `[0-9a-fA-F]{4,64}` — Source's shape guard, applied before any git argument.
fn is_hex_sha(value: &str) -> bool {
    (4..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A mission slug is one path segment under `kitty-specs/`.
fn is_safe_slug(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && !value.contains(['/', '\\', '\0'])
}

fn python_repr(value: &str) -> String {
    sk_kernel::python_json::python_repr(value)
}

impl GitRepository {
    /// Resolve a validated commit spec (a full or abbreviated hexadecimal SHA, or
    /// a plain branch/tracking ref name) to a commit id.
    ///
    /// `Ok(None)` when git cannot resolve it; a failed *invocation* stays an
    /// error, so "not a commit" is never confused with "git did not run".
    pub fn resolve_hex_commit(&self, sha: &str) -> Result<Option<Oid>, GitError> {
        if !is_hex_sha(sha) && !is_plain_ref(sha) {
            return Err(GitError::InvalidRefName {
                kind: "commit",
                name: sha.to_owned(),
                detail: "expected a hexadecimal SHA or a plain ref name",
            });
        }
        let spec = format!("{sha}^{{commit}}");
        let output = self.process().run(&["rev-parse", "--verify", &spec])?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = crate::path::utf8_field(&output.stdout, "resolved commit")?;
        let text = text.trim();
        Oid::parse(text).map(Some)
    }
}

/// Verify a supplied PR landing commit for `mission_slug` (see the module docs).
pub fn verify_pr_merge_evidence(
    repository: &GitRepository,
    mission_slug: &str,
    merge_commit: &str,
    target_ref: &str,
    attest_first_landing: bool,
) -> Result<PrMergeEvidence, PrMergeEvidenceError> {
    if !is_safe_slug(mission_slug) {
        return Err(PrMergeEvidenceError::message(format!(
            "refusing mission slug {}: it is not a single kitty-specs/ path segment",
            python_repr(mission_slug)
        )));
    }
    let supplied = merge_commit.trim();
    if supplied.is_empty() {
        return Err(PrMergeEvidenceError::message(
            "no merge commit was supplied",
        ));
    }
    if !is_hex_sha(supplied) {
        return Err(PrMergeEvidenceError::message(format!(
            "{} is not a commit SHA (expected a hexadecimal SHA, full or abbreviated, as shown on \
             the merged PR)",
            python_repr(supplied)
        )));
    }
    let pr_merge_commit = rev_verify(repository, supplied)?;

    let meta_path = format!("kitty-specs/{mission_slug}/meta.json");
    if !has_path_at(repository, &pr_merge_commit, &meta_path)? {
        return Err(PrMergeEvidenceError::message(format!(
            "commit {pr_merge_commit} does not carry {meta_path}; it is not the commit that landed \
             mission {mission_slug}"
        )));
    }

    let parents = repository
        .commit_parents(&pr_merge_commit)
        .map_err(git_failure)?;
    let Some(first_parent) = parents.first().cloned() else {
        return Err(PrMergeEvidenceError::message(format!(
            "commit {pr_merge_commit} has no first parent (it is a root commit) and cannot anchor a \
             baseline for mission {mission_slug}"
        )));
    };

    if has_path_at(repository, &first_parent, &meta_path)? {
        return Err(PrMergeEvidenceError::message(format!(
            "commit {pr_merge_commit} is not the landing commit for mission {mission_slug}: \
             {meta_path} already exists at its first parent {first_parent}. Supply the commit that \
             introduced the mission corpus on the target branch"
        )));
    }

    let mission_dir = format!("kitty-specs/{mission_slug}");
    if has_path_at(repository, &first_parent, &mission_dir)? {
        return Err(PrMergeEvidenceError::message(format!(
            "commit {pr_merge_commit} is not the landing commit for mission {mission_slug}: \
             {mission_dir}/ already exists at its first parent {first_parent} (staged corpus — part \
             of the mission landed before its meta.json). Anchoring there would skip the gate for \
             the already-landed part; supply the commit that introduced the mission corpus"
        )));
    }

    let target_tip = rev_verify(repository, target_ref)?;
    if !repository
        .is_ancestor(&pr_merge_commit, &target_tip)
        .map_err(git_failure)?
    {
        return Err(PrMergeEvidenceError::message(format!(
            "commit {pr_merge_commit} is not on target branch {}: it never landed there. Supply the \
             commit that landed mission {mission_slug} on {target_ref} — the merge or squash commit \
             shown on the merged PR. If the landing is newer than this checkout, fetch {target_ref} \
             first; if the PR targeted a different branch, pass it explicitly (--target-branch)",
            python_repr(target_ref)
        )));
    }

    let is_merge = parents.len() > 1;
    if !attest_first_landing {
        if is_merge {
            return Err(PrMergeEvidenceError::message(format!(
                "commit {pr_merge_commit} is a merge commit, but two parents alone do not prove its \
                 first parent {first_parent} is the pre-landing target tip: a merge performed on the \
                 target branch is graph-identical to an internal merge (the corpus branch merged \
                 into the implementation branch, the target then fast-forwarded to the result), \
                 whose first parent is an implementation commit — anchoring there would silently \
                 skip the dead-code gate for that work. If this merge was performed ON the target \
                 branch, pass --attest-first-landing-commit to record that operator attestation. \
                 Full forge commit-list evidence is tracked in #4277."
            )));
        }
        return Err(PrMergeEvidenceError::message(format!(
            "commit {pr_merge_commit} has a single parent, so git cannot prove its first parent \
             {first_parent} is the pre-landing target tip: for a squash or corpus-first stack \
             landing the parent IS the tip, but for a landing whose implementation commits preceded \
             the corpus the parent is an earlier same-PR commit, and the two are graph-identical — \
             no git check separates them. Recording it anyway would anchor the dead-code scan at the \
             wrong tip and silently skip part of the gate. If the supplied commit was the first \
             commit of the landing, pass --attest-first-landing-commit to record that operator \
             attestation. Full forge commit-list evidence is tracked in #4277."
        )));
    }
    let anchor_evidence = if is_merge {
        ANCHOR_EVIDENCE_MERGE_COMMIT_PARENT_ATTESTED
    } else {
        ANCHOR_EVIDENCE_CORPUS_PARENT_ATTESTED
    };

    Ok(PrMergeEvidence {
        pr_merge_commit,
        baseline_merge_commit: first_parent,
        anchor_evidence,
    })
}

/// Shape guard for a target ref: anything git would parse as an option, or that
/// could smuggle a revspec operator, is refused before it reaches argv.
fn is_plain_ref(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.contains("//")
        && !value.bytes().any(|byte| {
            byte < 0x20
                || byte == 0x7f
                || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
}

/// `git rev-parse --verify "<spec>^{commit}"`, or Source's refusal text.
///
/// The spec is shape-guarded first (a hexadecimal SHA or a plain ref name), so a
/// caller-supplied value can never arrive at argv as an option.
fn rev_verify(repository: &GitRepository, spec: &str) -> Result<Oid, PrMergeEvidenceError> {
    if !is_hex_sha(spec) && !is_plain_ref(spec) {
        return Err(PrMergeEvidenceError::message(format!(
            "cannot resolve {} to a commit in this repository (git rev-parse failed); fetch the \
             target branch first, then retry",
            python_repr(&format!("{spec}^{{commit}}"))
        )));
    }
    let revision = format!("{spec}^{{commit}}");
    let output = repository
        .process()
        .run(&["rev-parse", "--verify", &revision])
        .map_err(git_failure)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(PrMergeEvidenceError::message(format!(
            "cannot resolve {} to a commit in this repository ({}); fetch the target branch first, \
             then retry",
            python_repr(&revision),
            if stderr.is_empty() {
                "git rev-parse failed".to_owned()
            } else {
                stderr
            }
        )));
    }
    let text = crate::path::utf8_field(&output.stdout, "resolved commit").map_err(git_failure)?;
    Oid::parse(text.trim()).map_err(git_failure)
}

fn git_failure(error: GitError) -> PrMergeEvidenceError {
    PrMergeEvidenceError {
        message: error.to_string(),
        source: Some(error.to_string()),
    }
}

fn has_path_at(
    repository: &GitRepository,
    commit: &Oid,
    path: &str,
) -> Result<bool, PrMergeEvidenceError> {
    let git_path = GitPath::from_git_bytes(path.as_bytes()).map_err(|error| {
        PrMergeEvidenceError::message(format!("cannot inspect {path} at {commit}: {error}"))
    })?;
    let start = StartPoint::Oid(commit.clone());
    repository
        .tree_entry_at(&start, &git_path)
        .map(|entry| entry.is_some())
        .map_err(|error| PrMergeEvidenceError {
            message: format!("cannot inspect {path} at {commit}: {error}"),
            source: Some(error.to_string()),
        })
}
