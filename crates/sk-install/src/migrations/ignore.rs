//! The `.gitignore` effect surface shared by the ignore-entry migrations.
//!
//! Upstream routes every entry backfill through `GitignoreManager` plus
//! `read_ignore_file_text` / `write_gitignore_text`, and the two removal
//! migrations through a line filter with their own read/write calls. This
//! module is the native equivalent of that seam so each migration owns only
//! its *entries* and its *predicate*, not a private copy of the file format.
//!
//! Contracts preserved here (see `gitignore_manager.py` at the pinned
//! baseline):
//!
//! - `read_ignore_file_text` reads UTF-8 with the BOM stripped, refuses to
//!   follow a final symlink, and returns `""` for an absent file.
//! - Some callers pass `errors="ignore"`, which drops undecodable bytes; the
//!   strict callers treat them as a typed refusal. Both modes are offered.
//! - `GitignoreManager.ensure_entries` compares **whole lines** (never
//!   substrings), inserts the auto-managed marker once, keeps one blank line
//!   before the marker and one trailing newline, preserves the file's CRLF
//!   style, and appends every requested entry when `force_append` is set.
//! - `write_gitignore_text` replaces the file atomically, keeps an existing
//!   mode, refuses a symlink, and gives a brand-new file the mode an ordinary
//!   `open(..., "w")` under the process umask would have produced.
//!
//! Deviation, deliberate and fail-closed: the pinned `0.4.8` migration writes
//! through a symlinked `.gitignore` (`Path.write_text`), and its sibling reads
//! do too. Native code never follows one; a symlink is a typed
//! [`InstallError::IgnoreFileUnsafe`] refusal. `docs/rust/DECISIONS.md`
//! requires untrusted paths to be rejected at the owning boundary, and a
//! migration must not be the one place that clobbers an arbitrary path.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::InstallError;

/// The marker line `GitignoreManager` writes above its entries.
pub const MARKER: &str = "# Added by Spec Kitty CLI (auto-managed)";

/// Bounded read for an ignore file (upstream reads it whole; this caps abuse).
const MAX_IGNORE_BYTES: u64 = 8 * 1024 * 1024;

/// How undecodable bytes are handled, matching the two upstream read helpers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decode {
    /// Invalid UTF-8 is a typed refusal.
    Strict,
    /// `errors="ignore"`: undecodable bytes are dropped.
    IgnoreInvalid,
}

/// How line endings are handled, matching the two upstream read helpers.
///
/// The pinned module has both shapes and they are not interchangeable:
/// `read_gitignore_text` opens with `newline=""` (endings preserved, which is
/// how `ensure_entries` can detect and keep CRLF), while
/// `read_ignore_file_text` opens with the default `newline=None` (universal
/// newlines: `\r\n` and lone `\r` both arrive as `\n`). A migration that
/// rewrote a CRLF file through the second helper therefore writes LF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Newlines {
    /// `newline=""`: byte-preserving endings.
    Preserve,
    /// `newline=None`: universal-newline translation.
    Universal,
}

/// The `.gitignore` of one project.
#[derive(Clone, Debug)]
pub struct IgnoreFile {
    path: PathBuf,
}

impl IgnoreFile {
    /// The ignore file of `project_root`.
    pub fn new(project_root: &Path) -> Self {
        Self {
            path: project_root.join(".gitignore"),
        }
    }

    /// The on-disk path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the file, refusing to follow a final symlink.
    ///
    /// `Ok(None)` means the file does not exist (upstream's `read_gitignore_text`
    /// returns `None`); `Ok(Some(""))` is an existing empty file.
    pub fn read(&self, decode: Decode, newlines: Newlines) -> Result<Option<String>, InstallError> {
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(InstallError::io("inspect", &self.path, source)),
        };
        if metadata.file_type().is_symlink() {
            return Err(
                self.unsafe_file("ignore file is a symbolic link; refusing to read through it")
            );
        }
        if !metadata.file_type().is_file() {
            return Err(self.unsafe_file("ignore file is not a regular file"));
        }
        let raw = sk_kernel::read_bounded(&self.path, MAX_IGNORE_BYTES)
            .map_err(|error| InstallError::io("read", &self.path, std::io::Error::other(error)))?;
        let raw = strip_bom(&raw);
        let text = match decode {
            Decode::Strict => String::from_utf8(raw.to_vec()).map_err(|error| {
                self.unsafe_file(&format!("ignore file is not valid UTF-8: {error}"))
            })?,
            Decode::IgnoreInvalid => decode_dropping_invalid(raw),
        };
        let text = match newlines {
            Newlines::Preserve => text,
            Newlines::Universal => universal_newlines(&text),
        };
        Ok(Some(text))
    }

    /// Read with the strict decoder, treating an absent file as empty text.
    pub fn read_strict_or_empty(
        &self,
        decode: Decode,
        newlines: Newlines,
    ) -> Result<String, InstallError> {
        Ok(self.read(decode, newlines)?.unwrap_or_default())
    }

    /// Atomically replace the file (creating it when absent).
    pub fn write(&self, text: &str) -> Result<(), InstallError> {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.file_type().is_symlink()
        {
            return Err(self.unsafe_file("ignore file is a symbolic link; refusing to replace it"));
        }
        let existed = self.path.exists();
        if !existed {
            // Give a brand-new file the mode an ordinary `open(path, "w")` would
            // produce under the process umask. `atomic_write` preserves an
            // existing mode, so creating it first is what carries that mode into
            // the replacement (a bare temporary file lands at 0600).
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.path)
            {
                Ok(file) => {
                    drop(file);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => return Err(InstallError::io("create", &self.path, source)),
            }
        }
        match sk_kernel::atomic_write(&self.path, text.as_bytes()) {
            Ok(_) => Ok(()),
            Err(error) => {
                if !existed {
                    let _ = std::fs::remove_file(&self.path);
                }
                Err(InstallError::io(
                    "write ignore file",
                    &self.path,
                    std::io::Error::other(error),
                ))
            }
        }
    }

    /// Append `content` after the existing text, keeping one separating newline.
    ///
    /// This is the raw append the pinned `0.4.8` migration performs (it builds
    /// its own comment block instead of using the manager).
    pub fn append(&self, content: &str) -> Result<(), InstallError> {
        let mut existing = self
            .read(Decode::IgnoreInvalid, Newlines::Universal)?
            .unwrap_or_default();
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(content);
        self.write(&existing)
    }

    fn unsafe_file(&self, detail: &str) -> InstallError {
        InstallError::IgnoreFileUnsafe {
            path: self.path.clone(),
            detail: detail.to_string(),
        }
    }
}

/// The `GitignoreManager.ensure_entries` effect for `project_root`.
///
/// Returns whether the file changed. `force_append` re-asserts every requested
/// entry at EOF even when the same text already occurs earlier (upstream's
/// documented way to defeat a later negation without deleting operator rules).
pub fn ensure_entries(
    project_root: &Path,
    entries: &[&str],
    force_append: bool,
) -> Result<bool, InstallError> {
    if entries.is_empty() {
        return Ok(false);
    }
    let file = IgnoreFile::new(project_root);
    let content = file.read(Decode::Strict, Newlines::Preserve)?;
    let line_ending = match &content {
        Some(text) if text.contains("\r\n") => "\r\n",
        _ => "\n",
    };
    let mut lines: Vec<String> = content
        .as_deref()
        .map(|text| splitlines(text).into_iter().map(str::to_string).collect())
        .unwrap_or_default();
    let mut existing: BTreeSet<String> = lines.iter().cloned().collect();
    let mut changed = false;
    if force_append || entries.iter().any(|entry| !existing.contains(*entry)) {
        if !existing.contains(MARKER) {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(MARKER.to_string());
            existing.insert(MARKER.to_string());
            changed = true;
        }
        for entry in entries {
            if force_append || !existing.contains(*entry) {
                lines.push((*entry).to_string());
                existing.insert((*entry).to_string());
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(false);
    }
    if lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
    }
    file.write(&lines.join(line_ending))?;
    Ok(true)
}

/// Python `str.splitlines()`: split on every line boundary, dropping terminators.
///
/// Rust's `str::lines` only breaks on `\n`; the pinned helpers use
/// `splitlines()`, which also breaks on `\r`, `\x0b`, `\x0c`, `\x1c`-`\x1e`,
/// `\u{85}`, `\u{2028}` and `\u{2029}`. The difference shows up in a file with
/// classic-Mac line endings, and the removal migrations must number and join
/// those lines exactly as the source does.
pub fn splitlines(content: &str) -> Vec<&str> {
    split_lines_keepends(content)
        .into_iter()
        .map(|line| {
            line.strip_suffix("\r\n")
                .or_else(|| line.strip_suffix('\n'))
                .or_else(|| line.strip_suffix('\r'))
                .or_else(|| line.strip_suffix('\u{b}'))
                .or_else(|| line.strip_suffix('\u{c}'))
                .or_else(|| line.strip_suffix('\u{1c}'))
                .or_else(|| line.strip_suffix('\u{1d}'))
                .or_else(|| line.strip_suffix('\u{1e}'))
                .or_else(|| line.strip_suffix('\u{85}'))
                .or_else(|| line.strip_suffix('\u{2028}'))
                .or_else(|| line.strip_suffix('\u{2029}'))
                .unwrap_or(line)
        })
        .collect()
}

/// Python `str.splitlines(keepends=True)`.
pub fn split_lines_keepends(content: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut characters = content.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        let boundary = matches!(
            character,
            '\n' | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        let carriage_return = character == '\r';
        if !boundary && !carriage_return {
            continue;
        }
        let mut end = index + character.len_utf8();
        if carriage_return && let Some((next_index, '\n')) = characters.peek().copied() {
            end = next_index + 1;
            characters.next();
        }
        lines.push(&content[start..end]);
        start = end;
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

/// One line a removal predicate dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovedLine {
    /// 1-based line number, as the source reports it to the operator.
    pub number: usize,
    /// The line without its terminator, trimmed the way the source reports it.
    pub text: String,
}

/// The stripped, comment-free entry set the backfill `detect()` predicates use.
pub fn entry_set(content: &str) -> BTreeSet<String> {
    splitlines(content)
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Read the project `.gitignore` as an entry set (absent file → empty set).
///
/// `newlines` follows the helper the pinned migration calls: the removal and
/// backfill `detect()` predicates read through `read_ignore_file_text`
/// (universal), while the lint backfill reads through `read_gitignore_text`
/// (preserved).
pub fn read_entry_set(
    project_root: &Path,
    decode: Decode,
    newlines: Newlines,
) -> Result<BTreeSet<String>, InstallError> {
    Ok(entry_set(
        &IgnoreFile::new(project_root)
            .read(decode, newlines)?
            .unwrap_or_default(),
    ))
}

/// Remove every line matching `predicate`, collapsing runs of blank lines.
///
/// This is the pinned removal shape: filter the lines (keeping their original
/// endings), then rewrite with three or more consecutive newlines collapsed to
/// two. Returns the dropped lines, in document order, with their 1-based
/// numbers so the caller can reproduce the source's per-line change messages.
pub fn remove_lines(
    file: &IgnoreFile,
    predicate: impl Fn(&str) -> bool,
) -> Result<Vec<RemovedLine>, InstallError> {
    let Some(content) = file.read(Decode::IgnoreInvalid, Newlines::Universal)? else {
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    let mut kept = String::with_capacity(content.len());
    for (index, line) in split_lines_keepends(&content).into_iter().enumerate() {
        if predicate(line) {
            removed.push(RemovedLine {
                number: index + 1,
                text: line.trim().to_string(),
            });
        } else {
            kept.push_str(line);
        }
    }
    if removed.is_empty() {
        return Ok(removed);
    }
    file.write(&collapse_blank_runs(&kept))?;
    Ok(removed)
}

/// `re.sub(r"\n{3,}", "\n\n", text)`, without a regex engine.
fn collapse_blank_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newline_run = 0usize;
    for character in text.chars() {
        if character == '\n' {
            newline_run += 1;
            if newline_run > 2 {
                continue;
            }
        } else {
            newline_run = 0;
        }
        out.push(character);
    }
    out
}

/// Whether a managed root is safely ignored as a whole
/// (`is_gitignore_root_effectively_ignored`).
///
/// Git can adjudicate one concrete probe, but a later, narrower negation may
/// re-include another descendant while leaving that probe ignored. So this
/// requires an equivalent whole-root rule, rejects every later negation
/// conservatively, and only then trusts Git's verdict.
pub fn root_effectively_ignored(
    project_root: &Path,
    root_path: &str,
    equivalent_entries: &[&str],
    probe_path: &str,
) -> Result<bool, InstallError> {
    let root = project_root.join(root_path);
    match std::fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(InstallError::IgnoreFileUnsafe {
                path: root.clone(),
                detail: "refusing symlinked managed root".to_string(),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(InstallError::IgnoreFileUnsafe {
                path: root.clone(),
                detail: "managed root is not a directory".to_string(),
            });
        }
        _ => {}
    }

    let work_tree = git_work_tree_state(project_root)?;
    if work_tree == Some(true) {
        let tracked = super::git::success_stdout(
            project_root,
            &["ls-files", "--full-name", "-z", "--", root_path],
        )?;
        let tracked = match tracked {
            Some(output) if !output.is_empty() => true,
            Some(_) => false,
            None => {
                return Err(InstallError::UpgradeRefused {
                    detail: format!("Could not inspect tracked paths under {root_path}"),
                });
            }
        };
        if tracked {
            return Err(InstallError::UpgradeRefused {
                detail: format!(
                    "Tracked paths exist under {root_path}; .gitignore cannot protect tracked files. \
                     Review them, then untrack with `git rm -r --cached -- {root_path}` and rerun \
                     `spec-kitty upgrade` (or `spec-kitty init` during initialization)."
                ),
            });
        }
    }

    let content =
        IgnoreFile::new(project_root).read_strict_or_empty(Decode::Strict, Newlines::Universal)?;
    if content.is_empty() {
        return Ok(false);
    }
    let entries: Vec<&str> = splitlines(&content)
        .into_iter()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let Some(last_positive) = entries
        .iter()
        .rposition(|entry| equivalent_entries.contains(entry))
    else {
        return Ok(false);
    };
    if entries[last_positive + 1..]
        .iter()
        .any(|entry| entry.starts_with('!'))
    {
        return Ok(false);
    }
    if work_tree != Some(true) {
        return Ok(true);
    }
    super::git::check_ignore(project_root, probe_path)?.ok_or_else(|| {
        InstallError::UpgradeRefused {
            detail: "Git could not verify the managed root ignore state".to_string(),
        }
    })
}

/// `_git_work_tree_state`: `Some(true)`/`Some(false)`, or a typed refusal when
/// Git failed but this path looks like a repository.
fn git_work_tree_state(project_root: &Path) -> Result<Option<bool>, InstallError> {
    match super::git::success_stdout(project_root, &["rev-parse", "--is-inside-work-tree"])? {
        Some(output) if output == "true" => Ok(Some(true)),
        Some(_) => Ok(Some(false)),
        None => {
            if has_git_control_path(project_root) {
                Err(InstallError::UpgradeRefused {
                    detail: "Git could not verify the managed root ignore state".to_string(),
                })
            } else {
                Ok(Some(false))
            }
        }
    }
}

fn has_git_control_path(project_root: &Path) -> bool {
    project_root.ancestors().any(|candidate| {
        let dot_git = candidate.join(".git");
        dot_git.is_file() || (dot_git.is_dir() && dot_git.join("HEAD").is_file())
    })
}

/// Whether `relative` is tracked in the index (`kernel.git.is_tracked`).
///
/// A probe that cannot run is advisory here: it answers `false`, exactly as the
/// pinned migration's wrapper does, so hygiene never fails on an untracked
/// non-repository.
pub fn is_tracked(project_root: &Path, relative: &str) -> bool {
    match super::git::run(
        project_root,
        &[
            "--literal-pathspecs",
            "ls-files",
            "--error-unmatch",
            "--",
            relative,
        ],
    ) {
        Ok(output) => output.status.code() == Some(0),
        Err(_) => false,
    }
}

/// `git rm --cached -- <relative>`; `false` when Git refused.
pub fn untrack(project_root: &Path, relative: &str) -> bool {
    match super::git::run(project_root, &["rm", "--cached", "--", relative]) {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Python's universal-newline read translation (`newline=None`).
fn universal_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            out.push('\n');
        } else {
            out.push(character);
        }
    }
    out
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// Decode UTF-8 dropping undecodable sequences (Python `errors="ignore"`).
fn decode_dropping_invalid(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                return text;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                // `valid_up_to` is a boundary by construction.
                text.push_str(std::str::from_utf8(valid).unwrap_or_default());
                let skip = error.error_len().unwrap_or(after.len()).max(1);
                rest = &after[skip.min(after.len())..];
                if rest.is_empty() {
                    return text;
                }
            }
        }
    }
}

/// Write `content` to a fresh `.gitignore` for tests and callers that already
/// know the path is safe.
pub fn write_new(project_root: &Path, content: &str) -> Result<(), InstallError> {
    let file = IgnoreFile::new(project_root);
    let mut handle = std::fs::File::create(file.path())
        .map_err(|source| InstallError::io("create", file.path(), source))?;
    handle
        .write_all(content.as_bytes())
        .map_err(|source| InstallError::io("write", file.path(), source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp")
    }

    #[test]
    fn ensure_adds_marker_once_blank_line_and_trailing_newline() {
        let directory = project();
        let root = directory.path();
        std::fs::write(root.join(".gitignore"), "node_modules/").unwrap();
        assert!(ensure_entries(root, &["a/", "b/"], false).unwrap());
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(text, format!("node_modules/\n\n{MARKER}\na/\nb/\n"));
        assert!(!ensure_entries(root, &["a/", "b/"], false).unwrap());
    }

    #[test]
    fn ensure_preserves_crlf_and_is_idempotent() {
        let directory = project();
        let root = directory.path();
        std::fs::write(root.join(".gitignore"), "keep\r\n").unwrap();
        assert!(ensure_entries(root, &["a/"], false).unwrap());
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(text, format!("keep\r\n\r\n{MARKER}\r\na/\r\n"));
        assert!(!ensure_entries(root, &["a/"], false).unwrap());
    }

    #[test]
    fn force_append_reasserts_after_a_negation() {
        let directory = project();
        let root = directory.path();
        std::fs::write(root.join(".gitignore"), ".worktrees/\n!.worktrees/keep/\n").unwrap();
        assert!(ensure_entries(root, &[".worktrees/"], true).unwrap());
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(
            text,
            format!(".worktrees/\n!.worktrees/keep/\n\n{MARKER}\n.worktrees/\n")
        );
    }

    #[test]
    fn a_symlinked_ignore_file_is_refused_and_left_alone() {
        let directory = project();
        let root = directory.path();
        let target = root.join("elsewhere");
        std::fs::write(&target, "x\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, root.join(".gitignore")).unwrap();
        let error = ensure_entries(root, &["a/"], false).expect_err("refused");
        assert_eq!(error.code(), "ignore_file_unsafe");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x\n");
    }

    #[test]
    fn invalid_bytes_are_dropped_only_in_the_lossy_mode() {
        let directory = project();
        let root = directory.path();
        std::fs::write(root.join(".gitignore"), b"ok\n\xffbad\n").unwrap();
        let lossy = IgnoreFile::new(root)
            .read(Decode::IgnoreInvalid, Newlines::Preserve)
            .unwrap();
        assert_eq!(lossy.as_deref(), Some("ok\nbad\n"));
        let error = IgnoreFile::new(root)
            .read(Decode::Strict, Newlines::Preserve)
            .expect_err("strict");
        assert_eq!(error.code(), "ignore_file_unsafe");
    }

    #[test]
    fn removal_keeps_line_endings_and_collapses_blank_runs() {
        let directory = project();
        let root = directory.path();
        std::fs::write(
            root.join(".gitignore"),
            "a\nkitty-specs/\n\n\n\nb\nkitty-specs/\n",
        )
        .unwrap();
        let file = IgnoreFile::new(root);
        let removed = remove_lines(&file, |line| line.trim() == "kitty-specs/").unwrap();
        assert_eq!(removed.len(), 2);
        assert_eq!(
            removed[0],
            RemovedLine {
                number: 2,
                text: "kitty-specs/".to_string()
            }
        );
        assert_eq!(
            removed[1],
            RemovedLine {
                number: 7,
                text: "kitty-specs/".to_string()
            }
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            "a\n\nb\n"
        );
    }

    #[test]
    fn a_new_file_lands_with_an_umask_respecting_mode() {
        let directory = project();
        let root = directory.path();
        ensure_entries(root, &["a/"], false).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join(".gitignore"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            // 0666 minus the umask; never the 0600 a bare temporary file carries.
            assert_eq!(mode & 0o600, 0o600);
            assert!(
                mode & 0o044 != 0,
                "mode {mode:o} lost the group/other read bit"
            );
        }
    }
}
