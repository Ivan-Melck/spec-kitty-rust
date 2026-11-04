//! The captured operator-index authority.
//!
//! Git lets a caller redirect every index read and write with
//! `GIT_INDEX_FILE`. A writer that silently replaced that index with
//! `<git-dir>/index` would stage into, lock, and reconcile a different file
//! than the operator's own commands use. It would also be a silent mutation of
//! user-visible state: the caller's staging area would not move, or an
//! unrelated index would.
//!
//! This module captures the selector **once, at repository open**, and binds
//! every later operator-index operation to that captured value:
//!
//! * the process environment is never mutated;
//! * a relative value is resolved against the discovered work-tree root, which
//!   is the directory the child git actually runs in (git resolves a relative
//!   `GIT_INDEX_FILE` against its working directory, which it moves to the
//!   top of the work tree during setup);
//! * the value is passed to child git *verbatim*, so a path that contains a
//!   symlink as its final component keeps that spelling and git keeps its own
//!   follow-through behaviour;
//! * a plan records the captured authority and refuses to apply under a
//!   different one, so a plan prepared in one index namespace can never mutate
//!   another.
//!
//! The selector is not the only authority a writer needs. Git's own lock setup
//! chases a symlinked final path component and creates
//! `<resolved target>.lock`, so locking `<spelled>.lock` would not exclude git
//! for a captured index that is a symlink (or for a `.git/index` that is one).
//! [`resolve_final_symlink`] is the single implementation of that chase, and it
//! is applied at Native read-only lock checks and lock acquisition. Operator
//! Git commands still receive the raw selector and perform Git's own chase.
//! The Native lock endpoint is never frozen when the
//! repository was opened, because a symlink may legally be retargeted between a
//! plan and its apply. The captured spelling itself is never rewritten.
//!
//! The default index (`<git-dir>/index`) stays implicit: no `GIT_INDEX_FILE` is
//! invented for a repository whose environment did not set one. Only a private
//! scoped index generated inside the checkout's own git directory overrides
//! the captured value, and only for the specific commands that build a
//! candidate tree.
//!
//! Repository-redirecting variables other than `GIT_INDEX_FILE` remain
//! explicit refusals: this module captures an index, not a shared repository
//! authority.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

/// Where the operator index comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperatorIndexSource {
    /// No selector was set: git's ordinary per-worktree index.
    Default,
    /// `GIT_INDEX_FILE` was set when the repository was opened.
    Environment,
}

impl OperatorIndexSource {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Environment => "environment",
        }
    }
}

/// The index every operator-facing read and write of one repository handle uses.
///
/// Created by [`GitRepository::open`](crate::GitRepository::open); there is no
/// public constructor, and the capture is sticky for the lifetime of the
/// handle even if the process environment changes later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorIndex {
    source: OperatorIndexSource,
    /// The value handed to child git for `GIT_INDEX_FILE`, verbatim.
    raw: Option<OsString>,
    /// Absolute path used for lock checks and diagnostics. The final component
    /// is preserved as spelled; a symlinked index path is not resolved.
    absolute: Option<PathBuf>,
    /// The spelled lock candidate, `<absolute>.lock`. The *physical* endpoint
    /// git locks is derived from the spelling when it is observed (see
    /// [`resolve_final_symlink`]); this field is the candidate, not the answer.
    lock: Option<PathBuf>,
}

impl OperatorIndex {
    /// Capture the ambient selector. Called once, before root normalisation.
    pub(crate) fn capture(probe_root: &Path) -> Self {
        match std::env::var_os("GIT_INDEX_FILE") {
            None => Self {
                source: OperatorIndexSource::Default,
                raw: None,
                absolute: None,
                lock: None,
            },
            Some(raw) => {
                let spelling = Path::new(&raw);
                // Keep the existing captured spelling. join("") retains the
                // trailing separator, so suffixing `.lock` also handles the
                // empty selector without inventing a second separator when the
                // root already has one. Path equality ignores that separator;
                // the byte-level regression control deliberately does not.
                let absolute = if spelling.is_absolute() {
                    spelling.to_path_buf()
                } else {
                    probe_root.join(spelling)
                };
                let lock = lock_path_for(&absolute);
                Self {
                    source: OperatorIndexSource::Environment,
                    raw: Some(raw),
                    absolute: Some(absolute),
                    lock: Some(lock),
                }
            }
        }
    }

    /// Where the index authority came from.
    pub fn source(&self) -> OperatorIndexSource {
        self.source
    }

    /// Whether an inherited `GIT_INDEX_FILE` was captured.
    pub fn is_captured(&self) -> bool {
        self.source == OperatorIndexSource::Environment
    }

    /// The captured value exactly as git must receive it, or `None` when the
    /// default index applies.
    pub fn raw_value(&self) -> Option<&OsStr> {
        self.raw.as_deref()
    }

    /// The absolute index path, or `None` for git's default index.
    ///
    /// The final path component is preserved as spelled; a symlinked index path
    /// is not resolved here. This is the spelling git receives; the physical
    /// lock endpoint derived from it is
    /// [`GitRepository::operator_index_lock_path`](crate::GitRepository::operator_index_lock_path).
    pub fn explicit_path(&self) -> Option<&Path> {
        self.absolute.as_deref()
    }

    /// The spelled lock candidate `<index spelling>.lock`, or `None` for the
    /// default index.
    ///
    /// Do not use this candidate to check or acquire the operator index lock.
    /// It is retained for callers recording the captured spelling, not as a
    /// second lock authority. This is what a path check would name if the final
    /// component were never a symlink. Git resolves a symlinked final component and locks
    /// `<resolved target>.lock`, so the authoritative endpoint is derived at
    /// observation time by [`GitRepository::operator_index_lock_path`](crate::GitRepository::operator_index_lock_path)
    /// through this module's single final-component resolver. The value here is
    /// kept truthful for callers that record the captured spelling; it is not
    /// used as the lock name.
    pub fn explicit_lock_path(&self) -> Option<&Path> {
        self.lock.as_deref()
    }

    /// A synthetic explicit authority, for tests that need two handles with
    /// different captured indexes in one process without mutating the process
    /// environment.
    #[cfg(test)]
    pub(crate) fn explicit_for_test(path: &Path) -> Self {
        Self {
            source: OperatorIndexSource::Environment,
            raw: Some(path.as_os_str().to_os_string()),
            absolute: Some(path.to_path_buf()),
            lock: Some(lock_path_for(path)),
        }
    }
}

/// How many `readlink` attempts git's own lock setup makes on the final path
/// component before it gives up and uses the path it has.
///
/// Measured against host Git 2.43.0 (`git version 2.43.0`): a five-link chain is
/// chased to its end, a six-link chain stops at the sixth path and git then
/// replaces that link with a regular index file, and a two-link loop stops after
/// the same number of dereferences. This is git's observed depth, not a policy
/// invented here; the measured matrix is in
/// `crates/sk-git/INDEX-LOCK-ENDPOINT-REPORT.md`.
const FINAL_COMPONENT_READLINK_LIMIT: usize = 5;

/// Resolve the physical endpoint of a path the way git's lock setup does.
///
/// Git resolves **only the final component**, and it does so on the path as it
/// spells it:
///
/// * a relative symlink target is joined to the current path's parent without
///   normalising (`dir/index` → `../alt/real` becomes `dir/../alt/real`);
/// * an absolute target is used as written;
/// * parent components are never chased (a symlinked directory stays a
///   directory);
/// * at most [`FINAL_COMPONENT_READLINK_LIMIT`] `readlink` calls are made; a
///   still-symlinked path after that is the endpoint;
/// * **any** `readlink` error — `ENOENT` for a missing file or an empty
///   selector, `EINVAL` for a non-symlink or a looped final component,
///   `ENOTDIR` for a non-directory parent, `EACCES` — stops the chase and keeps
///   the current path. Resolution itself never fails, so it never invents a
///   refusal for an input git accepts;
/// * OS bytes are preserved exactly (no UTF-8 round trip), and no lexical
///   normalisation (`.`, `..`, trailing separator) is applied.
///
/// The result is the path git opens and renames, not a hardening decision. It is
/// computed when it is observed, so a symlink retargeted after a repository
/// handle was captured is followed to its new target.
pub(crate) fn resolve_final_symlink(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..FINAL_COMPONENT_READLINK_LIMIT {
        let Ok(target) = fs::read_link(&current) else {
            break;
        };
        current = if target.is_absolute() {
            target
        } else {
            match current.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent.join(target),
                _ => target,
            }
        };
    }
    current
}

/// `<index>.lock`, appended as bytes exactly as git names its lock file.
///
/// Appending (rather than replacing an extension) keeps `index.tar` →
/// `index.tar.lock`, keeps an empty selector's `<root>/` → `<root>/.lock`, and
/// keeps non-UTF-8 spellings byte-identical.
pub(crate) fn lock_path_for(index: &Path) -> PathBuf {
    let mut lock = index.as_os_str().to_os_string();
    lock.push(".lock");
    PathBuf::from(lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("create a temporary root")
    }

    #[test]
    fn a_path_that_is_not_a_symlink_is_its_own_endpoint() {
        let temp = temp_root();
        let plain = temp.path().join("index");
        fs::write(&plain, b"DIRC").unwrap();
        assert_eq!(resolve_final_symlink(&plain), plain);
        assert_eq!(
            resolve_final_symlink(&temp.path().join("absent")),
            temp.path().join("absent")
        );
    }

    #[test]
    fn a_missing_file_and_an_empty_spelling_keep_their_own_path() {
        let temp = temp_root();
        // An empty selector is the work-tree root with its separator; git's
        // rename target is that directory, so the lock is `<root>/.lock`.
        let root_with_separator = PathBuf::from(format!("{}/", temp.path().display()));
        assert_eq!(
            temp.path().join("").as_os_str(),
            root_with_separator.as_os_str()
        );
        #[cfg(unix)]
        assert_eq!(
            Path::new("/").join("index").as_os_str(),
            OsStr::new("/index")
        );
        assert_eq!(
            resolve_final_symlink(&root_with_separator),
            root_with_separator
        );
        assert_eq!(
            lock_path_for(&root_with_separator),
            temp.path().join(".lock")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_relative_target_is_joined_to_the_parent_without_normalisation() {
        let temp = temp_root();
        let current = temp.path().join("dir/index");
        fs::create_dir_all(temp.path().join("dir")).unwrap();
        std::os::unix::fs::symlink("../alt/real", &current).unwrap();
        let target = temp.path().join("alt/real");
        fs::create_dir_all(temp.path().join("alt")).unwrap();
        fs::write(&target, b"DIRC").unwrap();
        assert_eq!(
            resolve_final_symlink(&current),
            temp.path().join("dir/../alt/real"),
            "git joins the target and never normalises the spelling"
        );
        std::os::unix::fs::symlink("./sub/../real", current.with_file_name("dot")).unwrap();
        assert_eq!(
            resolve_final_symlink(&current.with_file_name("dot")),
            temp.path().join("dir/./sub/../real")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_absolute_target_is_used_as_written() {
        let temp = temp_root();
        let target = temp.path().join("real");
        fs::write(&target, b"DIRC").unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_final_symlink(&link), target);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_chain_stops_after_five_dereferences_like_git() {
        let temp = temp_root();
        let target = temp.path().join("real");
        fs::write(&target, b"DIRC").unwrap();
        // Five dereferences resolve fully: index -> 1 -> 2 -> 3 -> 4 -> real.
        for hop in (1..=5).rev() {
            let link = temp.path().join(format!("index{hop}"));
            let next = if hop == 5 {
                PathBuf::from("real")
            } else {
                PathBuf::from(format!("index{}", hop + 1))
            };
            std::os::unix::fs::symlink(next, &link).unwrap();
        }
        assert_eq!(resolve_final_symlink(&temp.path().join("index1")), target);

        // A sixth link is not chased: git stops after five dereferences and
        // replaces the still-symlinked fifth path with a regular index.
        // chain6 -> index1 -> index2 -> index3 -> index4 -> index5 -> real
        let six = temp.path().join("chain6");
        std::os::unix::fs::symlink("index1", &six).unwrap();
        assert_eq!(
            resolve_final_symlink(&six),
            temp.path().join("index5"),
            "the chase stops at git's own depth instead of inventing a larger cap"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_final_component_loop_is_not_chased_forever() {
        let temp = temp_root();
        let first = temp.path().join("a");
        let second = temp.path().join("b");
        std::os::unix::fs::symlink("b", &first).unwrap();
        std::os::unix::fs::symlink("a", &second).unwrap();
        // a -> b -> a -> b -> a -> b: five dereferences land on `b`.
        assert_eq!(resolve_final_symlink(&first), second);
    }

    #[cfg(unix)]
    #[test]
    fn a_parent_directory_symlink_is_never_chased() {
        let temp = temp_root();
        fs::create_dir_all(temp.path().join("realdir")).unwrap();
        std::os::unix::fs::symlink("realdir", temp.path().join("dir")).unwrap();
        let index = temp.path().join("dir/index");
        fs::write(temp.path().join("realdir/index"), b"DIRC").unwrap();
        assert_eq!(resolve_final_symlink(&index), index);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_target_still_resolves_and_a_non_utf8_name_is_preserved() {
        use std::os::unix::ffi::OsStrExt;

        let temp = temp_root();
        let dangling = temp.path().join("dangling");
        std::os::unix::fs::symlink("not-yet", &dangling).unwrap();
        assert_eq!(
            resolve_final_symlink(&dangling),
            temp.path().join("not-yet")
        );

        let raw = std::ffi::OsStr::from_bytes(b"bad\xffidx");
        let non_utf8 = temp.path().join(raw);
        fs::write(&non_utf8, b"DIRC").unwrap();
        let resolved = resolve_final_symlink(&non_utf8);
        assert_eq!(
            resolved.as_os_str().as_bytes(),
            non_utf8.as_os_str().as_bytes()
        );
        let link = temp.path().join("raw-target-link");
        std::os::unix::fs::symlink(raw, &link).unwrap();
        assert_eq!(
            resolve_final_symlink(&link).as_os_str().as_bytes(),
            non_utf8.as_os_str().as_bytes()
        );
    }

    #[test]
    fn the_lock_name_is_appended_not_substituted() {
        assert_eq!(
            lock_path_for(Path::new("/repo/index.tar")),
            PathBuf::from("/repo/index.tar.lock")
        );
        assert_eq!(
            lock_path_for(Path::new("/repo/.git/index")),
            PathBuf::from("/repo/.git/index.lock")
        );
        assert_eq!(
            lock_path_for(Path::new("/repo/./sub/../idx")),
            PathBuf::from("/repo/./sub/../idx.lock")
        );
    }
}
