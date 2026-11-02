//! Project-root discovery.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sk_kernel::discover_project_root;

use crate::commands::path_string;
use crate::output::{CliError, CommandData};

/// Resolve the root to inspect.
///
/// An explicit `--project-root` is authoritative, must exist, and must carry
/// a `.kittify`, `kitty-specs`, or `.git` marker. Otherwise the
/// kernel's bounded discovery (which honours `SPECIFY_REPO_ROOT`) walks from the
/// current directory.
pub fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    match explicit {
        Some(root) => canonical_existing_root(root),
        None => {
            let current = env::current_dir().map_err(|error| {
                CliError::io(format!("could not read the current directory: {error}"))
            })?;
            discover_project_root(&current).map_err(|error| {
                CliError::domain(format!(
                    "could not discover a project root from {}: {error}",
                    current.display()
                ))
            })
        }
    }
}

/// Resolve a root only when one is available, without failing when discovery
/// finds nothing. Used by commands that can operate entirely from explicit
/// seed files.
pub fn optional_root(explicit: Option<&Path>) -> Result<Option<PathBuf>, CliError> {
    match explicit {
        Some(root) => canonical_existing_root(root).map(Some),
        None => {
            let current = env::current_dir().map_err(|error| {
                CliError::io(format!("could not read the current directory: {error}"))
            })?;
            Ok(discover_project_root(&current).ok())
        }
    }
}

fn canonical_existing_root(root: &Path) -> Result<PathBuf, CliError> {
    let metadata = fs::metadata(root).map_err(|error| {
        CliError::domain(format!(
            "project root {} is not usable: {error}",
            root.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(CliError::domain(format!(
            "project root {} is not a directory",
            root.display()
        )));
    }
    if !root.join(".kittify").is_dir()
        && !root.join("kitty-specs").is_dir()
        && !root.join(".git").exists()
    {
        return Err(CliError::domain(format!(
            "explicit project root {} has no .kittify/, kitty-specs/, or .git marker; supply the repository root, not a subdirectory",
            root.display()
        )));
    }
    fs::canonicalize(root).map_err(|error| {
        CliError::io(format!(
            "could not canonicalize project root {}: {error}",
            root.display()
        ))
    })
}

/// Report the discovered root.
pub fn root(explicit: Option<&Path>) -> Result<CommandData, CliError> {
    let root = resolve_root(explicit)?;
    let source = if explicit.is_some() {
        "explicit"
    } else {
        "discovery"
    };
    Ok(CommandData::success(json!({
        "root": path_string(&root),
        "source": source,
        "read_only": true
    })))
}
