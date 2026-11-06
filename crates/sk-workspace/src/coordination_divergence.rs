//! Expected coordination-vs-target divergence (the T034/D6 exemption).
//!
//! A coordination branch that is *ahead of* its target is normal and expected:
//! creating a Mission seeds and records its own COORD-partition bookkeeping, and
//! that history lives on the coordination branch by design. The genuine error the
//! guard exists to catch is a coordination branch whose extra history is **not**
//! only this Mission's coordination bookkeeping, or a target whose own history
//! has started writing the same coordination files.
//!
//! This module is the native projection of upstream
//! `specify_cli/missions/_create.py::is_expected_coordination_divergence` and its
//! three helpers (`_merge_base_or_none`, `_commits_all_touch_only_coord_paths`,
//! `_commits_touch_any_coord_path`), plus the per-commit path read
//! (`kernel/git/listing.py::commit_paths(..., first_parent=True)`), which lives
//! in [`sk_git::commit_history`].
//!
//! # The predicate
//!
//! [`is_expected_coordination_divergence`] is true iff **all** of:
//!
//! 1. a merge base exists between the coordination branch and the target — a
//!    shallow clone or genuinely unrelated histories has none and fails
//!    **closed** to `false` ("fail toward reporting": an unreadable history must
//!    never pass the guard);
//! 2. every commit unique to the coordination branch (`target..coordination`)
//!    changes **only** this Mission's COORD-partition paths;
//! 3. the target's own commits since the merge base (`merge_base..target`) never
//!    touch those same COORD paths.
//!
//! Identical tips are vacuously expected: both ranges are empty, so an unchanged
//! coordination branch can never be a genuine divergence.
//!
//! Every unreadable input — a missing branch, a failed `merge-base`, a failed
//! range listing, an unreadable commit's paths — makes the answer `false`
//! (never "unknown/clean"), and [`classify_expected_coordination_divergence`]
//! reports *which* input failed so a caller can render an honest diagnostic
//! instead of a fabricated verdict.
//!
//! # The ONE partition authority, and the owner mapping
//!
//! Path classification is **not** reimplemented here. Upstream splits the job
//! across `mission_runtime/artifacts.py`:
//!
//! | Upstream | Native owner | This module's use |
//! | --- | --- | --- |
//! | `mission_runtime.artifacts.kind_for_mission_file` | [`crate::artifact::kind_for_mission_file`] | called once per changed path |
//! | `_PRIMARY_ARTIFACT_KINDS` (`is_primary_artifact_kind`) | [`crate::artifact::MissionArtifactKind::primary_kinds`] / [`crate::artifact::MissionArtifactKind::is_primary_partition`] | the partition question |
//! | `_PLACEMENT_ARTIFACT_KINDS` (the COORD partition) | [`crate::artifact::MissionArtifactKind::coord_kinds`] / [`crate::artifact::MissionArtifactKind::is_coord_partition`] | "is this a COORD path?" |
//!
//! [`crate::artifact`] is the canonical native partition classifier, so the
//! upstream *module names* (`missions/artifacts.py`, `missions/_create.py`) are
//! deliberately **not** mirrored as a second module or a second table: copying
//! or moving the table would create the split-brain authority DIRECTIVE_044
//! forbids and, in this crate, a dependency cycle. In particular
//! `sk-mission/src/artifacts.rs` is **task publication** (authoring
//! `plan.md`/`tasks.md`/WP prompts), not a partition classifier, so it is not
//! and must not become the owner of this question.
//!
//! A path this classifier cannot classify — outside `kitty-specs/`, another
//! Mission's directory, an unrecognised file under this Mission, or bytes that
//! are not valid UTF-8 — is **not** a COORD-only path. That single rule is what
//! makes an unexpected commit fail closed instead of slipping through as
//! "expected divergence", exactly as upstream's `None`-kind branch does.
//!
//! # Where it sits
//!
//! This module is a leaf: it depends only on [`crate::artifact`] and `sk-git`,
//! declares no verdict of its own, and writes nothing. The health/coordination
//! consumers that need the exemption (`coordination_health`'s `--check-staleness`
//! path and the doctor finding that currently declares
//! `UnavailableCheck::ExpectedCoordinationDivergence`) call it; it never calls
//! them, so wiring the exemption cannot create a cycle in either direction.
//!
//! # Read-only
//!
//! Only bounded read-only git queries are issued (branch tips, `merge-base`,
//! `rev-list`, and one `show --name-only` per commit). No ref, index, worktree,
//! object or lock is written, nothing is materialized, and no OID, path or
//! classification is invented: every [`Oid`] returned in a cause is one this
//! call actually read.

use sk_git::{BranchName, GitPath, GitRepository, Oid, commit_history};

use crate::artifact::kind_for_mission_file;

/// Which side of the comparison a read failed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DivergenceSide {
    /// The coordination branch.
    Coordination,
    /// The target branch.
    Target,
}

/// Which commit range a read failed on.
///
/// The two ranges are the predicate's own two: the commits unique to the
/// coordination branch, and the target's commits since the merge base.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DivergenceRange {
    /// `target..coordination` — the commits the coordination branch adds.
    TargetToCoordination,
    /// `merge_base..target` — the target's own commits since the fork point.
    MergeBaseToTarget,
}

/// Why a divergence is, or is not, expected — the observed facts, not a guess.
///
/// [`Self::Expected`] is the only cause that makes the predicate true; every
/// other cause is a fail-closed answer and names the exact input that could not
/// be read, or the exact commit and path that broke the rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedDivergenceCause {
    /// The coordination branch is ahead by design: every commit unique to it
    /// changed only this Mission's COORD paths, and the target has not touched
    /// those paths since the merge base.
    Expected {
        /// Commits unique to the coordination branch (`target..coordination`).
        coordination_commits: usize,
        /// The target's own commits since the merge base.
        target_commits_since_merge_base: usize,
    },
    /// A branch name does not resolve to a commit in this repository.
    MissingBranch {
        /// Which branch could not be read.
        side: DivergenceSide,
    },
    /// The two branches share no common ancestor (unrelated histories, or a
    /// shallow clone that has not fetched the fork point).
    NoMergeBase,
    /// `git merge-base` failed for a reason other than "no common ancestor".
    UnreadableMergeBase,
    /// A commit range could not be listed.
    UnreadableRange {
        /// Which range failed.
        range: DivergenceRange,
    },
    /// One commit's changed paths could not be read.
    UnreadableCommitPaths {
        /// The commit whose paths failed. Read from the range, never guessed.
        commit: Oid,
    },
    /// A commit unique to the coordination branch changed a path that is not
    /// this Mission's COORD bookkeeping, so the extra history is not by design.
    CoordinationCommitTouchesNonCoordPath {
        /// The offending commit.
        commit: Oid,
        /// The first changed path that is not a COORD path of this Mission.
        path: GitPath,
    },
    /// The target's own history since the merge base touched a COORD path of
    /// this Mission, so the coordination branch's extra history is now
    /// contested rather than expected.
    TargetCommitTouchesCoordPath {
        /// The offending target commit.
        commit: Oid,
        /// The COORD path it touched.
        path: GitPath,
    },
}

impl ExpectedDivergenceCause {
    /// Is this cause a positive, fully-probed "expected" answer?
    pub fn is_expected(&self) -> bool {
        matches!(self, Self::Expected { .. })
    }
}

/// The typed input to the predicate: two real branches and the Mission's
/// on-disk directory name.
///
/// `mission_dir_name` is the `kitty-specs/<mission_dir_name>/` component — the
/// *formatted* directory name that carries the `mid8` suffix (upstream
/// `mission_slug` at the call site), not a bare human slug. A path outside that
/// directory is never this Mission's COORD path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpectedCoordinationDivergenceRequest<'a> {
    /// The Mission's coordination branch (for example
    /// `kitty/mission-foo-01ABCDEF`).
    pub coordination_branch: &'a BranchName,
    /// The branch the coordination branch was parented off.
    pub target_branch: &'a BranchName,
    /// The `kitty-specs/<mission_dir_name>/` component this Mission owns.
    pub mission_dir_name: &'a str,
}

impl<'a> ExpectedCoordinationDivergenceRequest<'a> {
    /// Build a request from the two real branch names and the Mission directory.
    pub fn new(
        coordination_branch: &'a BranchName,
        target_branch: &'a BranchName,
        mission_dir_name: &'a str,
    ) -> Self {
        Self {
            coordination_branch,
            target_branch,
            mission_dir_name,
        }
    }
}

/// The typed outcome: the boolean answer plus the facts behind it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedCoordinationDivergenceOutcome {
    cause: ExpectedDivergenceCause,
}

impl ExpectedCoordinationDivergenceOutcome {
    /// The predicate's answer. `false` on every unreadable input.
    pub fn expected(&self) -> bool {
        self.cause.is_expected()
    }

    /// What was actually observed.
    pub fn cause(&self) -> &ExpectedDivergenceCause {
        &self.cause
    }
}

/// Is the coordination branch ahead of the target BY DESIGN?
///
/// This is the boolean the reference exposes and the one a caller asking
/// "should I report this as divergence?" wants. Every unreadable input answers
/// `false` (fail toward reporting). Use
/// [`classify_expected_coordination_divergence`] when the *reason* matters.
///
/// The read is entirely read-only.
pub fn is_expected_coordination_divergence(
    repo: &GitRepository,
    request: &ExpectedCoordinationDivergenceRequest<'_>,
) -> bool {
    classify_expected_coordination_divergence(repo, request).expected()
}

/// The same predicate, with the observed cause instead of only the boolean.
///
/// The probe order is the reference's: resolve both tips, then the merge base,
/// then the coordination-unique range, then the target-since-base range. The
/// order is load-bearing, because the first unreadable input decides the
/// fail-closed answer and a later range must not be read after an earlier one
/// already refuted or could not establish the premise.
pub fn classify_expected_coordination_divergence(
    repo: &GitRepository,
    request: &ExpectedCoordinationDivergenceRequest<'_>,
) -> ExpectedCoordinationDivergenceOutcome {
    let outcome = |cause| ExpectedCoordinationDivergenceOutcome { cause };

    let Some(coordination_tip) = branch_tip(repo, request.coordination_branch) else {
        return outcome(ExpectedDivergenceCause::MissingBranch {
            side: DivergenceSide::Coordination,
        });
    };
    let Some(target_tip) = branch_tip(repo, request.target_branch) else {
        return outcome(ExpectedDivergenceCause::MissingBranch {
            side: DivergenceSide::Target,
        });
    };

    let merge_base = match repo.merge_base(&coordination_tip, &target_tip) {
        Ok(Some(merge_base)) => merge_base,
        Ok(None) => return outcome(ExpectedDivergenceCause::NoMergeBase),
        Err(_) => return outcome(ExpectedDivergenceCause::UnreadableMergeBase),
    };

    // (2) `target..coordination`: every commit must be this Mission's COORD work.
    let coordination_commits =
        match commit_history::commits_in_range(repo, &target_tip, &coordination_tip) {
            Ok(commits) => commits,
            Err(_) => {
                return outcome(ExpectedDivergenceCause::UnreadableRange {
                    range: DivergenceRange::TargetToCoordination,
                });
            }
        };
    for commit in &coordination_commits {
        let paths = match commit_history::commit_paths(repo, commit) {
            Ok(paths) => paths,
            Err(_) => {
                return outcome(ExpectedDivergenceCause::UnreadableCommitPaths {
                    commit: commit.clone(),
                });
            }
        };
        if let Some(path) = paths
            .iter()
            .find(|path| !is_coord_only_path_git_path(path, request.mission_dir_name))
        {
            return outcome(
                ExpectedDivergenceCause::CoordinationCommitTouchesNonCoordPath {
                    commit: commit.clone(),
                    path: path.clone(),
                },
            );
        }
    }

    // (3) `merge_base..target`: the target must not have touched the same paths.
    let target_commits = match commit_history::commits_in_range(repo, &merge_base, &target_tip) {
        Ok(commits) => commits,
        Err(_) => {
            return outcome(ExpectedDivergenceCause::UnreadableRange {
                range: DivergenceRange::MergeBaseToTarget,
            });
        }
    };
    for commit in &target_commits {
        let paths = match commit_history::commit_paths(repo, commit) {
            Ok(paths) => paths,
            Err(_) => {
                return outcome(ExpectedDivergenceCause::UnreadableCommitPaths {
                    commit: commit.clone(),
                });
            }
        };
        if let Some(path) = paths
            .iter()
            .find(|path| is_coord_only_path_git_path(path, request.mission_dir_name))
        {
            return outcome(ExpectedDivergenceCause::TargetCommitTouchesCoordPath {
                commit: commit.clone(),
                path: path.clone(),
            });
        }
    }

    outcome(ExpectedDivergenceCause::Expected {
        coordination_commits: coordination_commits.len(),
        target_commits_since_merge_base: target_commits.len(),
    })
}

/// Is `path` a COORD-partition file of the Mission whose directory is
/// `mission_dir_name`?
///
/// This is upstream `_is_coord_only_path`: one call into the canonical
/// classifier ([`kind_for_mission_file`]), then the kind's partition. `None` —
/// an unclassifiable path, another Mission's directory, or a path outside
/// `kitty-specs/` — is **not** COORD, so it fails closed.
///
/// Valid UTF-8 is required: a path whose bytes are not valid UTF-8 has no `&str`
/// form, so it is reported as not COORD (upstream would classify its
/// autodecoded form, which for an unrecognised name is also not COORD). This is
/// a fail-closed narrowing, not a fabricated classification.
pub fn is_coord_only_path(path: &str, mission_dir_name: &str) -> bool {
    match kind_for_mission_file(path, Some(mission_dir_name)) {
        Some(kind) => kind.is_coord_partition(),
        None => false,
    }
}

/// [`is_coord_only_path`] for a path git printed, without a lossy conversion.
fn is_coord_only_path_git_path(path: &GitPath, mission_dir_name: &str) -> bool {
    match path.as_os_str().to_str() {
        Some(text) => is_coord_only_path(text, mission_dir_name),
        // Not representable as `&str`: never claim it is a COORD path.
        None => false,
    }
}

/// Resolve a branch to its tip, treating an unreadable tip as absent.
///
/// A missing branch and a failed read both mean "this predicate cannot be
/// established from real refs", which is the same fail-closed answer the
/// reference gets from `git merge-base` on an unresolvable revision.
fn branch_tip(repo: &GitRepository, branch: &BranchName) -> Option<Oid> {
    repo.read_branch_tip(branch).ok().flatten()
}
