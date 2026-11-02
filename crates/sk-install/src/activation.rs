//! Fresh-init mission-type activation provisioning.
//!
//! A fresh `spec-kitty init` writes the built-in pack's `default` preset mission
//! types into the project's activation authority, so skipping explicit charter
//! activation leaves the project exactly as `charter activate --preset default`
//! would. The list is **copied verbatim** from `presets/default.yaml` — never
//! re-derived by scanning the mission catalog, and never intersected with the
//! built-in set, so a project's custom mission types survive.
//!
//! The write is **additive**: the key is written only when it is entirely absent.
//! An authored empty list (`mission_type_activations: []`) and any present list
//! are left byte-identical. A missing, malformed or empty shipped preset is a
//! hard failure (`DEFAULT_PRESET_MISSING`), never an empty or implicit set.
//!
//! Target resolution mirrors the activation-authority seam: a fresh project (no
//! activation ledger and no legacy key) gets `.kittify/charter/activation.yaml`;
//! an unmigrated project keeps the file that already holds the legacy key
//! (`charter/charter.yaml` when it is the pointed-at charter, else
//! `config.yaml`). This module never creates an activation ledger beside legacy
//! keys.

use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::error::InstallError;
use crate::yaml_edit::{InsertPosition, YamlEditor, parse_mapping};

/// The activation-authority path for a fresh project.
pub const ACTIVATION_REL: &str = ".kittify/charter/activation.yaml";
/// The project configuration path.
pub const CONFIG_REL: &str = ".kittify/config.yaml";
/// The compiled charter path.
pub const CHARTER_REL: &str = ".kittify/charter/charter.yaml";
/// The activation key.
pub const ACTIVATION_KEY: &str = "mission_type_activations";

/// The resolved provisioning target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationTarget {
    /// A fresh/migrated project: `.kittify/charter/activation.yaml`.
    Ledger(PathBuf),
    /// An unmigrated project keeping its legacy surface.
    Legacy(PathBuf),
}

/// Read the built-in `default` preset's mission types, failing closed.
pub fn default_preset_mission_types(preset_path: &Path) -> Result<Vec<String>, InstallError> {
    let text = std::fs::read_to_string(preset_path).map_err(|error| {
        InstallError::ActivationPresetMissing {
            path: preset_path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let mapping = parse_mapping(preset_path, &text).map_err(|error| {
        InstallError::ActivationPresetMissing {
            path: preset_path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let values = mapping
        .get(Value::from(ACTIVATION_KEY))
        .and_then(Value::as_sequence)
        .map(|sequence| {
            sequence
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .ok_or_else(|| InstallError::ActivationPresetMissing {
            path: preset_path.to_path_buf(),
            detail: format!("preset declares no non-empty {ACTIVATION_KEY} list"),
        })?;
    if values.is_empty() {
        return Err(InstallError::ActivationPresetMissing {
            path: preset_path.to_path_buf(),
            detail: format!("preset declares no non-empty {ACTIVATION_KEY} list"),
        });
    }
    Ok(values)
}

/// Resolve where `project_root`'s mission-type activations belong.
pub fn resolve_activation_target(project_root: &Path) -> Result<ActivationTarget, InstallError> {
    let ledger = project_root.join(ACTIVATION_REL);
    if ledger.exists() {
        return Ok(ActivationTarget::Ledger(ledger));
    }
    let charter = project_root.join(CHARTER_REL);
    if has_activation_key(&charter)? {
        return Ok(ActivationTarget::Legacy(charter));
    }
    let config = project_root.join(CONFIG_REL);
    if has_activation_key(&config)? {
        return Ok(ActivationTarget::Legacy(config));
    }
    Ok(ActivationTarget::Ledger(ledger))
}

fn has_activation_key(path: &Path) -> Result<bool, InstallError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(InstallError::io("read", path, source)),
    };
    let mapping = parse_mapping(path, &text)?;
    Ok(mapping.contains_key(Value::from(ACTIVATION_KEY)))
}

/// Provision the activation key from `preset_path`; returns `true` when written.
pub fn provision_mission_type_activations(
    project_root: &Path,
    preset_path: &Path,
) -> Result<bool, InstallError> {
    let target = resolve_activation_target(project_root)?;
    let path = match &target {
        ActivationTarget::Ledger(path) | ActivationTarget::Legacy(path) => path.clone(),
    };
    let existing = match std::fs::read(&path) {
        Ok(raw) => String::from_utf8(raw).map_err(|error| InstallError::MetadataInvalid {
            path: path.clone(),
            detail: error.to_string(),
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(InstallError::io("read", &path, source)),
    };
    if !existing.trim().is_empty() {
        let mapping = parse_mapping(&path, &existing)?;
        if mapping.contains_key(Value::from(ACTIVATION_KEY)) {
            // Present (including an authored empty list): never rewritten.
            return Ok(false);
        }
    }
    let values = default_preset_mission_types(preset_path)?;
    let editor = YamlEditor::new(path.clone(), existing);
    let outcome = editor.insert_root_list(ACTIVATION_KEY, &values, InsertPosition::Bottom)?;
    if !outcome.changed {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    sk_kernel::atomic_write(&path, outcome.text.as_bytes()).map_err(|error| {
        InstallError::io("write activation", &path, std::io::Error::other(error))
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn preset(directory: &Path) -> PathBuf {
        let path = directory.join("presets/default.yaml");
        write(
            &path,
            "name: default\nmission_type_activations:\n  - software-dev\n  - documentation\n  - research\n  - plan\n",
        );
        path
    }

    #[test]
    fn seeds_a_fresh_project_activation_ledger() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let preset = preset(source.path());
        assert!(provision_mission_type_activations(project.path(), &preset).expect("provision"));
        let text = std::fs::read_to_string(project.path().join(ACTIVATION_REL)).unwrap();
        assert_eq!(
            text,
            "mission_type_activations:\n- software-dev\n- documentation\n- research\n- plan\n"
        );
        assert!(!provision_mission_type_activations(project.path(), &preset).expect("no-op"));
    }

    #[test]
    fn authored_empty_or_custom_lists_are_preserved() {
        for content in [
            "mission_type_activations: []\n",
            "mission_type_activations:\n- custom\n",
        ] {
            let project = tempfile::tempdir().expect("temp");
            let source = tempfile::tempdir().expect("temp");
            let preset = preset(source.path());
            write(&project.path().join(ACTIVATION_REL), content);
            assert!(!provision_mission_type_activations(project.path(), &preset).expect("no-op"));
            assert_eq!(
                std::fs::read_to_string(project.path().join(ACTIVATION_REL)).unwrap(),
                content
            );
        }
    }

    #[test]
    fn an_unmigrated_legacy_config_keeps_its_authority() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let preset = preset(source.path());
        write(
            &project.path().join(CONFIG_REL),
            "agents:\n  available:\n  - claude\nmission_type_activations:\n- legacy\n",
        );
        assert_eq!(
            resolve_activation_target(project.path()).expect("target"),
            ActivationTarget::Legacy(project.path().join(CONFIG_REL))
        );
        assert!(!provision_mission_type_activations(project.path(), &preset).expect("no-op"));
        assert!(!project.path().join(ACTIVATION_REL).exists());
    }

    #[test]
    fn missing_or_empty_presets_fail_closed() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let missing = source.path().join("presets/default.yaml");
        let error =
            provision_mission_type_activations(project.path(), &missing).expect_err("missing");
        assert_eq!(error.code(), "DEFAULT_PRESET_MISSING");
        assert!(!project.path().join(ACTIVATION_REL).exists());

        write(&missing, "name: default\nmission_type_activations: []\n");
        let error =
            provision_mission_type_activations(project.path(), &missing).expect_err("empty");
        assert_eq!(error.code(), "DEFAULT_PRESET_MISSING");
    }
}
