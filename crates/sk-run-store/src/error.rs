//! Typed failures for the governed run store.
//!
//! Every refusal is a value with a stable [`RunStoreError::code`]. The library
//! never prints: a caller (the coordinator CLI) owns rendering and exit codes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sk_kernel::KernelError;
use sk_runtime::RuntimeError;
use thiserror::Error;

/// A stable, machine-readable error code plus a human message and details.
///
/// This is the serializable projection a CLI can print as JSON without
/// depending on the `Display` text of the underlying error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

/// Everything the run store can refuse.
#[derive(Debug, Error)]
pub enum RunStoreError {
    /// A kernel primitive (bounded read, atomic write, display lock) failed.
    #[error(transparent)]
    Kernel(#[from] KernelError),

    /// The project root passed to [`crate::RunStore::open`] is unusable.
    #[error("project root {root} is not an existing directory")]
    RootNotDirectory { root: PathBuf },

    /// A required run artifact is not a regular file.
    #[error("{path} is not a regular file")]
    NotAFile { path: PathBuf },

    /// The frozen template or another runtime input exceeded the size bound.
    #[error("{path} is larger than the {limit}-byte limit")]
    TooLarge { path: PathBuf, limit: u64 },

    /// The persisted cursor is not valid baseline `state.json`.
    #[error("run state {path} is corrupt: {message}")]
    StateCorrupt { path: PathBuf, message: String },

    /// A live index entry points at a run whose cursor is gone.
    #[error(
        "run {run_id:?} is indexed but {run_dir}/state.json is missing; restore the run or repair .kittify/runtime/feature-runs.json (no run was silently restarted)"
    )]
    RunStateMissing { run_id: String, run_dir: PathBuf },

    /// No run matches the requested identity.
    #[error("no run recorded for {selector}")]
    RunNotFound { selector: String },

    /// The run index is not a valid JSON object of entries.
    #[error("run index {path} is corrupt: {message}")]
    IndexCorrupt { path: PathBuf, message: String },

    /// The index entry and the durable cursor disagree about identity.
    #[error("run identity mismatch: {message}")]
    IdentityMismatch { message: String },

    /// A legacy (identity-less) run cannot be adopted without verified ownership.
    #[error(
        "legacy run {run_id:?} for mission {mission_slug:?} has no mission_id, but the mission now identifies as {mission_id:?}; verify ownership and rekey .kittify/runtime/feature-runs.json before resuming (no new run was started)"
    )]
    RunIdentityMigrationRequired {
        mission_slug: String,
        mission_id: String,
        run_id: String,
    },

    /// A stored `run_dir` token resolves outside the invoking project.
    #[error(
        "run directory token {token:?} resolves to {resolved}; refusing to follow a nonportable runtime reference outside {root:?}"
    )]
    RunDirOutsideRepo {
        token: String,
        resolved: PathBuf,
        root: PathBuf,
    },

    /// A run id is not the canonical 32 lowercase hex form.
    #[error("invalid run id {value:?}: expected 32 lowercase hexadecimal characters")]
    InvalidRunId { value: String },

    /// A mission slug is not a safe single path segment.
    #[error("invalid mission slug {value:?}: expected a safe path segment")]
    InvalidMissionSlug { value: String },

    /// A mission identity token is not a safe key.
    #[error("invalid mission id {value:?}: expected a safe identity token")]
    InvalidMissionId { value: String },

    /// `sk-runtime` rejected the frozen template or a planner input.
    #[error("runtime template/planning input is invalid: {message}")]
    TemplateInvalid { message: String },

    /// The frozen template bytes no longer hash to the cursor's recorded hash.
    #[error(
        "frozen template drift for run {run_id:?}: cursor records {expected}, file hashes to {found}; the run is corrupt and was not advanced"
    )]
    FrozenTemplateDrift {
        run_id: String,
        expected: String,
        found: String,
    },

    /// The recorded live template cannot be read for the drift check.
    #[error("recorded live template {path} cannot be read: {message}")]
    LiveTemplateUnreadable { path: PathBuf, message: String },

    /// The event journal ends with bytes that are not a complete LF-terminated row.
    #[error(
        "event journal {path} has a torn final line ({torn_bytes} byte(s) after the last newline); nothing was appended"
    )]
    TornEventTail { path: PathBuf, torn_bytes: u64 },

    /// A complete journal line is not a valid event envelope.
    #[error("event journal {path} line {line} is corrupt: {message}")]
    EventLogCorrupt {
        path: PathBuf,
        line: usize,
        message: String,
    },

    /// The journal, cursor and index do not agree and must be repaired first.
    #[error("run {run_id:?} requires recovery before this operation: {}", details.join("; "))]
    RecoveryRequired {
        run_id: String,
        details: Vec<String>,
    },

    /// The journal cannot be reconciled with the cursor without guessing.
    #[error("run {run_id:?} journal is inconsistent: {}", details.join("; "))]
    InconsistentRunJournal {
        run_id: String,
        details: Vec<String>,
    },

    /// A plan was computed from a cursor the live run has moved past.
    #[error("run {run_id:?} changed after the advance was planned: {reason}; plan again")]
    StaleAdvancePlan { run_id: String, reason: String },

    /// The shared run-cursor lock was not acquired within the bounded wait.
    #[error(
        "could not acquire the run-cursor lock {path} within {timeout_ms} ms; retry after the other writer finishes"
    )]
    LockTimeout { path: PathBuf, timeout_ms: u64 },

    /// A decision id is not pending in this run.
    #[error("decision {decision_id:?} is not pending for run {run_id:?}")]
    DecisionNotFound { run_id: String, decision_id: String },

    /// An audit gate was answered by an actor that is not the mission owner.
    #[error("audit decision {decision_id:?} refused: {reason}")]
    AuditOwnerRequired {
        run_id: String,
        decision_id: String,
        reason: String,
    },

    /// The answer is not valid for the evaluated significance band.
    #[error("audit decision {decision_id:?} answer {answer:?} is invalid: {reason}")]
    AuditAnswerInvalid {
        decision_id: String,
        answer: String,
        reason: String,
    },

    /// An LLM actor answered without a delegation carrying a rationale.
    #[error("LLM actor {actor_id:?} is not delegated for decision {decision_id:?}: {reason}")]
    LlmDelegationRequired {
        decision_id: String,
        actor_id: String,
        reason: String,
    },

    /// An actor identity cannot be used for a governed answer.
    #[error("invalid actor identity: {reason}")]
    InvalidActor { reason: String },

    /// A prompt artifact path would escape the run directory.
    #[error("step id {step_id:?} does not name a contained prompt file (resolved {path:?})")]
    PromptPathEscape { step_id: String, path: PathBuf },
}

impl RunStoreError {
    /// The stable machine code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Kernel(_) => "KERNEL",
            Self::RootNotDirectory { .. } => "ROOT_NOT_DIRECTORY",
            Self::NotAFile { .. } => "NOT_A_FILE",
            Self::TooLarge { .. } => "TOO_LARGE",
            Self::StateCorrupt { .. } => "RUN_STATE_CORRUPT",
            Self::RunStateMissing { .. } => "RUN_STATE_MISSING",
            Self::RunNotFound { .. } => "RUN_NOT_FOUND",
            Self::IndexCorrupt { .. } => "RUN_INDEX_CORRUPT",
            Self::IdentityMismatch { .. } => "RUN_IDENTITY_MISMATCH",
            Self::RunIdentityMigrationRequired { .. } => "RUN_IDENTITY_MIGRATION_REQUIRED",
            Self::RunDirOutsideRepo { .. } => "RUN_DIR_OUTSIDE_REPO",
            Self::InvalidRunId { .. } => "INVALID_RUN_ID",
            Self::InvalidMissionSlug { .. } => "INVALID_MISSION_SLUG",
            Self::InvalidMissionId { .. } => "INVALID_MISSION_ID",
            Self::TemplateInvalid { .. } => "TEMPLATE_INVALID",
            Self::FrozenTemplateDrift { .. } => "FROZEN_TEMPLATE_DRIFT",
            Self::LiveTemplateUnreadable { .. } => "LIVE_TEMPLATE_UNREADABLE",
            Self::TornEventTail { .. } => "TORN_EVENT_TAIL",
            Self::EventLogCorrupt { .. } => "EVENT_LOG_CORRUPT",
            Self::RecoveryRequired { .. } => "RECOVERY_REQUIRED",
            Self::InconsistentRunJournal { .. } => "INCONSISTENT_RUN_JOURNAL",
            Self::StaleAdvancePlan { .. } => "STALE_ADVANCE_PLAN",
            Self::LockTimeout { .. } => "LOCK_TIMEOUT",
            Self::DecisionNotFound { .. } => "DECISION_NOT_FOUND",
            Self::AuditOwnerRequired { .. } => "AUDIT_OWNER_REQUIRED",
            Self::AuditAnswerInvalid { .. } => "AUDIT_ANSWER_INVALID",
            Self::LlmDelegationRequired { .. } => "LLM_DELEGATION_REQUIRED",
            Self::InvalidActor { .. } => "INVALID_ACTOR",
            Self::PromptPathEscape { .. } => "PROMPT_PATH_ESCAPE",
        }
    }

    /// A serializable `{code, message, details}` projection.
    pub fn info(&self) -> ErrorInfo {
        let mut details = BTreeMap::new();
        let mut insert = |key: &str, value: Value| {
            details.insert(key.to_string(), value);
        };
        match self {
            Self::RunStateMissing { run_id, run_dir } => {
                insert("run_id", Value::String(run_id.clone()));
                insert(
                    "run_dir",
                    Value::String(run_dir.to_string_lossy().into_owned()),
                );
            }
            Self::RunDirOutsideRepo {
                token,
                resolved,
                root,
            } => {
                insert("token", Value::String(token.clone()));
                insert(
                    "resolved",
                    Value::String(resolved.to_string_lossy().into_owned()),
                );
                insert("root", Value::String(root.to_string_lossy().into_owned()));
            }
            Self::FrozenTemplateDrift {
                run_id,
                expected,
                found,
            } => {
                insert("run_id", Value::String(run_id.clone()));
                insert("expected", Value::String(expected.clone()));
                insert("found", Value::String(found.clone()));
            }
            Self::TornEventTail { path, torn_bytes } => {
                insert("path", Value::String(path.to_string_lossy().into_owned()));
                insert("torn_bytes", Value::from(*torn_bytes));
            }
            Self::RecoveryRequired {
                run_id,
                details: items,
            } => {
                insert("run_id", Value::String(run_id.clone()));
                insert("details", serde_json::json!(items));
            }
            Self::InconsistentRunJournal {
                run_id,
                details: items,
            } => {
                insert("run_id", Value::String(run_id.clone()));
                insert("details", serde_json::json!(items));
            }
            Self::StaleAdvancePlan { run_id, reason } => {
                insert("run_id", Value::String(run_id.clone()));
                insert("reason", Value::String(reason.clone()));
            }
            Self::LockTimeout { path, timeout_ms } => {
                insert("path", Value::String(path.to_string_lossy().into_owned()));
                insert("timeout_ms", Value::from(*timeout_ms));
            }
            Self::AuditOwnerRequired {
                run_id,
                decision_id,
                reason,
            } => {
                insert("run_id", Value::String(run_id.clone()));
                insert("decision_id", Value::String(decision_id.clone()));
                insert("reason", Value::String(reason.clone()));
            }
            Self::AuditAnswerInvalid {
                decision_id,
                answer,
                reason,
            } => {
                insert("decision_id", Value::String(decision_id.clone()));
                insert("answer", Value::String(answer.clone()));
                insert("reason", Value::String(reason.clone()));
            }
            Self::LlmDelegationRequired {
                decision_id,
                actor_id,
                reason,
            } => {
                insert("decision_id", Value::String(decision_id.clone()));
                insert("actor_id", Value::String(actor_id.clone()));
                insert("reason", Value::String(reason.clone()));
            }
            Self::PromptPathEscape { step_id, path } => {
                insert("step_id", Value::String(step_id.clone()));
                insert("path", Value::String(path.to_string_lossy().into_owned()));
            }
            _ => {}
        }
        ErrorInfo {
            code: self.code().to_string(),
            message: self.to_string(),
            details,
        }
    }
}

impl From<RuntimeError> for RunStoreError {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::StaleRunState { run_id } => Self::StaleAdvancePlan {
                run_id,
                reason: "the live snapshot no longer equals the planned source".to_string(),
            },
            RuntimeError::StaleIssuedStep {
                run_id,
                expected,
                found,
            } => Self::StaleAdvancePlan {
                run_id,
                reason: format!(
                    "issued step changed since it was evaluated (expected {expected:?}, found {found:?})"
                ),
            },
            other => Self::TemplateInvalid {
                message: other.to_string(),
            },
        }
    }
}

impl From<std::io::Error> for RunStoreError {
    fn from(source: std::io::Error) -> Self {
        Self::Kernel(KernelError::Io {
            action: "access",
            path: PathBuf::from("<unknown>"),
            source,
        })
    }
}

pub(crate) fn timeout_error(path: &Path, timeout: Duration) -> RunStoreError {
    RunStoreError::LockTimeout {
        path: path.to_path_buf(),
        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
    }
}
