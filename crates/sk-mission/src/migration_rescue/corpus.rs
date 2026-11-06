//! The ONE canonical `kitty-specs/` Mission/residue classifier for the four
//! rescue migrations (Source `context/mission_resolver.py` + `runner`).
//!
//! Source has **three** residues senses and this module keeps them separate
//! instead of collapsing them into one filter (glossary `#residue`):
//!
//! | sense | Source predicate | used by |
//! | --- | --- | --- |
//! | write seam | `is_kitty_specs_residue` | the `3.0.0` derived-file walk, the `3.2.0a4` candidate walk, the runtime cutover |
//! | population | `_discover_features` (`meta.json` **on disk**) | the `3.0.0` identity/ownership/rebuild/strip walk |
//! | cutover | `_iter_mission_dirs` (every directory) + per-mission residue | `runtime_state_backfill` |
//!
//! A generic "list every `kitty-specs/` child" helper is deliberately **not**
//! changed here: other migrations' Source semantics differ (some Source walks
//! really do visit every directory), so each caller asks for the view it needs
//! through [`MissionCorpus`] instead of having a global filter imposed on it.
//!
//! Nothing here invents identity, history, anchors or paths: the classifier
//! reads files and the canonical Git index ([`sk_git::GitRepository::index_entries`]),
//! and every operator-facing notice is Source's own text.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sk_git::{GitError, GitRepository};

use crate::meta::{META_FILE_NAME, SPECS_DIR_NAME};

/// Files whose presence on disk keeps a folder out of residue (Source
/// `_MISSION_MARKER_FILES`).
const MARKER_FILES: [&str; 2] = ["spec.md", META_FILE_NAME];

/// Files whose presence keeps a folder out of residue when git cannot decide
/// (Source `_MISSION_CONTENT`).
const MISSION_CONTENT: [&str; 8] = [
    "spec.md",
    META_FILE_NAME,
    "plan.md",
    "tasks.md",
    "tasks",
    "status.events.jsonl",
    "status.json",
    "lanes.json",
];

/// The one operator-facing text for a residue directory (Source `RESIDUE_NOTICE`).
pub const RESIDUE_NOTICE: &str = "not a Mission (residue directory {path}: no mission_id, and no spec.md or meta.json on disk or in git); left untouched. To make it a Mission, add a meta.json with a mission_id (for example, re-create it with `spec-kitty agent mission create`), or remove the folder.";

/// Render [`RESIDUE_NOTICE`] for a relative POSIX path (Source `residue_notice`).
pub fn residue_notice(relative: &str) -> String {
    RESIDUE_NOTICE.replace("{path}", relative)
}

/// The residue notice for one mission directory, rendered against the checkout
/// root the way Source renders it (`kitty-specs/<slug>`).
pub fn residue_notice_for(repo_root: &Path, feature_dir: &Path) -> String {
    let relative = feature_dir
        .strip_prefix(repo_root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| {
            format!(
                "{SPECS_DIR_NAME}/{}",
                feature_dir
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        });
    residue_notice(&relative)
}

/// What one `kitty-specs/` child is, for the walk that is asking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissionClass {
    /// A Mission for this view.
    Mission,
    /// Residue: leave it untouched and report Source's notice.
    Residue,
}

/// One `kitty-specs/` child with its classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorpusEntry {
    /// Absolute directory path.
    pub path: PathBuf,
    /// The directory name (the slug Source reports).
    pub slug: String,
    /// The view's classification.
    pub class: MissionClass,
}

impl CorpusEntry {
    /// True when this entry is a Mission for the asking view.
    pub fn is_mission(&self) -> bool {
        self.class == MissionClass::Mission
    }
}

/// The `tracked=` probe result (Source `frozenset | None`).
///
/// `None` means "probed, no usable git" — the file-presence mode. A checkout
/// outside a work tree is also `None`; an unusable index degrades to `None`
/// with one warning, exactly like Source's `probe_tracked_mission_paths`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackedPaths {
    paths: Option<BTreeSet<String>>,
}

impl TrackedPaths {
    /// The git-aware mode: git tracks at least one path under `kitty-specs/`.
    fn is_git_aware(&self) -> bool {
        self.paths.as_ref().is_some_and(|paths| !paths.is_empty())
    }

    /// Whether `path` (relative to the checkout root, `/`-joined) is tracked.
    fn contains(&self, path: &str) -> bool {
        self.paths
            .as_ref()
            .is_some_and(|paths| paths.contains(path))
    }
}

/// One walk over a `kitty-specs/` corpus, with the per-walk Git probe memo.
///
/// A walker builds one of these and asks for the view it needs. The probe runs
/// lazily and at most once (Source `TrackedMissionPaths`), so a corpus where
/// every child carries an identity makes no Git call at all.
pub struct MissionCorpus<'a> {
    repo_root: &'a Path,
    specs_dir: PathBuf,
    tracked: std::cell::OnceCell<TrackedPaths>,
}

impl<'a> MissionCorpus<'a> {
    /// Open the corpus under `repo_root/kitty-specs` without probing Git yet.
    pub fn new(repo_root: &'a Path) -> Self {
        Self {
            repo_root,
            specs_dir: repo_root.join(SPECS_DIR_NAME),
            tracked: std::cell::OnceCell::new(),
        }
    }

    /// The checkout root this corpus belongs to.
    pub fn repo_root(&self) -> &'a Path {
        self.repo_root
    }

    /// Every direct child directory, sorted by name (Source `_iter_mission_dirs`).
    ///
    /// An absent `kitty-specs/` is an empty corpus; an unreadable one is an
    /// error the caller must surface (Source's `iterdir` raises).
    pub fn children(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        let entries = match std::fs::read_dir(&self.specs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut dirs: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
        dirs.sort();
        Ok(dirs)
    }

    /// The `3.0.0` population (Source `_discover_features`): a child directory
    /// holding `meta.json` **on disk**. A tracked-but-absent `meta.json` is not
    /// in this population — `3.0.0` never restores it.
    pub fn discover_features(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        Ok(self
            .children()?
            .into_iter()
            .filter(|path| path.join(META_FILE_NAME).exists())
            .collect())
    }

    /// The `3.0.0` derived-file walk (Source `_move_derived_files`): every child
    /// directory, skipping residue before the `status.json` read.
    pub fn derived_file_sources(&self) -> Result<Vec<CorpusEntry>, std::io::Error> {
        self.classified()
    }

    /// The `3.2.0a4` candidate walk (Source `_discover_candidates`): every child
    /// directory; residue is reported, never given an identity.
    pub fn lifecycle_candidates(&self) -> Result<Vec<CorpusEntry>, std::io::Error> {
        self.classified()
    }

    /// The cutover walk (Source `_iter_mission_dirs` + `cutover_mission`): every
    /// child directory; a residue child returns its residue answer before any
    /// seed or flip, so no `meta.json`/event-log stub is ever created for it.
    pub fn cutover_candidates(&self) -> Result<Vec<CorpusEntry>, std::io::Error> {
        self.classified()
    }

    /// Every child directory classified by the write-seam predicate.
    fn classified(&self) -> Result<Vec<CorpusEntry>, std::io::Error> {
        let mut entries = Vec::new();
        for path in self.children()? {
            let slug = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let class = if self.is_residue(&path) {
                MissionClass::Residue
            } else {
                MissionClass::Mission
            };
            entries.push(CorpusEntry { path, slug, class });
        }
        Ok(entries)
    }

    /// Source `is_kitty_specs_residue` — the write-seam rule.
    pub fn is_residue(&self, path: &Path) -> bool {
        if path.parent().and_then(Path::file_name) != Some(OsStr::new(SPECS_DIR_NAME))
            || markers_exist(path)
        {
            return false;
        }
        let tracked = self.tracked();
        if tracked.is_git_aware() {
            return !self.is_mission_dir(path, tracked);
        }
        if MISSION_CONTENT.iter().any(|name| path.join(name).exists()) {
            return false;
        }
        !self.is_mission_dir(path, tracked)
    }

    /// Source `is_mission_dir`: an identity-bearing `meta.json`, or a tracked
    /// marker (`spec.md` / `meta.json`); file presence when git cannot answer.
    pub fn is_mission_dir(&self, path: &Path, tracked: &TrackedPaths) -> bool {
        if carries_identity(path) {
            return true;
        }
        self.markers_tracked(path, tracked)
    }

    /// Source `_markers_tracked`.
    fn markers_tracked(&self, path: &Path, tracked: &TrackedPaths) -> bool {
        let mut relatives: Vec<String> = Vec::new();
        for name in MARKER_FILES {
            match relative_from(self.repo_root, &path.join(name)) {
                Some(relative) => relatives.push(relative),
                None => return markers_exist(path),
            }
        }
        if tracked.paths.is_some() {
            return relatives.iter().any(|relative| tracked.contains(relative));
        }
        if self.git_prefix().is_none() {
            return markers_exist(path);
        }
        self.tracked_paths_set()
            .is_some_and(|paths| relatives.iter().any(|relative| paths.contains(relative)))
    }

    /// The memoized `tracked=` probe (Source `probe_tracked_mission_paths`).
    pub fn tracked(&self) -> &TrackedPaths {
        self.tracked.get_or_init(|| TrackedPaths {
            paths: self.tracked_paths_set(),
        })
    }

    /// One batch `git ls-files -- kitty-specs` read, checkout-relative.
    fn tracked_paths_set(&self) -> Option<BTreeSet<String>> {
        let repository = GitRepository::open(self.repo_root).ok()?;
        let root = repository.root().to_path_buf();
        let prefix = relative_from(&root, self.repo_root).unwrap_or_default();
        let specifications = OsStr::new(SPECS_DIR_NAME);
        let entries = repository.index_entries(&[specifications]).ok()?;
        let mut paths = BTreeSet::new();
        for entry in entries {
            let relative = entry
                .path()
                .as_os_str()
                .to_string_lossy()
                .replace('\\', "/");
            let relative = match relative.strip_prefix(&prefix) {
                Some(stripped) => stripped.to_string(),
                None if prefix.is_empty() => relative,
                None => continue,
            };
            paths.insert(relative);
        }
        Some(paths)
    }

    /// `git rev-parse --show-prefix` equivalent: `None` when the checkout root
    /// is not inside a work tree, `Some("")` at the toplevel.
    fn git_prefix(&self) -> Option<String> {
        match GitRepository::open(self.repo_root) {
            Ok(repository) => {
                let root = repository.root().to_path_buf();
                Some(relative_from(&root, self.repo_root).unwrap_or_default())
            }
            Err(GitError::NotARepository { .. }) => None,
            // A git that runs and refuses: Source re-raises via
            // `_git_prefix`. There is no usable prefix, so the caller degrades
            // to file presence rather than inventing one.
            Err(_) => None,
        }
    }
}

/// `checks`/`root` as a POSIX-joined relative path (empty at the root).
fn relative_from(root: &Path, path: &Path) -> Option<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let relative = path.strip_prefix(&root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// Source `_markers_exist`.
fn markers_exist(path: &Path) -> bool {
    MARKER_FILES.iter().any(|name| path.join(name).exists())
}

/// Source `_carries_identity`: a `meta.json` that parses and holds a non-empty
/// string `mission_id`. A malformed copy carries no identity (it is not an
/// error for the classifier; the migration's own fail-closed reader decides).
pub fn carries_identity(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path.join(META_FILE_NAME)) else {
        return false;
    };
    let Ok(serde_json::Value::Object(meta)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return false;
    };
    meta.get("mission_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

/// True when the directory holds `meta.json` on disk (the `3.0.0` population
/// rule, exposed for callers that already have one mission path).
pub fn has_meta_on_disk(feature_dir: &Path) -> bool {
    feature_dir.join(META_FILE_NAME).exists()
}

/// The checkout root of a `kitty-specs/<slug>` mission directory.
pub fn repo_root_for(feature_dir: &Path) -> PathBuf {
    feature_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| feature_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "B7")
            .env("GIT_AUTHOR_EMAIL", "b7@example.invalid")
            .env("GIT_COMMITTER_NAME", "B7")
            .env("GIT_COMMITTER_EMAIL", "b7@example.invalid")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo(root: &Path) {
        git(root, &["init", "-q"]);
        git(root, &["config", "user.name", "B7"]);
        git(root, &["config", "user.email", "b7@example.invalid"]);
    }

    fn child(root: &Path, slug: &str) -> PathBuf {
        let dir = root.join(SPECS_DIR_NAME).join(slug);
        std::fs::create_dir_all(dir.join(crate::migration_rescue::TASKS_DIR_NAME)).expect("dirs");
        std::fs::write(dir.join("tasks/WP01.md"), "---\nlane: planned\n---\nbody\n").expect("wp");
        dir
    }

    #[test]
    fn git_aware_mode_classifies_a_marker_less_folder_as_residue() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        repo(root);
        let mission = child(root, "mission-a");
        std::fs::write(mission.join(META_FILE_NAME), "{\"mission_id\": \"01A\"}\n").expect("meta");
        let residue = child(root, "notes");
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "corpus"]);

        let corpus = MissionCorpus::new(root);
        assert!(corpus.is_residue(&residue), "tracked-notes must be residue");
        assert!(!corpus.is_residue(&mission));
        assert_eq!(corpus.discover_features().expect("discover").len(), 1);
        let entries = corpus.lifecycle_candidates().expect("candidates");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.iter().filter(|entry| entry.is_mission()).count(), 1);
        assert!(
            residue_notice_for(root, &residue)
                .starts_with("not a Mission (residue directory kitty-specs/notes:"),
            "{}",
            residue_notice_for(root, &residue)
        );
    }

    #[test]
    fn file_presence_mode_keeps_a_legacy_folder_repairable() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        // No git at all: the existence rule applies.
        let legacy = child(root, "legacy");
        let corpus = MissionCorpus::new(root);
        assert!(!corpus.is_residue(&legacy));
        let empty = root.join(SPECS_DIR_NAME).join("empty");
        std::fs::create_dir_all(&empty).expect("empty");
        assert!(corpus.is_residue(&empty));
    }

    #[test]
    fn identity_needs_a_non_empty_string_mission_id() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = temp.path().join(SPECS_DIR_NAME).join("mission-a");
        std::fs::create_dir_all(&dir).expect("dirs");
        std::fs::write(dir.join(META_FILE_NAME), "{\"mission_id\": \"\"}\n").expect("meta");
        assert!(!carries_identity(&dir));
        std::fs::write(dir.join(META_FILE_NAME), "{\"mission_id\": \"01A\"}\n").expect("meta");
        assert!(carries_identity(&dir));
    }

    #[test]
    fn discover_features_requires_meta_json_on_disk() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let with_meta = child(root, "with-meta");
        std::fs::write(with_meta.join(META_FILE_NAME), "{}\n").expect("meta");
        let _without = child(root, "without-meta");
        let found = MissionCorpus::new(root)
            .discover_features()
            .expect("discover");
        assert_eq!(found, vec![with_meta]);
    }
}
