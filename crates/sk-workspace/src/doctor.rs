//! Read-only `doctor` diagnosis.
//!
//! This module owns the **diagnosis** half of the upstream `spec-kitty doctor`
//! group: it inspects real repository state and returns typed rows and findings.
//! It never writes, never materializes a status snapshot, never rewrites
//! `meta.json`, never adopts an unknown file, and never walks a guessed
//! worktree. A missing, malformed, or ambiguous input is a *finding*, never a
//! healthy report.
//!
//! # Scope of this module
//!
//! Implemented diagnoses:
//!
//! * [`identity`] — the five-state Mission identity audit
//!   (`assigned`/`pending`/`legacy`/`orphan`/`residue` with residue reasons),
//!   duplicate numeric prefixes and ambiguous selectors.
//! * [`topology`] — each Mission's **stored** topology, plus the two structural
//!   findings (`SINGLE_BRANCH_CODE_LANES_UNMIGRATED`, `LANES_MANIFEST_UNREADABLE`)
//!   and their operator remedies.
//! * [`mission_type`] — the six-state mission-type resolution audit. The
//!   activated set and the layered mission-type roster are supplied by the
//!   caller ([`mission_type::MissionTypeHealth`]) so the activation ledger and
//!   the definition resolver keep their canonical owners.
//! * [`workspaces`] — `.worktrees/` husk detection with the registered-worktree
//!   annotation.
//! * [`state_roots`] — state-root presence, per-surface classification and
//!   gitignore-coverage warnings over a supplied surface registry.
//! * [`decisions`] — `decisions/index.json` vs the authoritative
//!   `status.events.jsonl` decision rows: missing, orphaned, unfoldable and
//!   stale-status findings (read-only; Source's `--repair` is refused upstream).
//! * [`invocation_pairing`] — unpaired `started` profile-invocation lifecycle
//!   records from `kitty-ops/lifecycle.jsonl` + `kitty-ops/lifecycle/*.json`.
//! * [`ops`] — open Op records (`kitty-ops/*.jsonl` with no `completed` event and
//!   no closure-spine entry); Source's mutating `--close-stale` sweep is refused
//!   upstream.
//!
//! Every Git question goes through the one `sk-git` seam; every bounded read
//! goes through [`sk_kernel::read_bounded`]. Nothing here shells out to Git
//! itself, and nothing here re-implements a parser or a model owned elsewhere.
//!
//! # Bounded inspection is a distinct capability
//!
//! Reads here are bounded (the per-file ceilings are named on each reader).
//! An input above a bound is refused with a named error and reported as a
//! finding; that is a *native bounded-inspection* decision, not a claim that
//! the source refuses the input.
//!
//! # Repair
//!
//! Upstream's mutating doctor arms (`--fix`, `--repair`, `--close-stale`) are
//! **not** silently accepted as no-ops. The adapter refuses each with a named
//! unsupported code and lists the coverage that is still missing; no write is
//! attempted here.

#![deny(missing_docs)]

pub mod decision_fold;
pub mod decisions;
pub mod identity;
pub mod invocation_pairing;
pub(crate) mod meta;
pub mod mission_type;
pub mod ops;
pub mod state_roots;
pub mod topology;
pub mod workspaces;

mod git_probe;

pub use decisions::{
    DECISION_EVENTS_FILE_NAME, DecisionStatusMismatch, DecisionsAudit, DecisionsReport,
    audit_decisions, audit_decisions_with_fork,
};
pub use identity::{
    IDENTITY_STATES, IdentityAudit, IdentityRow, IdentityState, RESIDUE_REASONS, ResidueReason,
    TrackedPosture, audit_identity,
};
pub use invocation_pairing::{
    InvocationPhase, LIFECYCLE_LOG_RELATIVE_PATH, LIFECYCLE_RECORD_SUFFIX,
    LIFECYCLE_RECORDS_RELATIVE_DIR, LifecycleRecord, OrphanEntry, PairingAudit,
    audit_invocation_pairing, lifecycle_log_path, lifecycle_records_dir, pairing_report,
    read_lifecycle_records,
};
pub use mission_type::{
    MISSION_TYPE_STATES, MissionTypeAudit, MissionTypeHealth, MissionTypeRow, MissionTypeState,
    UnknownFailOnStates, audit_mission_types, parse_fail_on,
};
pub use ops::{OpStoreLayout, OpsAudit, audit_open_ops};
pub use state_roots::{
    AuthorityClass, GitClass, STATE_ROOTS, STATE_SURFACES, StateRootEnvironment, StateRootInfo,
    StateRootKind, StateRootsAudit, StateSurfaceCheck, StateSurfaceSpec, audit_state_roots,
};
pub use topology::{
    LANES_MANIFEST_UNREADABLE, SINGLE_BRANCH_CODE_LANES_UNMIGRATED, TopologyAudit, TopologyFinding,
    TopologyRow, audit_topology,
};
pub use workspaces::{HuskEntry, HuskReport, scan_workspace_husks};

/// A read-only diagnosis failure.
///
/// Findings are data ([`IdentityRow`], [`TopologyRow`], …); this type is only
/// for a failure that prevents the diagnosis itself from answering at all.
#[derive(Debug, thiserror::Error)]
pub enum DoctorError {
    /// The project root does not exist or is not a directory.
    #[error("{path} is not a directory, so there is nothing to diagnose")]
    RootNotDirectory {
        /// The path the caller supplied.
        path: std::path::PathBuf,
    },

    /// Git could not answer a question the diagnosis depends on.
    ///
    /// A broken index is never degraded into "nothing is tracked": the audit
    /// refuses instead of producing a weaker answer.
    #[error("{what} could not be read from git: {detail}")]
    GitUnavailable {
        /// Which question failed.
        what: &'static str,
        /// Git's own diagnostic.
        detail: String,
    },

    /// A required filesystem read failed.
    #[error("could not {context} at {path}: {source}")]
    Io {
        /// What was being done.
        context: &'static str,
        /// The path involved.
        path: std::path::PathBuf,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// The Mission decision event log is present but cannot be scanned.
    ///
    /// Source raises here (an `OSError`, a `JSONDecodeError` or an
    /// `AttributeError`); the diagnosis refuses instead of degrading into an
    /// empty — and therefore clean — log.
    #[error("could not read the decision event log at {path}: {detail}")]
    DecisionEventsUnreadable {
        /// The event-log path.
        path: std::path::PathBuf,
        /// What is wrong with it.
        detail: String,
    },

    /// `decisions/index.json` is present but cannot be decoded.
    ///
    /// A *missing* index is the benign empty index (Source's rule); a present
    /// but corrupt one must never read as clean.
    #[error("could not read the decision index at {path}: {detail}")]
    DecisionIndexUnreadable {
        /// The index path.
        path: std::path::PathBuf,
        /// The typed decode failure.
        detail: String,
    },

    /// A two-surface fork fact carries an identity with no scalar form.
    ///
    /// A decision id can hold an unpaired surrogate, which no JSON/display
    /// surface can spell. The diagnosis refuses instead of inventing an escaped
    /// identity that no other reader would match.
    #[error("could not render a decision-stream identity: {detail}")]
    ForkIdentity {
        /// What could not be rendered.
        detail: String,
    },
}

impl DoctorError {
    /// A stable machine code for the adapter's `--json` envelope.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RootNotDirectory { .. } => "PROJECT_ROOT_NOT_FOUND",
            Self::GitUnavailable { .. } => "GIT_INDEX_UNREADABLE",
            Self::Io { .. } => "DOCTOR_INSPECTION_FAILED",
            Self::DecisionEventsUnreadable { .. } => "DECISION_EVENTS_UNREADABLE",
            Self::DecisionIndexUnreadable { .. } => "DECISION_INDEX_UNREADABLE",
            Self::ForkIdentity { .. } => "DECISION_FORK_IDENTITY_UNRENDERABLE",
        }
    }
}

/// The upstream `GIT_INDEX_UNREADABLE` remediation text (`doctor identity`).
pub const GIT_INDEX_REMEDIATION: &str = "Repair the git index: run `git reset`; if that fails too, delete the index file (its path is printed by `git rev-parse --git-path index`) and run `git reset` again, then re-run.";

pub(crate) fn require_directory(root: &std::path::Path) -> Result<(), DoctorError> {
    match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        _ => Err(DoctorError::RootNotDirectory {
            path: root.to_path_buf(),
        }),
    }
}
