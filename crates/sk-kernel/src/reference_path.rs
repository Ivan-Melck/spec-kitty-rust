//! Reference-path resolution: Python's non-strict `Path.resolve()` profile.
//!
//! Two path questions in this workspace deliberately keep **different**
//! policies, and neither is a substitute for the other:
//!
//! * **Containment** (`containment.rs`, `resolve_within`, and the private
//!   `fs_resolve::resolve_non_strict` behind it) decides whether a write may
//!   trust a path. It fails closed on any probe error other than `NotFound`
//!   and retains its separately reviewed 40-step expansion budget. This new
//!   reference API does not change that existing containment/lock contract.
//! * **Reference paths** (this module) record an operator-supplied reference
//!   exactly as the Python reference does, through
//!   `kernel.resolution.resolve_rejecting_loops`. That profile is non-strict
//!   `pathlib` tolerance: a missing, dangling or unreadable (`EACCES`)
//!   component is carried lexically, `..` is applied *after* the symlink in
//!   front of it is expanded, only a genuine cycle fails, and an acyclic chain
//!   of 41, 42 or more links resolves (there is no total-link budget). An
//!   interior NUL is refused, which is CPython's `ValueError`.
//!
//! This module does **not** widen the containment policy: `resolve_within`,
//! the event-log append protocol and the lock placement keep their existing
//! resolver byte-for-byte. Sharing the path *walk* is not the same as sharing
//! the error policy, and the two profiles' old semantics were not identical.
//!
//! Resolution is a value-level, read-only operation: it expands symlinks with
//! `symlink_metadata`/`read_link`, keeps a memo of already-resolved links
//! (like CPython's `seen` map, so repeated finite reuse stays cheap), and
//! detects loops by re-entering a link whose expansion is still in progress.
//! It takes no lock and mutates nothing.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Why reference-path resolution failed.
#[derive(Debug)]
pub enum ReferencePathError {
    /// The path contains an interior NUL byte (CPython raises `ValueError`).
    InteriorNul {
        /// The path that was supplied.
        path: PathBuf,
    },
    /// Following the links would re-enter a link that is still being expanded.
    SymlinkLoop {
        /// The path that was supplied.
        path: PathBuf,
    },
    /// The current directory or a symlink target could not be read.
    Io {
        /// What was being done when the failure occurred.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
}

impl std::fmt::Display for ReferencePathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InteriorNul { path } => {
                write!(
                    formatter,
                    "path {} contains an embedded NUL byte",
                    path.display()
                )
            }
            Self::SymlinkLoop { path } => {
                write!(formatter, "symlink loop while resolving {}", path.display())
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "failed to {action} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ReferencePathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InteriorNul { .. } | Self::SymlinkLoop { .. } => None,
        }
    }
}

/// One unresolved step of the walk.
#[derive(Debug)]
enum Step {
    /// Leave the current directory.
    Parent,
    /// Inspect `name` under the resolved prefix.
    Normal(OsString),
    /// A link expansion finished; record where it landed.
    Remember(PathBuf),
}

/// Resolve `path` to an absolute path with every existing symlink expanded.
///
/// Missing tail components are appended lexically, a dangling link resolves to
/// its target, `..` is applied to the *expanded* prefix, and only a real cycle
/// fails. See the module documentation for why this is not the containment
/// policy.
pub fn resolve_reference_path(path: &Path) -> Result<PathBuf, ReferencePathError> {
    if has_interior_nul(path) {
        return Err(ReferencePathError::InteriorNul {
            path: path.to_path_buf(),
        });
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|source| ReferencePathError::Io {
                action: "read the current directory",
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };
    resolve_absolute(&absolute, path)
}

fn has_interior_nul(path: &Path) -> bool {
    path.as_os_str().to_string_lossy().contains('\0')
}

fn resolve_absolute(absolute: &Path, supplied: &Path) -> Result<PathBuf, ReferencePathError> {
    let mut resolved = PathBuf::new();
    let mut pending: VecDeque<Step> = VecDeque::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => pending.push_back(Step::Parent),
            Component::Normal(name) => pending.push_back(Step::Normal(name.to_os_string())),
        }
    }

    // CPython's `seen` map: `None` while a link expands, then the resolved
    // path. Besides loop detection this keeps repeated finite reuse from
    // re-walking the same subtree.
    let mut seen: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    while let Some(step) = pending.pop_front() {
        match step {
            Step::Parent => {
                resolved.pop();
            }
            Step::Remember(link) => {
                seen.insert(link, Some(resolved.clone()));
            }
            Step::Normal(name) => {
                let candidate = resolved.join(&name);
                match fs::symlink_metadata(&candidate) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        match seen.get(&candidate) {
                            Some(Some(cached)) => {
                                resolved = cached.clone();
                                continue;
                            }
                            Some(None) => {
                                return Err(ReferencePathError::SymlinkLoop {
                                    path: supplied.to_path_buf(),
                                });
                            }
                            None => {}
                        }
                        seen.insert(candidate.clone(), None);
                        let target =
                            fs::read_link(&candidate).map_err(|source| ReferencePathError::Io {
                                action: "read a symbolic link",
                                path: candidate.clone(),
                                source,
                            })?;
                        let mut expansion: Vec<Step> = Vec::new();
                        for component in target.components() {
                            match component {
                                Component::Prefix(prefix) => {
                                    resolved.clear();
                                    resolved.push(prefix.as_os_str());
                                }
                                Component::RootDir => {
                                    resolved.clear();
                                    resolved.push(std::path::MAIN_SEPARATOR_STR);
                                }
                                Component::CurDir => {}
                                Component::ParentDir => expansion.push(Step::Parent),
                                Component::Normal(name) => {
                                    expansion.push(Step::Normal(name.to_os_string()));
                                }
                            }
                        }
                        pending.push_front(Step::Remember(candidate));
                        for step in expansion.into_iter().rev() {
                            pending.push_front(step);
                        }
                    }
                    // Missing, dangling, unreadable (EACCES) or non-directory
                    // components are carried lexically, exactly like
                    // non-strict `Path.resolve()`.
                    _ => resolved.push(&name),
                }
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(path: &Path) -> PathBuf {
        fs::canonicalize(path).expect("canonical path")
    }

    #[test]
    fn missing_tail_is_appended_lexically() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("no").join("such").join("file");
        assert_eq!(resolve_reference_path(&missing).unwrap(), missing);
    }

    #[test]
    fn an_interior_nul_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("not/a/real/path\u{0}");
        assert!(matches!(
            resolve_reference_path(&path),
            Err(ReferencePathError::InteriorNul { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn parent_components_are_applied_after_expansion() {
        // `..` after a symlink means the parent of the *target* (Python's
        // non-strict `resolve()`), not a lexical pop of the link's directory.
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir_all(&target).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // "<temp>/link/.." must resolve to the target's parent, not `<temp>`.
        assert_eq!(
            resolve_reference_path(&link.join("..")).unwrap(),
            canonical(temp.path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_link_resolves_to_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside.jsonl");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_eq!(
            resolve_reference_path(&link).unwrap(),
            canonical(temp.path()).join("outside.jsonl")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loop_fails() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("a");
        let second = temp.path().join("b");
        std::os::unix::fs::symlink(&second, &first).unwrap();
        std::os::unix::fs::symlink(&first, &second).unwrap();
        assert!(matches!(
            resolve_reference_path(&first),
            Err(ReferencePathError::SymlinkLoop { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_self_link_is_a_loop() {
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("self");
        std::os::unix::fs::symlink("self", &link).unwrap();
        assert!(matches!(
            resolve_reference_path(&link),
            Err(ReferencePathError::SymlinkLoop { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn long_acyclic_chains_are_not_capped() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("target.txt"), "x").unwrap();
        for count in [41_usize, 42, 200] {
            let name = format!("chain{count}");
            let link = |index: usize| temp.path().join(format!("{name}-{index:05}"));
            std::os::unix::fs::symlink("target.txt", link(count - 1)).unwrap();
            for index in (0..count - 1).rev() {
                let next = format!("{name}-{:05}", index + 1);
                std::os::unix::fs::symlink(&next, link(index)).unwrap();
            }
            assert_eq!(
                resolve_reference_path(&link(0)).unwrap(),
                canonical(temp.path()).join("target.txt"),
                "a {count}-link acyclic chain must resolve"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_finite_repeated_link_is_not_a_loop() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("leaf.md"), "x").unwrap();
        std::os::unix::fs::symlink("target", temp.path().join("reused")).unwrap();
        // The same link twice in one path, with `..` between: not a cycle.
        assert_eq!(
            resolve_reference_path(&temp.path().join("reused/../reused/leaf.md")).unwrap(),
            canonical(&target).join("leaf.md")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_intermediate_is_tolerated_lexically() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let locked = temp.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("f.md"), "x").unwrap();
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&locked, permissions).unwrap();
        let discriminating = fs::read_dir(&locked).is_err();

        let resolved = resolve_reference_path(&locked.join("f.md"));
        let mut restore = fs::metadata(&locked).unwrap().permissions();
        restore.set_mode(0o700);
        fs::set_permissions(&locked, restore).unwrap();

        if !discriminating {
            eprintln!("permission-bypass privileges: EACCES case is not discriminating here");
            return;
        }
        // Python's non-strict resolve carries the unreadable component and
        // keeps the tail; it must not surface an EACCES error.
        assert_eq!(
            resolved.unwrap(),
            canonical(temp.path()).join("locked/f.md")
        );
    }
}
