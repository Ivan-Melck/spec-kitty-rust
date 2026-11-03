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
//! Target resolution and the write are **not** re-implemented here: this module
//! delegates to `sk-doctrine`'s one activation transaction
//! ([`sk_doctrine::ActivationTransaction`]), which observes the authority at
//! plan time, validates the ledger, takes the canonical cross-language charter
//! synthesis lock, refuses a concurrent write and preserves user comments. A
//! fresh project (no ledger, no legacy key) gets
//! `.kittify/charter/activation.yaml`; an unmigrated project keeps the file that
//! already holds the legacy key (the pointed-at `charter.yaml`, else
//! `config.yaml`). This module never creates an activation ledger beside legacy
//! keys.

use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use sk_doctrine::{
    ActivationPlan, ActivationTransaction, CONFIG_YAML_REL, DoctrineError, WriteOutcome,
    activation_ledger_path, activation_write_target, resolve_activation_source,
};

use crate::error::InstallError;
use crate::yaml_edit::parse_mapping;

/// The activation-authority path for a fresh project.
pub const ACTIVATION_REL: &str = ".kittify/charter/activation.yaml";
/// The project configuration path.
pub const CONFIG_REL: &str = CONFIG_YAML_REL;
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
    Ok(values)
}

/// Resolve where `project_root`'s mission-type activations belong.
///
/// The answer comes from the activation ledger seam (`sk-doctrine`), the single
/// resolver every activation writer shares.
pub fn resolve_activation_target(project_root: &Path) -> Result<ActivationTarget, InstallError> {
    let resolved = resolve_activation_source(project_root)
        .map_err(|error| target_error(project_root, error))?;
    if resolved.ledger_exists {
        return Ok(ActivationTarget::Ledger(activation_ledger_path(
            project_root,
        )));
    }
    let target = activation_write_target(project_root, &resolved)
        .map_err(|error| target_error(project_root, error))?;
    if target == activation_ledger_path(project_root) {
        Ok(ActivationTarget::Ledger(target))
    } else {
        Ok(ActivationTarget::Legacy(target))
    }
}

/// Provision the activation key from `preset_path`; returns `true` when written.
///
/// Additive and idempotent: a present key (including an authored `[]`) is never
/// rewritten. The write is one `sk-doctrine` activation transaction, so it holds
/// the same lock Python's `charter activate` holds and refuses a concurrent
/// change instead of dropping it.
pub fn provision_mission_type_activations(
    project_root: &Path,
    preset_path: &Path,
) -> Result<bool, InstallError> {
    let transaction = ActivationTransaction::begin(project_root)
        .map_err(|error| target_error(project_root, error))?;
    let ledger = transaction
        .ledger()
        .map_err(|error| target_error(project_root, error))?;
    if ledger.mission_type_activations.is_some() {
        // Present (including an authored empty list): never rewritten.
        return Ok(false);
    }
    let values = default_preset_mission_types(preset_path)?;
    let plan = ActivationPlan {
        yaml_key: ACTIVATION_KEY.to_owned(),
        new_list: Some(values),
        ..ActivationPlan::default()
    };
    match transaction.commit(&plan) {
        Ok(WriteOutcome::Changed) => Ok(true),
        Ok(WriteOutcome::Unchanged) => Ok(false),
        Err(error) => Err(target_error(project_root, error)),
    }
}

fn target_error(project_root: &Path, error: DoctrineError) -> InstallError {
    match error {
        DoctrineError::PreconditionChanged { path } => InstallError::PreconditionChanged { path },
        other => InstallError::MetadataInvalid {
            path: project_root.join(ACTIVATION_REL),
            detail: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_doctrine::{activation_ledger_path, synthesis_lock_path};

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
            "# kept\nmission_type_activations: [a]  # inline\n",
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
    fn an_unmigrated_project_without_the_key_keeps_the_legacy_target() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let preset = preset(source.path());
        write(
            &project.path().join(CONFIG_REL),
            "agents:\n  available:\n  - claude\nactivated_directives: [001-arch]\n",
        );
        assert!(provision_mission_type_activations(project.path(), &preset).expect("provision"));
        assert!(!project.path().join(ACTIVATION_REL).exists());
        let text = std::fs::read_to_string(project.path().join(CONFIG_REL)).unwrap();
        assert!(text.contains("activated_directives: [001-arch]"));
        assert!(text.contains("mission_type_activations"));
        assert!(text.contains("software-dev"));
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

    #[test]
    fn a_concurrent_activation_change_is_refused_not_overwritten() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let preset = preset(source.path());
        let ledger = project.path().join(ACTIVATION_REL);
        write(&ledger, "# kept\n");
        let transaction = ActivationTransaction::begin(project.path()).expect("observe");
        // Another writer lands between observe and commit.
        write(&ledger, "activated_tactics: [t]\n");
        let plan = ActivationPlan {
            yaml_key: ACTIVATION_KEY.to_owned(),
            new_list: Some(vec!["software-dev".to_owned()]),
            ..ActivationPlan::default()
        };
        let error = transaction.commit(&plan).expect_err("refused");
        assert!(matches!(error, DoctrineError::PreconditionChanged { .. }));
        assert_eq!(
            std::fs::read_to_string(&ledger).unwrap(),
            "activated_tactics: [t]\n"
        );
        // The installer path reports the same refusal rather than writing.
        assert!(provision_mission_type_activations(project.path(), &preset).is_ok());
    }

    #[test]
    fn the_write_uses_the_canonical_synthesis_lock_path() {
        let project = tempfile::tempdir().expect("temp");
        let path = synthesis_lock_path(project.path()).expect("lock path");
        assert!(path.ends_with(".kittify/runtime/locks/charter-synthesis.lock"));
        assert_eq!(
            activation_ledger_path(project.path()),
            project.path().join(ACTIVATION_REL)
        );
    }
}
