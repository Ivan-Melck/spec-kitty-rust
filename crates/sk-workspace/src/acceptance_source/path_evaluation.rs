//! Source `evaluate_path_conventions` inputs and the fact mapping.
//!
//! Source's acceptance path decides path-convention facts from four inputs that
//! this module assembles through their canonical owners:
//!
//! * the resolved Mission definition's declared `paths` (the definition reader),
//! * the research deliverables prefix (`mission.get_deliverables_path`),
//! * the approved lanes' worktrees, which may legitimately hold a build path the
//!   primary checkout does not have yet (`_approved_lane_source_roots`, #4254),
//! * the project's `project.path_conventions` remap, the `meta.json` contracts
//!   waiver, and the mission-type slug used by the remap hint — all read by the
//!   evaluator itself, so this module never re-reads them.
//!
//! The evaluator returns text and tokens; [`path_convention_facts`] maps them
//! into the Status carrier. Strict vs lenient (`--lenient`) selects which text
//! and which dedup set Source produces, so the caller must pass the same option
//! it will pass to the policy.

use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_mission::path_conventions::evaluate::{
    EvaluatePathConventionsInputs, evaluate_path_conventions,
};
use sk_mission::path_conventions::research_path_prefix;
use sk_mission::{MissionDefinition, SafeDocumentDecoder};
use sk_status::acceptance::source::{PathConventionFacts, SourceWpFacts};

use crate::error::WorkspaceError;
use crate::lanes::LanesManifest;
use crate::naming::{PLANNING_LANE_ID, worktree_path};

/// Everything one path-convention evaluation needs, as explicit inputs.
#[derive(Clone, Copy)]
pub struct PathConventionRequest<'a> {
    /// The resolved definition, or `None` when Source found no Mission.
    pub definition: Option<&'a MissionDefinition>,
    /// The resolver's mission-type slug (`ResolvedMissionDefinition::mission_type_slug`).
    pub mission_type_slug: Option<&'a str>,
    /// The repository/worktree root the caller passed to acceptance.
    pub repo_root: &'a Path,
    /// The Mission's PRIMARY-surface directory (`kitty-specs/<mission>/`).
    pub feature_dir: &'a Path,
    /// Where planning reads resolve (the same PRIMARY surface here).
    pub planning_read_dir: &'a Path,
    /// The Mission directory name (Source `feature_dir.name`).
    pub mission_slug: &'a str,
    /// Build paths of approved lanes, per [`approved_lane_source_roots`].
    pub candidate_source_roots: &'a [PathBuf],
    /// Source's `strict_metadata` (`not lenient`).
    pub strict_metadata: bool,
    /// The YAML decoder for `config.yaml` / `meta.json` reads.
    pub decoder: &'a dyn SafeDocumentDecoder,
}

/// Source `evaluate_path_conventions`, mapped onto the acceptance carrier.
///
/// A definition that declares no paths (or that did not resolve) is Source's
/// no-op [`PathConventionFacts::NotDeclared`]. Otherwise the evaluator's real
/// result is reported, with the contracts-waiver notice kept separate from the
/// blocking/warning text because the policy appends it.
///
/// # Errors
///
/// A malformed `project.path_conventions` section, or a declared path that
/// cannot be analysed (an unencodable authored path), is a Source exception on
/// this path; it is surfaced as [`WorkspaceError::InvalidArtifact`] rather than
/// being reported as a satisfied or absent convention.
pub fn path_convention_facts(
    request: PathConventionRequest<'_>,
) -> Result<PathConventionFacts, WorkspaceError> {
    let Some(definition) = request.definition else {
        return Ok(PathConventionFacts::NotDeclared);
    };
    if definition.paths().is_empty() {
        return Ok(PathConventionFacts::NotDeclared);
    }

    let path_prefix = research_path_prefix(
        definition.domain.is_research(),
        request.planning_read_dir,
        request.mission_slug,
    );
    let mission_type_slug = request.mission_type_slug.map(PythonString::from_scalars);

    let evaluation = evaluate_path_conventions(
        EvaluatePathConventionsInputs {
            definition: Some(definition),
            repo_root: request.repo_root,
            feature_dir: request.feature_dir,
            planning_read_dir: request.planning_read_dir,
            path_prefix: path_prefix.as_ref(),
            mission_type_slug: mission_type_slug.as_ref(),
            strict_metadata: request.strict_metadata,
            candidate_source_roots: request.candidate_source_roots,
            path_overrides: None,
            waiver: None,
        },
        request.decoder,
    )
    .map_err(|error| WorkspaceError::InvalidArtifact {
        artifact: "mission path conventions",
        detail: error.to_string(),
    })?;

    let satisfied = evaluation.result.missing_paths.is_empty();
    // The policy owns the strict/lenient choice; hand it the raw rendered text
    // and keep the waiver notice out of it so it is appended exactly once.
    let (blocking_text, warning_text) = if satisfied {
        (None, None)
    } else if request.strict_metadata {
        (Some(text_of(&evaluation.result.error_text())), None)
    } else {
        (None, Some(text_of(&evaluation.result.warning_text())))
    };
    Ok(PathConventionFacts::Evaluated {
        satisfied,
        blocking_text,
        warning_text,
        notice: evaluation.waiver.notice.as_ref().map(text_of),
        dedup_tokens: evaluation.dedup_tokens.iter().map(text_of).collect(),
    })
}

/// Source `_approved_lane_source_roots`: the worktrees of this Mission's fully
/// approved code lanes.
///
/// Only lanes whose every work package is already `approved`/`done` qualify, so
/// an unapproved lane (or any other checkout on disk) can never satisfy a
/// declared path. The planning lane has no worktree (its work lands in the
/// write checkout the check already reads) and a lane with no work packages
/// contributes nothing. A missing or corrupt manifest yields no roots at all,
/// exactly as Source's `read_lanes_json` guard does.
pub fn approved_lane_source_roots(
    repo_root: &Path,
    manifest: Option<&LanesManifest>,
    work_packages: &[SourceWpFacts],
) -> Vec<PathBuf> {
    let accepted: Vec<&str> = work_packages
        .iter()
        .filter(|wp| matches!(wp.lane.as_deref(), Some("approved" | "done")))
        .map(|wp| wp.wp_id.as_str())
        .collect();
    if accepted.is_empty() {
        return Vec::new();
    }
    let Some(manifest) = manifest else {
        return Vec::new();
    };

    let mut roots = Vec::new();
    for lane in &manifest.lanes {
        if lane.lane_id == PLANNING_LANE_ID || lane.wp_ids.is_empty() {
            continue;
        }
        if !lane
            .wp_ids
            .iter()
            .all(|wp_id| accepted.contains(&wp_id.as_str()))
        {
            continue;
        }
        let Ok(candidate) = worktree_path(repo_root, &manifest.mission_slug, &lane.lane_id) else {
            // Source's `predict_lane_worktree` raises for exactly the planning
            // lane (already skipped); any other composition failure means no
            // approved root rather than an invented one.
            continue;
        };
        if candidate.is_dir() {
            roots.push(candidate);
        }
    }
    roots
}

/// Exact code points when the text is scalar-representable, Python `repr`
/// escaping otherwise (never a lossy drop).
fn text_of(value: &PythonString) -> String {
    match value.to_scalar_string() {
        Ok(text) => text,
        Err(_) => value.repr(),
    }
}
