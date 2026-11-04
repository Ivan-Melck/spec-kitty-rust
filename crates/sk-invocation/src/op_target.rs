//! Source-ordered Op target adjudication: judged destination `J` and physical
//! publication `P`.
//!
//! Source: `specify_cli/invocation/executor.py` and
//! `specify_cli/git/commit_helpers.py` at the pinned baseline. The reference
//! uses three different readings of the checked-out ref and they are not
//! interchangeable:
//!
//! * **J** — `executor._current_branch`: `symbolic-ref --quiet --short HEAD`
//!   then Python `stdout.strip()`. J is the *judged* destination for the
//!   source's HEAD assertion, existence check, protection decision, refusal
//!   name, and admission verdict.
//! * **H** — `commit_helpers._read_worktree_head`: the **full**
//!   `symbolic-ref HEAD`, Python `stdout.strip()` of the whole line, then
//!   `removeprefix("refs/heads/")`. Step 4 compares H with J. Stripping happens
//!   before prefix removal, so `topic\u{a0}` matches `topic` while
//!   `\u{a0}topic` does not.
//! * **P** — the exact physical suffix of the full symbolic `HEAD` ref
//!   (`refs/heads/<P>`). Only P is handed to the canonical native publisher, so
//!   its HEAD assertion, tip/CAS transaction, captured index, and evidence stay
//!   exact and truthful.
//!
//! [`OpCommitService::judge_record_target`] is the one per-call authority shared
//! by admission and the real close. It reproduces `preflight_commit`'s order:
//! 1 shape, (2 non-empty paths), 3 worktree relationship, 4 H == J,
//! 5 `refs/heads/J` exists, 6a/6b source path proof, 6c protection on J. Only
//! after the source's own guard allows does it resolve P and ask the canonical
//! planner for a message-free target plan.
//!
//! A source preflight failure **admits the audit write** (it is a
//! `SafeCommitError` in the reference) but **refuses the actual commit**. A
//! source-allowed shape the native publisher cannot represent (unborn P, a
//! `refs/`-prefixed short name, a native-only pre-guard refusal) is reported as
//! [`OpTargetVerdict::SourceAllowedButNativeCannotPublish`] — never as a source
//! refusal, a fake success, or a plan without a real request. No parent is
//! seeded or fabricated.
//!
//! Read-only: only `symbolic-ref`, `for-each-ref`, config reads, filesystem
//! observation, and the planner's own read-only checks run here.

use std::fmt;
use std::path::PathBuf;

use sk_git::{
    BranchName, CommitGuardCapability, GitError, GitPath, ProtectedBranches, RefName,
    SafeCommitTargetPlan, SourcePathRefusal, evaluate_commit_guard_decision,
    plan_source_commit_path,
};
use sk_workspace::WorkspaceError;

use crate::op_commit::{
    OpCommitService, ProtectionRead, build_safe_commit_target, git_path_from_relative,
};
use crate::op_commit_policy::strip_git_stdout;

/// The source-judged destination `J`.
///
/// Constructible only from the `--short` stdout, and only through Python
/// `str.strip()` semantics; it is never derived from the full ref and never
/// forced through `BranchName`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JudgedDestination(String);

impl JudgedDestination {
    /// `executor._current_branch`'s normalisation of `--short` stdout.
    pub(crate) fn from_short_stdout(raw: &str) -> Option<Self> {
        let stripped = strip_git_stdout(raw);
        (!stripped.is_empty()).then(|| Self(stripped.to_owned()))
    }

    /// The judged name.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The sealed pair produced only by [`OpCommitService::judge_record_target`].
#[derive(Clone, Debug)]
pub(crate) struct OpBranchTarget {
    judged: JudgedDestination,
    publication: BranchName,
}

impl OpBranchTarget {
    /// The source-judged name.
    pub(crate) fn judged(&self) -> &JudgedDestination {
        &self.judged
    }

    /// The exact physical branch the canonical publisher receives.
    pub(crate) fn publication(&self) -> &BranchName {
        &self.publication
    }
}

/// Why the reference's preflight refuses before its protection guard.
#[derive(Debug)]
pub(crate) enum OpTargetError {
    /// Step 1: `J` is a fully-qualified-looking short name.
    DestinationRefShape {
        /// The judged name.
        judged: String,
    },
    /// Step 3: the record root is not a worktree of the project root.
    NotAWorktree {
        /// The record checkout.
        worktree_root: PathBuf,
    },
    /// Step 4: the full symbolic HEAD does not agree with `J`.
    HeadMismatch {
        /// The judged name.
        judged: String,
        /// The stripped full-ref remainder (or `<detached>`).
        observed: String,
        /// The record checkout.
        worktree_root: PathBuf,
    },
    /// Step 5: `refs/heads/J` is absent (or not a legal ref).
    DestinationMissing {
        /// The judged name.
        judged: String,
    },
}

impl fmt::Display for OpTargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DestinationRefShape { judged } => write!(
                formatter,
                "safe_commit: destination_ref must be a short branch name, got {judged:?}"
            ),
            Self::NotAWorktree { worktree_root } => write!(
                formatter,
                "safe_commit: {} is not a git worktree. Pass a resolved worktree path.",
                worktree_root.display()
            ),
            Self::HeadMismatch {
                judged,
                observed,
                worktree_root,
            } => write!(
                formatter,
                "safe_commit: worktree HEAD {observed:?} does not match destination ref {judged:?} in {}",
                worktree_root.display()
            ),
            Self::DestinationMissing { judged } => write!(
                formatter,
                "safe_commit: destination ref {judged:?} does not exist in the repo. Create the branch first, or check the spelling."
            ),
        }
    }
}

impl std::error::Error for OpTargetError {}

/// The one per-call adjudication result.
///
/// `Ready` is the only variant that carries a plan; it can be built only after
/// the source guard allowed and the canonical planner accepted the exact
/// physical destination. `SourceAllowedButNativeCannotPublish` is deliberately
/// explicit: the reference allows the commit but this native capability cannot
/// prepare it yet, and no fake plan/parent is invented.
#[derive(Debug)]
pub(crate) enum OpTargetVerdict {
    /// A source preflight refusal (steps 1/3/4/5). Admission admits.
    SourceRefuses(OpTargetError),
    /// A source path refusal (2/6a/6b, or an absolute-path loop). Admission
    /// admits; the close refuses.
    SourcePathRefuses(GitError),
    /// The source reaches 6c and its guard refuses under the judged name.
    Protected {
        /// The judged name the refusal must report.
        judged: String,
    },
    /// The source reaches 6c and its protection configuration is unreadable.
    InvalidConfig {
        /// The canonical policy detail.
        detail: String,
    },
    /// A git probe needed to answer the source question could not run.
    Repository(GitError),
    /// A protection read failed for a reason other than malformed config.
    Protection(WorkspaceError),
    /// The source allows the commit and the canonical native plan is ready.
    Ready {
        /// The sealed judged/physical pair.
        target: OpBranchTarget,
        /// The message-free canonical plan.
        plan: Box<SafeCommitTargetPlan>,
    },
    /// The source allows the commit, but the native publisher cannot prepare a
    /// plan for this exact physical shape yet.
    SourceAllowedButNativeCannotPublish {
        /// The judged name.
        judged: String,
        /// The truthful native cause.
        cause: GitError,
    },
}

/// The operator-facing reason for the source-allow/native-nonpublishable case.
///
/// It names the native limitation instead of mislabelling it as a source or
/// policy refusal; the source preflight allowed this request, which is not a
/// promise that its later staging/hooks/commit would succeed.
pub(crate) fn native_nonpublishable_reason(judged: &str, cause: &GitError) -> String {
    format!(
        "source preflight allows this commit on {judged:?}, but the native publisher cannot prepare a plan: {cause}"
    )
}

impl OpCommitService {
    /// The source-judged destination `J`, or `None` for a non-symbolic HEAD.
    pub(crate) fn judged_destination(&self) -> Result<Option<JudgedDestination>, GitError> {
        let Some(repository) = self.open_repository() else {
            return Ok(None);
        };
        Ok(repository
            .current_branch()?
            .and_then(|branch| JudgedDestination::from_short_stdout(&branch)))
    }

    /// Judge one record target in source preflight order.
    ///
    /// This is the single authority admission and the real close both consume;
    /// neither caches an authorization. It never mutates the repository.
    pub(crate) fn judge_record_target(
        &self,
        judged: &JudgedDestination,
        path: &GitPath,
    ) -> OpTargetVerdict {
        let Some(repository) = self.open_repository() else {
            return OpTargetVerdict::Repository(GitError::NotARepository {
                path: self.record_root().to_path_buf(),
            });
        };
        let worktree_root = self.record_root().to_path_buf();

        // 1. Shape: the source refuses a `refs/heads/`-prefixed judged name
        //    before it looks at roots, HEAD, or existence.
        if judged.as_str().starts_with("refs/heads/") {
            return OpTargetVerdict::SourceRefuses(OpTargetError::DestinationRefShape {
                judged: judged.as_str().to_owned(),
            });
        }

        // 3. The record checkout is a worktree of the project root.
        if !self.roots_are_linked() {
            return OpTargetVerdict::SourceRefuses(OpTargetError::NotAWorktree { worktree_root });
        }

        // 4. The source's HEAD assertion, on the FULL symbolic ref (strip the
        //    whole stdout, then remove the branch prefix), compared with J.
        let head = match repository.observe_symbolic_head() {
            // A completed nonzero has no target; successful malformed output
            // remains an error rather than an invented detached/source refusal.
            Ok(observed) => observed.target_text().map(str::to_owned),
            Err(error) => return OpTargetVerdict::Repository(error),
        };
        let observed_full = head
            .as_ref()
            .map(|reference| strip_git_stdout(reference.as_str()).to_owned());
        let observed = observed_full
            .as_deref()
            .map(|full| full.strip_prefix("refs/heads/").unwrap_or(full).to_owned());
        if observed.as_deref() != Some(judged.as_str()) {
            return OpTargetVerdict::SourceRefuses(OpTargetError::HeadMismatch {
                judged: judged.as_str().to_owned(),
                observed: observed.unwrap_or_else(|| "<detached>".to_owned()),
                worktree_root,
            });
        }

        // 5. `refs/heads/J` exists. This is `rev-parse --verify --quiet
        //    refs/heads/<J>`, NOT "HEAD has a commit": an unborn physical ref
        //    with an existing judged shadow passes here, exactly like the source.
        let judged_ref = format!("refs/heads/{}", judged.as_str());
        let exists = match RefName::parse(&judged_ref) {
            Ok(reference) => match repository.ref_exists(&reference) {
                Ok(exists) => exists,
                Err(error) => return OpTargetVerdict::Repository(error),
            },
            // The reference's `rev-parse --verify` returns non-zero for a name
            // that is not a legal ref, which `_destination_ref_exists` reports
            // as absent.
            Err(_) => false,
        };
        if !exists {
            return OpTargetVerdict::SourceRefuses(OpTargetError::DestinationMissing {
                judged: judged.as_str().to_owned(),
            });
        }

        // 6a/6b. The source's own path proof, from the shared canonical
        // observation authority. A native-only path refusal is NOT relabelled
        // as this.
        // The source hands preflight the writer's record-root/path spelling,
        // normally absolute. Passing only the relative staging path would skip
        // its parent resolution and absolute-leaf loop checks.
        let source_path = match git_path_from_relative(&self.record_root().join(path.to_path_buf()))
        {
            Ok(path) => path,
            Err(error) => return OpTargetVerdict::Repository(error),
        };
        let source_path = match plan_source_commit_path(repository, &source_path) {
            Ok(path) => path,
            Err(SourcePathRefusal::Source { error }) => {
                return OpTargetVerdict::SourcePathRefuses(error);
            }
            Err(SourcePathRefusal::Unreadable { error }) => {
                return OpTargetVerdict::Repository(error);
            }
        };

        // 6c. Protection is decided on J, then the ONE canonical guard core.
        let protected = match self.judged_protection(judged) {
            Ok(protected) => protected,
            Err(ProtectionRead::InvalidConfig(detail)) => {
                return OpTargetVerdict::InvalidConfig { detail };
            }
            Err(ProtectionRead::Unavailable(error)) => {
                return OpTargetVerdict::Protection(error);
            }
        };
        if !evaluate_commit_guard_decision(protected, CommitGuardCapability::Standard) {
            return OpTargetVerdict::Protected {
                judged: judged.as_str().to_owned(),
            };
        }

        // The source allows the commit. Resolve the exact physical publication
        // identity without re-deriving J, then ask the canonical planner.
        let Some(full) = head else {
            // Unreachable after step 4; a non-symbolic HEAD is a mismatch.
            return OpTargetVerdict::SourceRefuses(OpTargetError::HeadMismatch {
                judged: judged.as_str().to_owned(),
                observed: "<detached>".to_owned(),
                worktree_root,
            });
        };
        let Some(publication) = full.as_str().strip_prefix("refs/heads/") else {
            return OpTargetVerdict::SourceAllowedButNativeCannotPublish {
                judged: judged.as_str().to_owned(),
                cause: GitError::Unsupported {
                    feature: "publishing an Op through a non-branch symbolic HEAD target",
                },
            };
        };
        let publication = match BranchName::parse(publication) {
            Ok(publication) => publication,
            Err(cause) => {
                return OpTargetVerdict::SourceAllowedButNativeCannotPublish {
                    judged: judged.as_str().to_owned(),
                    cause,
                };
            }
        };
        let target = OpBranchTarget {
            judged: judged.clone(),
            publication,
        };
        // The source decided this destination is unprotected under J; the
        // projection is that verdict, not a second lookup of P's spelling.
        // That preserves the source's actual-only-protection quirk (a config
        // that protects only the raw P does not stop this commit).
        let request = build_safe_commit_target(
            target.publication(),
            source_path.normalized(),
            ProtectedBranches::default(),
        );
        match repository.plan_safe_commit_target(&request) {
            Ok(plan) => OpTargetVerdict::Ready {
                target,
                plan: Box::new(plan),
            },
            Err(failure) => OpTargetVerdict::SourceAllowedButNativeCannotPublish {
                judged: judged.as_str().to_owned(),
                cause: failure.into_error(),
            },
        }
    }
}
