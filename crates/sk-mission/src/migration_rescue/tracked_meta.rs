//! Source-compatible tracked `meta.json` recovery (Source `mission_metadata`
//! `read_tracked_meta` / `restore_tracked_meta` and `upgrade.tracked_meta_notice`).
//!
//! A Mission whose `meta.json` is tracked by Git but missing from the working
//! tree (an accidental `rm`, a sparse checkout) still has an identity its
//! history uses. Minting a fresh one would fork the Mission, so the repair
//! either restores the tracked copy or REFUSES — never a silent skip and never
//! a fresh identity.
//!
//! The protocol mirrors Source exactly:
//!
//! * the committed copy is `HEAD`'s blob at `<checkout-prefix>meta.json` — what
//!   `git checkout -- <path>` restores;
//! * an index-only copy (staged, never committed) is NOT history's identity;
//! * a committed copy whose index blob differs (a staged edit, or a staged
//!   deletion) must not be overwritten by `HEAD`'s bytes;
//! * a Git that *runs and refuses* (dubious ownership, an unreadable `.git`)
//!   with a `.git` anchor above the mission is "unavailable", never "nothing
//!   tracked";
//! * an unborn `HEAD` holds no identity for any path.
//!
//! Every read goes through the one canonical Git adapter ([`sk_git`]) — there is
//! no second Git process here.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_git::{BlobLookup, GitError, GitPath, GitRepository};

use crate::meta::META_FILE_NAME;
use crate::migration_rescue::RescueError;

/// What Git holds for one Mission's `meta.json`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackedMeta {
    /// The parsed tracked content, when Git returned a usable JSON object.
    pub meta: Option<Value>,
    /// Git knows the path but its content cannot be used — the refusal signal.
    pub unavailable: bool,
    /// The only copy is staged and was never committed.
    pub index_only: bool,
    /// The committed copy exists but is not a JSON object.
    pub corrupt: bool,
    /// The committed copy's index blob differs (a staged edit or deletion).
    pub staged_differs: bool,
}

impl TrackedMeta {
    /// True when a restorable tracked copy exists.
    pub fn is_restorable(&self) -> bool {
        self.meta.is_some() && !self.unavailable
    }
}

/// The repository-relative path of a Mission's `meta.json`, and the repository
/// that owns it.
fn resolve(feature_dir: &Path) -> Result<Option<(GitRepository, GitPath, PathBuf)>, RescueError> {
    let repository = match GitRepository::open(feature_dir) {
        Ok(repository) => repository,
        Err(GitError::NotARepository { .. }) => {
            // Source distinguishes "git positively answered: outside a work tree,
            // and no `.git` anchor above" (nothing tracked) from "git refused"
            // (unavailable). A `.git` anchor above means the answer is unknown.
            if has_git_anchor(feature_dir) {
                return Err(RescueError::Git(
                    "git refused to answer for a checkout with a .git anchor".to_string(),
                ));
            }
            return Ok(None);
        }
        Err(error) => return Err(RescueError::Git(error.to_string())),
    };
    let root = repository.root().to_path_buf();
    let canonical_dir =
        std::fs::canonicalize(feature_dir).unwrap_or_else(|_| feature_dir.to_path_buf());
    let canonical_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let Ok(relative_dir) = canonical_dir.strip_prefix(&canonical_root) else {
        // The mission directory is not inside this work tree: nothing tracked.
        return Ok(None);
    };
    let relative = relative_dir.join(META_FILE_NAME);
    let bytes = relative.as_os_str().as_encoded_bytes().to_vec();
    let path =
        GitPath::from_git_bytes(&bytes).map_err(|error| RescueError::Git(error.to_string()))?;
    Ok(Some((repository, path, relative)))
}

fn has_git_anchor(feature_dir: &Path) -> bool {
    feature_dir
        .ancestors()
        .any(|candidate| candidate.join(".git").exists())
}

/// The index's blob bytes for `path`, or `None` when the index holds none.
fn index_blob_bytes(
    repository: &GitRepository,
    path: &GitPath,
) -> Result<Option<Vec<u8>>, RescueError> {
    let entries = repository
        .index_entries(&[path.as_os_str()])
        .map_err(|error| RescueError::Git(error.to_string()))?;
    // A stage-0 entry is the resolved index copy; conflict stages mean the index
    // is mid-merge and cannot be treated as a clean copy of the file.
    let Some(entry) = entries.iter().find(|entry| entry.stage() == 0) else {
        return Ok(None);
    };
    if entries.iter().any(|entry| entry.stage() != 0) {
        return Ok(None);
    }
    let bytes = repository
        .read_blob_object(entry.oid())
        .map_err(|error| RescueError::Git(error.to_string()))?;
    Ok(Some(bytes))
}

/// Read the tracked `meta.json` of `feature_dir` — the one answer to "what does
/// Git hold".
pub fn read_tracked_meta(feature_dir: &Path) -> Result<TrackedMeta, RescueError> {
    let Some((repository, path, _relative)) = resolve(feature_dir)? else {
        return Ok(TrackedMeta::default());
    };
    let committed = match repository.read_blob_at_head(&path) {
        Ok(BlobLookup::Present(bytes)) => Some(bytes),
        Ok(BlobLookup::Absent) | Ok(BlobLookup::NoRevision) => None,
        Err(error) => return Err(RescueError::Git(error.to_string())),
    };
    let index = index_blob_bytes(&repository, &path)?;
    let Some(committed) = committed else {
        if index.is_some() {
            return Ok(TrackedMeta {
                index_only: true,
                unavailable: true,
                ..TrackedMeta::default()
            });
        }
        return Ok(TrackedMeta::default());
    };
    if index.as_deref() != Some(committed.as_slice()) {
        return Ok(TrackedMeta {
            staged_differs: true,
            unavailable: true,
            ..TrackedMeta::default()
        });
    }
    match serde_json::from_slice::<Value>(&committed) {
        Ok(value) if value.is_object() => Ok(TrackedMeta {
            meta: Some(value),
            ..TrackedMeta::default()
        }),
        Ok(_) | Err(_) => Ok(TrackedMeta {
            corrupt: true,
            unavailable: true,
            ..TrackedMeta::default()
        }),
    }
}

/// Write the tracked `meta.json` Git holds back into the working tree.
///
/// Idempotent and a no-op when the working-tree copy is present or Git holds no
/// usable copy. The write goes through the canonical `meta.json` writer under
/// the Mission lock, like every other metadata mutation.
pub fn restore_tracked_meta(feature_dir: &Path) -> Result<bool, RescueError> {
    if feature_dir.join(META_FILE_NAME).exists() {
        return Ok(false);
    }
    let tracked = read_tracked_meta(feature_dir)?;
    let Some(Value::Object(meta)) = tracked.meta else {
        return Ok(false);
    };
    let lock = sk_status::mutation::acquire_mission_birth_lock(
        feature_dir,
        None,
        sk_kernel::LockWait::Bounded(std::time::Duration::from_secs(30)),
    )
    .map_err(|error| RescueError::Writer(error.to_string()))?;
    let written = (|| {
        if feature_dir.join(META_FILE_NAME).exists() {
            return Ok(false);
        }
        crate::migration_rescue::write_restored_meta(feature_dir, &meta)?;
        Ok(true)
    })();
    drop(lock);
    written
}

/// The one operator notice for a Mission whose tracked `meta.json` is unusable
/// (Source `upgrade.tracked_meta_notice.MissingTrackedMeta`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedMetaNotice {
    /// The operator-facing line.
    pub notice: String,
    /// True when the seam must NOT repair the folder (an identity would be
    /// fabricated).
    pub refuse: bool,
    /// The stable refusal code, when refusing.
    pub code: Option<&'static str>,
}

/// Stable code: git knows the path but cannot produce a usable copy.
pub const CODE_UNAVAILABLE: &str = "TRACKED_META_UNAVAILABLE";

/// Stable code: the index copy differs from the committed one.
pub const CODE_STAGED_DIFFERS: &str = "TRACKED_META_STAGED_DIFFERS";

const LEAD_ABSENT: &str = "is missing from the working tree";
const LEAD_NO_IDENTITY: &str = "has lost its mission_id";

fn display_path(feature_dir: &Path, repo_root: &Path) -> String {
    let relative = feature_dir
        .strip_prefix(repo_root)
        .unwrap_or(feature_dir)
        .join(META_FILE_NAME);
    relative.to_string_lossy().replace('\\', "/")
}

fn restore_notice(path: &str) -> String {
    format!(
        "{path}: tracked meta.json is missing from the working tree; its identity comes from the \
         copy git holds (restore yours with `git checkout -- {path}`)"
    )
}

fn refusal_notice(path: &str, tracked: &TrackedMeta, lead: &str) -> String {
    if tracked.staged_differs {
        return format!(
            "{path}: meta.json {lead} and the index copy differs from the committed one (a staged \
             edit); no identity was minted and your staged edit was not discarded — commit it, or \
             unstage it with `git restore --staged {path}`, then restore the file with \
             `git checkout -- {path}`"
        );
    }
    if tracked.index_only {
        return format!(
            "{path}: meta.json {lead} and git holds it only in the index (not committed); \
             no identity was minted — restore it with `git checkout -- {path}` and commit it"
        );
    }
    if tracked.corrupt {
        return format!(
            "{path}: tracked meta.json {lead} and the committed copy is not a JSON object; \
             no identity was minted — repair the committed copy (`git show HEAD:{path}` shows it), \
             commit the fix, then restore it with `git checkout -- {path}`"
        );
    }
    format!(
        "{path}: tracked meta.json {lead} and git could not produce its content; no identity was \
         minted — restore it with `git checkout -- {path}`"
    )
}

/// Classify `feature_dir`'s missing `meta.json` (Source
/// `classify_missing_tracked_meta`); `None` when there is nothing to report.
///
/// `None` covers both "the working-tree copy is there" and "git holds nothing
/// for the path" (a pre-identity Mission): the existing repair is correct and no
/// notice is owed.
pub fn missing_tracked_meta(
    feature_dir: &Path,
    repo_root: &Path,
) -> Result<Option<TrackedMetaNotice>, RescueError> {
    if feature_dir.join(META_FILE_NAME).exists() {
        return Ok(None);
    }
    let tracked = read_tracked_meta(feature_dir)?;
    let path = display_path(feature_dir, repo_root);
    if tracked.unavailable {
        let code = if tracked.staged_differs {
            CODE_STAGED_DIFFERS
        } else {
            CODE_UNAVAILABLE
        };
        return Ok(Some(TrackedMetaNotice {
            notice: format!("{} [{code}]", refusal_notice(&path, &tracked, LEAD_ABSENT)),
            refuse: true,
            code: Some(code),
        }));
    }
    if tracked.meta.is_none() {
        return Ok(None);
    }
    Ok(Some(TrackedMetaNotice {
        notice: restore_notice(&path),
        refuse: false,
        code: None,
    }))
}

/// The refusal for a PRESENT `meta.json` that lost its `mission_id` while git's
/// copy cannot supply one (Source `tracked_meta_refusal`).
pub fn tracked_meta_refusal(
    feature_dir: &Path,
    repo_root: &Path,
    tracked: &TrackedMeta,
) -> Option<TrackedMetaNotice> {
    if !tracked.unavailable {
        return None;
    }
    let code = if tracked.staged_differs {
        CODE_STAGED_DIFFERS
    } else {
        CODE_UNAVAILABLE
    };
    Some(TrackedMetaNotice {
        notice: format!(
            "{} [{code}]",
            refusal_notice(
                &display_path(feature_dir, repo_root),
                tracked,
                LEAD_NO_IDENTITY
            )
        ),
        refuse: true,
        code: Some(code),
    })
}

/// The checkout root Source uses for the project-relative notice path: the Git
/// worktree root when resolvable, else the Mission directory's grandparent
/// (`<root>/kitty-specs/<slug>` → `<root>`).
pub fn notice_repo_root(feature_dir: &Path) -> PathBuf {
    if let Ok(repository) = GitRepository::open(feature_dir) {
        return repository.root().to_path_buf();
    }
    feature_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| feature_dir.to_path_buf())
}

/// Convenience: the notice for this mission, using [`notice_repo_root`].
pub fn tracked_meta_notice(feature_dir: &Path) -> Result<Option<TrackedMetaNotice>, RescueError> {
    missing_tracked_meta(feature_dir, &notice_repo_root(feature_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository(root: &Path) -> PathBuf {
        let mission = root.join("kitty-specs/mission-t");
        std::fs::create_dir_all(mission.join("tasks")).expect("dirs");
        run_git(root, &["init", "-q"]);
        run_git(root, &["config", "user.email", "b5@example.invalid"]);
        run_git(root, &["config", "user.name", "B5"]);
        mission
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn a_committed_meta_is_read_and_restored() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let mission = repository(root);
        write(
            &mission.join("meta.json"),
            "{\n  \"mission_id\": \"01KJ0000000000000000000MT\"\n}\n",
        );
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "seed"]);
        let tracked = read_tracked_meta(&mission).expect("read");
        assert!(tracked.is_restorable());
        assert_eq!(
            tracked
                .meta
                .as_ref()
                .and_then(|meta| meta.get("mission_id"))
                .and_then(Value::as_str),
            Some("01KJ0000000000000000000MT")
        );
        // Lose the working-tree copy: the notice appears and the restore writes
        // the tracked bytes back.
        std::fs::remove_file(mission.join("meta.json")).expect("rm");
        assert!(tracked_meta_notice(&mission).expect("notice").is_some());
        assert!(restore_tracked_meta(&mission).expect("restore"));
        let restored = std::fs::read_to_string(mission.join("meta.json")).expect("read");
        assert!(restored.contains("01KJ0000000000000000000MT"));
    }

    #[test]
    fn a_staged_edit_makes_the_copy_unavailable() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let mission = repository(root);
        write(
            &mission.join("meta.json"),
            "{\n  \"mission_id\": \"01KJ0000000000000000000MT\"\n}\n",
        );
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "seed"]);
        write(
            &mission.join("meta.json"),
            "{\n  \"mission_id\": \"01KJ0000000000000000000MZ\"\n}\n",
        );
        run_git(root, &["add", "."]);
        std::fs::remove_file(mission.join("meta.json")).expect("rm");
        let tracked = read_tracked_meta(&mission).expect("read");
        assert!(tracked.unavailable);
        assert!(tracked.staged_differs);
        assert!(!tracked.is_restorable());
        assert!(!restore_tracked_meta(&mission).expect("restore"));
        assert!(!mission.join("meta.json").exists());
    }

    #[test]
    fn an_untracked_missing_meta_is_not_unavailable() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let mission = repository(root);
        write(&mission.join("tasks/WP01.md"), "---\n---\n\nbody\n");
        let tracked = read_tracked_meta(&mission).expect("read");
        assert!(!tracked.unavailable);
        assert!(tracked.meta.is_none());
        assert!(tracked_meta_notice(&mission).expect("notice").is_none());
    }

    /// A `.git` entry above the mission that is not a usable repository is a
    /// REFUSAL, never "nothing tracked": Source raises `_GitRefuses` for it so a
    /// caller cannot mint an identity over a copy Git might hold. (The shared VM
    /// carries a stale `/tmp/.git`, which is exactly this shape.)
    #[test]
    fn a_stale_git_anchor_is_a_refusal_not_nothing_tracked() {
        let temp = tempfile::tempdir().expect("temp");
        let mission = temp.path().join("kitty-specs/mission-t");
        std::fs::create_dir_all(&mission).expect("dirs");
        let error = read_tracked_meta(&mission).expect_err("refusal");
        assert!(error.to_string().contains("git refused"), "{error}");
    }
}
