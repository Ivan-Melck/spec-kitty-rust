//! Non-strict path resolution that expands symlinks even when the final
//! target does not exist.
//!
//! `std::fs::canonicalize` requires the whole path to exist. Containment
//! checks must also classify a broken symlink by the target it *would* point
//! at, otherwise a dangling link to an outside directory would be accepted
//! until the target is created. This module walks the path one component at a
//! time, expanding every symlink it encounters and carrying missing tail
//! components lexically.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::KernelError;

/// Mirrors the common host symlink-resolution ceiling. A longer chain is
/// treated as a loop even if it is technically acyclic.
const MAX_SYMLINKS: usize = 40;

/// How the single non-strict walker handles component metadata failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveIoPolicy {
    /// Preserve the existing inspection contract: only missing tails are lexical.
    MissingTail,
    /// Continue component-by-component after any lstat error, as pathlib does.
    /// Successfully expanded symlink prefixes and later `..` traversal survive.
    IgnoreMetadataErrors,
}

#[derive(Debug)]
pub(crate) enum ResolveFailure {
    Io(io::Error),
    Loop,
}

impl From<io::Error> for ResolveFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) fn map_resolve_failure(path: &Path, failure: ResolveFailure) -> KernelError {
    match failure {
        ResolveFailure::Io(source) => KernelError::io("resolve", path, source),
        ResolveFailure::Loop => KernelError::SymlinkLoop {
            path: path.to_path_buf(),
        },
    }
}

#[derive(Debug)]
enum OwnedComponent {
    Prefix(OsString),
    Root,
    Cur,
    Parent,
    Normal(OsString),
}

impl OwnedComponent {
    fn as_os_str(&self) -> &std::ffi::OsStr {
        match self {
            Self::Prefix(value) | Self::Normal(value) => value,
            Self::Root => std::ffi::OsStr::new(std::path::MAIN_SEPARATOR_STR),
            Self::Cur => std::ffi::OsStr::new("."),
            Self::Parent => std::ffi::OsStr::new(".."),
        }
    }
}

fn owned_components(path: &Path) -> Vec<OwnedComponent> {
    path.components()
        .map(|component| match component {
            Component::Prefix(prefix) => OwnedComponent::Prefix(prefix.as_os_str().to_os_string()),
            Component::RootDir => OwnedComponent::Root,
            Component::CurDir => OwnedComponent::Cur,
            Component::ParentDir => OwnedComponent::Parent,
            Component::Normal(name) => OwnedComponent::Normal(name.to_os_string()),
        })
        .collect()
}

/// Resolve `path` to an absolute path with all existing symlinks expanded.
///
/// Nonexistent tail components are appended lexically. A dangling symlink is
/// still expanded (the link node exists even when its target does not), which
/// is what makes containment fail closed.
pub(crate) fn resolve_non_strict(path: &Path) -> Result<PathBuf, ResolveFailure> {
    resolve_non_strict_with_io_policy(path, ResolveIoPolicy::MissingTail)
}

pub(crate) fn resolve_non_strict_with_io_policy(
    path: &Path,
    io_policy: ResolveIoPolicy,
) -> Result<PathBuf, ResolveFailure> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    resolve_absolute(&absolute, MAX_SYMLINKS, io_policy)
}

fn resolve_absolute(
    path: &Path,
    budget: usize,
    io_policy: ResolveIoPolicy,
) -> Result<PathBuf, ResolveFailure> {
    if budget == 0 {
        return Err(ResolveFailure::Loop);
    }

    let components = owned_components(path);
    let mut resolved = PathBuf::new();
    let mut index = 0;

    while index < components.len() {
        match &components[index] {
            OwnedComponent::Prefix(prefix) => {
                resolved.push(prefix);
                index += 1;
            }
            OwnedComponent::Root => {
                resolved.push(std::path::MAIN_SEPARATOR_STR);
                index += 1;
            }
            OwnedComponent::Cur => {
                index += 1;
            }
            OwnedComponent::Parent => {
                resolved.pop();
                index += 1;
            }
            OwnedComponent::Normal(name) => {
                let candidate = resolved.join(name);
                match fs::symlink_metadata(&candidate) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        let target = fs::read_link(&candidate)?;
                        let mut next = if target.is_absolute() {
                            target
                        } else {
                            resolved.join(target)
                        };
                        for remaining in &components[index + 1..] {
                            next.push(remaining.as_os_str());
                        }
                        return resolve_absolute(&next, budget - 1, io_policy);
                    }
                    Ok(_) => {
                        resolved.push(name);
                        index += 1;
                    }
                    Err(_) if io_policy == ResolveIoPolicy::IgnoreMetadataErrors => {
                        resolved.push(name);
                        index += 1;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        // Nothing below a missing component can be a symlink
                        // at this instant, so the rest is a lexical join.
                        resolved.push(name);
                        index += 1;
                        for component in &components[index..] {
                            match component {
                                OwnedComponent::Prefix(prefix) => resolved.push(prefix),
                                OwnedComponent::Root => {
                                    resolved.push(std::path::MAIN_SEPARATOR_STR);
                                }
                                OwnedComponent::Cur => {}
                                OwnedComponent::Parent => {
                                    resolved.pop();
                                }
                                OwnedComponent::Normal(name) => resolved.push(name),
                            }
                        }
                        return Ok(resolved);
                    }
                    Err(error) => return Err(ResolveFailure::Io(error)),
                }
            }
        }
    }

    Ok(resolved)
}
