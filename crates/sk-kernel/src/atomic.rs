use std::fs::{self, Permissions};
use std::io::{self, Write};
use std::path::Path;

use crate::KernelError;

/// Atomically write `bytes` to `path`, returning whether the content changed.
///
/// The write goes to a temporary file in the destination directory and is
/// renamed over the destination, so a reader observes either the complete old
/// content or the complete new content, never a partial file. The temporary is
/// flushed and `sync_all`ed before the rename; on Unix the parent directory is
/// then `sync_all`ed, so the rename itself is durable. A parent-directory sync
/// failure is returned as [`KernelError::AtomicCommitIndeterminate`], never
/// downgraded to success or confused with a mutation that did not happen.
///
/// The operation is no-op stable: when the existing bytes already equal
/// `bytes`, it returns `Ok(false)` without creating a temporary file, replacing
/// the inode, or touching the mtime. Successful changes return `Ok(true)`.
///
/// Metadata and hazard policy:
///
/// - An existing regular destination's permission bits are preserved where the
///   platform can express them. The temporary receives those bits before the
///   rename, so the replacement keeps them without a post-rename window. A
///   read-only destination stays read-only (upstream #6228 parity).
/// - New files keep the temporary-file default (`0600` on Unix), matching the
///   upstream `mkstemp` behaviour.
/// - A destination symlink is followed for the equality read but the rename
///   replaces the link node itself; its target is never modified.
/// - FIFOs, devices, directories, and symlinks to non-regular files are refused
///   through the shared bounded equality reader before any temporary is
///   created, so they cannot hang or exhaust memory.
/// - The parent directory must already exist (matching upstream
///   `kernel.atomic.atomic_write`'s default `mkdir=False`).
///
/// Owner, ACLs, extended attributes, hard-link identity, sparse layout, and
/// Windows parent-directory durability are not preserved/claimed; see the
/// crate README's deliberate gaps.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<bool, KernelError> {
    atomic_write_with_options(path, bytes, AtomicWriteOptions::default())
}

/// Options for [`atomic_write_with_options`].
///
/// The default matches [`atomic_write`] and upstream's `mkdir=False`: the
/// parent directory must already exist. Setting `create_parents` mirrors
/// upstream `atomic_write(..., mkdir=True)`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AtomicWriteOptions {
    /// Create missing parent directories before writing.
    pub create_parents: bool,
}

/// Atomically write `bytes` with explicit parent-directory policy.
pub fn atomic_write_with_options(
    path: &Path,
    bytes: &[u8],
    options: AtomicWriteOptions,
) -> Result<bool, KernelError> {
    if options.create_parents {
        fs::create_dir_all(parent_directory(path))
            .map_err(|source| KernelError::io("create parent of", path, source))?;
    }
    atomic_write_with_parent_sync(path, bytes, sync_parent_directory)
}

/// Atomically install bytes with explicit permissions on the temporary inode.
///
/// Unlike the default writer, this is a managed-owner mode assertion. Equal
/// content with a different mode is replaced; permissions are never applied by
/// reopening the destination after publication.
pub fn atomic_write_with_permissions(
    path: &Path,
    bytes: &[u8],
    permissions: Permissions,
) -> Result<bool, KernelError> {
    atomic_write_with_permissions_and_parent_sync(
        path,
        bytes,
        Some(permissions),
        None,
        sync_parent_directory,
    )
}

/// Publish with a Unix creation mode, letting the OS apply the process umask.
///
/// This is Source `kernel.yaml_io.write_mapping_atomic`'s execution policy:
/// always publish (including equal bytes), do not preserve destination modes or
/// read the destination, sync the file before rename, and best-effort sync the
/// parent afterward. The process umask is never read or changed here. Existing
/// default and explicit-permissions writers retain their distinct contracts.
#[cfg(unix)]
pub fn atomic_write_with_requested_mode(
    path: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<bool, KernelError> {
    atomic_write_with_permissions_and_parent_sync(path, bytes, None, Some(mode), |parent, path| {
        let _ = sync_parent_directory(parent, path);
        Ok(())
    })
}

fn atomic_write_with_parent_sync<F>(
    path: &Path,
    bytes: &[u8],
    sync_parent: F,
) -> Result<bool, KernelError>
where
    F: FnOnce(&Path, &Path) -> Result<(), KernelError>,
{
    atomic_write_with_permissions_and_parent_sync(path, bytes, None, None, sync_parent)
}

fn atomic_write_with_permissions_and_parent_sync<F>(
    path: &Path,
    bytes: &[u8],
    permissions: Option<Permissions>,
    requested_mode: Option<u32>,
    sync_parent: F,
) -> Result<bool, KernelError>
where
    F: FnOnce(&Path, &Path) -> Result<(), KernelError>,
{
    // Equality cannot require reading more than the new payload. Reuse the
    // shared regular-file/nonblocking opener so FIFOs and devices cannot hang
    // or exhaust memory before an otherwise atomic replacement.
    if requested_mode.is_none() {
        match crate::read_bounded(path, bytes.len() as u64) {
            Ok(existing) if existing == bytes => {
                let mode_matches = permissions.as_ref().is_none_or(|desired| {
                    fs::symlink_metadata(path).is_ok_and(|metadata| {
                        if !metadata.is_file() {
                            return false;
                        }
                        let actual = metadata.permissions();
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            actual.mode() & 0o7777 == desired.mode() & 0o7777
                        }
                        #[cfg(not(unix))]
                        {
                            actual == *desired
                        }
                    })
                });
                if mode_matches {
                    return Ok(false);
                }
            }
            Ok(_) | Err(KernelError::TooLarge { .. }) => {}
            Err(KernelError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    let parent = parent_directory(path);
    let destination = if requested_mode.is_none() {
        Some(DestinationState::inspect(path)?)
    } else {
        None
    };
    let mut builder = tempfile::Builder::new();
    builder.prefix(".sk-kernel-atomic-").suffix(".tmp");
    #[cfg(unix)]
    if let Some(mode) = requested_mode {
        use std::os::unix::fs::PermissionsExt;
        // tempfile's Unix creator passes this to OpenOptions::mode at O_EXCL
        // creation; unlike fchmod, the OS applies the actual umask.
        builder.permissions(Permissions::from_mode(mode));
    }
    let mut temporary = builder
        .tempfile_in(parent)
        .map_err(|source| KernelError::io("create temporary file in", parent, source))?;

    temporary
        .write_all(bytes)
        .map_err(|source| KernelError::io("write temporary file for", path, source))?;
    temporary
        .flush()
        .map_err(|source| KernelError::io("flush temporary file for", path, source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| KernelError::io("sync temporary file for", path, source))?;

    if let Some(permissions) = permissions.as_ref().or_else(|| {
        destination
            .as_ref()
            .and_then(|state| state.preserve.as_ref())
    }) {
        temporary
            .as_file()
            .set_permissions(permissions.clone())
            .map_err(|source| KernelError::io("preserve mode of", path, source))?;
    }

    #[cfg(windows)]
    let relaxed = destination
        .as_ref()
        .map(|state| state.relax_for_replace(path))
        .transpose()?
        .unwrap_or(false);

    match temporary.persist(path) {
        Ok(_) => {
            sync_parent(parent, path).map_err(|source| KernelError::AtomicCommitIndeterminate {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
            Ok(true)
        }
        Err(error) => {
            #[cfg(windows)]
            if let Some(destination) = &destination {
                destination.restore_after_failure(path, relaxed);
            }
            Err(KernelError::io("replace", path, error.error))
        }
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Destination metadata captured before the rename.
struct DestinationState {
    /// Permission bits to apply to the replacement, when the destination is an
    /// existing regular file.
    preserve: Option<Permissions>,
    /// On Windows only: the original permissions a read-only destination must
    /// be restored to if the rename fails.
    #[cfg(windows)]
    original: Option<Permissions>,
}

impl DestinationState {
    fn inspect(path: &Path) -> Result<Self, KernelError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Ok(Self {
                preserve: None,
                #[cfg(windows)]
                original: None,
            }),
            Ok(metadata) if metadata.is_file() => Ok(Self {
                preserve: Some(metadata.permissions()),
                #[cfg(windows)]
                original: Some(metadata.permissions()),
            }),
            Ok(_) => Err(KernelError::io(
                "inspect destination",
                path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination is not a regular file",
                ),
            )),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Self {
                preserve: None,
                #[cfg(windows)]
                original: None,
            }),
            Err(source) => Err(KernelError::io("inspect destination", path, source)),
        }
    }

    /// On Windows, `os.replace` refuses a read-only destination. Relax the
    /// owner-write bit just for the rename; the temporary already carries the
    /// original bits, so a successful rename restores them by construction.
    #[cfg(windows)]
    fn relax_for_replace(&self, path: &Path) -> Result<bool, KernelError> {
        let Some(original) = &self.original else {
            return Ok(false);
        };
        if !original.readonly() {
            return Ok(false);
        }
        let mut writable = original.clone();
        writable.set_readonly(false);
        fs::set_permissions(path, writable)
            .map_err(|source| KernelError::io("relax read-only mode of", path, source))?;
        Ok(true)
    }

    #[cfg(windows)]
    fn restore_after_failure(&self, path: &Path, relaxed: bool) {
        if relaxed && let Some(original) = &self.original {
            let _ = fs::set_permissions(path, original.clone());
        }
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path, path: &Path) -> Result<(), KernelError> {
    let directory = fs::File::open(parent)
        .map_err(|source| KernelError::io("open parent directory of", path, source))?;
    directory
        .sync_all()
        .map_err(|source| KernelError::io("sync parent directory of", path, source))
}

/// Windows cannot open a directory with `std::fs::File` and therefore cannot
/// express a directory fsync through the safe standard library. The rename is
/// still atomic; only the directory-entry durability claim is platform-limited.
/// This is reported in the crate README rather than hidden as a silent fallback.
#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path, _path: &Path) -> Result<(), KernelError> {
    Ok(())
}

/// Whether two payloads are equal for no-op-stability purposes.
///
/// This is the default (no volatile-field projection) form of upstream
/// `kernel.atomic.substantively_equal`: a plain byte comparison. The caller
/// owns reading the existing bytes and deciding what an absent file means.
pub fn substantively_equal(existing: &[u8], candidate: &[u8]) -> bool {
    existing == candidate
}

/// Compare two UTF-8 payloads after a caller-supplied volatile-field
/// projection.
///
/// The projection receives decoded text and returns the substantive form to
/// compare. It is the Rust equivalent of upstream's `strip(text,
/// volatile_keys)` callback; a caller that needs volatile keys captures them
/// in the closure rather than baking a format assumption into the kernel.
/// Invalid UTF-8 is [`KernelError::InvalidUtf8`], never lossy replacement.
pub fn substantively_equal_projected<F>(
    existing: &[u8],
    candidate: &[u8],
    strip: F,
) -> Result<bool, KernelError>
where
    F: Fn(&str) -> String,
{
    let existing = std::str::from_utf8(existing).map_err(|_| KernelError::InvalidUtf8 {
        path: std::path::PathBuf::from("<existing payload>"),
    })?;
    let candidate = std::str::from_utf8(candidate).map_err(|_| KernelError::InvalidUtf8 {
        path: std::path::PathBuf::from("<candidate payload>"),
    })?;
    Ok(strip(existing) == strip(candidate))
}

#[cfg(test)]
mod commit_outcome_tests {
    use super::*;

    #[test]
    fn substantive_equality_is_byte_exact_without_a_projection() {
        assert!(substantively_equal(b"same", b"same"));
        assert!(!substantively_equal(b"same", b"same\n"));
    }

    #[test]
    fn a_projection_can_ignore_volatile_fields() {
        let strip = |text: &str| {
            text.lines()
                .filter(|line| !line.starts_with("generated_at:"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(
            substantively_equal_projected(
                b"generated_at: 1\nbody\n",
                b"generated_at: 2\nbody\n",
                strip
            )
            .unwrap()
        );
        assert!(!substantively_equal_projected(b"body\n", b"other\n", strip).unwrap());
    }

    #[test]
    fn a_post_rename_sync_failure_reports_an_indeterminate_commit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        fs::write(&path, b"old").unwrap();
        let result = atomic_write_with_parent_sync(&path, b"new", |_, destination| {
            Err(KernelError::io(
                "sync parent directory of",
                destination,
                io::Error::other("injected sync failure"),
            ))
        });
        let error = result.unwrap_err();
        assert!(error.mutation_may_have_committed());
        assert!(matches!(
            error,
            KernelError::AtomicCommitIndeterminate { .. }
        ));
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }
}
