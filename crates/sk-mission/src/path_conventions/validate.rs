//! The path-convention validator: Source `validate_mission_paths`.
//!
//! Rules reproduced here, in Source's order:
//!
//! 1. **Project remap first.** A project override is merged into the *declared*
//!    map before prefixing and before artifact routing: only keys the Mission
//!    already declares are remapped (an override never adds a requirement), an
//!    artifact-routed key (`deliverables`) is never remapped, and an override
//!    whose *value* is a declared artifact token is dropped with a warning so a
//!    build path can never be satisfied by a mission artifact.
//! 2. **Prefix** (research deliverables mode) is applied to every declared path,
//!    preserving a directory hint and leaving absolute declarations alone.
//! 3. **Routing.** A declared path whose value is a declared artifact token
//!    resolves on the Mission's primary surface (`feature_dir`), never the
//!    repository root — there is no repo-root fallback. Everything else resolves
//!    against `project_root`.
//! 4. **Candidate roots.** A build path missing at `project_root` may be
//!    satisfied by a caller-supplied candidate source root (the acceptance caller
//!    passes approved lanes only), recorded in `satisfied_by`. Artifact paths
//!    never consult candidate roots.
//! 5. **Waivers.** A declared artifact-token path whose token the Mission waived
//!    (`meta.json` `"contracts": "none"`) is skipped entirely. A build path can
//!    never be waived.
//! 6. **Reporting.** Missing entries name the location actually tested (the
//!    primary-surface path for an artifact), existing entries keep the declared
//!    text, the directory hint survives, remappable keys are recorded, and — for
//!    a missing declared artifact — the normalized token the caller dedups
//!    against.
//!
//! `exists()` has `os.path.exists` semantics: files and directories both count,
//! and an unreadable/missing path is simply absent.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;

use crate::definition::error::DefinitionError;
use crate::definition::model::VALID_PATH_KEYS;
use crate::definition::value::OrderedMap;
use crate::path_conventions::project::{ARTIFACT_ROUTED_KEYS, ProjectPathConventions};
use crate::path_conventions::result::{PathConventionRefusal, PathValidationResult};
use crate::path_conventions::tokens::{
    authored_path, contains_token, normalize_path_token, path_text, prefix_required_path,
    suggest_directory_creation,
};

/// Everything the validation needs, as explicit inputs.
///
/// The caller supplies the resolved definition facts (name, declared paths,
/// declared artifact tokens, mission-type slug) and the roots it has approved.
/// Nothing is guessed: `feature_dir` is used only when supplied, and a candidate
/// source root only qualifies when the caller passes it.
#[derive(Clone, Copy, Debug)]
pub struct PathValidationInputs<'a> {
    /// The Mission's display name (`mission.name`).
    pub mission_name: &'a PythonString,
    /// Declared path conventions in authored order (`mission.config.paths`).
    pub declared_paths: &'a OrderedMap<PythonString>,
    /// Normalized declared artifact tokens (required + optional).
    pub artifact_tokens: &'a [PythonString],
    /// The mission-type slug used to name the project-override route.
    pub mission_type_slug: Option<&'a PythonString>,
    /// The workspace/worktree root.
    pub project_root: &'a Path,
    /// Research deliverables prefix, when that mode applies.
    pub path_prefix: Option<&'a PythonString>,
    /// The Mission's PRIMARY-surface directory (`kitty-specs/<mission>/`).
    pub feature_dir: Option<&'a Path>,
    /// The project `path_conventions` override, when the project declares one.
    pub path_overrides: Option<&'a ProjectPathConventions>,
    /// Approved candidate source roots (§#4254).
    pub candidate_source_roots: &'a [PathBuf],
    /// Normalized artifact tokens the Mission explicitly waived.
    pub waived_artifact_tokens: &'a [PythonString],
    /// Raise on missing paths instead of returning the result.
    pub strict: bool,
}

/// The validator's output: the result plus the `warnings.warn` channel.
///
/// Source emits some diagnostics through `warnings.warn` (an ignored
/// artifact-colliding override) rather than through the result's `warnings`
/// field; [`PathValidationOutcome::notices`] is that channel, kept separate so
/// the result's own fields stay exactly comparable with Source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathValidationOutcome {
    /// The validation result.
    pub result: PathValidationResult,
    /// Module-level warnings Source emitted while validating.
    pub notices: Vec<PythonString>,
}

/// A path-convention failure: a strict refusal, a malformed project
/// `path_conventions` section, or an input that cannot be analysed (an authored
/// path with no filesystem form, non-UTF-8 path text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathConventionError {
    /// Strict mode with missing paths (Source `PathValidationError`).
    Refused(Box<PathConventionRefusal>),
    /// The project `path_conventions` section is malformed (Source
    /// `PathConventionsConfigError`).
    Config(crate::path_conventions::project::PathConventionsConfigError),
    /// The inputs could not be analysed at all.
    Invalid(Box<DefinitionError>),
}

impl From<DefinitionError> for PathConventionError {
    fn from(error: DefinitionError) -> Self {
        PathConventionError::Invalid(Box::new(error))
    }
}

impl std::fmt::Display for PathConventionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathConventionError::Refused(refusal) => refusal.fmt(formatter),
            PathConventionError::Config(error) => error.fmt(formatter),
            PathConventionError::Invalid(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PathConventionError {}

/// Validate a Mission's declared paths (Source `validate_mission_paths`).
pub fn validate_mission_paths(
    inputs: PathValidationInputs<'_>,
) -> Result<PathValidationOutcome, PathConventionError> {
    // Artifact routing is active only on the primary surface and never in
    // research's prefix mode.
    let artifact_routing: &[PythonString] = match (inputs.feature_dir, inputs.path_prefix) {
        (Some(_), None) => inputs.artifact_tokens,
        _ => &[],
    };

    let mut notices = Vec::new();
    let declared = inputs.declared_paths;
    let safe_overrides: Option<OrderedMap<PythonString>> = inputs.path_overrides.map(|overrides| {
        drop_overrides_colliding_with_artifacts(overrides, declared, artifact_routing, &mut notices)
    });
    let remapped = remap_declared_paths(declared, safe_overrides.as_ref());

    let mut required_paths = OrderedMap::new();
    for (key, relative) in remapped.iter() {
        required_paths.insert(
            key.clone(),
            prefix_required_path(inputs.path_prefix, relative)?,
        );
    }

    let mut result = PathValidationResult {
        mission_name: inputs.mission_name.clone(),
        required_paths,
        mission_type: inputs.mission_type_slug.cloned(),
        ..PathValidationResult::default()
    };
    if result.required_paths.is_empty() {
        return Ok(PathValidationOutcome { result, notices });
    }

    let waived: BTreeSet<Vec<u32>> = inputs
        .waived_artifact_tokens
        .iter()
        .map(|token| token.codepoints().to_vec())
        .collect();
    let required: Vec<(PythonString, PythonString)> = result
        .required_paths
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    for (key, relative_path) in required {
        let candidate = authored_path(&relative_path)?;
        let declared_value = remapped
            .get(&key)
            .expect("required paths derive from declared");
        let token = normalize_path_token(declared_value);
        let is_artifact_tagged =
            !candidate.is_absolute() && contains_token(artifact_routing, &token);

        let full_path = if candidate.is_absolute() {
            candidate.clone()
        } else if is_artifact_tagged {
            let waived_key = token.codepoints().to_vec();
            if waived.contains(&waived_key) {
                // #5298: the Mission declared it has no such artifact.
                continue;
            }
            inputs
                .feature_dir
                .expect("artifact routing requires a feature dir")
                .join(&candidate)
        } else {
            inputs.project_root.join(&candidate)
        };

        if full_path.exists() {
            result.existing_paths.push(relative_path);
            continue;
        }

        if !is_artifact_tagged
            && !inputs.candidate_source_roots.is_empty()
            && !candidate.is_absolute()
        {
            let satisfying = inputs
                .candidate_source_roots
                .iter()
                .find(|root| root.join(&candidate).exists());
            if let Some(root) = satisfying {
                result.existing_paths.push(relative_path.clone());
                result.satisfied_by.insert(relative_path, path_text(root)?);
                continue;
            }
        }

        let mut resolved = match full_path.strip_prefix(inputs.project_root) {
            Ok(relative) => path_text(relative)?,
            Err(_) => path_text(&full_path)?,
        };
        if relative_path.codepoints().last() == Some(&u32::from(b'/'))
            && !resolved.is_empty()
            && resolved.codepoints().last() != Some(&u32::from(b'/'))
        {
            resolved.push_scalars("/");
        }

        result.missing_paths.push(resolved.clone());
        let mut warning = inputs.mission_name.clone();
        warning.push_scalars(" expects ");
        warning.append(&key);
        warning.push_scalars(" path: ");
        warning.append(&resolved);
        warning.push_scalars(" (not found)");
        result.warnings.push(warning);
        if !is_artifact_routed_key(&key) {
            result.remappable_missing_keys.push(key.clone());
        }
        if is_artifact_tagged {
            result
                .missing_artifact_tokens
                .push(normalize_path_token(&relative_path));
        }
    }

    if !result.missing_paths.is_empty() {
        result.suggestions = suggest_directory_creation(&result.missing_paths);
        if inputs.strict {
            return Err(PathConventionError::Refused(Box::new(
                PathConventionRefusal { result },
            )));
        }
    }

    Ok(PathValidationOutcome { result, notices })
}

fn is_artifact_routed_key(key: &PythonString) -> bool {
    ARTIFACT_ROUTED_KEYS
        .iter()
        .any(|routed| key.to_scalar_string().is_ok_and(|scalar| scalar == *routed))
}

/// Source `_drop_overrides_colliding_with_artifacts`: drop (and warn about) an
/// override whose value is a declared artifact token.
fn drop_overrides_colliding_with_artifacts(
    overrides: &ProjectPathConventions,
    declared: &OrderedMap<PythonString>,
    artifact_tokens: &[PythonString],
    notices: &mut Vec<PythonString>,
) -> OrderedMap<PythonString> {
    if artifact_tokens.is_empty() {
        return overrides.overrides.clone();
    }
    let mut safe = OrderedMap::new();
    for (key, value) in overrides.overrides.iter() {
        if declared.contains_key(key)
            && contains_token(artifact_tokens, &normalize_path_token(value))
        {
            let mut notice = PythonString::from_scalars("project.path_conventions override for ");
            notice.push_scalars(&key.repr());
            notice.push_scalars(" (");
            notice.push_scalars(&value.repr());
            notice.push_scalars(
                ") collides with a mission artifact token and was ignored to avoid flipping \
                 path-resolution routing.",
            );
            notices.push(notice);
            continue;
        }
        safe.insert(key.clone(), value.clone());
    }
    safe
}

/// Source `_remap_declared_paths`: remap-only, artifact-routed keys excluded.
fn remap_declared_paths(
    declared: &OrderedMap<PythonString>,
    overrides: Option<&OrderedMap<PythonString>>,
) -> OrderedMap<PythonString> {
    let Some(overrides) = overrides else {
        return declared.clone();
    };
    let mut remapped = OrderedMap::new();
    for (key, value) in declared.iter() {
        let replacement = if is_artifact_routed_key(key) {
            None
        } else {
            overrides.get(key)
        };
        remapped.insert(
            key.clone(),
            replacement.cloned().unwrap_or_else(|| value.clone()),
        );
    }
    remapped
}

/// The declared keys a project override could remap (`VALID_PATH_KEYS` minus the
/// artifact-routed ones).
pub fn remappable_keys() -> Vec<&'static str> {
    VALID_PATH_KEYS
        .iter()
        .copied()
        .filter(|key| !ARTIFACT_ROUTED_KEYS.contains(key))
        .collect()
}
