//! Source `Path.rglob("*.yaml")`, the one offering-source discovery walk.
//!
//! Behaviour is adapted from CPython 3.11.15 `pathlib`
//! (`_Selector.select_from`, `_WildcardSelector`, `_RecursiveWildcardSelector`
//! and the fixed `*.yaml` fnmatch pattern), under the retained PSF custody
//! notice `docs/rust/dependencies/cpython-3.11.15-reference-LICENSE.txt`; the
//! precise provenance/adaptation note is
//! `docs/rust/dependencies/cpython-pathlib-rglob-reference.md`.
//!
//! Deliberate Source facts this module preserves:
//!
//! * every candidate whose name ends in `.yaml` is yielded — a regular file, a
//!   directory, or a dangling symlink. There is no `is_file` strengthening and
//!   no existence check on the candidate;
//! * recursion follows *real* child directories only (`entry.is_dir(
//!   follow_symlinks=False)`): a symlinked child directory is not descended,
//!   while an explicitly passed symlinked root is scanned;
//! * a `scandir` `PermissionError` stops that subtree silently, while every
//!   other scan error is a typed cause (never a healthy empty result);
//! * `entry.is_dir()` operational errors in `_ignore_error`'s set
//!   (`ENOENT`/`ENOTDIR`/`EBADF`/`ELOOP`) mean "not a directory"; a
//!   `PermissionError` there stops the subtree; every other error propagates;
//! * no artificial depth, count or recursion cap is added.
//!
//! The fixed pattern is matched on the native filename bytes (POSIX), so a
//! non-UTF8 name is still a candidate and is decoded later by the ONE captured
//! filesystem codec. Ordering is `scandir` order; the caller sorts through the
//! canonical Path component comparator, exactly like the reference's
//! `sorted(base.rglob("*.yaml"))`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A scan failure outside the reference's ignored cases.
#[derive(Debug, thiserror::Error)]
pub enum SourceGlobError {
    /// The root/directory stat or an unignored directory scan failure.
    #[error("could not scan offering source directory {path}: {source}")]
    Io {
        /// The directory being scanned (or the root being inspected).
        path: PathBuf,
        /// The actual filesystem failure.
        #[source]
        source: io::Error,
    },
}

/// Source `sorted(base.rglob("*.yaml"))`'s candidate discovery (unsorted).
///
/// `root` is the per-kind offering directory. A root that is absent or not a
/// directory yields no candidates (Source's `_Selector.select_from` checks
/// `is_dir` first); an unignored stat failure is a typed cause.
///
/// # Errors
///
/// [`SourceGlobError::Io`] for unignored directory/entry failures.
pub fn source_rglob_yaml(root: &Path) -> Result<Vec<PathBuf>, SourceGlobError> {
    if !crate::paths::source_path_is_dir(root).map_err(|source| SourceGlobError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        return Ok(Vec::new());
    }
    let mut matches = Vec::new();
    walk_yaml(root, &mut matches)?;
    Ok(matches)
}

fn walk_yaml(directory: &Path, matches: &mut Vec<PathBuf>) -> Result<(), SourceGlobError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        // `_WildcardSelector`/`_RecursiveWildcardSelector` catch PermissionError
        // and stop this subtree.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(()),
        Err(source) => {
            return Err(SourceGlobError::Io {
                path: directory.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(()),
            Err(source) => {
                return Err(SourceGlobError::Io {
                    path: directory.to_path_buf(),
                    source,
                });
            }
        };
        let name = entry.file_name();
        let path = directory.join(&name);
        if name_ends_with_yaml(&name) {
            matches.push(path.clone());
        }
        // Source: `entry.is_dir(follow_symlinks=False)`; `file_type()` is the
        // no-follow (lstat) answer.
        match entry.file_type() {
            Ok(file_type) => {
                if file_type.is_dir() {
                    walk_yaml(&path, matches)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(()),
            Err(error) if crate::paths::source_ignores_os_error(&error) => {}
            Err(source) => {
                return Err(SourceGlobError::Io {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    Ok(())
}

/// `fnmatch.translate("*.yaml")` over the native filename.
///
/// POSIX compare is case-sensitive on the exact bytes; the name is never
/// decoded here, so undecodable bytes remain candidates.
#[cfg(unix)]
fn name_ends_with_yaml(name: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    name.as_bytes().ends_with(b".yaml")
}

/// Windows' ASCII literal suffix compares case-insensitively without decoding
/// or replacing any native wide units in the filename.
#[cfg(windows)]
fn name_ends_with_yaml(name: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let units: Vec<u16> = name.encode_wide().collect();
    let suffix: Vec<u16> = ".yaml".encode_utf16().collect();
    units.len() >= suffix.len()
        && units[units.len() - suffix.len()..]
            .iter()
            .zip(suffix)
            .all(|(&unit, expected)| {
                unit == expected
                    || (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) && unit + 32 == expected
            })
}

/// Non-Windows platforms use the ASCII literal suffix without a lossy native
/// filename conversion. No cross-platform certification is implied.
#[cfg(not(any(unix, windows)))]
fn name_ends_with_yaml(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().ends_with(b".yaml")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn names(root: &Path) -> Vec<String> {
        let mut found: Vec<String> = source_rglob_yaml(root)
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn candidates_include_directories_and_dangling_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("a.yaml"), "x: 1\n");
        fs::create_dir_all(root.join("dir.yaml")).unwrap();
        std::os::unix::fs::symlink("missing-target", root.join("dangling.yaml")).unwrap();
        write(&root.join("nested/b.yaml"), "x: 1\n");
        write(&root.join("ignored.txt"), "x: 1\n");
        assert_eq!(
            names(root),
            vec![
                "a.yaml".to_owned(),
                "dangling.yaml".to_owned(),
                "dir.yaml".to_owned(),
                "nested/b.yaml".to_owned(),
            ]
        );
    }

    #[test]
    fn child_directory_symlinks_are_not_descended() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("offering");
        let outside = temp.path().join("outside");
        fs::create_dir_all(root.join("real")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        write(&root.join("real/in.yaml"), "x: 1\n");
        write(&outside.join("outside.yaml"), "x: 1\n");
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        assert_eq!(names(&root), vec!["real/in.yaml".to_owned()]);
    }

    #[test]
    fn an_explicit_symlinked_root_is_scanned() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        write(&real.join("in.yaml"), "x: 1\n");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(names(&link), vec!["in.yaml".to_owned()]);
    }

    #[test]
    fn non_directory_and_absent_roots_yield_no_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.yaml");
        write(&file, "x: 1\n");
        assert!(source_rglob_yaml(&file).unwrap().is_empty());
        assert!(
            source_rglob_yaml(&temp.path().join("absent"))
                .unwrap()
                .is_empty()
        );
    }
}
