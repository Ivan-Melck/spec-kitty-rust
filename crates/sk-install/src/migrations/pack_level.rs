//! `4.0.0rc7_pack_level_backfill`: add `pack_level: project` to the project pack.
//!
//! The project layer is the single flat pack at `.kittify/charter-packs/`. A
//! legacy project may have no `pack.yaml` (so `charter pack validate` has
//! nothing to read) or an authored descriptor that predates the `pack_level`
//! key. This migration writes a minimal descriptor only when the directory
//! already exists, and adds `pack_level` to an existing mapping without
//! touching comments, key order, a differing `pack_id`, or any other field.
//!
//! It never creates the pack directory, never reaches `packs/built-in` or
//! `packs/internal`, and treats an unreadable/malformed descriptor as a
//! not-applicable project rather than rewriting it.

use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

const PROJECT_PACK_DIRNAME: &str = "charter-packs";
const PACK_DESCRIPTOR_FILENAME: &str = "pack.yaml";
const PACK_LEVEL_KEY: &str = "pack_level";
const PROJECT_LEVEL: &str = "project";

fn project_pack_root(project_root: &Path) -> PathBuf {
    project_root.join(".kittify").join(PROJECT_PACK_DIRNAME)
}

fn descriptor_path(pack_root: &Path) -> PathBuf {
    pack_root.join(PACK_DESCRIPTOR_FILENAME)
}

fn display_path(descriptor: &Path, project_root: &Path) -> String {
    descriptor
        .strip_prefix(project_root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| {
            descriptor
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
}

fn needs_backfill(pack_root: &Path) -> Result<bool, String> {
    let descriptor = descriptor_path(pack_root);
    if !descriptor.is_file() {
        return Ok(true);
    }
    let text = std::fs::read_to_string(&descriptor)
        .map_err(|error| format!("{}: {error}", descriptor.display()))?;
    let value: Value = serde_yaml_ng::from_str(&text)
        .map_err(|error| format!("{}: {error}", descriptor.display()))?;
    let Some(mapping) = value.as_mapping() else {
        // A malformed/non-mapping descriptor is not something this migration
        // rewrites; leave it for `charter pack validate` to flag.
        return Ok(false);
    };
    Ok(!mapping.contains_key(Value::from(PACK_LEVEL_KEY)))
}

fn write_fresh_descriptor(pack_root: &Path, project_root: &Path) -> Result<String, InstallError> {
    let descriptor = descriptor_path(pack_root);
    let name = pack_root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| PROJECT_PACK_DIRNAME.to_string());
    let mut mapping = serde_yaml_ng::Mapping::new();
    mapping.insert(
        Value::from("pack_id"),
        Value::from(ulid::Ulid::new().to_string()),
    );
    mapping.insert(Value::from("name"), Value::from(name));
    mapping.insert(Value::from(PACK_LEVEL_KEY), Value::from(PROJECT_LEVEL));
    let text = serde_yaml_ng::to_string(&Value::Mapping(mapping)).map_err(|error| {
        InstallError::YamlEdit {
            path: descriptor.clone(),
            detail: error.to_string(),
        }
    })?;
    sk_kernel::atomic_write(&descriptor, text.as_bytes()).map_err(|error| {
        InstallError::io(
            "write pack descriptor",
            &descriptor,
            std::io::Error::other(error),
        )
    })?;
    Ok(format!(
        "Wrote {} with {PACK_LEVEL_KEY}: {PROJECT_LEVEL}",
        display_path(&descriptor, project_root)
    ))
}

fn add_pack_level(pack_root: &Path, project_root: &Path) -> Result<String, InstallError> {
    let descriptor = descriptor_path(pack_root);
    let text = std::fs::read_to_string(&descriptor)
        .map_err(|source| InstallError::io("read pack descriptor", &descriptor, source))?;
    let before: Value = serde_yaml_ng::from_str(&text).map_err(|error| InstallError::YamlEdit {
        path: descriptor.clone(),
        detail: error.to_string(),
    })?;
    let before = before
        .as_mapping()
        .cloned()
        .ok_or_else(|| InstallError::YamlEdit {
            path: descriptor.clone(),
            detail: "document root is not a mapping; refusing to edit it".to_string(),
        })?;
    if before.contains_key(Value::from(PACK_LEVEL_KEY)) {
        return Ok(String::new());
    }
    let mut expected = before.clone();
    expected.insert(Value::from(PACK_LEVEL_KEY), Value::from(PROJECT_LEVEL));

    // The source performs a round-trip load/mutate/dump that keeps comments
    // and key order. Appending one scalar line is that same edit for a flat
    // descriptor, and the parse comparison below proves nothing else moved.
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut updated = text;
    if !updated.ends_with('\n') {
        updated.push_str(newline);
    }
    updated.push_str(&format!("{PACK_LEVEL_KEY}: {PROJECT_LEVEL}{newline}"));
    let after: Value =
        serde_yaml_ng::from_str(&updated).map_err(|error| InstallError::YamlEdit {
            path: descriptor.clone(),
            detail: error.to_string(),
        })?;
    if after != Value::Mapping(expected) {
        return Err(InstallError::YamlEdit {
            path: descriptor.clone(),
            detail: "byte-preserving edit changed the document beyond the requested field"
                .to_string(),
        });
    }
    sk_kernel::atomic_write(&descriptor, updated.as_bytes()).map_err(|error| {
        InstallError::io(
            "write pack descriptor",
            &descriptor,
            std::io::Error::other(error),
        )
    })?;
    Ok(format!(
        "Added {PACK_LEVEL_KEY}: {PROJECT_LEVEL} to {}",
        display_path(&descriptor, project_root)
    ))
}

/// Detection: an existing project pack needs the descriptor key.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    let pack_root = project_pack_root(project_root);
    if !pack_root.is_dir() {
        return Ok(false);
    }
    // An unreadable/malformed descriptor selects the migration so `can_apply`
    // can refuse it visibly instead of silently skipping it.
    Ok(needs_backfill(&pack_root).unwrap_or(true))
}

/// `can_apply`: only an existing project with readable descriptors.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.exists() {
        return Ok(Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        )));
    }
    let pack_root = project_pack_root(project_root);
    if !pack_root.is_dir() {
        return Ok(None);
    }
    match needs_backfill(&pack_root) {
        Ok(_) => Ok(None),
        Err(reason) => Ok(Some(reason)),
    }
}

/// Apply the backfill.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let pack_root = project_pack_root(project_root);
    if !pack_root.is_dir() {
        return Ok(MigrationOutcome::skipped());
    }
    match needs_backfill(&pack_root) {
        Ok(true) => {}
        Ok(false) => return Ok(MigrationOutcome::skipped()),
        Err(reason) => return Err(InstallError::UpgradeRefused { detail: reason }),
    }
    let has_descriptor = descriptor_path(&pack_root).is_file();
    let change = if has_descriptor {
        add_pack_level(&pack_root, project_root)?
    } else {
        write_fresh_descriptor(&pack_root, project_root)?
    };
    if change.is_empty() {
        Ok(MigrationOutcome::skipped())
    } else {
        Ok(MigrationOutcome::changed(vec![change]))
    }
}
