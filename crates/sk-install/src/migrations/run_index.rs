//! `4_0_0rc5_heal_run_index_paths`: heal absolute `run_dir` values.
//!
//! The run index (`.kittify/runtime/feature-runs.json`) is written by the
//! runtime's single locked writer. Older CLIs stored an absolute `run_dir`,
//! which points at the original checkout after a copy or move. This migration
//! reads the index, rewrites every absolute entry to a portable token using the
//! same classification the source uses, and writes it back under the index's
//! own lock with Python's `json.dumps(indent=2, sort_keys=True)` bytes.
//!
//! An absolute path inside this project is relativized (preserving any
//! subpath); an absolute path outside it is re-anchored to
//! `.kittify/runtime/runs/<run_id>` (falling back to the path's trailing
//! component when the entry has no `run_id`). A relative token is already
//! portable and is left alone.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

use super::json::python_indented_ascii;

/// The run-index filename (frozen legacy name).
pub const FEATURE_RUNS_FILENAME: &str = "feature-runs.json";
const RUNS_TOKEN_PREFIX: &str = ".kittify/runtime/runs";
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// The run-index path for a project.
pub fn feature_runs_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".kittify")
        .join("runtime")
        .join(FEATURE_RUNS_FILENAME)
}

fn is_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    if value.starts_with("\\\\") || value.starts_with("//") {
        return true;
    }
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn is_absolute(value: &str) -> bool {
    Path::new(value).is_absolute() || is_windows_absolute(value)
}

fn normalize_absolute(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    normalized
}

/// Python `Path.resolve()` without requiring the tail to exist, falling back to
/// the input path exactly as the upstream `_safe_resolve` does on `OSError`.
fn safe_resolve(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            normalize_absolute(path)
        } else {
            std::env::current_dir()
                .map(|cwd| normalize_absolute(&cwd.join(path)))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

fn is_within(child: &Path, parent: &Path) -> bool {
    child.starts_with(parent)
}

fn as_posix(value: &str) -> String {
    // `Path(value).as_posix()` on an already-relative token only normalizes
    // redundant separators and `.` components. Preserve `..` as authored.
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(value).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => parts.push("..".to_string()),
            Component::Normal(name) => parts.push(name.to_string_lossy().to_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    parts.join("/")
}

fn serialize_run_dir(run_dir: &str, project_root: &Path) -> String {
    let candidate = Path::new(run_dir);
    if !candidate.is_absolute() {
        return as_posix(run_dir);
    }
    let resolved = safe_resolve(candidate);
    let root = safe_resolve(project_root);
    if is_within(&resolved, &root) {
        return resolved
            .strip_prefix(&root)
            .unwrap_or(&resolved)
            .to_string_lossy()
            .replace('\\', "/");
    }
    run_dir.to_string()
}

fn run_id_basename(value: &str) -> String {
    let separator = if is_windows_absolute(value) {
        |character: char| character == '\\' || character == '/'
    } else {
        |character: char| character == '/'
    };
    value
        .rsplit(separator)
        .find(|part| !part.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn healed_token(value: &str, project_root: &Path, run_id: Option<&str>) -> String {
    let tokenized = serialize_run_dir(value, project_root);
    if is_absolute(&tokenized) {
        let anchor = run_id
            .filter(|candidate| !candidate.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| run_id_basename(value));
        return format!("{RUNS_TOKEN_PREFIX}/{anchor}");
    }
    tokenized
}

fn load_index(path: &Path) -> Result<Map<String, Value>, InstallError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        // Python catches `OSError` and `JSONDecodeError`, not a decode error:
        // an undecodable index is a real failure, not an empty index.
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(InstallError::UpgradeRefused {
                detail: format!("{FEATURE_RUNS_FILENAME} is not valid UTF-8: {error}"),
            });
        }
        Err(_) => return Ok(Map::new()),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return Ok(Map::new()),
    };
    match value {
        Value::Object(mapping) => Ok(mapping),
        other => Err(InstallError::UpgradeRefused {
            detail: format!(
                "{} root is {}, not an object; the run index cannot be healed",
                FEATURE_RUNS_FILENAME,
                match other {
                    Value::Array(_) => "an array",
                    Value::Null => "null",
                    Value::Bool(_) => "a boolean",
                    Value::Number(_) => "a number",
                    Value::String(_) => "a string",
                    Value::Object(_) => "an object",
                }
            ),
        }),
    }
}

fn healable_entries(index: &Map<String, Value>) -> Vec<(String, String)> {
    let mut healable = Vec::new();
    for (key, entry) in index {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(run_dir) = entry.get("run_dir").and_then(Value::as_str) else {
            continue;
        };
        if !run_dir.is_empty() && is_absolute(run_dir) {
            healable.push((key.clone(), run_dir.to_string()));
        }
    }
    healable
}

fn planned_changes(index: &mut Map<String, Value>, project_root: &Path) -> Vec<String> {
    let mut changes = Vec::new();
    for (key, entry) in index.iter_mut() {
        let Some(entry) = entry.as_object_mut() else {
            continue;
        };
        let Some(run_dir) = entry
            .get("run_dir")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        if run_dir.is_empty() || !is_absolute(&run_dir) {
            continue;
        }
        let run_id = entry.get("run_id").and_then(Value::as_str);
        let new_value = healed_token(&run_dir, project_root, run_id);
        changes.push(format!(
            "{FEATURE_RUNS_FILENAME}[{key}].run_dir: {run_dir} -> {new_value}"
        ));
        entry.insert("run_dir".to_string(), Value::String(new_value));
    }
    changes
}

/// Whether any entry carries an absolute `run_dir` (the idempotency guard).
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    let index = load_index(&feature_runs_path(project_root))?;
    Ok(!healable_entries(&index).is_empty())
}

/// Preview the heal without writing.
pub fn preview(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let mut index = load_index(&feature_runs_path(project_root))?;
    Ok(planned_changes(&mut index, project_root))
}

/// Apply the heal under the index lock.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let path = feature_runs_path(project_root);
    if !path.exists() {
        return Ok(MigrationOutcome::skipped());
    }
    let lock_path = path.with_file_name(format!("{FEATURE_RUNS_FILENAME}.lock"));
    let _lock =
        sk_kernel::FileLock::acquire(&lock_path, LOCK_TIMEOUT).map_err(|error| match error {
            sk_kernel::KernelError::LockTimeout { .. } => InstallError::LockUnavailable {
                path: lock_path.clone(),
                waited_ms: LOCK_TIMEOUT.as_millis() as u64,
                source: std::io::Error::new(std::io::ErrorKind::TimedOut, "index lock held"),
            },
            other => InstallError::io("lock run index", &lock_path, std::io::Error::other(other)),
        })?;
    let mut index = load_index(&path)?;
    let changes = planned_changes(&mut index, project_root);
    if changes.is_empty() {
        return Ok(MigrationOutcome::skipped());
    }
    let text = python_indented_ascii(&Value::Object(index));
    sk_kernel::atomic_write_with_options(
        &path,
        text.as_bytes(),
        sk_kernel::AtomicWriteOptions {
            create_parents: true,
        },
    )
    .map_err(|error| InstallError::io("write run index", &path, std::io::Error::other(error)))?;
    Ok(MigrationOutcome {
        changed: true,
        changes,
        warnings: Vec::new(),
    })
}

/// `can_apply`: the project must exist and the pack is not needed.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.exists() {
        return Ok(Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        )));
    }
    Ok(None)
}
