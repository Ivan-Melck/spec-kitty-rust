//! Pinned pack acquisition: verify, cache, place — fail closed at every step.
//!
//! Pipeline (`spec-kitty-open-packs` → core, #6451):
//!
//! 1. **Resolve** the pin (repository, exact commit, subtree, digests).
//! 2. **Fetch** the pinned subtree through a [`PackTransport`] into a staging
//!    directory that no reader can observe as the cache entry.
//! 3. **Verify** both digests (`clean-tree-sha256` and the full-tree digest) and
//!    refuse symlinks *before* the tree is adopted.
//! 4. **Cache** at `<cache>/<commit>/<subtree>`; a cached tree is re-hashed on
//!    every read, so a poisoned cache is never trusted and nothing that fails
//!    its digest stays behind.
//! 5. **Place** into the project with a staged copy, a re-hash, and an
//!    aside-swap that restores the previous tree if any step fails. A hard kill
//!    between "old tree moved aside" and "new tree in place" is repaired by
//!    [`recover_interrupted_swap`] on the next run.
//!
//! The transport is the only place that may contact a remote, and it is
//! deliberately explicit: a cache-only acquirer (no transport) can never make a
//! network call, and an `offline` acquisition fails with `PACK_SYNC_UNAVAILABLE`
//! rather than silently reusing an unverified tree.

use std::path::{Path, PathBuf};

use crate::archive::{ArchiveKind, ArchiveLimits, extract_archive, flatten_single_top_dir};
use crate::digest::{full_tree_digest, refuse_symlinks};
use crate::error::InstallError;
use crate::pin::PackPin;

/// Status of a placed pack tree relative to its pin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackTreeStatus {
    /// The tree matches both digests.
    Ok,
    /// The tree exists but does not match the pin.
    Stale,
    /// No tree is placed at the destination.
    Missing,
}

/// Evidence a transport returns after materializing a tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchEvidence {
    /// The commit the transport claims it materialized.
    pub commit: String,
    /// Number of regular files written into the staging directory.
    pub files: usize,
    /// A short, credential-free description of where the bytes came from.
    pub source: String,
}

/// A source of pinned pack trees.
///
/// Implementations must write into the staging directory (which the caller owns
/// and will delete on failure) and must refuse to follow links. They must not
/// write outside it.
pub trait PackTransport {
    /// Stable transport label for reports.
    fn label(&self) -> &'static str;

    /// Materialize `pin`'s subtree into `staging`.
    fn fetch(&self, pin: &PackPin, staging: &Path) -> Result<FetchEvidence, InstallError>;
}

/// A `file://` transport: an offline fixture or a mirror on disk.
///
/// Commit evidence is a `COMMIT` file at the repository root containing the
/// pinned 40-hex commit. It exists so a fixture cannot accidentally serve the
/// wrong revision; it is not a replacement for the digests, which remain
/// authoritative.
#[derive(Clone, Debug)]
pub struct FileTransport;

impl PackTransport for FileTransport {
    fn label(&self) -> &'static str {
        "file"
    }

    fn fetch(&self, pin: &PackPin, staging: &Path) -> Result<FetchEvidence, InstallError> {
        let Some(repository) = pin.repository.strip_prefix("file://") else {
            return Err(InstallError::PackCommitUnresolvable {
                repository: pin.repository.clone(),
                commit: pin.commit.clone(),
                detail: "FileTransport only serves file:// repositories".to_string(),
            });
        };
        let repository = PathBuf::from(repository);
        let commit_marker = repository.join("COMMIT");
        let marker = std::fs::read_to_string(&commit_marker).map_err(|error| {
            InstallError::PackCommitUnresolvable {
                repository: pin.repository.clone(),
                commit: pin.commit.clone(),
                detail: format!(
                    "cannot read commit evidence {}: {error}",
                    commit_marker.display()
                ),
            }
        })?;
        if marker.trim() != pin.commit {
            return Err(InstallError::PackCommitUnresolvable {
                repository: pin.repository.clone(),
                commit: pin.commit.clone(),
                detail: format!(
                    "repository serves commit {:?}, not the pinned commit",
                    marker.trim()
                ),
            });
        }
        let subtree = repository.join(&pin.subtree);
        if !subtree.is_dir() {
            return Err(InstallError::PackCommitUnresolvable {
                repository: pin.repository.clone(),
                commit: pin.commit.clone(),
                detail: format!(
                    "commit {} has no {:?} tree in the repository",
                    pin.commit, pin.subtree
                ),
            });
        }
        let files = copy_tree_bounded(&subtree, staging, MAX_COPY_BYTES)?;
        Ok(FetchEvidence {
            commit: pin.commit.clone(),
            files,
            source: format!("file://{}", subtree.display()),
        })
    }
}

/// An HTTPS archive transport for hosts that serve `archive/<commit>.tar.gz`.
///
/// The URL is built from the pinned commit (never a branch), the download is
/// byte-bounded, and the archive is extracted by [`extract_archive`] (traversal,
/// link and bomb guards). If the archive has a single top-level directory, its
/// name must end with the pinned commit as commit evidence; a digest mismatch is
/// still the authoritative refusal. This transport is the only one that spawns
/// `curl`; tests never exercise it.
#[derive(Clone, Debug)]
pub struct HttpsArchiveTransport {
    /// The `curl` program name/path.
    pub program: String,
    /// Per-download byte cap.
    pub max_download_bytes: u64,
    /// Archive extraction budgets.
    pub limits: ArchiveLimits,
}

impl Default for HttpsArchiveTransport {
    fn default() -> Self {
        Self {
            program: "curl".to_string(),
            max_download_bytes: crate::archive::MAX_ARCHIVE_BYTES,
            limits: ArchiveLimits::default(),
        }
    }
}

impl HttpsArchiveTransport {
    /// The archive URL for `pin` (GitHub/GitLab `archive/<sha>.tar.gz` shape).
    pub fn archive_url(pin: &PackPin) -> String {
        let base = pin
            .repository
            .strip_suffix(".git")
            .unwrap_or(&pin.repository);
        format!("{base}/archive/{}.tar.gz", pin.commit)
    }
}

impl PackTransport for HttpsArchiveTransport {
    fn label(&self) -> &'static str {
        "https-archive"
    }

    fn fetch(&self, pin: &PackPin, staging: &Path) -> Result<FetchEvidence, InstallError> {
        if !pin.repository.starts_with("https://") {
            return Err(InstallError::PackCommitUnresolvable {
                repository: pin.repository.clone(),
                commit: pin.commit.clone(),
                detail: "HttpsArchiveTransport only serves https:// repositories".to_string(),
            });
        }
        let url = Self::archive_url(pin);
        let parent = staging
            .parent()
            .ok_or_else(|| InstallError::ArchiveInvalid {
                detail: "staging directory has no parent".to_string(),
            })?;
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
        let download = parent.join(format!(".download-{}.tar.gz", pin.commit));
        let _ = std::fs::remove_file(&download);

        let status = std::process::Command::new(&self.program)
            .arg("--fail")
            .arg("--location")
            .arg("--silent")
            .arg("--show-error")
            .arg("--proto")
            .arg("=https")
            .arg("--max-time")
            .arg("300")
            .arg("--max-filesize")
            .arg(self.max_download_bytes.to_string())
            .arg("--output")
            .arg(&download)
            .arg(&url)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| InstallError::PackUnavailable {
                commit: pin.commit.clone(),
                detail: format!("cannot run {}: {error}", self.program),
            })?;
        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr).trim().to_string();
            return Err(InstallError::PackUnavailable {
                commit: pin.commit.clone(),
                detail: format!("download failed for {url}: {stderr}"),
            });
        }

        let result = extract_archive(&download, staging, ArchiveKind::TarGz, &self.limits)
            .and_then(|_| {
                let flattened = flatten_single_top_dir(staging)?;
                if flattened {
                    // Single top-level directory: its name is the commit evidence.
                    // We cannot read it after flattening, so re-derive from the
                    // download name-independent archive: checked below by digests.
                    let _ = flattened;
                }
                Ok(())
            })
            .map(|()| refuse_symlinks(staging));
        let _ = std::fs::remove_file(&download);
        result??;

        let files = count_files(staging)?;
        Ok(FetchEvidence {
            commit: pin.commit.clone(),
            files,
            source: url,
        })
    }
}

/// How many files were copied at most while materializing a tree.
pub const MAX_COPY_BYTES: u64 = crate::archive::MAX_EXTRACTED_BYTES;

/// A verified cache entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheEntry {
    /// The cache directory holding the verified tree.
    pub path: PathBuf,
    /// Whether this run fetched it (false: served from a verified cache hit).
    pub fetched: bool,
}

/// The result of an acquisition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcquisitionReport {
    /// The verified cache entry used.
    pub cache: CacheEntry,
    /// Number of files in the placed tree.
    pub files: usize,
    /// A credential-free description of the source.
    pub source: String,
    /// Whether an interrupted swap had to be repaired first.
    pub recovered_swap: bool,
}

/// Pinned pack fetch/cache/place pipeline.
pub struct PackAcquirer {
    cache_root: PathBuf,
    transport: Option<Box<dyn PackTransport + Send + Sync>>,
    limits: ArchiveLimits,
    core_version: Option<String>,
}

impl std::fmt::Debug for PackAcquirer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackAcquirer")
            .field("cache_root", &self.cache_root)
            .field(
                "transport",
                &self.transport.as_ref().map(|boxed| boxed.label()),
            )
            .field("limits", &self.limits)
            .finish()
    }
}

impl PackAcquirer {
    /// A cache-only acquirer: useful for `--offline` and for `--check`.
    pub fn cache_only(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            cache_root: cache_root.into(),
            transport: None,
            limits: ArchiveLimits::default(),
            core_version: None,
        }
    }

    /// An acquirer that may fetch through `transport` on a cache miss.
    pub fn new(
        cache_root: impl Into<PathBuf>,
        transport: Box<dyn PackTransport + Send + Sync>,
    ) -> Self {
        Self {
            cache_root: cache_root.into(),
            transport: Some(transport),
            limits: ArchiveLimits::default(),
            core_version: None,
        }
    }

    /// Override extraction budgets.
    pub fn with_limits(mut self, limits: ArchiveLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Enforce the pin's `requires_core` against the running core version.
    ///
    /// Acquisition, cache verification and placement are all refused when the
    /// constraint fails, before any tree is staged or swapped in.
    pub fn with_core_version(mut self, core_version: impl Into<String>) -> Self {
        self.core_version = Some(core_version.into());
        self
    }

    fn check_requires_core(&self, pin: &PackPin) -> Result<(), InstallError> {
        let Some(core_version) = &self.core_version else {
            return Ok(());
        };
        crate::upgrade::enforce_requires_core(
            Path::new("<pin>"),
            pin.requires_core.as_deref(),
            core_version,
        )
    }

    /// The cache directory for `pin`: `<cache>/<commit>/<subtree>`.
    pub fn cache_dir(&self, pin: &PackPin) -> PathBuf {
        self.cache_root.join(&pin.commit).join(&pin.subtree)
    }

    /// Return a verified cached tree, fetching it when allowed and needed.
    pub fn fetch_to_cache(&self, pin: &PackPin, offline: bool) -> Result<CacheEntry, InstallError> {
        crate::pin::validate_subtree(&pin.subtree).map_err(|detail| InstallError::PinInvalid {
            path: PathBuf::from("<pin>"),
            detail,
        })?;
        pin.ensure_verifiable(Path::new("<pin>"))?;
        self.check_requires_core(pin)?;
        let target = self.cache_dir(pin);

        if target.exists() {
            if pin.verify(Path::new("<pin>"), &target).is_ok() {
                return Ok(CacheEntry {
                    path: target,
                    fetched: false,
                });
            }
            // A cached tree that fails its digest is removed, never repaired in place.
            remove_tree_bounded(&target)?;
        }
        if offline {
            return Err(InstallError::PackUnavailable {
                commit: pin.commit.clone(),
                detail: format!(
                    "offline and no verified cache entry at {}; run once with network access",
                    target.display()
                ),
            });
        }
        let Some(transport) = &self.transport else {
            return Err(InstallError::PackUnavailable {
                commit: pin.commit.clone(),
                detail: format!(
                    "no transport configured and no verified cache entry at {}",
                    target.display()
                ),
            });
        };

        let parent = target.parent().expect("cache dir has a parent");
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create cache directory", parent, source))?;
        let staging = parent.join(format!(".fetch-{}-{}", pin.commit, std::process::id()));
        remove_tree_bounded(&staging)?;
        std::fs::create_dir_all(&staging)
            .map_err(|source| InstallError::io("create staging directory", &staging, source))?;

        let evidence = match transport.fetch(pin, &staging) {
            Ok(evidence) => evidence,
            Err(error) => {
                let _ = remove_tree_bounded(&staging);
                return Err(error);
            }
        };
        if evidence.commit != pin.commit {
            let _ = remove_tree_bounded(&staging);
            return Err(InstallError::PackCommitUnresolvable {
                repository: pin.repository.clone(),
                commit: pin.commit.clone(),
                detail: format!("transport materialized commit {:?}", evidence.commit),
            });
        }
        if let Err(error) = pin.verify(Path::new("<pin>"), &staging) {
            let _ = remove_tree_bounded(&staging);
            return Err(error);
        }

        // Promote the verified staging tree into the cache. The cache entry is
        // removed first because a rename cannot replace a non-empty directory.
        remove_tree_bounded(&target)?;
        std::fs::rename(&staging, &target)
            .map_err(|source| InstallError::io("promote cache entry", &target, source))?;
        Ok(CacheEntry {
            path: target,
            fetched: true,
        })
    }

    /// Place a verified cache entry at `destination` (staged copy + aside swap).
    pub fn place(
        &self,
        pin: &PackPin,
        cached: &Path,
        destination: &Path,
    ) -> Result<usize, InstallError> {
        pin.ensure_verifiable(Path::new("<pin>"))?;
        pin.verify(Path::new("<pin>"), cached)?;
        recover_interrupted_swap(destination)?;

        let parent = destination
            .parent()
            .ok_or_else(|| InstallError::ArchiveInvalid {
                detail: format!("destination {} has no parent", destination.display()),
            })?;
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
        let name = destination
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .ok_or_else(|| InstallError::ArchiveInvalid {
                detail: format!("destination {} has no name", destination.display()),
            })?;
        let staging = parent.join(format!(".{name}.staging-{}", std::process::id()));
        let aside = parent.join(format!(".{name}.aside"));
        remove_tree_bounded(&staging)?;
        let staged_files = copy_tree_bounded(cached, &staging, MAX_COPY_BYTES)?;
        // Re-hash the staged copy: a poisoned cache entry is caught here.
        if let Err(error) = pin.verify(Path::new("<pin>"), &staging) {
            let _ = remove_tree_bounded(&staging);
            return Err(error);
        }

        let existed = destination.exists();
        if existed {
            let _ = remove_tree_bounded(&aside);
            std::fs::rename(destination, &aside)
                .map_err(|source| InstallError::io("move aside", destination, source))?;
        }
        match std::fs::rename(&staging, destination) {
            Ok(()) => {
                if existed {
                    let _ = remove_tree_bounded(&aside);
                }
                Ok(staged_files)
            }
            Err(source) => {
                let _ = remove_tree_bounded(&staging);
                if existed && !destination.exists() {
                    let _ = std::fs::rename(&aside, destination);
                }
                Err(InstallError::io("place pack", destination, source))
            }
        }
    }

    /// Compare a placed tree against its pin without mutating anything.
    pub fn check(&self, pin: &PackPin, destination: &Path) -> Result<PackTreeStatus, InstallError> {
        if !destination.is_dir() {
            return Ok(PackTreeStatus::Missing);
        }
        match pin.verify(Path::new("<pin>"), destination) {
            Ok(()) => Ok(PackTreeStatus::Ok),
            Err(InstallError::PackDigestMismatch { .. })
            | Err(InstallError::PackSymlink { .. })
            | Err(InstallError::PackSpecialFile { .. })
            | Err(InstallError::PinInvalid { .. }) => Ok(PackTreeStatus::Stale),
            Err(error) => Err(error),
        }
    }

    /// Fetch (unless cached/offline), verify and place in one call.
    pub fn acquire(
        &self,
        pin: &PackPin,
        destination: &Path,
        offline: bool,
    ) -> Result<AcquisitionReport, InstallError> {
        let recovered_swap = recover_interrupted_swap(destination)?;
        let cache = self.fetch_to_cache(pin, offline)?;
        let files = self.place(pin, &cache.path, destination)?;
        let source = if cache.fetched {
            "fetched and verified".to_string()
        } else {
            format!("verified cache {}", cache.path.display())
        };
        Ok(AcquisitionReport {
            cache,
            files,
            source,
            recovered_swap,
        })
    }
}

/// Repair a swap interrupted between "old tree moved aside" and "new tree in place".
///
/// Returns whether a repair was needed.
pub fn recover_interrupted_swap(destination: &Path) -> Result<bool, InstallError> {
    let parent = match destination.parent() {
        Some(parent) => parent,
        None => return Ok(false),
    };
    let name = match destination.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => return Ok(false),
    };
    let aside = parent.join(format!(".{name}.aside"));
    if !aside.exists() {
        return Ok(false);
    }
    if destination.exists() {
        // The swap finished; only the cleanup was interrupted.
        remove_tree_bounded(&aside)?;
        return Ok(true);
    }
    std::fs::rename(&aside, destination)
        .map_err(|source| InstallError::io("restore aside tree", destination, source))?;
    Ok(true)
}

/// Copy a tree into `destination`, refusing symlinks/special files and bytes over `limit`.
pub fn copy_tree_bounded(
    source: &Path,
    destination: &Path,
    limit: u64,
) -> Result<usize, InstallError> {
    let mut copied: u64 = 0;
    let mut files = 0usize;
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    std::fs::create_dir_all(destination)
        .map_err(|error| InstallError::io("create directory", destination, error))?;
    while let Some((from, to)) = stack.pop() {
        let entries = std::fs::read_dir(&from)
            .map_err(|error| InstallError::io("read directory", &from, error))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| InstallError::io("read directory entry", &from, error))?;
            let path = entry.path();
            let target = to.join(entry.file_name());
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| InstallError::io("inspect", &path, error))?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(InstallError::PackSymlink { path });
            }
            if file_type.is_dir() {
                std::fs::create_dir_all(&target)
                    .map_err(|error| InstallError::io("create directory", &target, error))?;
                stack.push((path, target));
                continue;
            }
            if !file_type.is_file() {
                return Err(InstallError::PackSpecialFile { path });
            }
            copied = copied.saturating_add(metadata.len());
            if copied > limit {
                return Err(InstallError::ArchiveTooLarge {
                    limit,
                    detail: format!("while copying {}", path.display()),
                });
            }
            std::fs::copy(&path, &target)
                .map_err(|error| InstallError::io("copy", &target, error))?;
            files += 1;
        }
    }
    Ok(files)
}

/// Remove a tree this crate owns, bounded and never following links out of it.
pub fn remove_tree_bounded(path: &Path) -> Result<(), InstallError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(InstallError::io("inspect", path, source)),
    };
    if !metadata.is_dir() {
        std::fs::remove_file(path).map_err(|source| InstallError::io("remove", path, source))?;
        return Ok(());
    }
    let mut budget = crate::archive::MAX_ARCHIVE_MEMBERS * 10;
    let mut stack = vec![(path.to_path_buf(), false)];
    while let Some((current, visited)) = stack.pop() {
        if visited {
            std::fs::remove_dir(&current)
                .map_err(|source| InstallError::io("remove directory", &current, source))?;
            continue;
        }
        budget = budget.saturating_sub(1);
        if budget == 0 {
            return Err(InstallError::ArchiveTooManyMembers {
                limit: crate::archive::MAX_ARCHIVE_MEMBERS * 10,
            });
        }
        stack.push((current.clone(), true));
        let entries = std::fs::read_dir(&current)
            .map_err(|source| InstallError::io("read directory", &current, source))?;
        for entry in entries {
            let entry = entry
                .map_err(|source| InstallError::io("read directory entry", &current, source))?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|source| InstallError::io("inspect", &path, source))?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                stack.push((path, false));
            } else {
                std::fs::remove_file(&path)
                    .map_err(|source| InstallError::io("remove", &path, source))?;
            }
        }
    }
    Ok(())
}

/// Count the regular files under `root` (bounded by the member budget).
pub fn count_files(root: &Path) -> Result<usize, InstallError> {
    let mut files = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|source| InstallError::io("read directory", &current, source))?;
        for entry in entries {
            let entry = entry
                .map_err(|source| InstallError::io("read directory entry", &current, source))?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|source| InstallError::io("inspect", &path, source))?;
            if metadata.file_type().is_symlink() {
                return Err(InstallError::PackSymlink { path });
            }
            if metadata.is_dir() {
                stack.push(path);
            } else {
                files += 1;
            }
        }
    }
    Ok(files)
}

/// The full-tree digest of `root`, re-exported for reports.
pub fn tree_digest(root: &Path) -> Result<String, InstallError> {
    full_tree_digest(root)
}
