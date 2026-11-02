//! Worktree allocation and guarded removal.
//!
//! A worktree plan is a value: it records the path validation, the branch
//! state, and (for removal) the exact HEAD/branch/status fingerprint that was
//! approved. Execution re-derives that fingerprint and refuses with
//! [`GitError::PlanStale`] if anything moved, so a concurrent commit, checkout,
//! or newly written file cannot be silently destroyed.
//!
//! Removal is destructive and therefore opt-in: tracked changes are **always**
//! refused; untracked files require [`WorktreeDiscardPolicy::AllowUntracked`];
//! ignored files additionally require
//! [`WorktreeDiscardPolicy::AllowAll`]. A locked worktree, the primary
//! checkout, a path inside a git directory, and any operation in progress are
//! refused before a plan exists.
//!
//! Placement policy (where lane worktrees live, which branch a lane gets) is an
//! application concern. This module validates the path it is given but does not
//! decide the layout.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::GitError;
use crate::oid::Oid;
use crate::refname::{BranchName, StartPoint};
use crate::repository::GitRepository;
use crate::status::{StatusEntry, StatusOptions, UntrackedMode};
use crate::worktree::Worktree;

/// How a worktree gets its branch.
#[derive(Debug, Clone)]
pub enum WorktreeStart {
    /// Create a new branch at this start point.
    NewBranch(StartPoint),
    /// Attach an existing branch (recovery / reuse).
    ExistingBranch,
}

/// How much local state a removal is allowed to discard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorktreeDiscardPolicy {
    /// Refuse any status entry at all (the default).
    #[default]
    RefuseDirty,
    /// Permit untracked files (`??`) but refuse ignored and tracked state.
    AllowUntracked,
    /// Permit untracked and ignored files but still refuse tracked state.
    AllowAll,
}

/// A validated worktree allocation plan.
#[derive(Debug, Clone)]
pub struct WorktreeAddPlan {
    path: PathBuf,
    branch: BranchName,
    start: WorktreeStart,
    start_oid: Option<Oid>,
    branch_tip: Option<Oid>,
    creates_branch: bool,
}

impl WorktreeAddPlan {
    /// The absolute, normalised path the worktree will occupy.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The branch that will be checked out.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// Whether the plan creates a branch (as opposed to attaching one).
    pub fn creates_branch(&self) -> bool {
        self.creates_branch
    }

    /// The commit a new branch will start from.
    pub fn start_oid(&self) -> Option<&Oid> {
        self.start_oid.as_ref()
    }

    /// The tip observed for an existing branch.
    pub fn branch_tip(&self) -> Option<&Oid> {
        self.branch_tip.as_ref()
    }
}

/// The evidence left by a successful allocation.
#[derive(Debug, Clone)]
pub struct WorktreeAddOutcome {
    path: PathBuf,
    branch: BranchName,
    head: Oid,
    created_branch: bool,
}

impl WorktreeAddOutcome {
    /// The new checkout path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Its branch.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// Its HEAD after creation.
    pub fn head(&self) -> &Oid {
        &self.head
    }

    /// Whether the operation created the branch.
    pub fn created_branch(&self) -> bool {
        self.created_branch
    }
}

/// The exact state a removal was approved for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeFingerprint {
    head: Option<Oid>,
    branch: Option<BranchName>,
    detached: bool,
    locked: Option<String>,
    entries: Vec<StatusEntry>,
}

/// A validated, approved worktree removal.
#[derive(Debug, Clone)]
pub struct WorktreeRemovalPlan {
    path: PathBuf,
    policy: WorktreeDiscardPolicy,
    fingerprint: WorktreeFingerprint,
    discarded: Vec<String>,
}

impl WorktreeRemovalPlan {
    /// The absolute path that will be removed.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The discarding policy the plan was approved under.
    pub fn policy(&self) -> WorktreeDiscardPolicy {
        self.policy
    }

    /// The HEAD the plan observed.
    pub fn expected_head(&self) -> Option<&Oid> {
        self.fingerprint.head.as_ref()
    }

    /// The branch the plan observed.
    pub fn expected_branch(&self) -> Option<&BranchName> {
        self.fingerprint.branch.as_ref()
    }

    /// Display lines for the local state the plan was approved to discard.
    pub fn discarded(&self) -> &[String] {
        &self.discarded
    }
}

/// The evidence left by a successful removal.
#[derive(Debug, Clone)]
pub struct WorktreeRemovalOutcome {
    path: PathBuf,
    previous_head: Option<Oid>,
    previous_branch: Option<BranchName>,
    discarded: Vec<String>,
}

impl WorktreeRemovalOutcome {
    /// The path that was removed.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The HEAD it held when removed.
    pub fn previous_head(&self) -> Option<&Oid> {
        self.previous_head.as_ref()
    }

    /// The branch it had checked out, if any.
    pub fn previous_branch(&self) -> Option<&BranchName> {
        self.previous_branch.as_ref()
    }

    /// The local state that was discarded (empty for a clean checkout).
    pub fn discarded(&self) -> &[String] {
        &self.discarded
    }
}

impl GitRepository {
    /// Prepare a worktree allocation.
    ///
    /// Validates that the path is absolute and normalised, outside every git
    /// directory, not the primary checkout, not already registered, and either
    /// absent or an empty directory. A new branch must be absent; an existing
    /// branch must resolve and must not already be checked out anywhere. No
    /// operation may be in progress.
    pub fn plan_worktree_add(
        &self,
        path: PathBuf,
        branch: &BranchName,
        start: WorktreeStart,
    ) -> Result<WorktreeAddPlan, GitError> {
        if self.is_operation_in_progress()? {
            let operations = self.in_progress_operations()?;
            return Err(GitError::OperationInProgress {
                operation: operations
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        let path = self.validate_new_worktree_path(&path)?;

        let (start_oid, branch_tip, creates_branch) = match &start {
            WorktreeStart::NewBranch(start_point) => {
                if let Some(tip) = self.read_branch_tip(branch)? {
                    return Err(GitError::BranchAlreadyExists {
                        branch: branch.as_str().to_string(),
                        tip: tip.to_string(),
                    });
                }
                (Some(self.resolve_commit(start_point)?), None, true)
            }
            WorktreeStart::ExistingBranch => {
                let Some(tip) = self.read_branch_tip(branch)? else {
                    return Err(GitError::BranchMissing {
                        branch: branch.as_str().to_string(),
                    });
                };
                let checkouts = self.checkouts_of(branch)?;
                if !checkouts.is_empty() {
                    return Err(GitError::BranchCheckedOut {
                        branch: branch.as_str().to_string(),
                        worktrees: checkouts,
                    });
                }
                (None, Some(tip), false)
            }
        };

        Ok(WorktreeAddPlan {
            path,
            branch: branch.clone(),
            start,
            start_oid,
            branch_tip,
            creates_branch,
        })
    }

    /// Execute a prepared allocation.
    ///
    /// The parent directory is created (a worktree's own directory is git's to
    /// create). Before running git the plan's branch/path facts are rechecked.
    /// On failure the error reports whether a branch or registration was left
    /// behind.
    pub fn apply_worktree_add(
        &self,
        plan: &WorktreeAddPlan,
    ) -> Result<WorktreeAddOutcome, GitError> {
        self.validate_new_worktree_path(&plan.path)?;
        match &plan.start {
            WorktreeStart::NewBranch(_) => {
                if let Some(tip) = self.read_branch_tip(&plan.branch)? {
                    return Err(GitError::PlanStale {
                        what: plan.branch.as_ref_name().to_string(),
                        detail: format!("planned absent, observed {tip}"),
                    });
                }
            }
            WorktreeStart::ExistingBranch => {
                let current = self.read_branch_tip(&plan.branch)?;
                if current != plan.branch_tip {
                    return Err(GitError::PlanStale {
                        what: plan.branch.as_ref_name().to_string(),
                        detail: format!("planned {:?}, observed {:?}", plan.branch_tip, current),
                    });
                }
            }
        }
        if let Some(parent) = plan.path.parent() {
            fs::create_dir_all(parent).map_err(|source| GitError::Io {
                context: "create the worktree parent directory",
                source,
            })?;
        }

        let path = plan.path.as_os_str();
        let branch = plan.branch.as_str();
        let mut args: Vec<std::ffi::OsString> = vec!["worktree".into(), "add".into()];
        match (&plan.start, &plan.start_oid) {
            (WorktreeStart::NewBranch(_), Some(oid)) => {
                args.push("-b".into());
                args.push(branch.into());
                args.push(path.into());
                args.push(oid.as_str().into());
            }
            (WorktreeStart::ExistingBranch, _) => {
                args.push(path.into());
                args.push(branch.into());
            }
            _ => {
                return Err(GitError::InvalidPlan {
                    detail: "worktree add plan has no resolved start object".to_string(),
                });
            }
        }

        let output = self.process().run(&args)?;
        if !output.status.success() {
            let branch_created = self.read_branch_tip(&plan.branch)?.is_some();
            let registered = self.registered_worktree_path(&plan.path)?.is_some();
            return Err(GitError::WorktreeAddFailed {
                path: plan.path.clone(),
                branch: plan.branch.as_str().to_string(),
                detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                branch_created,
                registered,
            });
        }

        let registered =
            self.registered_worktree_path(&plan.path)?
                .ok_or_else(|| GitError::InvalidOutput {
                    detail: format!(
                        "git worktree add succeeded but {} is not registered",
                        plan.path.display()
                    ),
                })?;
        let head = registered
            .head()
            .ok_or_else(|| GitError::InvalidOutput {
                detail: format!("{} has no HEAD after creation", plan.path.display()),
            })
            .and_then(Oid::parse)?;
        Ok(WorktreeAddOutcome {
            path: plan.path.clone(),
            branch: plan.branch.clone(),
            head,
            created_branch: plan.creates_branch,
        })
    }

    /// Prepare a guarded worktree removal.
    ///
    /// The worktree must be registered, not the primary checkout, not locked,
    /// and free of any in-progress operation. `expected_head`/`expected_branch`
    /// are optional extra assertions; when supplied, a mismatch refuses. Local
    /// state is refused according to `policy`.
    pub fn plan_worktree_removal(
        &self,
        path: &Path,
        expected_head: Option<&Oid>,
        expected_branch: Option<&BranchName>,
        policy: WorktreeDiscardPolicy,
    ) -> Result<WorktreeRemovalPlan, GitError> {
        let path = normalize_absolute(path)?;
        let fingerprint = self.worktree_fingerprint(&path)?;
        if let Some(expected) = expected_head
            && fingerprint.head.as_ref() != Some(expected)
        {
            return Err(GitError::PlanStale {
                what: path.display().to_string(),
                detail: format!(
                    "expected HEAD {expected}, observed {}",
                    fingerprint
                        .head
                        .as_ref()
                        .map(Oid::to_string)
                        .unwrap_or_else(|| "detached/absent".to_string())
                ),
            });
        }
        if let Some(expected) = expected_branch
            && fingerprint.branch.as_ref() != Some(expected)
        {
            return Err(GitError::PlanStale {
                what: path.display().to_string(),
                detail: format!(
                    "expected branch {expected}, observed {}",
                    fingerprint
                        .branch
                        .as_ref()
                        .map(BranchName::to_string)
                        .unwrap_or_else(|| "detached".to_string())
                ),
            });
        }
        let discarded = classify_dirty(&path, &fingerprint.entries, policy)?;
        Ok(WorktreeRemovalPlan {
            path,
            policy,
            fingerprint,
            discarded,
        })
    }

    /// Execute a prepared removal after proving the approved fingerprint still
    /// holds.
    pub fn apply_worktree_removal(
        &self,
        plan: &WorktreeRemovalPlan,
    ) -> Result<WorktreeRemovalOutcome, GitError> {
        let current = self.worktree_fingerprint(&plan.path)?;
        if current != plan.fingerprint {
            return Err(GitError::PlanStale {
                what: plan.path.display().to_string(),
                detail: describe_fingerprint_change(&plan.fingerprint, &current),
            });
        }
        // Re-apply the policy: the fingerprint equality above already proves
        // the entries are identical, but this keeps the refusal vocabulary
        // identical between plan and apply.
        let _ = classify_dirty(&plan.path, &current.entries, plan.policy)?;

        let args: Vec<OsString> = vec![
            OsString::from("worktree"),
            OsString::from("remove"),
            OsString::from("--force"),
            plan.path.as_os_str().to_os_string(),
        ];
        let output = self.process().run(&args)?;
        if !output.status.success() {
            let still_registered = self.registered_worktree_path(&plan.path)?.is_some();
            return Err(GitError::WorktreeRemoveFailed {
                path: plan.path.clone(),
                detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                still_registered,
            });
        }

        Ok(WorktreeRemovalOutcome {
            path: plan.path.clone(),
            previous_head: plan.fingerprint.head.clone(),
            previous_branch: plan.fingerprint.branch.clone(),
            discarded: plan.discarded.clone(),
        })
    }

    /// Resolve and validate a path for a *new* worktree.
    fn validate_new_worktree_path(&self, path: &Path) -> Result<PathBuf, GitError> {
        let path = normalize_absolute(path)?;
        if path == self.root() {
            return Err(GitError::WorktreePathUnavailable {
                path,
                detail: "the primary checkout root is not a worktree allocation target".to_string(),
            });
        }
        let dirs = self.git_dirs()?;
        let resolved = normalized_existing_or_lexical(&path)?;
        if resolved.starts_with(dirs.git_dir()) || resolved.starts_with(dirs.common_dir()) {
            return Err(GitError::UnsafePath {
                path,
                detail: "lies inside a git directory".to_string(),
            });
        }
        if let Some(registered) = self.registered_worktree_path(&path)? {
            return Err(GitError::WorktreePathUnavailable {
                path,
                detail: format!(
                    "already registered as a worktree at {}",
                    registered.path().display()
                ),
            });
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(GitError::UnsafePath {
                        path,
                        detail: "exists and is not a real directory".to_string(),
                    });
                }
                let mut entries = fs::read_dir(&path).map_err(|source| GitError::Io {
                    context: "read the prospective worktree directory",
                    source,
                })?;
                if entries.next().is_some() {
                    return Err(GitError::WorktreePathUnavailable {
                        path,
                        detail: "directory exists and is not empty".to_string(),
                    });
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                require_directory_ancestor(&path)?;
            }
            Err(source) => {
                return Err(GitError::Io {
                    context: "inspect the prospective worktree path",
                    source,
                });
            }
        }
        Ok(path)
    }

    fn registered_worktree_path(&self, path: &Path) -> Result<Option<Worktree>, GitError> {
        let wanted = normalized_existing_or_lexical(path)?;
        for worktree in self.worktrees()? {
            if normalized_existing_or_lexical(worktree.path())? == wanted {
                return Ok(Some(worktree));
            }
        }
        Ok(None)
    }

    fn worktree_fingerprint(&self, path: &Path) -> Result<WorktreeFingerprint, GitError> {
        let Some(worktree) = self.registered_worktree_path(path)? else {
            return Err(GitError::WorktreeNotRegistered {
                path: path.to_path_buf(),
            });
        };
        if normalized_existing_or_lexical(worktree.path())?
            == normalized_existing_or_lexical(self.root())?
        {
            return Err(GitError::WorktreePathUnavailable {
                path: path.to_path_buf(),
                detail: "the primary checkout cannot be removed as a linked worktree".to_string(),
            });
        }
        if let Some(reason) = worktree.locked() {
            return Err(GitError::WorktreePathUnavailable {
                path: path.to_path_buf(),
                detail: format!("worktree is locked ({reason})"),
            });
        }
        let handle = GitRepository::open_with_options(worktree.path(), self.options())?;
        if handle.is_operation_in_progress()? {
            let operations = handle.in_progress_operations()?;
            return Err(GitError::OperationInProgress {
                operation: operations
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        let entries = crate::status::status_at(
            &self.process_at(worktree.path().to_path_buf()),
            &StatusOptions {
                untracked: UntrackedMode::All,
                ignored: true,
            },
        )?;
        let head = worktree.head().map(Oid::parse).transpose()?;
        let branch = worktree.branch().map(BranchName::parse).transpose()?;
        Ok(WorktreeFingerprint {
            head,
            branch,
            detached: worktree.is_detached(),
            locked: worktree.locked().map(str::to_string),
            entries,
        })
    }
}

/// Every existing ancestor of `path` must be a real directory (not a symlink
/// or a regular file), so `create_dir_all` cannot be walked through an
/// unexpected filesystem object.
fn require_directory_ancestor(path: &Path) -> Result<(), GitError> {
    let mut cursor = path.parent();
    while let Some(candidate) = cursor {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(GitError::UnsafePath {
                        path: path.to_path_buf(),
                        detail: format!(
                            "ancestor {} exists and is not a real directory",
                            candidate.display()
                        ),
                    });
                }
                return Ok(());
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                cursor = candidate.parent();
            }
            Err(source) => {
                return Err(GitError::Io {
                    context: "inspect a prospective worktree ancestor",
                    source,
                });
            }
        }
    }
    Ok(())
}

fn classify_dirty(
    path: &Path,
    entries: &[StatusEntry],
    policy: WorktreeDiscardPolicy,
) -> Result<Vec<String>, GitError> {
    let mut refused = Vec::new();
    let mut discarded = Vec::new();
    for entry in entries {
        let display = entry.display_lossy();
        let allowed = if entry.is_ignored() {
            policy == WorktreeDiscardPolicy::AllowAll
        } else if entry.is_untracked() {
            policy != WorktreeDiscardPolicy::RefuseDirty
        } else {
            false
        };
        if allowed {
            discarded.push(display);
        } else {
            refused.push(display);
        }
    }
    if !refused.is_empty() {
        return Err(GitError::DirtyWorktree {
            path: path.to_path_buf(),
            entries: refused,
        });
    }
    Ok(discarded)
}

fn describe_fingerprint_change(
    planned: &WorktreeFingerprint,
    current: &WorktreeFingerprint,
) -> String {
    let mut changes = Vec::new();
    if planned.head != current.head {
        changes.push(format!("HEAD {:?} -> {:?}", planned.head, current.head));
    }
    if planned.branch != current.branch {
        changes.push(format!(
            "branch {:?} -> {:?}",
            planned.branch, current.branch
        ));
    }
    if planned.detached != current.detached {
        changes.push("detached state changed".to_string());
    }
    if planned.locked != current.locked {
        changes.push("lock state changed".to_string());
    }
    if planned.entries != current.entries {
        changes.push(format!(
            "status {} entries -> {} entries",
            planned.entries.len(),
            current.entries.len()
        ));
    }
    if changes.is_empty() {
        "state changed".to_string()
    } else {
        changes.join(", ")
    }
}

fn normalize_absolute(path: &Path) -> Result<PathBuf, GitError> {
    if !path.is_absolute() {
        return Err(GitError::UnsafePath {
            path: path.to_path_buf(),
            detail: "a worktree path must be absolute".to_string(),
        });
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(GitError::UnsafePath {
                    path: path.to_path_buf(),
                    detail: "must not contain `..`".to_string(),
                });
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    Ok(normalized)
}

/// Compare paths by their canonical form when they exist, and lexically when
/// they do not (a planned worktree directory does not exist yet).
fn normalized_existing_or_lexical(path: &Path) -> Result<PathBuf, GitError> {
    let mut missing: Vec<OsString> = Vec::new();
    let mut cursor = path;
    loop {
        match fs::canonicalize(cursor) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = cursor.file_name() else {
                    return Err(GitError::UnsafePath {
                        path: path.to_path_buf(),
                        detail: "has no resolvable parent".to_string(),
                    });
                };
                missing.push(name.to_os_string());
                match cursor.parent() {
                    Some(parent) => cursor = parent,
                    None => {
                        return Err(GitError::UnsafePath {
                            path: path.to_path_buf(),
                            detail: "has no resolvable parent".to_string(),
                        });
                    }
                }
            }
            Err(source) => {
                return Err(GitError::Io {
                    context: "canonicalise the worktree path",
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_relative_and_parent_traversal_paths() {
        assert!(normalize_absolute(Path::new("relative/wt")).is_err());
        assert!(normalize_absolute(Path::new("/tmp/../etc/wt")).is_err());
        assert_eq!(
            normalize_absolute(Path::new("/tmp/./wt")).unwrap(),
            PathBuf::from("/tmp/wt")
        );
    }

    #[test]
    fn policy_never_allows_tracked_changes() {
        // The classification unit is covered through real repositories in the
        // acceptance suite; this pins the closed default.
        assert_eq!(
            WorktreeDiscardPolicy::default(),
            WorktreeDiscardPolicy::RefuseDirty
        );
    }
}
