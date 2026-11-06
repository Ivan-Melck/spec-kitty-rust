//! Acceptance-facing path-convention evaluation, and the read-only facts the
//! parent collector maps into Status.
//!
//! Source `acceptance.summary_core.evaluate_path_conventions` owns the *policy*
//! part of this seam: mission path conventions block acceptance by default, are
//! advisory under `--lenient`, the `contracts` waiver is honoured (and dedups the
//! optional-artifact warning), and a mission with no declared paths is a no-op.
//! Reproducing it here keeps `PathConventionFacts::Evaluated` and a truthful
//! `NotDeclared` reachable without a second policy: the parent decides *when* to
//! call this, and this module decides *what the facts and text are*.
//!
//! The parent maps [`MissionPathFacts`] into its own carrier. Nothing in this
//! module writes, mutates a status surface, or wires a CLI.

use std::path::{Path, PathBuf};

use sk_kernel::PythonString;

use crate::definition::error::DefinitionError;
use crate::definition::model::MissionDefinition;
use crate::path_conventions::project::{ProjectPathConventions, load_project_path_conventions};
use crate::path_conventions::result::{PATH_CONVENTIONS_NOT_SATISFIED, PathValidationResult};
use crate::path_conventions::tokens::{artifact_tokens_for, contains_token, normalize_path_token};
use crate::path_conventions::validate::{
    PathConventionError, PathValidationInputs, PathValidationOutcome, validate_mission_paths,
};
use crate::path_conventions::waiver::{ContractsWaiver, read_contracts_waiver_from_meta};

/// The mission-artifact token the `meta.json` waiver governs.
pub const CONTRACTS_ARTIFACT: &str = "contracts";

/// Inputs for one path-convention evaluation.
#[derive(Clone, Copy, Debug)]
pub struct EvaluatePathConventionsInputs<'a> {
    /// The resolved definition, or `None` when it could not be loaded (Source
    /// treats that as "no declared paths" on this path).
    pub definition: Option<&'a MissionDefinition>,
    /// The repository/worktree root.
    pub repo_root: &'a Path,
    /// The Mission's PRIMARY-surface directory (`kitty-specs/<mission>/`).
    pub feature_dir: &'a Path,
    /// Where planning reads resolve (the primary read dir; usually
    /// `feature_dir`).
    pub planning_read_dir: &'a Path,
    /// The research deliverables prefix (`None` for every other domain).
    pub path_prefix: Option<&'a PythonString>,
    /// The mission-type slug, as `ResolvedMissionDefinition::mission_type_slug`
    /// derives it. Source reads it off the Mission's resolver path; a caller that
    /// only holds a definition supplies it from the resolution it performed.
    pub mission_type_slug: Option<&'a PythonString>,
    /// `strict_metadata` is Source's `not lenient`.
    pub strict_metadata: bool,
    /// Approved candidate source roots (§#4254).
    pub candidate_source_roots: &'a [PathBuf],
    /// The project override, when the caller already read it.
    pub path_overrides: Option<&'a ProjectPathConventions>,
    /// The waiver, when the caller already read it.
    pub waiver: Option<&'a ContractsWaiver>,
}

/// Source `_contracts_waiver_effect` output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContractsWaiverEffect {
    /// Artifact tokens the waiver removes from the requirement set.
    pub waived_tokens: Vec<PythonString>,
    /// The operator-facing notice (rationale, contradiction, or malformed
    /// warning), when there is one to show.
    pub notice: Option<PythonString>,
}

/// Source `_contracts_waiver_effect`.
///
/// Consulted only when `contracts` is BOTH a declared path convention and a
/// declared mission artifact resolved on the mission surface (and not research's
/// prefix mode) — exactly the case `validate_mission_paths` can waive — so the
/// notice never claims a waiver the validator did not apply.
pub fn contracts_waiver_effect(
    definition: Option<&MissionDefinition>,
    planning_read_dir: &Path,
    path_prefix: Option<&PythonString>,
    waiver: Option<&ContractsWaiver>,
) -> Result<ContractsWaiverEffect, DefinitionError> {
    let Some(definition) = definition else {
        return Ok(ContractsWaiverEffect::default());
    };
    let declared_token = definition
        .paths()
        .values()
        .map(normalize_path_token)
        .any(|value| crate::definition::model::python_equals(&value, CONTRACTS_ARTIFACT));
    let artifact_tokens = artifact_tokens_for(definition);
    let waivable = declared_token
        && contains_token(
            &artifact_tokens,
            &PythonString::from_scalars(CONTRACTS_ARTIFACT),
        );
    if !waivable || path_prefix.is_some() {
        return Ok(ContractsWaiverEffect::default());
    }
    let read;
    let waiver = match waiver {
        Some(waiver) => waiver,
        None => {
            read = read_contracts_waiver_from_meta(planning_read_dir)?;
            &read
        }
    };
    if !waiver.waived {
        return Ok(ContractsWaiverEffect {
            waived_tokens: Vec::new(),
            notice: waiver.warning.clone(),
        });
    }
    let rationale = waiver
        .rationale
        .clone()
        .unwrap_or_else(|| PythonString::from_scalars(""));
    let notice = if planning_read_dir.join(CONTRACTS_ARTIFACT).is_dir() {
        // The Mission declares no contracts yet ships a contracts/ directory:
        // the directory satisfies the requirement, so nothing was waived. Say so.
        let mut text = PythonString::from_scalars("Warning: meta.json declares contracts: none (");
        text.append(&rationale);
        text.push_scalars(
            ") but a contracts/ directory exists; remove the declaration or the directory.",
        );
        text
    } else {
        let mut text = PythonString::from_scalars(
            "contracts/ requirement waived by meta.json (contracts: none): ",
        );
        text.append(&rationale);
        text
    };
    Ok(ContractsWaiverEffect {
        waived_tokens: vec![PythonString::from_scalars(CONTRACTS_ARTIFACT)],
        notice: Some(notice),
    })
}

/// Source `_with_notice`.
fn with_notice(text: PythonString, notice: Option<&PythonString>) -> PythonString {
    match notice {
        Some(notice) => {
            let mut out = text;
            out.push_scalars("\n");
            out.append(notice);
            out
        }
        None => text,
    }
}

/// The full evaluation: violations (strict), warning (lenient), removed tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathConventionEvaluation {
    /// Blocking path-convention violations (strict mode only).
    pub violations: Vec<PythonString>,
    /// The advisory warning text (lenient mode, or a waiver notice with nothing
    /// missing).
    pub warning: Option<PythonString>,
    /// Normalized tokens removed from the optional-artifact warning: declared
    /// missing artifacts, plus any waived token.
    pub dedup_tokens: Vec<PythonString>,
    /// Diagnostics Source emitted through `warnings.warn` (ignored
    /// artifact-colliding overrides, ignored artifact-routed override keys).
    pub notices: Vec<PythonString>,
    /// The underlying validation facts.
    pub result: PathValidationResult,
    /// The waiver effect that applied.
    pub waiver: ContractsWaiverEffect,
}

/// Source `evaluate_path_conventions`.
pub fn evaluate_path_conventions(
    inputs: EvaluatePathConventionsInputs<'_>,
    decoder: &dyn crate::definition::dialect::SafeDocumentDecoder,
) -> Result<PathConventionEvaluation, PathConventionError> {
    let Some(definition) = inputs.definition else {
        return Ok(empty_evaluation());
    };
    if definition.paths().is_empty() {
        return Ok(empty_evaluation());
    }

    let mut notices = Vec::new();
    let project_overrides;
    let path_overrides = match inputs.path_overrides {
        Some(overrides) => Some(overrides),
        None => {
            project_overrides = load_project_path_conventions(inputs.repo_root, decoder)
                .map_err(PathConventionError::Config)?;
            notices.extend(project_overrides.warnings.iter().cloned());
            Some(&project_overrides)
        }
    };

    let waiver = contracts_waiver_effect(
        Some(definition),
        inputs.planning_read_dir,
        inputs.path_prefix,
        inputs.waiver,
    )?;
    let artifact_tokens = artifact_tokens_for(definition);

    let outcome: PathValidationOutcome = validate_mission_paths(PathValidationInputs {
        mission_name: &definition.name,
        declared_paths: definition.paths(),
        artifact_tokens: &artifact_tokens,
        mission_type_slug: inputs.mission_type_slug,
        project_root: inputs.repo_root,
        path_prefix: inputs.path_prefix,
        feature_dir: Some(inputs.planning_read_dir),
        path_overrides,
        candidate_source_roots: inputs.candidate_source_roots,
        waived_artifact_tokens: &waiver.waived_tokens,
        strict: false,
    })?;
    notices.extend(outcome.notices.iter().cloned());

    let result = outcome.result;
    let mut dedup_tokens: Vec<PythonString> = Vec::new();
    if result.missing_paths.is_empty() {
        return Ok(PathConventionEvaluation {
            violations: Vec::new(),
            warning: waiver.notice.clone(),
            dedup_tokens: waiver.waived_tokens.clone(),
            notices,
            result,
            waiver,
        });
    }

    if inputs.strict_metadata {
        let artifact_tokens = artifact_tokens_for(definition);
        for token in &result.missing_artifact_tokens {
            let normalized = normalize_path_token(token);
            if contains_token(&artifact_tokens, &normalized) && !dedup_tokens.contains(&normalized)
            {
                dedup_tokens.push(normalized);
            }
        }
        for token in &waiver.waived_tokens {
            if !dedup_tokens.contains(token) {
                dedup_tokens.push(token.clone());
            }
        }
        let text = with_notice(result.error_text(), waiver.notice.as_ref());
        return Ok(PathConventionEvaluation {
            violations: vec![text],
            warning: None,
            dedup_tokens,
            notices,
            result,
            waiver,
        });
    }

    let text = with_notice(result.warning_text(), waiver.notice.as_ref());
    Ok(PathConventionEvaluation {
        violations: Vec::new(),
        warning: Some(text),
        dedup_tokens: waiver.waived_tokens.clone(),
        notices,
        result,
        waiver,
    })
}

fn empty_evaluation() -> PathConventionEvaluation {
    PathConventionEvaluation {
        violations: Vec::new(),
        warning: None,
        dedup_tokens: Vec::new(),
        notices: Vec::new(),
        result: PathValidationResult::default(),
        waiver: ContractsWaiverEffect::default(),
    }
}

/// Source `build_warnings`: the summary's non-blocking warning list.
pub fn build_warnings(
    missing_optional: &[PythonString],
    path_violations: &[PythonString],
    path_convention_warning: Option<&PythonString>,
) -> Vec<PythonString> {
    let mut warnings = Vec::new();
    if !missing_optional.is_empty() {
        let mut text = PythonString::from_scalars("Optional artifacts missing: ");
        for (index, entry) in missing_optional.iter().enumerate() {
            if index > 0 {
                text.push_scalars(", ");
            }
            text.append(entry);
        }
        warnings.push(text);
    }
    if !path_violations.is_empty() {
        warnings.push(PythonString::from_scalars(PATH_CONVENTIONS_NOT_SATISFIED));
    } else if let Some(warning) = path_convention_warning {
        warnings.push(warning.clone());
    }
    warnings
}

/// The `kitty-specs/<mission>/` deliverables prefix for a research Mission
/// (Source `_path_prefix_for_mission` + `mission.get_deliverables_path`).
///
/// `deliverables_path` from `meta.json` wins; otherwise a research Mission
/// defaults to `docs/research/<slug>/`. A non-research Mission has no prefix.
/// A caller that needs the prefix without a definition supplies the domain.
pub fn research_path_prefix(
    is_research: bool,
    planning_read_dir: &Path,
    mission_slug: &str,
) -> Option<PythonString> {
    if !is_research {
        return None;
    }
    get_deliverables_path(planning_read_dir, mission_slug)
        .map(|path| PythonString::from_scalars(&path))
}

/// Source `mission.get_deliverables_path`.
///
/// Reads `deliverables_path` from `meta.json`; a research Mission without one
/// defaults to `docs/research/<slug>/`. `mission_slug` is the Mission directory
/// name (Source passes `feature_dir.name`). Meta reads are deliberately lenient
/// here (Source uses `load_meta_or_empty`).
pub fn get_deliverables_path(planning_read_dir: &Path, mission_slug: &str) -> Option<String> {
    let meta_path = planning_read_dir.join(crate::meta::META_FILE_NAME);
    let meta = crate::meta::load_meta_object(&meta_path, crate::meta::DEFAULT_META_BYTE_LIMIT).ok();
    if let Some(meta) = &meta {
        if let Some(serde_json::Value::String(value)) = meta.get("deliverables_path")
            && !value.is_empty()
        {
            return Some(value.clone());
        }
        if recorded_type_is_research(meta) {
            let slug = match meta.get("slug") {
                Some(serde_json::Value::String(value)) if !value.is_empty() => value.clone(),
                _ => mission_slug.to_owned(),
            };
            return Some(format!("docs/research/{slug}/"));
        }
    }
    if !mission_slug.is_empty() {
        // No meta.json at all: Source still offers the research default when the
        // caller supplied a slug. The caller only asks for a prefix for a
        // research Mission, so honour the same shape.
        return Some(format!("docs/research/{mission_slug}/"));
    }
    None
}

fn recorded_type_is_research(meta: &serde_json::Map<String, serde_json::Value>) -> bool {
    matches!(
        meta.get("mission_type"),
        Some(serde_json::Value::String(value)) if value.trim() == "research"
    )
}

/// Read-only path/artifact facts for the parent collector.
///
/// The parent maps this into its own carrier (Status
/// `PathConventionFacts::Evaluated` when `evaluated` is true). `evaluated` is
/// false only when the definition declares no paths — Source's genuine no-op —
/// never because this service could not read something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionPathFacts {
    /// The Mission's display name.
    pub mission_name: PythonString,
    /// The Mission domain token.
    pub domain: &'static str,
    /// Declared path conventions in authored order.
    pub declared_paths: Vec<(PythonString, PythonString)>,
    /// Declared required artifacts in authored order.
    pub required_artifacts: Vec<PythonString>,
    /// Declared optional artifacts in authored order.
    pub optional_artifacts: Vec<PythonString>,
    /// The mission-type slug, when derivable.
    pub mission_type_slug: Option<PythonString>,
    /// Whether the definition declares path conventions at all.
    pub evaluated: bool,
    /// The validation result (empty when `evaluated` is false).
    pub result: PathValidationResult,
    /// Waiver effect applied.
    pub waiver: ContractsWaiverEffect,
}

/// Build the parent-facing facts from a definition and one validation.
pub fn mission_path_facts(
    definition: &MissionDefinition,
    mission_type_slug: Option<&PythonString>,
    evaluation: &PathConventionEvaluation,
) -> MissionPathFacts {
    MissionPathFacts {
        mission_name: definition.name.clone(),
        domain: definition.domain.as_str(),
        declared_paths: definition
            .paths()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        required_artifacts: definition.required_artifacts().to_vec(),
        optional_artifacts: definition.optional_artifacts().to_vec(),
        mission_type_slug: mission_type_slug.cloned(),
        evaluated: !definition.paths().is_empty(),
        result: evaluation.result.clone(),
        waiver: evaluation.waiver.clone(),
    }
}
