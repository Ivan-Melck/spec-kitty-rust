use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::KernelError;
use crate::fs_resolve::{map_resolve_failure, resolve_non_strict};

/// Resolve an untrusted relative path beneath `root`, rejecting traversal and
/// symlink escapes.
///
/// The returned path is absolute and has existing symlinks expanded; a
/// missing tail is appended lexically. Containment is checked against the
/// resolved root, so a symlink that leaves the root fails closed even when its
/// target does not exist yet.
pub fn resolve_within(root: &Path, relative: &Path) -> Result<PathBuf, KernelError> {
    reject_non_relative(relative)?;

    let resolved_root =
        resolve_non_strict(root).map_err(|failure| map_resolve_failure(root, failure))?;
    match fs::metadata(&resolved_root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(KernelError::RootNotDirectory {
                root: root.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(KernelError::RootNotDirectory {
                root: root.to_path_buf(),
            });
        }
        Err(source) => return Err(KernelError::io("inspect root", root, source)),
    }

    let candidate = resolved_root.join(relative);
    let resolved = resolve_non_strict(&candidate)
        .map_err(|failure| map_resolve_failure(&candidate, failure))?;

    if !resolved.starts_with(&resolved_root) {
        return Err(KernelError::PathEscape {
            root: resolved_root,
            resolved,
        });
    }

    Ok(resolved)
}

fn reject_non_relative(relative: &Path) -> Result<(), KernelError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
        || has_windows_absolute_form(relative)
    {
        return Err(KernelError::AbsoluteRelativePath {
            path: relative.to_path_buf(),
        });
    }

    let has_parent_component = relative
        .components()
        .any(|component| matches!(component, Component::ParentDir));
    let has_windows_parent_component = relative
        .to_str()
        .is_some_and(|text| text.split(['/', '\\']).any(|part| part == ".."));
    if has_parent_component || has_windows_parent_component {
        return Err(KernelError::ParentTraversal {
            path: relative.to_path_buf(),
        });
    }

    Ok(())
}

/// Detect Windows absolute, rooted, UNC, and drive-relative spellings even
/// when the current host parses backslashes as ordinary filename bytes.
fn has_windows_absolute_form(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    let bytes = text.as_bytes();

    if bytes.first() == Some(&b'\\') {
        return true;
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }

    false
}
