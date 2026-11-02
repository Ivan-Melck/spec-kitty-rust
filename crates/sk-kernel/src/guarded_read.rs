//! Format-agnostic guarded reads.
//!
//! A guarded read performs exactly one bounded read and then hands the bytes to
//! a caller-supplied parser. Declared parser failures collapse into one typed
//! [`KernelError::GuardedRead`] carrying the path and reason; the parser is
//! never called twice, and the kernel never imports a schema or format library.
//!
//! This is the Rust counterpart of upstream `kernel.guarded_read`: the parser
//! owns format knowledge (`serde_json`, YAML, TOML, hand-written decoders),
//! while the kernel owns read-once plus bounded, regular-file, nonblocking I/O.

use std::fmt;
use std::path::Path;

use crate::KernelError;
use crate::read_bounded;

/// Read `path` once (at most `max_bytes`), then parse the bytes.
///
/// A parser error `E` is collapsed into [`KernelError::GuardedRead`] with its
/// `Display` text. Read failures keep their own typed variants (`Io`,
/// `TooLarge`).
pub fn read_guarded<T, E, F>(path: &Path, max_bytes: u64, parse: F) -> Result<T, KernelError>
where
    E: fmt::Display,
    F: FnOnce(&[u8]) -> Result<T, E>,
{
    let bytes = read_bounded(path, max_bytes)?;
    parse(&bytes).map_err(|error| KernelError::GuardedRead {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Read `path` once as UTF-8, then parse the text.
///
/// Invalid UTF-8 is refused before the parser runs, so a caller's parser is
/// never handed lossily decoded content.
pub fn read_guarded_text<T, E, F>(path: &Path, max_bytes: u64, parse: F) -> Result<T, KernelError>
where
    E: fmt::Display,
    F: FnOnce(&str) -> Result<T, E>,
{
    let bytes = read_bounded(path, max_bytes)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| KernelError::InvalidUtf8 {
        path: path.to_path_buf(),
    })?;
    parse(text).map_err(|error| KernelError::GuardedRead {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Shared collapse helper for callers that already hold bytes and must map a
/// declared parser failure set onto the kernel's typed error.
///
/// The closure `parse` is run once. Any error it returns is rendered with its
/// `Display` implementation; genuine bugs are not distinguished here because
/// Rust's typed `Result` already makes the declared set explicit.
pub fn collapse_guarded<T, E, F>(path: &Path, parse: F) -> Result<T, KernelError>
where
    E: fmt::Display,
    F: FnOnce() -> Result<T, E>,
{
    parse().map_err(|error| KernelError::GuardedRead {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Read a bounded regular file without following a final symlink.
///
/// Convenience re-export so guarded-read consumers do not need two imports for
/// the two halves of the same safety policy.
pub fn read_bounded_no_follow(path: &Path, max_bytes: u64) -> Result<Vec<u8>, KernelError> {
    crate::no_follow::read_no_follow(path, max_bytes)
}
