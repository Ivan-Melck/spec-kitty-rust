//! Derived mission lifecycle phase.
//!
//! The phase answers exactly one question — *where in its consolidation
//! lifecycle is this mission, from durable signals alone* — and it is never
//! stored. It is recomputed from the mission's baseline marker, the target
//! ref's existence, and terminal-completion evidence.
//!
//! # Failure is not absence
//!
//! A probe that cannot answer (Git failed, the revision is invalid, a value is
//! corrupt) must never be read as "the thing is absent". [`Probe`] therefore
//! has three states, and [`derive_lifecycle_phase`] refuses rather than
//! silently choosing the pre-consolidation default when a probe failed. This
//! is the direct Rust projection of upstream
//! `LifecyclePhaseProbeError` (why the two-step `_git_object_present` probe
//! exists).

use sk_git::{BranchName, GitRepository, Oid};
use sk_mission::Mission;

use crate::error::WorkspaceError;

/// A mission's consolidation lifecycle phase.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    /// `baseline_merge_commit` absent. This is also the safe default for a
    /// legacy null-baseline mission.
    PreConsolidation,
    /// `baseline_merge_commit` present and the target ref still exists:
    /// lane consolidation landed but is not yet published to trunk (E1).
    Consolidated,
    /// `baseline_merge_commit` present, the target ref is absent, and terminal
    /// completion evidence exists: published to trunk (E2).
    Published,
}

impl LifecyclePhase {
    /// The canonical wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreConsolidation => "pre_consolidation",
            Self::Consolidated => "consolidated",
            Self::Published => "published",
        }
    }
}

impl std::fmt::Display for LifecyclePhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A three-state probe result: present, genuinely absent, or failed.
///
/// `Failed` is never collapsed into `Absent`. Callers that own an
/// absence-tolerant read must say so explicitly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Probe<T> {
    /// The value was observed.
    Present(T),
    /// The value is genuinely absent.
    Absent,
    /// The probe could not answer.
    Failed(String),
}

impl<T> Probe<T> {
    /// Is this the genuinely-absent answer?
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// Is this the failed answer?
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    /// Map a present value.
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> Probe<U> {
        match self {
            Self::Present(value) => Probe::Present(map(value)),
            Self::Absent => Probe::Absent,
            Self::Failed(detail) => Probe::Failed(detail),
        }
    }
}

/// The durable evidence a lifecycle phase is derived from.
///
/// The default is "no evidence": an absent baseline, an absent target ref, and
/// no terminal completion. It derives `pre_consolidation`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleEvidence {
    /// The mission's `baseline_merge_commit` marker.
    pub baseline_merge_commit: Probe<String>,
    /// Whether the target ref still exists locally.
    pub target_ref: Probe<Oid>,
    /// Whether terminal completion evidence exists (an assigned mission number,
    /// or every work package is `done`).
    pub terminal_completion: bool,
}

impl Default for LifecycleEvidence {
    fn default() -> Self {
        Self {
            baseline_merge_commit: Probe::Absent,
            target_ref: Probe::Absent,
            terminal_completion: false,
        }
    }
}

/// Derive the lifecycle phase from durable evidence.
///
/// * no baseline → [`LifecyclePhase::PreConsolidation`], with zero Git probes;
/// * baseline present and target present → [`LifecyclePhase::Consolidated`];
/// * baseline present, target absent, terminal evidence → [`LifecyclePhase::Published`];
/// * baseline present, target absent, no terminal evidence → [`LifecyclePhase::PreConsolidation`]
///   (a never-created target ref is also "absent"; the safe default is never a
///   false E2);
/// * any probe failed → [`WorkspaceError::LifecycleProbeFailed`].
pub fn derive_lifecycle_phase(
    evidence: &LifecycleEvidence,
) -> Result<LifecyclePhase, WorkspaceError> {
    let baseline = match &evidence.baseline_merge_commit {
        Probe::Absent => return Ok(LifecyclePhase::PreConsolidation),
        Probe::Failed(detail) => {
            return Err(WorkspaceError::LifecycleProbeFailed {
                probe: "baseline_merge_commit",
                detail: detail.clone(),
            });
        }
        Probe::Present(value) => value.trim().to_owned(),
    };
    if baseline.is_empty() {
        return Ok(LifecyclePhase::PreConsolidation);
    }

    match &evidence.target_ref {
        Probe::Present(_) => Ok(LifecyclePhase::Consolidated),
        Probe::Absent => Ok(if evidence.terminal_completion {
            LifecyclePhase::Published
        } else {
            LifecyclePhase::PreConsolidation
        }),
        Probe::Failed(detail) => Err(WorkspaceError::LifecycleProbeFailed {
            probe: "target_ref",
            detail: detail.clone(),
        }),
    }
}

/// Read the baseline marker from a decoded `meta.json` map.
///
/// A missing key, `null`, or a blank string is [`Probe::Absent`]. A non-string,
/// non-null value is [`Probe::Failed`]: it is corrupt metadata, not absence.
pub fn baseline_merge_commit_from_meta(
    meta: &serde_json::Map<String, serde_json::Value>,
) -> Probe<String> {
    match meta.get("baseline_merge_commit") {
        None | Some(serde_json::Value::Null) => Probe::Absent,
        Some(serde_json::Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Probe::Absent
            } else {
                Probe::Present(trimmed.to_owned())
            }
        }
        Some(other) => Probe::Failed(format!(
            "baseline_merge_commit must be a string, observed {other}"
        )),
    }
}

/// Read the baseline marker from a loaded [`Mission`].
pub fn baseline_merge_commit_from_mission(mission: &Mission) -> Probe<String> {
    baseline_merge_commit_from_meta(mission.meta())
}

/// Probe whether a local branch exists, distinguishing failure from absence.
///
/// Delegates to `sk-git`'s read-only `read_branch_tip`; it never runs a Git
/// process of its own, so there is one subprocess seam in the workspace.
pub fn probe_target_ref(repository: &GitRepository, branch: &BranchName) -> Probe<Oid> {
    match repository.read_branch_tip(branch) {
        Ok(Some(oid)) => Probe::Present(oid),
        Ok(None) => Probe::Absent,
        Err(error) => Probe::Failed(error.to_string()),
    }
}

/// Terminal-completion evidence from a mission number and the reduced WP lanes.
///
/// `work_packages` is `(wp_id, lane)` for every work package in the status
/// snapshot. Evidence exists when a mission number was assigned, or when there
/// is at least one work package and every one is `done`. A never-created
/// mission cannot fake either signal.
pub fn terminal_completion(
    mission_number: Option<u32>,
    work_packages: &[(String, String)],
) -> bool {
    if mission_number.is_some() {
        return true;
    }
    !work_packages.is_empty() && work_packages.iter().all(|(_, lane)| lane == "done")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(baseline: Probe<String>, target: Probe<Oid>, terminal: bool) -> LifecycleEvidence {
        LifecycleEvidence {
            baseline_merge_commit: baseline,
            target_ref: target,
            terminal_completion: terminal,
        }
    }

    fn oid(value: &str) -> Oid {
        Oid::parse(&format!("{value:0>40}")).expect("valid object id")
    }

    #[test]
    fn absent_baseline_is_pre_consolidation_and_ignores_git() {
        let phase = derive_lifecycle_phase(&evidence(
            Probe::Absent,
            Probe::Failed("should not be consulted".to_owned()),
            false,
        ))
        .unwrap();
        assert_eq!(phase, LifecyclePhase::PreConsolidation);
    }

    #[test]
    fn baseline_with_target_is_consolidated() {
        let phase = derive_lifecycle_phase(&evidence(
            Probe::Present("abcdef".to_owned()),
            Probe::Present(oid("1")),
            false,
        ))
        .unwrap();
        assert_eq!(phase, LifecyclePhase::Consolidated);
    }

    #[test]
    fn baseline_absent_target_with_terminal_evidence_is_published() {
        let phase = derive_lifecycle_phase(&evidence(
            Probe::Present("abcdef".to_owned()),
            Probe::Absent,
            true,
        ))
        .unwrap();
        assert_eq!(phase, LifecyclePhase::Published);
    }

    #[test]
    fn baseline_absent_target_without_terminal_evidence_is_pre_consolidation() {
        let phase = derive_lifecycle_phase(&evidence(
            Probe::Present("abcdef".to_owned()),
            Probe::Absent,
            false,
        ))
        .unwrap();
        assert_eq!(phase, LifecyclePhase::PreConsolidation);
    }

    #[test]
    fn failed_target_probe_is_a_loud_error_not_absence() {
        let error = derive_lifecycle_phase(&evidence(
            Probe::Present("abcdef".to_owned()),
            Probe::Failed("git rev-parse exited 128".to_owned()),
            true,
        ))
        .unwrap_err();
        assert!(matches!(
            error,
            WorkspaceError::LifecycleProbeFailed {
                probe: "target_ref",
                ..
            }
        ));
    }

    #[test]
    fn failed_baseline_probe_is_a_loud_error() {
        let error = derive_lifecycle_phase(&evidence(
            Probe::Failed("corrupt meta".to_owned()),
            Probe::Present(oid("1")),
            true,
        ))
        .unwrap_err();
        assert!(matches!(
            error,
            WorkspaceError::LifecycleProbeFailed {
                probe: "baseline_merge_commit",
                ..
            }
        ));
    }

    #[test]
    fn baseline_metadata_shapes() {
        let mut meta = serde_json::Map::new();
        assert_eq!(baseline_merge_commit_from_meta(&meta), Probe::Absent);
        meta.insert("baseline_merge_commit".into(), serde_json::Value::Null);
        assert_eq!(baseline_merge_commit_from_meta(&meta), Probe::Absent);
        meta.insert(
            "baseline_merge_commit".into(),
            serde_json::Value::String("   ".into()),
        );
        assert_eq!(baseline_merge_commit_from_meta(&meta), Probe::Absent);
        meta.insert(
            "baseline_merge_commit".into(),
            serde_json::Value::String(" deadbeef ".into()),
        );
        assert_eq!(
            baseline_merge_commit_from_meta(&meta),
            Probe::Present("deadbeef".into())
        );
        meta.insert(
            "baseline_merge_commit".into(),
            serde_json::Value::Bool(true),
        );
        assert!(baseline_merge_commit_from_meta(&meta).is_failed());
    }

    #[test]
    fn terminal_completion_needs_any_wp_and_all_done() {
        assert!(!terminal_completion(None, &[]));
        assert!(!terminal_completion(
            None,
            &[
                ("WP01".into(), "done".into()),
                ("WP02".into(), "in_progress".into())
            ]
        ));
        assert!(terminal_completion(
            None,
            &[
                ("WP01".into(), "done".into()),
                ("WP02".into(), "done".into())
            ]
        ));
        assert!(terminal_completion(
            Some(7),
            &[("WP01".into(), "planned".into())]
        ));
    }
}
