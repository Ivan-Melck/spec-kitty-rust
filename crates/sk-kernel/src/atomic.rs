use std::io::{self, Write};
use std::path::Path;

use crate::KernelError;

/// Atomically write `bytes` to `path`, returning whether the content changed.
///
/// The write goes to a temporary file in the destination directory and is
/// renamed over the destination, so a reader observes either the complete old
/// content or the complete new content, never a partial file.
///
/// The operation is no-op stable: when the existing bytes already equal
/// `bytes`, it returns `Ok(false)` without creating a temporary file, replacing
/// the inode, or touching the mtime. Successful changes return `Ok(true)`.
///
/// The parent directory must already exist (matching upstream
/// `kernel.atomic.atomic_write`'s default `mkdir=False`). Destination symlinks
/// are followed for the equality read and replaced by the rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<bool, KernelError> {
    // Equality cannot require reading more than the new payload. Reuse the
    // regular-file/nonblocking opener so FIFOs and devices cannot hang or
    // exhaust memory before an otherwise atomic replacement.
    match crate::read_bounded(path, bytes.len() as u64) {
        Ok(existing) if existing == bytes => return Ok(false),
        Ok(_) | Err(KernelError::TooLarge { .. }) => {}
        Err(KernelError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temporary = tempfile::Builder::new()
        .prefix(".sk-kernel-atomic-")
        .suffix(".tmp")
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

    temporary
        .persist(path)
        .map_err(|error| KernelError::io("replace", path, error.error))?;

    Ok(true)
}
