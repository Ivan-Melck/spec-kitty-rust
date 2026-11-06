//! Raw path handling.
//!
//! Git prints paths as bytes. On Unix those bytes are not necessarily UTF-8
//! (and a valid UTF-8 string cannot represent every filename), so the adapter
//! keeps them as `OsString` and never round-trips a path through a `String`.
//! On Windows, git emits UTF-8 path bytes under `-z`; they are decoded to
//! `OsString` from UTF-8 without loss for well-formed output.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;

use crate::error::GitError;

/// A path as git printed it, relative to the repository root.
///
/// Equality, ordering, and hashing use the exact platform `OsString` bytes;
/// two paths that differ only in lossy display representation stay distinct.
///
/// Use [`GitPath::as_os_str`] or [`GitPath::to_path_buf`] to hand the path to
/// filesystem APIs, and [`GitPath::display_lossy`] only for human-readable
/// diagnostics.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitPath(OsString);

impl GitPath {
    /// Build a path from the raw bytes git printed.
    ///
    /// On Unix every byte sequence is accepted and retained. On other
    /// platforms the bytes must be valid UTF-8 (git for Windows emits UTF-8
    /// under `-z`), otherwise [`GitError::NonUtf8`] is returned.
    pub fn from_git_bytes(raw: &[u8]) -> Result<Self, GitError> {
        os_string_from_git_bytes(raw, "path").map(Self)
    }

    /// The path as an `OsStr`, preserving the exact platform representation.
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }

    /// A copy of the path as an owned `OsString`.
    pub fn to_os_string(&self) -> OsString {
        self.0.clone()
    }

    /// The path as a `PathBuf` suitable for filesystem calls.
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.0)
    }

    /// Whether this is the empty repository-relative path (the root).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A lossy, human-readable rendering. Never parse this back into a path.
    pub fn display_lossy(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl AsRef<OsStr> for GitPath {
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl From<GitPath> for PathBuf {
    fn from(path: GitPath) -> Self {
        PathBuf::from(path.0)
    }
}

impl fmt::Display for GitPath {
    /// Lossy display for diagnostics only; use [`GitPath::as_os_str`] when the
    /// exact bytes matter.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display_lossy())
    }
}

impl fmt::Debug for GitPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GitPath(")?;
        for &byte in raw_bytes(self.0.as_os_str()).as_ref() {
            match byte {
                b' '..=b'~' if byte != b'\\' && byte != b'"' => {
                    formatter.write_str(&char::from(byte).to_string())?;
                }
                b'\\' => formatter.write_str("\\\\")?,
                b'"' => formatter.write_str("\\\"")?,
                _ => write!(formatter, "\\x{byte:02x}")?,
            }
        }
        formatter.write_str(")")
    }
}

/// Convert git's raw path bytes into an `OsString` without loss.
pub(crate) fn os_string_from_git_bytes(
    raw: &[u8],
    field: &'static str,
) -> Result<OsString, GitError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let _ = field;
        Ok(OsString::from_vec(raw.to_vec()))
    }
    #[cfg(not(unix))]
    {
        match std::str::from_utf8(raw) {
            Ok(text) => Ok(OsString::from(text)),
            Err(_) => Err(GitError::NonUtf8 { field }),
        }
    }
}

/// The platform bytes behind an `OsStr`, borrowed where possible.
fn raw_bytes(value: &OsStr) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(value.as_bytes())
    }
    #[cfg(not(unix))]
    {
        Cow::Owned(value.to_string_lossy().into_owned().into_bytes())
    }
}

/// Strip exactly one trailing line feed from git's single-line output.
pub(crate) fn strip_line_feed(raw: &[u8]) -> &[u8] {
    raw.strip_suffix(b"\n").unwrap_or(raw)
}

/// Decode git output that the public API exposes as a `String`.
pub(crate) fn utf8_field(raw: &[u8], field: &'static str) -> Result<String, GitError> {
    std::str::from_utf8(raw)
        .map(str::to_string)
        .map_err(|_| GitError::NonUtf8 { field })
}
