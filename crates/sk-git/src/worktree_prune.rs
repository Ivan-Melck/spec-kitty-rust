//! Guarded pruning of proven-stale worktree registrations.
//!
//! `git worktree prune` is a repository-wide operation: it removes *every*
//! stale administrative registration, not one named entry. Repairing an
//! interrupted materialization therefore needs a plan that proves exactly which
//! registrations are stale before anything is removed.
//!
//! A registration is prunable here only when all of the following hold:
//!
//! 1. Git itself reports it `prunable` in `git worktree list --porcelain`
//!    (git has already decided the administrative link is broken);
//! 2. its checkout directory does not exist (a live directory is never
//!    pruned, whatever git's reason string says);
//! 3. its administrative directory under `<common>/worktrees/<id>` exists and
//!    its `gitdir` file names that checkout, so the plan can name the exact
//!    registration id;
//! 4. it is not the primary checkout.
//!
//! Applying the plan re-derives the same set, refuses if any registration
//! appeared, recovered, or vanished (`PlanStale`), asks git for a `--dry-run`
//! and requires the ids git would remove to equal the planned ids exactly, and
//! only then prunes. Live registrations (including the primary checkout) are
//! verified unchanged afterwards; a live checkout losing its directory is a
//! refusal, never a silent prune.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::repository::GitRepository;

/// The oldest git that supports `worktree prune --expire <time>`.
pub const WORKTREE_PRUNE_EXPIRE_MIN_VERSION: (u32, u32) = (2, 31);

/// One registration the plan proves stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePruneEntry {
    path: PathBuf,
    reason: Option<String>,
    admin_id: String,
}

impl WorktreePruneEntry {
    /// The checkout path the registration names (it does not exist).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Git's prunable reason, when it gave one.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The administrative directory name under `<common>/worktrees/`.
    pub fn admin_id(&self) -> &str {
        &self.admin_id
    }
}

/// A validated plan to prune exactly the proven-stale registrations.
#[derive(Debug, Clone)]
pub struct WorktreePrunePlan {
    prunable: Vec<WorktreePruneEntry>,
    live: Vec<PathBuf>,
}

impl WorktreePrunePlan {
    /// The registrations that will be pruned.
    pub fn prunable(&self) -> &[WorktreePruneEntry] {
        &self.prunable
    }

    /// The live registrations (including the primary checkout) that must
    /// survive unchanged.
    pub fn live(&self) -> &[PathBuf] {
        &self.live
    }
}

/// The evidence left by a guarded prune.
#[derive(Debug, Clone)]
pub struct WorktreePruneOutcome {
    pruned: Vec<WorktreePruneEntry>,
    live: Vec<PathBuf>,
}

impl WorktreePruneOutcome {
    /// The registrations that were pruned.
    pub fn pruned(&self) -> &[WorktreePruneEntry] {
        &self.pruned
    }

    /// The live registrations still present after the prune.
    pub fn live(&self) -> &[PathBuf] {
        &self.live
    }
}

impl GitRepository {
    /// Plan a prune of exactly the registrations git proves stale.
    ///
    /// Nothing is written. An empty plan means there is nothing to prune.
    pub fn plan_worktree_prune(&self) -> Result<WorktreePrunePlan, GitError> {
        let (prunable, live) = self.prune_candidates()?;
        Ok(WorktreePrunePlan { prunable, live })
    }

    /// Execute a prepared prune.
    ///
    /// Re-derives the stale set and refuses if it changed. A `--dry-run`
    /// must name exactly the planned administrative ids before the real prune
    /// runs, so a registration that became stale after planning is not removed
    /// without approval.
    pub fn apply_worktree_prune(
        &self,
        plan: &WorktreePrunePlan,
    ) -> Result<WorktreePruneOutcome, GitError> {
        let (mut planned, planned_live) = self.prune_candidates()?;
        planned.sort_by(|left, right| left.admin_id.cmp(&right.admin_id));
        let mut expected = plan.prunable.clone();
        expected.sort_by(|left, right| left.admin_id.cmp(&right.admin_id));
        if planned != expected || planned_live != plan.live {
            return Err(GitError::PlanStale {
                what: "worktree registrations".to_string(),
                detail: format!(
                    "planned {} prunable / {} live, observed {} prunable / {} live",
                    expected.len(),
                    plan.live.len(),
                    planned.len(),
                    planned_live.len()
                ),
            });
        }
        if expected.is_empty() {
            return Ok(WorktreePruneOutcome {
                pruned: Vec::new(),
                live: planned_live,
            });
        }

        let (major, minor, _patch) = self.git_version()?;
        if (major, minor) < WORKTREE_PRUNE_EXPIRE_MIN_VERSION {
            return Err(GitError::Unsupported {
                feature: "git worktree prune --expire (requires git >= 2.31)",
            });
        }

        let dry_run =
            self.process()
                .checked(&["worktree", "prune", "--dry-run", "--expire", "now", "-v"])?;
        // `-v` reports on stderr; accept both streams so a future git that
        // changes the stream does not silently disable the guard.
        let mut verbose = dry_run.stdout.clone();
        verbose.extend_from_slice(&dry_run.stderr);
        let dry_run_ids = parse_prune_verbose_ids(&verbose)?;
        let expected_ids: BTreeSet<String> = expected
            .iter()
            .map(|entry| entry.admin_id.clone())
            .collect();
        if dry_run_ids != expected_ids {
            return Err(GitError::PlanStale {
                what: "worktree prune preview".to_string(),
                detail: format!("planned ids {expected_ids:?}, git would prune {dry_run_ids:?}"),
            });
        }

        self.process()
            .checked(&["worktree", "prune", "--expire", "now", "-v"])?;

        let after = self.worktrees()?;
        for entry in &expected {
            if after
                .iter()
                .any(|worktree| same_path(worktree.path(), &entry.path))
            {
                return Err(GitError::WorktreeNotStale {
                    path: entry.path.clone(),
                    detail: "git reported success but the registration is still listed".to_string(),
                });
            }
        }
        let mut remaining: Vec<PathBuf> = after
            .iter()
            .map(|worktree| worktree.path().to_path_buf())
            .collect();
        remaining.sort();
        if remaining != planned_live {
            return Err(GitError::PlanStale {
                what: "worktree registrations".to_string(),
                detail: format!(
                    "live registrations changed during the prune: planned {planned_live:?}, observed {remaining:?}"
                ),
            });
        }

        Ok(WorktreePruneOutcome {
            pruned: expected,
            live: remaining,
        })
    }

    /// The stale-and-proven set plus the live registration paths.
    fn prune_candidates(&self) -> Result<(Vec<WorktreePruneEntry>, Vec<PathBuf>), GitError> {
        let worktrees = self.worktrees()?;
        let admin = self.worktree_admin_index()?;
        let mut prunable = Vec::new();
        let mut live = Vec::new();
        for worktree in &worktrees {
            match worktree.prunable() {
                None => live.push(worktree.path().to_path_buf()),
                Some(reason) => {
                    if same_path(worktree.path(), self.root()) {
                        return Err(GitError::WorktreeNotStale {
                            path: worktree.path().to_path_buf(),
                            detail: "the primary checkout is never prunable".to_string(),
                        });
                    }
                    match std::fs::symlink_metadata(worktree.path()) {
                        Ok(_) => {
                            return Err(GitError::WorktreeNotStale {
                                path: worktree.path().to_path_buf(),
                                detail: format!(
                                    "git marked it prunable ({reason:?}) but its directory still exists"
                                ),
                            });
                        }
                        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                        Err(source) => {
                            return Err(GitError::Io {
                                context: "inspect a prunable worktree path",
                                source,
                            });
                        }
                    }
                    let Some(admin_id) = admin
                        .iter()
                        .find(|(_, registered)| same_path(registered, worktree.path()))
                        .map(|(id, _)| id.clone())
                    else {
                        return Err(GitError::WorktreeNotStale {
                            path: worktree.path().to_path_buf(),
                            detail: "no administrative registration names this checkout"
                                .to_string(),
                        });
                    };
                    prunable.push(WorktreePruneEntry {
                        path: worktree.path().to_path_buf(),
                        reason: if reason.is_empty() {
                            None
                        } else {
                            Some(reason.to_string())
                        },
                        admin_id,
                    });
                }
            }
        }
        prunable.sort_by(|left, right| left.path.cmp(&right.path));
        live.sort();
        Ok((prunable, live))
    }

    /// Map `<common>/worktrees/<id>` to the checkout its `gitdir` file names.
    fn worktree_admin_index(&self) -> Result<Vec<(String, PathBuf)>, GitError> {
        let root = self.git_dirs()?.common_dir().join("worktrees");
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(GitError::Io {
                    context: "read the worktree administrative directory",
                    source,
                });
            }
        };
        let mut index = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| GitError::Io {
                context: "read a worktree administrative entry",
                source,
            })?;
            let file_type = entry.file_type().map_err(|source| GitError::Io {
                context: "inspect a worktree administrative entry",
                source,
            })?;
            if !file_type.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let gitdir = entry.path().join("gitdir");
            let bytes = match sk_kernel::no_follow::read_no_follow(&gitdir, 8192) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let text = String::from_utf8_lossy(&bytes);
            let raw = text.trim();
            if raw.is_empty() {
                continue;
            }
            let registered = PathBuf::from(raw);
            // A linked worktree's `gitdir` file names `<checkout>/.git`.
            let checkout = match registered.file_name() {
                Some(name) if name == std::ffi::OsStr::new(".git") => registered
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or(registered.clone()),
                _ => registered.clone(),
            };
            index.push((id, checkout));
        }
        Ok(index)
    }
}

/// Parse `git worktree prune -v --dry-run` output into the distinct admin ids.
fn parse_prune_verbose_ids(raw: &[u8]) -> Result<BTreeSet<String>, GitError> {
    let text = crate::path::utf8_field(raw, "worktree prune output")?;
    let mut ids = BTreeSet::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("Removing ") else {
            continue;
        };
        let Some((id, _)) = rest.split_once(':') else {
            return Err(GitError::InvalidOutput {
                detail: format!("malformed worktree prune line {line:?}"),
            });
        };
        let id = id.trim();
        let Some(id) = id.strip_prefix("worktrees/") else {
            return Err(GitError::InvalidOutput {
                detail: format!("unexpected worktree prune id {id:?}"),
            });
        };
        ids.insert(id.to_string());
    }
    Ok(ids)
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
