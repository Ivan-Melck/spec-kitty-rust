//! Canonical Git-topology discovery **without running Git**.
//!
//! Upstream `kernel.git_topology` shells out to `git rev-parse
//! --git-common-dir`, `--show-toplevel`, and `git config --local --get
//! core.worktree`. Kernel locks must never do that: a lock may be held across
//! the call, and spawning a child while a lock is held hands the lock's
//! lifetime to a process whose exit the holder cannot control. This module
//! reproduces the same answers from the filesystem, including the environment
//! overrides (`GIT_DIR`, `GIT_COMMON_DIR`, `GIT_OBJECT_DIRECTORY`,
//! `GIT_CEILING_DIRECTORIES`, `GIT_WORK_TREE`,
//! `GIT_DISCOVERY_ACROSS_FILESYSTEM`), linked worktrees, submodules, separate
//! git dirs, bare repositories, and `.git` pointer files.
//!
//! The contract is intentionally the **same** three probes upstream exposes:
//!
//! - [`git_common_dir`] — the shared repository git directory. A path inside
//!   that directory, or the bare repository root itself, is
//!   [`KernelError::NotAGitRepository`], matching upstream's interior check.
//! - [`git_toplevel`] — the working-tree root. A bare repository (or a git
//!   directory with no work tree) is [`KernelError::GitTopologyUnavailable`].
//! - [`git_configured_worktree`] — `core.worktree`, resolved against the
//!   common dir (the submodule / separate-git-dir case).
//!
//! `git_common_dir` follows Git's *lenient* discovery: an incomplete `.git`
//! directory is ignored and the walk continues, exactly as `git rev-parse`
//! does. The interim event-log lock uses [`strict_git_common_dir`], which
//! instead refuses a marker that claims HEAD but is not a valid repository.
//! That is a deliberate, documented safety divergence: it keeps a lock from
//! silently moving to a different authority, while the parity probes above
//! remain byte-equal with Git and upstream Python.
//!
//! No function here creates, modifies, or locks anything.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::KernelError;
use crate::fs_resolve::{map_resolve_failure, resolve_non_strict};

const GIT_DIR: &str = ".git";
const GITDIR_PREFIX: &str = "gitdir:";
const ENV_GIT_DIR: &str = "GIT_DIR";
const ENV_GIT_COMMON_DIR: &str = "GIT_COMMON_DIR";
const ENV_GIT_OBJECT_DIRECTORY: &str = "GIT_OBJECT_DIRECTORY";
const ENV_GIT_WORK_TREE: &str = "GIT_WORK_TREE";
const ENV_GIT_CEILING: &str = "GIT_CEILING_DIRECTORIES";
const ENV_DISCOVERY_ACROSS_FILESYSTEM: &str = "GIT_DISCOVERY_ACROSS_FILESYSTEM";
const MAX_MARKER_BYTES: u64 = 64 * 1024;
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// A discovered Git repository layout.
///
/// `common_dir` is the shared `.git` directory every worktree of the
/// repository uses for locks. `worktree` is absent for a bare repository or a
/// git directory entered directly (for example `/repo/.git`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitLayout {
    /// The repository's own git directory (may be a worktree gitdir).
    pub git_dir: PathBuf,
    /// The shared git common directory.
    pub common_dir: PathBuf,
    /// The working-tree root, when one exists.
    pub worktree: Option<PathBuf>,
    /// Whether the repository is bare (or was entered as a git directory).
    pub bare: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiscoveryMode {
    /// Match `git rev-parse`: ignore an incomplete `.git` marker.
    Lenient,
    /// Refuse a marker that claims to be a repository but is incomplete.
    Strict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum WorktreeSource {
    /// A `.git` directory or pointer inside this working-tree root.
    Marker(PathBuf),
    /// A direct git directory (bare repository or a path inside `.git`).
    Direct,
    /// `GIT_DIR` was supplied; Git uses the probe directory as the worktree.
    Environment,
}

/// Return the canonical `--git-common-dir` for `path`.
///
/// `path` may be a file or directory; a file is probed through its parent.
/// The result is absolute with symlinks expanded.
pub fn git_common_dir(path: &Path) -> Result<PathBuf, KernelError> {
    probe(path, |resolved, start| {
        let layout = discover(start, DiscoveryMode::Lenient)?.ok_or_else(|| {
            KernelError::NotAGitRepository {
                path: resolved.to_path_buf(),
            }
        })?;
        // Upstream rejects a path that IS the common dir or lives under it:
        // `.git` interiors are never a project root, and a bare repository's
        // own root is not a working checkout.
        if resolved.starts_with(&layout.common_dir) {
            return Err(KernelError::NotAGitRepository {
                path: resolved.to_path_buf(),
            });
        }
        Ok(layout.common_dir)
    })
}

/// Return the canonical `--show-toplevel` for `path`.
pub fn git_toplevel(path: &Path) -> Result<PathBuf, KernelError> {
    probe(path, |resolved, start| {
        let layout = discover(start, DiscoveryMode::Lenient)?.ok_or_else(|| {
            KernelError::NotAGitRepository {
                path: resolved.to_path_buf(),
            }
        })?;
        layout.worktree.filter(|_| !layout.bare).ok_or_else(|| {
            KernelError::GitTopologyUnavailable {
                path: resolved.to_path_buf(),
                detail: "this operation must be run in a work tree".to_owned(),
            }
        })
    })
}

/// Return `core.worktree` from `common_dir`'s local config, when set.
///
/// A relative value is resolved against `common_dir`, matching Git's own rule
/// for a submodule's absorbed git directory.
pub fn git_configured_worktree(common_dir: &Path) -> Result<Option<PathBuf>, KernelError> {
    let resolved = resolve(common_dir)?;
    let config = read_local_config(&resolved)?;
    let Some(value) = config.get("core.worktree") else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Err(KernelError::GitTopologyUnavailable {
            path: resolved,
            detail: "core.worktree is set but empty".to_owned(),
        });
    }
    let worktree = absolutize(&resolved, Path::new(value));
    let worktree = resolve(&worktree)?;
    Ok(Some(worktree))
}

/// Discover the repository layout for `path`, or `None` when no repository
/// (and no Git environment override) applies.
///
/// This is the additive shared surface for callers that need the git dir and
/// worktree as well as the common dir. It matches [`git_common_dir`]'s lenient
/// discovery.
pub fn discover_git_layout(path: &Path) -> Result<Option<GitLayout>, KernelError> {
    let resolved = resolve(path)?;
    let start = probe_directory(&resolved);
    discover(&start, DiscoveryMode::Lenient)
}

/// Strict common-dir discovery for durable writers.
///
/// Returns `Ok(None)` only when no repository marker exists anywhere on the
/// walk. A marker or environment override that exists but cannot be validated
/// is [`KernelError::LockUnavailable`], never a silent fallback to a sibling
/// lock or to a different repository. No marker with a HEAD entry is ignored.
pub(crate) fn strict_git_common_dir(start: &Path) -> Result<Option<PathBuf>, KernelError> {
    ensure_writer_git_environment(start)?;
    let resolved = resolve(start)?;
    let start = probe_directory(&resolved);
    match discover(&start, DiscoveryMode::Strict) {
        Ok(Some(layout)) => Ok(Some(layout.common_dir)),
        Ok(None) => Ok(None),
        Err(KernelError::LockUnavailable { .. }) => Err(KernelError::LockUnavailable {
            path: start.to_path_buf(),
            reason: "the Git topology is incomplete or unreadable; refusing to choose a different lock authority".to_owned(),
        }),
        Err(KernelError::GitTopologyUnavailable { detail, .. }) => Err(KernelError::LockUnavailable {
            path: start.to_path_buf(),
            reason: detail,
        }),
        Err(KernelError::NotAGitRepository { .. }) => Err(KernelError::LockUnavailable {
            path: start.to_path_buf(),
            reason: "the Git directory named by the environment is not a repository".to_owned(),
        }),
        Err(error) => Err(error),
    }
}

/// Refuse inherited repository selectors at writer authority boundaries.
///
/// Read-only parity probes intentionally follow Git's environment. The bounded
/// Git adapter instead selects its explicitly opened checkout. Until a shared
/// captured repository context owns both, writers must not derive a lock from
/// one selection and mutate through the other. This is an explicit fail-closed
/// restriction, not a claim of environment-override mutation parity.
pub fn ensure_writer_git_environment(path: &Path) -> Result<(), KernelError> {
    for variable in [
        ENV_GIT_DIR,
        ENV_GIT_COMMON_DIR,
        ENV_GIT_OBJECT_DIRECTORY,
        ENV_GIT_WORK_TREE,
        ENV_GIT_CEILING,
        ENV_DISCOVERY_ACROSS_FILESYSTEM,
    ] {
        if env_value(variable).is_some() {
            return Err(KernelError::LockUnavailable {
                path: path.to_path_buf(),
                reason: format!(
                    "inherited {variable} can disagree with the explicitly opened Git adapter; clear repository-selection variables before writing (lock authority divergence)"
                ),
            });
        }
    }
    Ok(())
}

fn probe<T>(
    path: &Path,
    operation: impl FnOnce(&Path, &Path) -> Result<T, KernelError>,
) -> Result<T, KernelError> {
    let resolved = resolve(path)?;
    let start = probe_directory(&resolved);
    operation(&resolved, &start)
}

fn resolve(path: &Path) -> Result<PathBuf, KernelError> {
    resolve_non_strict(path).map_err(|failure| map_resolve_failure(path, failure))
}

/// Upstream normalises a file input to its parent before probing.
fn probe_directory(resolved: &Path) -> PathBuf {
    if resolved.is_file() {
        resolved
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| resolved.to_path_buf())
    } else {
        resolved.to_path_buf()
    }
}

fn discover(start: &Path, mode: DiscoveryMode) -> Result<Option<GitLayout>, KernelError> {
    if let Some(git_dir) = env_value(ENV_GIT_DIR) {
        let git_dir = resolve_env_git_dir(start, Path::new(&git_dir), mode)?;
        if !is_git_directory(&git_dir, start)? {
            return Err(non_repository(mode, start));
        }
        return layout_from_git_dir(start, git_dir, WorktreeSource::Environment, mode).map(Some);
    }

    let ceilings = git_ceiling_dirs();
    let across_filesystems = env_flag(ENV_DISCOVERY_ACROSS_FILESYSTEM).unwrap_or(false);
    let mut directory = start.to_path_buf();

    loop {
        // Git checks the directory itself first: a bare repository, or a path
        // inside a git directory, resolves through any ancestor that is one.
        if is_git_directory(&directory, start)? {
            return layout_from_git_dir(start, directory, WorktreeSource::Direct, mode).map(Some);
        }

        let marker = directory.join(GIT_DIR);
        match fs::metadata(&marker) {
            Ok(metadata) if metadata.is_dir() => {
                if is_git_directory(&marker, start)? {
                    return layout_from_git_dir(
                        start,
                        marker,
                        WorktreeSource::Marker(directory.clone()),
                        mode,
                    )
                    .map(Some);
                }
                if mode == DiscoveryMode::Strict && marker_has_head(&marker) {
                    return Err(KernelError::LockUnavailable {
                        path: marker,
                        reason: "a .git directory claims HEAD but is not a complete repository"
                            .to_owned(),
                    });
                }
            }
            Ok(metadata) if metadata.is_file() => {
                let pointer = crate::read_bounded(&marker, MAX_MARKER_BYTES).map_err(|error| {
                    topology_failure(
                        mode,
                        marker.clone(),
                        format!("cannot read the .git pointer: {error}"),
                    )
                })?;
                let git_dir = parse_gitdir_pointer(&directory, &pointer)
                    .map_err(|error| topology_failure(mode, marker.clone(), error))?;
                if !is_git_directory(&git_dir, start)? {
                    // Git leaks this as "not a git repository: <target>",
                    // which upstream classifies as a non-repository rather
                    // than an unavailable probe.
                    return Err(if mode == DiscoveryMode::Strict {
                        KernelError::LockUnavailable {
                            path: marker,
                            reason: "the .git pointer does not name a complete repository"
                                .to_owned(),
                        }
                    } else {
                        KernelError::NotAGitRepository {
                            path: start.to_path_buf(),
                        }
                    });
                }
                return layout_from_git_dir(
                    start,
                    git_dir,
                    WorktreeSource::Marker(directory.clone()),
                    mode,
                )
                .map(Some);
            }
            Ok(_) => {
                return Err(topology_failure(
                    mode,
                    marker,
                    "the .git entry is neither a directory nor a regular file".to_owned(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(topology_failure(
                    mode,
                    marker,
                    format!("cannot inspect the .git entry: {error}"),
                ));
            }
        }

        let Some(parent) = directory.parent() else {
            break;
        };
        if ceilings.iter().any(|ceiling| ceiling == parent) {
            break;
        }
        if !across_filesystems && crosses_filesystem(&directory, parent) {
            break;
        }
        directory = parent.to_path_buf();
    }

    Ok(None)
}

fn non_repository(mode: DiscoveryMode, path: &Path) -> KernelError {
    if mode == DiscoveryMode::Strict {
        KernelError::LockUnavailable {
            path: path.to_path_buf(),
            reason: "the Git environment does not name a repository".to_owned(),
        }
    } else {
        KernelError::NotAGitRepository {
            path: path.to_path_buf(),
        }
    }
}

fn resolve_env_git_dir(
    start: &Path,
    raw: &Path,
    mode: DiscoveryMode,
) -> Result<PathBuf, KernelError> {
    let candidate = absolutize(start, raw);
    let metadata = match fs::metadata(&candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(non_repository(mode, start));
        }
        Err(source) => {
            return Err(KernelError::io("inspect GIT_DIR", &candidate, source));
        }
    };
    if metadata.is_file() {
        let bytes = crate::read_bounded(&candidate, MAX_MARKER_BYTES).map_err(|error| {
            topology_failure(
                mode,
                candidate.clone(),
                format!("cannot read GIT_DIR: {error}"),
            )
        })?;
        let parent = candidate
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| start.to_path_buf());
        let target = parse_gitdir_pointer(&parent, &bytes)
            .map_err(|detail| topology_failure(mode, candidate.clone(), detail))?;
        return resolve(&target);
    }
    resolve(&candidate)
}

fn topology_failure(mode: DiscoveryMode, path: PathBuf, detail: String) -> KernelError {
    if mode == DiscoveryMode::Strict {
        KernelError::LockUnavailable {
            path,
            reason: detail,
        }
    } else {
        KernelError::GitTopologyUnavailable { path, detail }
    }
}

fn marker_has_head(marker: &Path) -> bool {
    !matches!(
        fs::symlink_metadata(marker.join("HEAD")),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    )
}

fn parse_gitdir_pointer(candidate: &Path, pointer: &[u8]) -> Result<PathBuf, String> {
    let text = std::str::from_utf8(pointer)
        .map_err(|error| format!("the .git pointer is not UTF-8: {error}"))?;
    let remainder = text
        .trim()
        .strip_prefix(GITDIR_PREFIX)
        .ok_or_else(|| "the .git file is not a gitdir pointer".to_owned())?
        .trim();
    if remainder.is_empty() {
        return Err("the .git pointer names no gitdir".to_owned());
    }
    let git_dir = PathBuf::from(remainder);
    Ok(absolutize(candidate, &git_dir))
}

fn layout_from_git_dir(
    start: &Path,
    git_dir: PathBuf,
    source: WorktreeSource,
    mode: DiscoveryMode,
) -> Result<GitLayout, KernelError> {
    let git_dir = resolve(&git_dir)?;
    let common_dir = common_dir_for(start, &git_dir, mode)?;
    let common_config = read_local_config(&common_dir)?;
    let configured_bare = common_config
        .get("core.bare")
        .map(parse_git_bool)
        .transpose()
        .map_err(|detail| topology_failure(mode, common_dir.clone(), detail))?;

    let (worktree, bare) = match source {
        // Git reports the directory that owns the `.git` marker, even when a
        // GIT_COMMON_DIR override points at a bare common directory.
        WorktreeSource::Marker(worktree) => (Some(worktree), false),
        // Entered directly (a bare repository, or a path inside a gitdir):
        // an explicit GIT_WORK_TREE still wins; otherwise a non-bare gitdir
        // with `core.worktree` (an absorbed submodule) names its worktree;
        // everything else has none.
        WorktreeSource::Direct => {
            let git_dir_config = read_local_config(&git_dir)?;
            let git_dir_bare = git_dir_config
                .get("core.bare")
                .map(parse_git_bool)
                .transpose()
                .map_err(|detail| topology_failure(mode, git_dir.clone(), detail))?
                .or(configured_bare);
            if let Some(worktree) = env_value(ENV_GIT_WORK_TREE) {
                (
                    Some(resolve(&absolutize(start, Path::new(&worktree)))?),
                    false,
                )
            } else if git_dir_bare == Some(true) {
                (None, true)
            } else if let Some(value) = git_dir_config
                .get("core.worktree")
                .filter(|value| !value.trim().is_empty())
            {
                (
                    Some(resolve(&absolutize(&git_dir, Path::new(value)))?),
                    false,
                )
            } else {
                (None, true)
            }
        }
        WorktreeSource::Environment => {
            // `GIT_DIR` names *this* gitdir; `core.bare` comes from its own
            // config, not from an overridden common dir.
            let git_dir_config = read_local_config(&git_dir)?;
            let git_dir_bare = git_dir_config
                .get("core.bare")
                .map(parse_git_bool)
                .transpose()
                .map_err(|detail| topology_failure(mode, git_dir.clone(), detail))?
                .or(configured_bare);
            let explicit_worktree = env_value(ENV_GIT_WORK_TREE)
                .map(|value| absolutize(start, Path::new(&value)))
                .or_else(|| {
                    common_config
                        .get("core.worktree")
                        .filter(|value| !value.trim().is_empty())
                        .map(|value| absolutize(&git_dir, Path::new(value)))
                });
            match explicit_worktree {
                Some(worktree) => (Some(resolve(&worktree)?), false),
                None if git_dir_bare == Some(true) => (None, true),
                None => (Some(start.to_path_buf()), false),
            }
        }
    };

    let bare = bare || worktree.is_none();
    Ok(GitLayout {
        git_dir,
        common_dir,
        worktree,
        bare,
    })
}

fn common_dir_for(
    start: &Path,
    git_dir: &Path,
    mode: DiscoveryMode,
) -> Result<PathBuf, KernelError> {
    if let Some(common) = env_value(ENV_GIT_COMMON_DIR) {
        let common = absolutize(start, Path::new(&common));
        let common = resolve(&common)?;
        if !is_git_directory(&common, start)? {
            return Err(non_repository(mode, &common));
        }
        return Ok(common);
    }

    let commondir = git_dir.join("commondir");
    match fs::symlink_metadata(&commondir) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
            let bytes = crate::read_bounded(&commondir, MAX_MARKER_BYTES).map_err(|error| {
                topology_failure(
                    mode,
                    commondir.clone(),
                    format!("cannot read commondir: {error}"),
                )
            })?;
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                topology_failure(
                    mode,
                    commondir.clone(),
                    format!("commondir is not UTF-8: {error}"),
                )
            })?;
            let value = text.trim();
            if value.is_empty() {
                return Err(topology_failure(
                    mode,
                    commondir,
                    "commondir is empty".to_owned(),
                ));
            }
            let common = absolutize(git_dir, Path::new(value));
            let common = resolve(&common)?;
            if !is_git_directory(&common, start)? {
                return Err(topology_failure(
                    mode,
                    common,
                    "the commondir target is not a complete repository".to_owned(),
                ));
            }
            Ok(common)
        }
        Ok(_) => Err(topology_failure(
            mode,
            commondir,
            "commondir is not a regular file".to_owned(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(git_dir.to_path_buf()),
        Err(error) => Err(topology_failure(
            mode,
            commondir,
            format!("cannot inspect commondir: {error}"),
        )),
    }
}

fn is_git_directory(directory: &Path, base: &Path) -> Result<bool, KernelError> {
    let metadata = match fs::metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(KernelError::io("inspect Git directory", directory, source)),
    };
    if !metadata.is_dir() {
        return Ok(false);
    }

    let objects_override =
        env_value(ENV_GIT_OBJECT_DIRECTORY).map(|value| absolutize(base, Path::new(&value)));
    let has_objects = match &objects_override {
        Some(path) => path.is_dir(),
        None => directory.join("objects").is_dir(),
    };
    let has_refs = directory.join("refs").is_dir();
    let has_head = valid_head(&directory.join("HEAD"))?;
    if has_objects && has_refs && has_head {
        return Ok(true);
    }

    // A linked worktree's gitdir keeps HEAD but shares objects/refs through
    // `commondir`.
    let commondir = directory.join("commondir");
    if commondir.is_file() {
        let bytes = crate::read_bounded(&commondir, MAX_MARKER_BYTES)?;
        let text =
            std::str::from_utf8(&bytes).map_err(|error| KernelError::GitTopologyUnavailable {
                path: commondir.clone(),
                detail: format!("commondir is not UTF-8: {error}"),
            })?;
        let value = text.trim();
        if value.is_empty() {
            return Ok(false);
        }
        let common = resolve(&absolutize(directory, Path::new(value)))?;
        let common_head = valid_head(&common.join("HEAD"))?;
        let common_objects = match &objects_override {
            Some(path) => path.is_dir(),
            None => common.join("objects").is_dir(),
        };
        return Ok((has_head || common_head) && common_objects && common.join("refs").is_dir());
    }

    Ok(false)
}

fn valid_head(path: &Path) -> Result<bool, KernelError> {
    let bytes = match crate::read_bounded(path, MAX_MARKER_BYTES) {
        Ok(bytes) => bytes,
        Err(KernelError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let head = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    let detached = matches!(head.len(), 40 | 64) && head.iter().all(u8::is_ascii_hexdigit);
    let symbolic = head.strip_prefix(b"ref: ").is_some_and(|reference| {
        reference.starts_with(b"refs/")
            && reference.len() > 5
            && !reference
                .iter()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    });
    Ok(detached || symbolic)
}

fn env_value(name: &str) -> Option<OsString> {
    match std::env::var_os(name) {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn env_flag(name: &str) -> Option<bool> {
    env_value(name).map(|value| parse_git_bool(&value.to_string_lossy()).unwrap_or(true))
}

fn parse_git_bool(value: &str) -> Result<bool, String> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "" | "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        other => Err(format!("invalid Git boolean value {other:?}")),
    }
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn crosses_filesystem(directory: &Path, parent: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(directory), fs::metadata(parent)) {
            (Ok(directory), Ok(parent)) => directory.dev() != parent.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, parent);
        false
    }
}

fn git_ceiling_dirs() -> Vec<PathBuf> {
    let Some(raw) = std::env::var_os(ENV_GIT_CEILING) else {
        return Vec::new();
    };
    std::env::split_paths(&raw)
        .filter(|path| path.is_absolute())
        .filter_map(|path| resolve(&path).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Local config (`core.worktree`, `core.bare`) without running Git
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LocalConfig {
    values: Vec<(String, String)>,
}

impl LocalConfig {
    fn get(&self, key: &str) -> Option<&str> {
        // Git's `--get` returns the last value for a repeated key.
        self.values
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    }
}

fn read_local_config(common_dir: &Path) -> Result<LocalConfig, KernelError> {
    let path = common_dir.join("config");
    let bytes = match crate::read_bounded(&path, MAX_CONFIG_BYTES) {
        Ok(bytes) => bytes,
        Err(KernelError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(LocalConfig::default());
        }
        Err(KernelError::TooLarge { .. }) => {
            return Err(KernelError::GitTopologyUnavailable {
                path,
                detail: "the repository config is too large".to_owned(),
            });
        }
        Err(error) => return Err(error),
    };
    let text = String::from_utf8_lossy(&bytes);
    parse_local_config(&text).map_err(|detail| KernelError::GitTopologyUnavailable {
        path: path.clone(),
        detail,
    })
}

fn parse_local_config(text: &str) -> Result<LocalConfig, String> {
    let mut config = LocalConfig::default();
    let mut section = String::new();
    let mut lines = text.lines().peekable();
    while let Some(raw) = lines.next() {
        let mut logical = raw.trim_end().to_owned();
        while logical.ends_with('\\') {
            logical.pop();
            let Some(next) = lines.next() else {
                break;
            };
            logical.push_str(next.trim_end());
        }
        let line = logical.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let Some(end) = rest.find(']') else {
                return Err(format!("unterminated section header: {line:?}"));
            };
            let header = rest[..end].trim();
            let name_end = header
                .find(|character: char| character.is_whitespace() || character == '"')
                .unwrap_or(header.len());
            section = header[..name_end].to_ascii_lowercase();
            // Only a section without a subsection contributes `core.<key>`.
            if name_end != header.trim_end().len() {
                section.push_str(".\u{0}subsection");
            }
            continue;
        }
        let (key, value) = split_config_key(line)?;
        if section.is_empty() || section.contains('\u{0}') {
            continue;
        }
        config
            .values
            .push((format!("{section}.{}", key.to_ascii_lowercase()), value));
    }
    Ok(config)
}

fn split_config_key(line: &str) -> Result<(&str, String), String> {
    let key_end = line
        .find(|character: char| character.is_whitespace() || character == '=')
        .unwrap_or(line.len());
    let key = &line[..key_end];
    let remainder = line[key_end..].trim_start();
    let value = remainder.strip_prefix('=').map_or("true", |value| value);
    Ok((key, parse_config_value(value)?))
}

fn parse_config_value(value: &str) -> Result<String, String> {
    let mut output = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.trim_start().chars() {
        if escaped {
            output.push(match character {
                'n' => '\n',
                't' => '\t',
                'b' => '\u{8}',
                '\\' => '\\',
                '"' => '"',
                other => other,
            });
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => break,
            character => output.push(character),
        }
    }
    Ok(output.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parser_reads_sections_case_insensitively() {
        let config = parse_local_config(
            "[Core]\n\tworktree = ../../../sub\n[core \"x\"]\n worktree = ignored\n",
        )
        .unwrap();
        assert_eq!(config.get("core.worktree"), Some("../../../sub"));
        assert_eq!(config.get("core.bare"), None);
    }

    #[test]
    fn config_parser_strips_comments_only_outside_quotes() {
        let config = parse_local_config("[core]\n\tworktree = \"a#b\" # trailing\n").unwrap();
        assert_eq!(config.get("core.worktree"), Some("a#b"));
    }

    #[test]
    fn git_booleans_are_parsed_like_git() {
        assert_eq!(parse_git_bool("true"), Ok(true));
        assert_eq!(parse_git_bool("0"), Ok(false));
        assert!(parse_git_bool("maybe").is_err());
    }

    #[test]
    fn a_path_component_helper_preserves_relative_components() {
        assert_eq!(
            absolutize(Path::new("/a"), Path::new("b")),
            Path::new("/a/b")
        );
        assert_eq!(
            absolutize(Path::new("/a"), Path::new("/b")),
            Path::new("/b")
        );
        assert_eq!(
            std::path::Component::ParentDir,
            Path::new("..").components().next().unwrap()
        );
    }
}
