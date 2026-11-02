//! Bookkeeping destination policy: the protected-branch refusal.
//!
//! This is the pure Rust projection of the destination-ref half of
//! `specify_cli/coordination/policy.py`. It inspects a candidate destination
//! ref, the mission's stored topology, its coordination branch, and the
//! caller-supplied protected set, and returns a verdict. It performs no I/O and
//! never mutates anything, so a transaction can refuse *before* it writes.
//!
//! The protected set itself is an application-layer fact supplied by the
//! caller, matching `sk-git`'s boundary. This module never reads
//! `.kittify/config.yaml`, an environment escape hatch, or a commit-guard hook:
//! an override must be an explicit, non-empty human reason.

use sk_git::{BranchName, ProtectedBranches};
use sk_mission::{MissionTopology, routes_through_coordination};

/// Stable error code for a protected-destination refusal.
pub const PROTECTED_BRANCH_REFUSED: &str = "PROTECTED_BRANCH_REFUSED";

/// Stable error code for an unusable destination spelling.
pub const DESTINATION_REF_INVALID: &str = "DESTINATION_REF_INVALID";

/// The outcome of evaluating a candidate bookkeeping destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyVerdict {
    /// The destination is writable.
    Allowed {
        /// The destination ref, without a `refs/heads/` prefix.
        destination_ref: String,
        /// Why an otherwise-protected destination was allowed, when overridden.
        override_reason: Option<String>,
    },
    /// The destination must not be written.
    Refused {
        /// Stable machine-readable code.
        error_code: &'static str,
        /// The destination ref, without a `refs/heads/` prefix.
        destination_ref: String,
        /// Human explanation.
        message: String,
        /// The followable remedy.
        next_step: String,
    },
}

impl PolicyVerdict {
    /// Is the destination writable?
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }

    /// Is the destination refused?
    pub fn is_refused(&self) -> bool {
        matches!(self, Self::Refused { .. })
    }
}

/// Evaluate bookkeeping destinations against a protected set.
#[derive(Clone, Debug)]
pub struct MutationPolicy {
    protected: ProtectedBranches,
}

impl MutationPolicy {
    /// Build a policy from an application-supplied protected set.
    pub fn new(protected: ProtectedBranches) -> Self {
        Self { protected }
    }

    /// The protected set this policy enforces.
    pub fn protected(&self) -> &ProtectedBranches {
        &self.protected
    }

    /// Evaluate `destination_ref` without an override.
    pub fn evaluate_destination(
        &self,
        destination_ref: &str,
        topology: MissionTopology,
        coordination_branch: Option<&str>,
    ) -> PolicyVerdict {
        self.evaluate_destination_with_override(
            destination_ref,
            topology,
            coordination_branch,
            None,
        )
    }

    /// Evaluate `destination_ref`, optionally carrying an explicit override.
    ///
    /// An override must be a non-empty (after trimming) human reason. A blank
    /// reason is not an override: it cannot silently unlock a protected
    /// branch.
    pub fn evaluate_destination_with_override(
        &self,
        destination_ref: &str,
        topology: MissionTopology,
        coordination_branch: Option<&str>,
        override_reason: Option<&str>,
    ) -> PolicyVerdict {
        let normalized = destination_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(destination_ref);
        let Ok(branch) = BranchName::parse(normalized) else {
            return PolicyVerdict::Refused {
                error_code: DESTINATION_REF_INVALID,
                destination_ref: normalized.to_owned(),
                message: format!(
                    "refusing to record bookkeeping: {destination_ref:?} is not a usable branch name"
                ),
                next_step: "pass a valid local branch name, or let the placement seam resolve the destination"
                    .to_owned(),
            };
        };
        if !self.protected.contains(&branch) {
            return PolicyVerdict::Allowed {
                destination_ref: normalized.to_owned(),
                override_reason: None,
            };
        }
        if let Some(reason) = override_reason
            && !reason.trim().is_empty()
        {
            return PolicyVerdict::Allowed {
                destination_ref: normalized.to_owned(),
                override_reason: Some(reason.trim().to_owned()),
            };
        }

        let routes = routes_through_coordination(topology);
        let (message, next_step) = match (routes, coordination_branch) {
            (true, Some(coordination)) if coordination != normalized => (
                format!(
                    "refusing to record bookkeeping on protected branch {normalized:?}: this mission routes bookkeeping through its coordination branch"
                ),
                format!(
                    "commit to the coordination branch {coordination:?} through the coordination transaction; the coordination worktree is resolved automatically"
                ),
            ),
            (true, Some(coordination)) => (
                format!(
                    "refusing to record bookkeeping on protected branch {normalized:?}: the mission's own coordination branch is on the protected-branch list"
                ),
                format!(
                    "remove {coordination:?} from the protected-branch list, or record this write with an explicit operator reason"
                ),
            ),
            (true, None) => (
                format!(
                    "refusing to record bookkeeping on protected branch {normalized:?}: the mission declares coordination routing but no coordination branch"
                ),
                "repair the mission's coordination_branch before recording bookkeeping".to_owned(),
            ),
            (false, _) => (
                format!(
                    "refusing to record bookkeeping on protected branch {normalized:?}: this mission's topology mints no coordination worktree to redirect to"
                ),
                format!(
                    "commit to a non-protected branch, or declare {normalized:?} unprotected for this repository"
                ),
            ),
        };
        PolicyVerdict::Refused {
            error_code: PROTECTED_BRANCH_REFUSED,
            destination_ref: normalized.to_owned(),
            message,
            next_step,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> MutationPolicy {
        MutationPolicy::new(ProtectedBranches::parse(["main"]).unwrap())
    }

    #[test]
    fn unprotected_destination_is_allowed() {
        let verdict = policy().evaluate_destination(
            "kitty/mission-my-mission-01KNXQS9",
            MissionTopology::Lanes,
            None,
        );
        assert!(verdict.is_allowed());
    }

    #[test]
    fn protected_destination_refuses_with_the_coordination_redirect() {
        let verdict = policy().evaluate_destination(
            "refs/heads/main",
            MissionTopology::Coord,
            Some("kitty/mission-my-mission-01KNXQS9"),
        );
        assert!(verdict.is_refused());
        if let PolicyVerdict::Refused {
            error_code,
            destination_ref,
            next_step,
            ..
        } = verdict
        {
            assert_eq!(error_code, PROTECTED_BRANCH_REFUSED);
            assert_eq!(destination_ref, "main");
            assert!(next_step.contains("kitty/mission-my-mission-01KNXQS9"));
        }
    }

    #[test]
    fn coord_less_topology_has_no_redirect() {
        let verdict = policy().evaluate_destination("main", MissionTopology::SingleBranch, None);
        assert!(verdict.is_refused());
        if let PolicyVerdict::Refused { message, .. } = verdict {
            assert!(message.contains("no coordination worktree"));
        }
    }

    #[test]
    fn a_blank_override_is_not_an_override() {
        let verdict = policy().evaluate_destination_with_override(
            "main",
            MissionTopology::Lanes,
            None,
            Some("   "),
        );
        assert!(verdict.is_refused());
        let allowed = policy().evaluate_destination_with_override(
            "main",
            MissionTopology::Lanes,
            None,
            Some("operator approved a hotfix"),
        );
        assert!(allowed.is_allowed());
    }

    #[test]
    fn invalid_destination_spelling_is_refused() {
        let verdict = policy().evaluate_destination("bad..ref", MissionTopology::Lanes, None);
        assert!(matches!(
            verdict,
            PolicyVerdict::Refused {
                error_code: DESTINATION_REF_INVALID,
                ..
            }
        ));
    }
}
