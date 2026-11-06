//! Shared path/ownership plumbing for the pre-3.x migration family.
//!
//! These helpers are deliberately small and mechanical: the *rules* live in
//! the family modules that state which files a migration owns. Everything here
//! is either a constant set copied from the pinned source or a filesystem
//! primitive (sorted traversal, exact-byte tree removal, frontmatter `lane:`
//! rewriting) that several migrations genuinely share.

use std::path::{Path, PathBuf};

use crate::error::InstallError;

/// The project-local State directory.
pub const KITTIFY: &str = ".kittify";
/// The Mission corpus directory.
pub const KITTY_SPECS: &str = "kitty-specs";
/// The lane worktree root.
pub const WORKTREES: &str = ".worktrees";

/// The four directory-based task lanes (`0.9.0`/`0.9.1`).
pub const LANE_DIRS: [&str; 4] = ["planned", "doing", "for_review", "done"];

/// Operating-system noise that never keeps a lane directory alive
/// (`0.9.0`/`0.9.1` `IGNORE_FILES`).
pub const IGNORE_FILES: [&str; 5] = [
    ".gitkeep",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    ".directory",
];

/// Whether a file name is OS noise (the pinned `_should_ignore_file`).
pub fn should_ignore_file(name: &str) -> bool {
    IGNORE_FILES.contains(&name) || name.starts_with("._")
}

/// Directory entries, sorted by path, directories first-class (callers filter).
pub fn entries(directory: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

/// Child directories, sorted, excluding symlinks-to-nothing and non-directories.
pub fn directories(directory: &Path) -> Vec<PathBuf> {
    entries(directory)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

/// Real (non-noise) contents of a lane directory.
pub fn real_contents(directory: &Path) -> Vec<PathBuf> {
    entries(directory)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .map(|name| !should_ignore_file(&name.to_string_lossy()))
                .unwrap_or(false)
        })
        .collect()
}

/// The lane worktrees of `project_root`, sorted (`Mockup`: every directory child).
pub fn worktrees(project_root: &Path) -> Vec<PathBuf> {
    directories(&project_root.join(WORKTREES))
}

/// Read a file as UTF-8, tolerating a byte-order mark (the source's
/// `encoding="utf-8-sig"` reads).
pub fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
}

/// Write file bytes through the crate's canonical atomic writer.
pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<bool, InstallError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create directory", parent, error))?;
    }
    sk_kernel::atomic_write(path, bytes)
        .map_err(|error| InstallError::io("write", path, std::io::Error::other(error)))
}

/// Write UTF-8 text, never following a symlink at the destination.
pub fn write_text(path: &Path, text: &str) -> Result<bool, InstallError> {
    if path.is_symlink() {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "refusing to write through the symlink {}; replace or remove it first",
                path.display()
            ),
        });
    }
    write_bytes(path, text.as_bytes())
}

/// Remove a tool-owned tree the way `kernel.tree_removal` does: the caller
/// names the owned root, and nothing is deleted when a git checkout sits
/// between the target and that root.
///
/// `dry_run` reports the same verdict without touching the filesystem.
pub fn remove_tool_owned_tree(
    path: &Path,
    tool_root: &Path,
    reason: &str,
    dry_run: bool,
) -> Result<bool, InstallError> {
    if !(path.exists() || path.is_symlink()) {
        return Ok(false);
    }
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = std::fs::canonicalize(tool_root).unwrap_or_else(|_| tool_root.to_path_buf());
    if resolved != root && !resolved.starts_with(&root) {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "TOOL_OWNED_PATH_UNPROVEN: {} is not inside the owned root {} ({reason}); nothing was deleted",
                path.display(),
                tool_root.display()
            ),
        });
    }
    let mut cursor = Some(resolved.as_path());
    while let Some(directory) = cursor {
        if directory.join(".git").exists() || directory.join(".git").is_symlink() {
            return Err(InstallError::UpgradeRefused {
                detail: format!(
                    "TOOL_OWNED_PATH_UNPROVEN: {} is a git checkout ({reason}); nothing was deleted",
                    directory.display()
                ),
            });
        }
        if directory == root {
            break;
        }
        cursor = directory.parent();
    }
    if dry_run {
        return Ok(true);
    }
    if path.is_symlink() {
        std::fs::remove_file(path)
            .map_err(|error| InstallError::io("remove symlink", path, error))?;
        return Ok(true);
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path)
            .map_err(|error| InstallError::io("remove tree", path, error))?;
    } else {
        std::fs::remove_file(path).map_err(|error| InstallError::io("remove file", path, error))?;
    }
    Ok(true)
}

/// Create a symlink at `link` pointing at `original` (relative targets stay
/// relative, so a checkout remains relocatable).
///
/// Windows cannot create a relative link without first knowing whether the
/// target is a file or a directory, so both are attempted in order; the caller
/// falls back to a copy when this fails, as the pinned modules do.
pub fn symlink(original: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(original, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(original, link)
            .or_else(|_| std::os::windows::fs::symlink_dir(original, link))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (original, link);
        Err(std::io::Error::other(
            "symlinks are unsupported on this platform",
        ))
    }
}

/// Display spelling for a diagnostic: project-relative when possible.
pub fn relative_display(project_root: &Path, path: &Path) -> String {
    crate::preserve::relative_display(path, project_root)
}

/// Remove `directory` when it is empty (the source's `rmdir` after a sweep).
pub fn remove_if_empty(directory: &Path) -> bool {
    if !directory.is_dir() {
        return false;
    }
    let mut read = match std::fs::read_dir(directory) {
        Ok(read) => read,
        Err(_) => return false,
    };
    if read.next().is_some() {
        return false;
    }
    std::fs::remove_dir(directory).is_ok()
}

/// The pinned `_ensure_lane_in_frontmatter`: make the YAML frontmatter carry the
/// lane the source directory states.
///
/// This is a *line* rewrite, not a YAML round-trip: the source also rewrites
/// only the `lane:` line and preserves every other byte of the file, which is
/// what keeps comments, ordering and quoting of other fields intact.
pub fn ensure_lane_in_frontmatter(content: &str, expected_lane: &str) -> String {
    let header = format!("---\nlane: \"{expected_lane}\"\n---\n");
    if !content.starts_with("---") {
        return format!("{header}{content}");
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let mut closing = None;
    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            closing = Some(index);
            break;
        }
    }
    let Some(closing) = closing else {
        return format!("{header}{content}");
    };
    let mut frontmatter: Vec<String> = Vec::new();
    let mut lane_found = false;
    for line in &lines[1..closing] {
        if let Some(rest) = line.strip_prefix("lane:") {
            lane_found = true;
            let current = rest.trim().trim_matches(['"', '\'']);
            if current != expected_lane {
                frontmatter.push(format!("lane: \"{expected_lane}\""));
            } else {
                frontmatter.push((*line).to_string());
            }
            continue;
        }
        frontmatter.push((*line).to_string());
    }
    if !lane_found {
        let insert_at = frontmatter
            .iter()
            .position(|line| line.starts_with("history:"))
            .unwrap_or(frontmatter.len());
        frontmatter.insert(insert_at, format!("lane: \"{expected_lane}\""));
    }
    let mut rebuilt: Vec<String> = vec!["---".to_string()];
    rebuilt.extend(frontmatter);
    rebuilt.push("---".to_string());
    rebuilt.extend(lines[closing + 1..].iter().map(|line| (*line).to_string()));
    rebuilt.join("\n")
}

/// Split a markdown document into `(frontmatter_lines, body)`.
///
/// `None` when the document has no well-formed `---` fence, which is the
/// source's `FrontmatterError` case (`read` raises and the caller skips).
pub fn split_frontmatter(content: &str) -> Option<(Vec<String>, String)> {
    let text = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end() != "---" {
        return None;
    }
    let mut frontmatter = Vec::new();
    let mut consumed = first.len();
    let mut closed = false;
    for line in lines {
        consumed += line.len();
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        frontmatter.push(line.trim_end_matches('\n').to_string());
    }
    if !closed {
        return None;
    }
    Some((frontmatter, text[consumed..].to_string()))
}

/// The raw scalar a `key: value` frontmatter line carries, unquoted.
pub fn frontmatter_value(frontmatter: &[String], key: &str) -> Option<String> {
    for line in frontmatter {
        if let Some(rest) = line.strip_prefix(&format!("{key}:")) {
            let value = rest.trim();
            if value.is_empty() {
                return Some(String::new());
            }
            return Some(value.trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

/// Render `lane: "<lane>"` in place of the existing lane line, keeping the rest
/// of the frontmatter and body byte-identical (the source's normalisation of
/// one field under `FrontmatterManager`).
pub fn normalized_lane_document(content: &str, lane: &str) -> Option<String> {
    let (frontmatter, body) = split_frontmatter(content)?;
    let mut rendered: Vec<String> = Vec::new();
    for line in &frontmatter {
        if line.starts_with("lane:") {
            rendered.push(format!("lane: \"{lane}\""));
        } else {
            rendered.push(line.clone());
        }
    }
    if !rendered.iter().any(|line| line.starts_with("lane:")) {
        rendered.insert(0, format!("lane: \"{lane}\""));
    }
    Some(format!("---\n{}\n---\n{body}", rendered.join("\n")))
}

/// Every `markdown` file under `root`, sorted (the source's `sorted(glob)`).
pub fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in entries(&directory) {
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry);
                continue;
            }
            if metadata.is_file() && entry.extension().is_some_and(|ext| ext == "md") {
                files.push(entry);
            }
        }
    }
    files.sort();
    files
}

/// The `.kittify/` child directories a shared-asset sweep classifies
/// (`runtime/migrate.py`).
pub const SHARED_ASSET_DIRS: [&str; 4] = ["templates", "missions", "scripts", "command-templates"];

/// The `.kittify/` root files that are shared assets.
pub const SHARED_ASSET_FILES: [&str; 1] = ["AGENTS.md"];

/// The `.kittify/` paths that are always project-specific.
pub const PROJECT_SPECIFIC_PATHS: [&str; 7] = [
    "config.yaml",
    "metadata.yaml",
    "memory",
    "workspaces",
    "logs",
    "overrides",
    "merge-state.json",
];

/// Whether a `.kittify/`-relative path is a shared (replaceable) asset.
pub fn is_shared_asset(relative: &Path) -> bool {
    let top_level = relative
        .components()
        .next()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    SHARED_ASSET_DIRS.contains(&top_level.as_str())
        || relative
            .file_name()
            .map(|name| SHARED_ASSET_FILES.contains(&name.to_string_lossy().as_ref()))
            .unwrap_or(false)
}
