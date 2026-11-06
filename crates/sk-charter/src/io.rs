//! Bounded reads, YAML mapping access, digests and contained path resolution.
//!
//! This is the security boundary for every user- or pack-supplied path in the
//! crate. It rejects absolute paths, Windows drive/UNC forms, `..` components,
//! NUL bytes and symlink escapes. Containment and bounded regular-file reads
//! always delegate to `sk-kernel`: build flags cannot weaken the safety policy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::CharterError;

/// A value read from a YAML mapping field.
pub(crate) enum FieldValue<'a> {
    Absent,
    Null,
    String(&'a str),
    Other(&'a serde_yaml_ng::Value),
}

/// Look up `key` in a YAML mapping without assuming the key order.
pub(crate) fn field<'a>(mapping: &'a serde_yaml_ng::Mapping, key: &str) -> FieldValue<'a> {
    for (candidate, value) in mapping {
        if candidate.as_str() == Some(key) {
            return match value {
                serde_yaml_ng::Value::Null => FieldValue::Null,
                serde_yaml_ng::Value::String(string) => FieldValue::String(string),
                other => FieldValue::Other(other),
            };
        }
    }
    FieldValue::Absent
}

/// Read a YAML document and require a mapping at the top level.
///
/// An empty document is treated as an empty mapping (absence, not malformed).
pub(crate) fn parse_yaml_mapping(
    path: &Path,
    bytes: &[u8],
) -> Result<serde_yaml_ng::Mapping, CharterError> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(bytes).map_err(|source| CharterError::InvalidYaml {
            path: path.to_path_buf(),
            source,
        })?;
    match value {
        serde_yaml_ng::Value::Null => Ok(serde_yaml_ng::Mapping::new()),
        serde_yaml_ng::Value::Mapping(mapping) => Ok(mapping),
        _ => Err(CharterError::ExpectedMapping {
            path: path.to_path_buf(),
        }),
    }
}

/// Read at most `limit` bytes, failing closed when the file is larger.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, CharterError> {
    sk_kernel::read_bounded(path, limit).map_err(|error| map_kernel_error(error, path))
}

/// Hex-encoded SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Lexically validate a relative path and return its normalized components.
///
/// Rejects an empty path, absolute POSIX/Windows/UNC forms, `..` components and
/// NUL bytes. `.` and repeated separators are normalized away.
pub(crate) fn validate_relative(relative: &str) -> Result<Vec<&str>, CharterError> {
    if relative.trim().is_empty() {
        return Err(CharterError::EmptyPath {
            path: relative.to_owned(),
        });
    }
    if relative.contains('\0') {
        return Err(CharterError::NulInPath {
            path: relative.to_owned(),
        });
    }
    let bytes = relative.as_bytes();
    let windows_drive = bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic();
    if relative.starts_with('/') || relative.starts_with('\\') || windows_drive {
        return Err(CharterError::AbsolutePath {
            path: relative.to_owned(),
        });
    }
    let mut parts = Vec::new();
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                return Err(CharterError::ParentTraversal {
                    path: relative.to_owned(),
                });
            }
            normal => parts.push(normal),
        }
    }
    if parts.is_empty() {
        return Err(CharterError::EmptyPath {
            path: relative.to_owned(),
        });
    }
    Ok(parts)
}

/// Resolve `relative` beneath `root`, enforcing containment.
///
/// The lexical checks run first, then the deepest existing ancestor is
/// canonicalized so a symlink cannot redirect the result outside `root`. A
/// candidate that does not exist yet is still returned with the canonical
/// existing prefix, matching the Python primitive's `strict=False` behavior.
pub(crate) fn resolve_contained(root: &Path, relative: &str) -> Result<PathBuf, CharterError> {
    let parts = validate_relative(relative)?;

    let normalized: PathBuf = parts.iter().collect();
    let candidate = sk_kernel::resolve_within(root, &normalized)
        .map_err(|error| map_kernel_error(error, &root.join(relative)))?;
    verify_contained(root, &candidate)
}

/// Canonicalize `root` and prove `candidate` stays beneath it.
fn verify_contained(root: &Path, candidate: &Path) -> Result<PathBuf, CharterError> {
    let canonical_root = fs::canonicalize(root).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            CharterError::RootNotDirectory {
                root: root.to_path_buf(),
            }
        } else {
            CharterError::Io {
                path: root.to_path_buf(),
                source,
            }
        }
    })?;
    if !canonical_root.is_dir() {
        return Err(CharterError::RootNotDirectory {
            root: root.to_path_buf(),
        });
    }

    match fs::canonicalize(candidate) {
        Ok(resolved) => {
            if resolved.starts_with(&canonical_root) {
                Ok(resolved)
            } else {
                Err(CharterError::PathEscape {
                    path: resolved,
                    root: canonical_root,
                })
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // The final component does not exist (a missing asset blob, for
            // example). Canonicalize the deepest existing ancestor and check the
            // containment of that prefix, then re-attach the missing suffix.
            let mut suffix: Vec<&std::ffi::OsStr> = Vec::new();
            let mut current = candidate;
            loop {
                let Some(name) = current.file_name() else {
                    return Err(CharterError::PathEscape {
                        path: candidate.to_path_buf(),
                        root: canonical_root,
                    });
                };
                suffix.push(name);
                let Some(parent) = current.parent() else {
                    return Err(CharterError::PathEscape {
                        path: candidate.to_path_buf(),
                        root: canonical_root,
                    });
                };
                match fs::canonicalize(parent) {
                    Ok(resolved) => {
                        if !resolved.starts_with(&canonical_root) {
                            return Err(CharterError::PathEscape {
                                path: resolved,
                                root: canonical_root,
                            });
                        }
                        let mut result = resolved;
                        for part in suffix.iter().rev() {
                            result.push(part);
                        }
                        return Ok(result);
                    }
                    Err(inner) if inner.kind() == io::ErrorKind::NotFound => {
                        current = parent;
                    }
                    Err(inner) => {
                        return Err(CharterError::Io {
                            path: parent.to_path_buf(),
                            source: inner,
                        });
                    }
                }
            }
        }
        Err(source) => Err(CharterError::Io {
            path: candidate.to_path_buf(),
            source,
        }),
    }
}

/// Validate a canonical ULID (26 Crockford base32 characters, first digit 0-7).
pub(crate) fn is_ulid(value: &str) -> bool {
    const FORBIDDEN: [u8; 4] = *b"ILOU";
    let bytes = value.as_bytes();
    bytes.len() == 26
        && bytes[0] <= b'7'
        && bytes.iter().all(|byte| {
            (byte.is_ascii_digit() || byte.is_ascii_uppercase()) && !FORBIDDEN.contains(byte)
        })
}

fn map_kernel_error(error: sk_kernel::KernelError, path: &Path) -> CharterError {
    use sk_kernel::KernelError as Kernel;

    match error {
        Kernel::RootNotDirectory { root } => CharterError::RootNotDirectory { root },
        Kernel::AbsoluteRelativePath { path } => CharterError::AbsolutePath {
            path: path.display().to_string(),
        },
        Kernel::ParentTraversal { path } => CharterError::ParentTraversal {
            path: path.display().to_string(),
        },
        Kernel::PathEscape { root, resolved } => CharterError::PathEscape {
            path: resolved,
            root,
        },
        Kernel::TooLarge { limit, .. } => CharterError::TooLarge {
            path: path.to_path_buf(),
            limit,
        },
        Kernel::Io { source, .. } => CharterError::Io {
            path: path.to_path_buf(),
            source,
        },
        other => CharterError::ContainmentRefused {
            path: path.to_path_buf(),
            reason: other.to_string(),
        },
    }
}
