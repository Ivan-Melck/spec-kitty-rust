//! Bounded, fail-closed archive extraction for pack bundles.
//!
//! Every archive is treated as hostile until proven otherwise. Before a single
//! byte reaches the destination this module enforces:
//!
//! - **member count** (`MAX_ARCHIVE_MEMBERS`) — a bomb of millions of empty
//!   entries is refused before iteration can hurt;
//! - **raw and extracted byte budgets** (`MAX_ARCHIVE_BYTES`,
//!   `MAX_EXTRACTED_BYTES`) — compression ratio is irrelevant because the
//!   *declared* and *actually copied* bytes are both counted;
//! - **path containment** — absolute paths, Windows drive/UNC spellings, `..`
//!   components, backslashes, NUL bytes and empty names are refused;
//! - **node type** — symlinks, hardlinks, devices, FIFOs and other special
//!   members are refused (a symlink member is the classic escape: create
//!   `etc -> /etc`, then write `etc/passwd`);
//! - **declared vs actual size** — a tar header that lies about its size is
//!   caught by the copied-byte counter, not by the header.
//!
//! Extraction is staged by the caller: files are written into a fresh directory
//! that is only promoted after the pack's digests verify, so a rejected archive
//! can never partially replace a working tree.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::InstallError;

/// Maximum decompressed content copied out of one archive.
pub const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum size of the archive file itself.
pub const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024;
/// Maximum number of archive members.
pub const MAX_ARCHIVE_MEMBERS: usize = 10_000;

/// Byte/member budgets for extraction.
#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    /// Maximum archive file size.
    pub max_archive_bytes: u64,
    /// Maximum total bytes copied out.
    pub max_extracted_bytes: u64,
    /// Maximum member count.
    pub max_members: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: MAX_ARCHIVE_BYTES,
            max_extracted_bytes: MAX_EXTRACTED_BYTES,
            max_members: MAX_ARCHIVE_MEMBERS,
        }
    }
}

/// The archive formats a pack bundle may use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveKind {
    /// `.tar.gz` / `.tgz`
    TarGz,
    /// `.zip`
    Zip,
}

impl ArchiveKind {
    /// Infer the format from a file name, if it is one we support.
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if name.ends_with(".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }

    /// Infer the format from a MIME-ish content type, if it is one we support.
    pub fn from_content_type(content_type: &str) -> Option<Self> {
        let lowered = content_type.to_ascii_lowercase();
        if lowered.contains("gzip") || lowered.contains("x-tar") {
            Some(Self::TarGz)
        } else if lowered.contains("zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }
}

/// Extract `archive` into `target`, returning the number of regular files written.
///
/// `target` must be a fresh directory owned by the caller; existing files are
/// never overwritten (a collision is an error, not a silent replace).
pub fn extract_archive(
    archive: &Path,
    target: &Path,
    kind: ArchiveKind,
    limits: &ArchiveLimits,
) -> Result<usize, InstallError> {
    let metadata = std::fs::symlink_metadata(archive)
        .map_err(|source| InstallError::io("inspect archive", archive, source))?;
    if !metadata.is_file() {
        return Err(InstallError::ArchiveInvalid {
            detail: format!("{} is not a regular file", archive.display()),
        });
    }
    if metadata.len() > limits.max_archive_bytes {
        return Err(InstallError::ArchiveTooLarge {
            limit: limits.max_archive_bytes,
            detail: format!("archive is {} bytes", metadata.len()),
        });
    }
    std::fs::create_dir_all(target)
        .map_err(|source| InstallError::io("create extraction root", target, source))?;
    match kind {
        ArchiveKind::TarGz => extract_tar_gz(archive, target, limits),
        ArchiveKind::Zip => extract_zip(archive, target, limits),
    }
}

fn extract_tar_gz(
    archive: &Path,
    target: &Path,
    limits: &ArchiveLimits,
) -> Result<usize, InstallError> {
    let file = std::fs::File::open(archive)
        .map_err(|source| InstallError::io("open archive", archive, source))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);

    let mut copied: u64 = 0;
    let mut written = 0usize;
    let mut members = 0usize;
    let mut buffer = vec![0_u8; 64 * 1024];

    let entries = tar
        .entries()
        .map_err(|error| InstallError::ArchiveInvalid {
            detail: error.to_string(),
        })?;
    for entry in entries {
        members += 1;
        if members > limits.max_members {
            return Err(InstallError::ArchiveTooManyMembers {
                limit: limits.max_members,
            });
        }
        let mut entry = entry.map_err(|error| InstallError::ArchiveInvalid {
            detail: error.to_string(),
        })?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();
        let raw_name = entry.path_bytes().to_vec();
        let name =
            String::from_utf8(raw_name).map_err(|error| InstallError::ArchiveInvalidPath {
                member: error.to_string(),
            })?;
        let relative = validate_member_path(&name)?;
        let destination = target.join(&relative);

        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(InstallError::ArchiveLink { member: name });
        }
        if entry_type.is_dir() {
            std::fs::create_dir_all(&destination)
                .map_err(|source| InstallError::io("create directory", &destination, source))?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(InstallError::ArchiveSpecialFile { member: name });
        }

        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|source| InstallError::io("create", &destination, source))?;
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| InstallError::ArchiveInvalid {
                    detail: format!("reading {name}: {error}"),
                })?;
            if read == 0 {
                break;
            }
            copied = copied.saturating_add(read as u64);
            if copied > limits.max_extracted_bytes {
                return Err(InstallError::ArchiveTooLarge {
                    limit: limits.max_extracted_bytes,
                    detail: format!("while copying {name}"),
                });
            }
            output
                .write_all(&buffer[..read])
                .map_err(|source| InstallError::io("write", &destination, source))?;
        }
        output
            .flush()
            .map_err(|source| InstallError::io("flush", &destination, source))?;
        written += 1;
    }
    Ok(written)
}

fn extract_zip(
    archive: &Path,
    target: &Path,
    limits: &ArchiveLimits,
) -> Result<usize, InstallError> {
    let file = std::fs::File::open(archive)
        .map_err(|source| InstallError::io("open archive", archive, source))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| InstallError::ArchiveInvalid {
        detail: error.to_string(),
    })?;
    if zip.len() > limits.max_members {
        return Err(InstallError::ArchiveTooManyMembers {
            limit: limits.max_members,
        });
    }

    let mut copied: u64 = 0;
    let mut written = 0usize;
    let mut buffer = vec![0_u8; 64 * 1024];

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| InstallError::ArchiveInvalid {
                detail: error.to_string(),
            })?;
        let name = entry.name().to_string();
        let relative = validate_member_path(&name)?;
        let destination = target.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&destination)
                .map_err(|source| InstallError::io("create directory", &destination, source))?;
            continue;
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170_000;
            if kind == 0o120_000 {
                return Err(InstallError::ArchiveLink { member: name });
            }
            if kind != 0 && kind != 0o100_000 {
                return Err(InstallError::ArchiveSpecialFile { member: name });
            }
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|source| InstallError::io("create", &destination, source))?;
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| InstallError::ArchiveInvalid {
                    detail: format!("reading {name}: {error}"),
                })?;
            if read == 0 {
                break;
            }
            copied = copied.saturating_add(read as u64);
            if copied > limits.max_extracted_bytes {
                return Err(InstallError::ArchiveTooLarge {
                    limit: limits.max_extracted_bytes,
                    detail: format!("while copying {name}"),
                });
            }
            output
                .write_all(&buffer[..read])
                .map_err(|source| InstallError::io("write", &destination, source))?;
        }
        output
            .flush()
            .map_err(|source| InstallError::io("flush", &destination, source))?;
        written += 1;
    }
    Ok(written)
}

/// Validate one archive member name and return its safe relative path.
pub fn validate_member_path(name: &str) -> Result<PathBuf, InstallError> {
    if name.is_empty() || name.contains('\0') || name.contains('\\') {
        return Err(InstallError::ArchiveInvalidPath {
            member: name.to_string(),
        });
    }
    let trimmed = name.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(InstallError::ArchiveInvalidPath {
            member: name.to_string(),
        });
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
                    | std::path::Component::ParentDir
                    | std::path::Component::CurDir
            )
        })
    {
        return Err(InstallError::ArchiveTraversal {
            member: name.to_string(),
        });
    }
    // Windows drive-relative spellings (`C:evil`) are not `Component::Prefix` on
    // every host, so reject them by shape as well.
    if trimmed.len() > 1 && trimmed.as_bytes()[1] == b':' {
        return Err(InstallError::ArchiveTraversal {
            member: name.to_string(),
        });
    }
    Ok(candidate.to_path_buf())
}

/// Hoist a single top-level directory's contents to `target` (bundle archives).
///
/// Only applies when `target` holds exactly one child directory and no files;
/// otherwise the tree is left untouched.
pub fn flatten_single_top_dir(target: &Path) -> Result<bool, InstallError> {
    let entries: Vec<PathBuf> = std::fs::read_dir(target)
        .map_err(|source| InstallError::io("read directory", target, source))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| InstallError::io("read directory entry", target, source))?;
    if entries.len() != 1 {
        return Ok(false);
    }
    let inner = &entries[0];
    let metadata = std::fs::symlink_metadata(inner)
        .map_err(|source| InstallError::io("inspect", inner, source))?;
    if !metadata.file_type().is_dir() {
        return Ok(false);
    }
    let children: Vec<PathBuf> = std::fs::read_dir(inner)
        .map_err(|source| InstallError::io("read directory", inner, source))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| InstallError::io("read directory entry", inner, source))?;
    for child in children {
        let name = child
            .file_name()
            .ok_or_else(|| InstallError::ArchiveInvalid {
                detail: format!("cannot derive a name for {}", child.display()),
            })?
            .to_os_string();
        std::fs::rename(&child, target.join(name))
            .map_err(|source| InstallError::io("move", &child, source))?;
    }
    std::fs::remove_dir(inner).map_err(|source| InstallError::io("remove", inner, source))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute_members() {
        assert!(validate_member_path("../escape.md").is_err());
        assert!(validate_member_path("a/../../escape.md").is_err());
        assert!(validate_member_path("/etc/passwd").is_err());
        assert!(validate_member_path("C:evil").is_err());
        assert!(validate_member_path("pack\\file.md").is_err());
        assert!(validate_member_path("./file.md").is_err());
        assert_eq!(
            validate_member_path("pack/file.md").expect("safe"),
            PathBuf::from("pack/file.md")
        );
        assert_eq!(
            validate_member_path("pack/dir/").expect("safe dir"),
            PathBuf::from("pack/dir")
        );
    }
}
