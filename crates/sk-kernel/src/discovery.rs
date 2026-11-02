use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::KernelError;
use crate::fs_resolve::{map_resolve_failure, resolve_non_strict};

const KITTIFY_DIR: &str = ".kittify";
const GIT_DIR: &str = ".git";
const GITDIR_PREFIX: &str = "gitdir:";
const ENV_PROJECT_ROOT: &str = "SPECIFY_REPO_ROOT";
const ENV_GIT_CEILING: &str = "GIT_CEILING_DIRECTORIES";

/// Discover the main Spec Kitty project root from `start`.
///
/// Resolution mirrors the bounded upstream `locate_project_root` semantics:
///
/// 1. `SPECIFY_REPO_ROOT`, when it names an existing directory, is
///    authoritative even without a `.kittify` marker (Tier 1).
/// 2. Otherwise walk from `start` to the filesystem root. A `.git` file with
///    the `<main>/.git/worktrees/<name>` topology re-anchors a linked worktree
///    at its primary checkout when that checkout carries `.kittify`.
/// 3. A `.git` directory is a checkout boundary when it carries `.kittify` or
///    a `HEAD` file; an empty `.git` directory stops the walk rather than
///    climbing into an enclosing checkout.
/// 4. A real `.kittify` directory is the fallback marker for non-Git projects.
/// 5. Absolute `GIT_CEILING_DIRECTORIES` entries bound the walk. The ceiling
///    directory itself is examined; nothing above it is.
///
/// The returned path is absolute with symlinks expanded.
pub fn discover_project_root(start: &Path) -> Result<PathBuf, KernelError> {
    if let Some(root) = project_root_override() {
        return Ok(root);
    }

    let resolved_start =
        resolve_non_strict(start).map_err(|failure| map_resolve_failure(start, failure))?;
    let ceilings = git_ceiling_dirs();

    for candidate in resolved_start.ancestors() {
        match candidate_status(candidate)? {
            CandidateStatus::Found(root) => return Ok(root),
            CandidateStatus::Stop => break,
            CandidateStatus::Continue => {}
        }

        if ceilings.iter().any(|ceiling| ceiling == candidate) {
            break;
        }
    }

    Err(KernelError::ProjectRootNotFound {
        start: start.to_path_buf(),
    })
}

enum CandidateStatus {
    Found(PathBuf),
    Continue,
    Stop,
}

fn candidate_status(candidate: &Path) -> Result<CandidateStatus, KernelError> {
    let git_marker = candidate.join(GIT_DIR);
    match fs::metadata(&git_marker) {
        Ok(metadata) if metadata.is_file() => {
            if let Some(primary) = linked_worktree_primary(candidate)? {
                if primary.join(KITTIFY_DIR).is_dir() {
                    let resolved = resolve_non_strict(&primary)
                        .map_err(|failure| map_resolve_failure(&primary, failure))?;
                    return Ok(CandidateStatus::Found(resolved));
                }
            }
        }
        Ok(metadata) if metadata.is_dir() => {
            if candidate.join(KITTIFY_DIR).is_dir() || git_marker.join("HEAD").is_file() {
                return Ok(CandidateStatus::Found(candidate.to_path_buf()));
            }
            return Ok(CandidateStatus::Stop);
        }
        _ => {}
    }

    if candidate.join(KITTIFY_DIR).is_dir() {
        return Ok(CandidateStatus::Found(candidate.to_path_buf()));
    }

    Ok(CandidateStatus::Continue)
}

/// Return the primary checkout named by a linked-worktree `.git` pointer.
///
/// A pointer only counts when its gitdir has the
/// `<main>/.git/worktrees/<name>` (or bare `<repo>.git/worktrees/<name>`)
/// topology. Submodule and separate-git-dir pointers are not worktrees and
/// must not re-anchor the walk.
fn linked_worktree_primary(candidate: &Path) -> Result<Option<PathBuf>, KernelError> {
    let git_marker = candidate.join(GIT_DIR);
    // A pointer is tiny, untrusted input, including on the override path.
    // Using the shared opener also refuses FIFOs/devices without blocking.
    let bytes = match crate::read_bounded(&git_marker, 64 * 1024) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let content = String::from_utf8_lossy(&bytes);
    let Some(remainder) = content.trim().strip_prefix(GITDIR_PREFIX) else {
        return Ok(None);
    };
    let gitdir = remainder.trim();
    if gitdir.is_empty() {
        return Ok(None);
    }

    let gitdir = PathBuf::from(gitdir);
    if !is_worktree_gitdir(&gitdir) {
        return Ok(None);
    }

    Ok(gitdir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf))
}

fn is_worktree_gitdir(gitdir: &Path) -> bool {
    let Some(worktrees_dir) = gitdir.parent() else {
        return false;
    };
    let Some(git_dir) = worktrees_dir.parent() else {
        return false;
    };

    worktrees_dir
        .file_name()
        .is_some_and(|name| name == "worktrees")
        && git_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(GIT_DIR))
}

fn project_root_override() -> Option<PathBuf> {
    let raw = env::var_os(ENV_PROJECT_ROOT)?;
    if raw.is_empty() {
        return None;
    }

    let path = PathBuf::from(raw);
    let resolved = resolve_non_strict(&path).ok()?;
    if resolved.is_dir() {
        Some(primary_checkout(&resolved))
    } else {
        None
    }
}

/// Follow a linked-worktree pointer to the primary checkout.
fn primary_checkout(current: &Path) -> PathBuf {
    if let Some(primary) = linked_worktree_primary(current).ok().flatten() {
        return resolve_non_strict(&primary).unwrap_or(primary);
    }

    resolve_non_strict(current).unwrap_or_else(|_| current.to_path_buf())
}

fn git_ceiling_dirs() -> Vec<PathBuf> {
    let Some(raw) = env::var_os(ENV_GIT_CEILING) else {
        return Vec::new();
    };

    env::split_paths(&raw)
        .filter(|path| path.is_absolute())
        .filter_map(|path| resolve_non_strict(&path).ok())
        .collect()
}
