//! Source `acceptance.non_wp_runtime` — artifact-only (non-WP) acceptance facts.
//!
//! Source accepts exactly ONE artifact-producing mission family through its
//! recorded runtime instead of through work packages: `plan`
//! (`_ARTIFACT_ONLY_MISSION_TYPE`). `collect_feature_summary` asks this branch
//! FIRST; when it answers, the WP gathering is replaced entirely
//! (`lanes` stay empty, no metadata issues, no canonical-state issues), the
//! required trio is replaced by the runtime's declared blocking artifacts, the
//! clarification set is the same declared list, `unchecked_tasks` is empty, and
//! no lane gate — `lanes.json`, branch, acceptance matrix — is ever run.
//!
//! Everything below is a faithful port of that branch:
//!
//! * the mission type must be `plan` (`is_artifact_only_acceptance`);
//! * the recorded runtime (never started, advanced or repaired here — the
//!   provider is read-only) must exist and must NOT declare work-package
//!   iteration; a runtime that does declare it keeps the WP path;
//! * any `tasks/WP*.md` file keeps the WP path, even for a `plan` Mission;
//! * a runtime whose recorded family disagrees with the Mission's `mission_type`
//!   is a typed refusal (Source `AcceptanceError`), never a WP fallback;
//! * completeness is judged from the frozen runtime itself: `review` must be a
//!   declared step, every step must be completed, and no issued step, blocked
//!   reason or pending decision may remain;
//! * the blocking artifacts are the mission type's DECLARED
//!   `expected-artifacts.yaml` rows for exactly the runtime's steps
//!   (`required_always` + `required_by_step[step]`, blocking rows only), read
//!   through [`sk_mission::expected_artifacts`] — no hardcoded trio;
//! * per-step guard failures come from the recorded family's guard chain. Only
//!   the `plan` family reaches this branch, so this module ports Source's
//!   `_evaluate_plan_guards` (`specify` → `spec.md`, `research` →
//!   `research.md`, `plan` → `plan.md`, `review` → approval gate only) over a
//!   presence set gathered the way Source's `gather_artifact_presence` gathers
//!   it: the family's projected artifact vocabulary (every `path_pattern` in the
//!   manifest, in projection order) plus Source's two unconditional tasks tags.
//!   That matters: a project override that drops `research.md` from the manifest
//!   must make the `research` guard report it missing even when the file exists,
//!   exactly as Source's vocabulary-scoped presence set does.
//!
//! Nothing here decides acceptance: the facts are handed to
//! [`sk_status::acceptance::source::evaluate_source_acceptance`], which owns the
//! verdict.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_mission::expected_artifacts::{
    ExpectedArtifactManifest, ExpectedArtifactRoots, LoadedExpectedArtifacts,
    load_expected_artifacts_with_origin,
};
use sk_mission::{DefinitionProviders, SafeDocumentDecoder};
use sk_status::acceptance::source::PLANNING_ARTIFACT_ONLY_SKIP_REASON;

use crate::acceptance_source::mission_definition::project_root_for_feature_dir;
use crate::error::WorkspaceError;

/// Source `non_wp_runtime._ARTIFACT_ONLY_MISSION_TYPE`: the one family accepted
/// through its completed runtime instead of work packages.
pub const ARTIFACT_ONLY_MISSION_TYPE: &str = "plan";

/// Source `runtime_bridge_cores.SPEC_ARTIFACT`.
const SPEC_ARTIFACT: &str = "spec.md";
/// Source `runtime_bridge_cores.PLAN_ARTIFACT`.
const PLAN_ARTIFACT: &str = "plan.md";
/// Source `runtime_bridge_cores.TASKS_ARTIFACT`.
const TASKS_ARTIFACT: &str = "tasks.md";
/// Source `runtime_bridge_cores.TASKS_INDEX_ARTIFACT`.
const TASKS_INDEX_ARTIFACT: &str = "wps.yaml";
/// The `research` guard's artifact tag (Source `_evaluate_plan_guards`).
const RESEARCH_ARTIFACT: &str = "research.md";

/// Source `runtime_bridge_cores.MISSING_ARTIFACT_MESSAGE` (`"Required artifact
/// missing: {name}"`) reduced to its prefix, which is how
/// `collect_non_wp_runtime_facts` separates a missing artifact from any other
/// guard failure.
const MISSING_ARTIFACT_PREFIX: &str = "Required artifact missing: ";

/// Source `kernel.mission_runtime_shape.step_ids_declare_wp_iteration`: the
/// sequence declares the canonical `implement` step.
const WP_ITERATION_STEP_ID: &str = "implement";

/// Source `non_wp_runtime.NonWpRuntimeFacts` (minus `wp_driven`, which is
/// derived from `step_ids` here so the rule stays in one place).
///
/// These are read-only facts from a Mission's EXISTING run and frozen workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NonWpRuntimeFacts {
    /// `state.json`'s recorded mission key (the runtime family).
    pub mission_family: String,
    /// Step ids of the run's FROZEN template, in authored order.
    pub step_ids: Vec<String>,
    /// The cursor's completed step ids.
    pub completed_step_ids: Vec<String>,
    /// The currently issued step, when one is outstanding.
    pub issued_step_id: Option<String>,
    /// The recorded blocked reason, when the run is blocked.
    pub blocked_reason: Option<String>,
    /// Pending decision ids, when the runtime awaits a decision.
    pub pending_decision_ids: Vec<String>,
}

impl NonWpRuntimeFacts {
    /// Source `step_ids_declare_wp_iteration(step_ids)`.
    pub fn wp_driven(&self) -> bool {
        self.step_ids
            .iter()
            .any(|step| step == WP_ITERATION_STEP_ID)
    }
}

/// The read-only runtime-facts port.
///
/// The run store and frozen-template authorities live in `sk-run-store` /
/// `sk-runtime`, which this crate does not depend on; the composition root (the
/// CLI adapter) implements this port over
/// [`sk_run_store::RunStore::run_for_mission`] +
/// [`sk_run_store::RunStore::read_run_template`]. A missing implementation is
/// NOT "no runtime recorded": `None` means this build composed no runtime port,
/// which is reported as a collected gap by the caller, never as a refusal.
pub trait NonWpRuntimeFactsProvider {
    /// The recorded runtime facts for `mission_slug`, or `None` when no run has
    /// been recorded for it. Never starts, advances or repairs a run.
    ///
    /// # Errors
    ///
    /// A run that exists but cannot be read (corrupt cursor, frozen-template
    /// drift, unreadable frozen template) is a typed error, never `None`.
    fn runtime_acceptance_facts(
        &self,
        mission_slug: &str,
    ) -> Result<Option<NonWpRuntimeFacts>, WorkspaceError>;
}

/// Source `collect_non_wp_runtime_facts`'s output: the part of an acceptance
/// summary a non-WP runtime supplies.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NonWpAcceptanceFacts {
    /// Runtime-completeness / guard activity issues, in Source's order.
    pub activity_issues: Vec<String>,
    /// Declared blocking artifact names, sorted the way Source returns them.
    pub required_artifacts: Vec<String>,
    /// Guard-reported missing artifacts, sorted.
    pub missing_artifacts: Vec<String>,
    /// The manifest the blocking artifacts were read from (the mission type's
    /// `expected-artifacts.yaml`, with the tier chain's own origin label), or
    /// `None` when no consulted tier had one. Disclosure only: the caller
    /// reports where the names came from instead of inferring it.
    pub required_artifacts_source: Option<String>,
}

/// Everything the non-WP probe reads.
#[derive(Clone, Copy)]
pub struct NonWpRuntimeRequest<'a> {
    /// The repository root the acceptance run was invoked from.
    pub repo_root: &'a Path,
    /// The Mission's PRIMARY-surface directory (`kitty-specs/<mission>/`).
    pub feature_dir: &'a Path,
    /// The Mission directory name (Source `feature_dir.name`).
    pub mission_slug: &'a str,
    /// The recorded `mission_type`, when the Mission declares one.
    pub mission_type: Option<&'a str>,
    /// Every directory whose `tasks/WP*.md` files would make this a WP Mission.
    pub wp_task_homes: &'a [PathBuf],
    /// Mission-definition providers (org pack roots + packaged tree).
    pub providers: &'a dyn DefinitionProviders,
    /// YAML decoder for `expected-artifacts.yaml`.
    pub decoder: &'a dyn SafeDocumentDecoder,
    /// The runtime-facts port; `None` when this build composed none.
    pub runtime: Option<&'a dyn NonWpRuntimeFactsProvider>,
}

/// Source `is_artifact_only_acceptance`.
pub fn is_artifact_only_acceptance(mission_type: Option<&str>) -> bool {
    mission_type == Some(ARTIFACT_ONLY_MISSION_TYPE)
}

/// Source `collect_non_wp_runtime_facts`: `Some(facts)` selects the non-WP
/// acceptance branch, `None` keeps the work-package path.
///
/// # Errors
///
/// A recorded runtime that cannot be read, or a runtime whose family disagrees
/// with the Mission's `mission_type`, is a typed [`WorkspaceError`]: Source
/// raises `AcceptanceError` for both rather than silently falling back to the
/// work-package path. A found-but-invalid `expected-artifacts.yaml` keeps its
/// own typed failure.
pub fn collect_non_wp_runtime_facts(
    request: NonWpRuntimeRequest<'_>,
) -> Result<Option<NonWpAcceptanceFacts>, WorkspaceError> {
    if !is_artifact_only_acceptance(request.mission_type) {
        return Ok(None);
    }
    let Some(runtime) = request.runtime else {
        // No runtime port composed: Source's `collect_non_wp_runtime_facts`
        // always has one, so this build cannot answer the question. The
        // caller discloses it instead of pretending the Mission is WP-driven
        // or that the runtime completed.
        return Ok(None);
    };
    let recorded = runtime
        .runtime_acceptance_facts(request.mission_slug)
        .map_err(|error| WorkspaceError::InvalidArtifact {
            artifact: "mission runtime",
            detail: format!(
                "Cannot read canonical mission runtime for acceptance; no state was changed: {error}"
            ),
        })?;
    let Some(recorded) = recorded else {
        return Ok(None);
    };
    if recorded.wp_driven() {
        return Ok(None);
    }
    if wp_task_files_present(request.wp_task_homes)? {
        return Ok(None);
    }
    let mission_type = request.mission_type.unwrap_or_default();
    if recorded.mission_family != mission_type {
        return Err(WorkspaceError::InvalidArtifact {
            artifact: "mission runtime",
            detail: "Mission type does not match its recorded runtime; acceptance refused"
                .to_owned(),
        });
    }

    let loaded = load_plan_manifest(request)?;
    let manifest = loaded.as_ref().map(|loaded| &loaded.manifest);
    let vocabulary = presence_vocabulary(manifest);
    let present = gather_presence(request.feature_dir, &vocabulary)?;

    let mut activity: Vec<String> = Vec::new();
    if !recorded.step_ids.iter().any(|step| step == "review") {
        activity.push("Plan runtime has no review action; acceptance refused".to_owned());
    }
    let incomplete: Vec<&str> = recorded
        .step_ids
        .iter()
        .filter(|step| !recorded.completed_step_ids.iter().any(|done| done == *step))
        .map(String::as_str)
        .collect();
    let mut sorted_incomplete = incomplete.clone();
    sorted_incomplete.sort_unstable();
    let outstanding_decision_or_result = recorded.issued_step_id.is_some()
        || recorded.blocked_reason.is_some()
        || !recorded.pending_decision_ids.is_empty();
    if recorded.step_ids.is_empty() || !incomplete.is_empty() || outstanding_decision_or_result {
        let remaining = if sorted_incomplete.is_empty() {
            "pending runtime decision/result".to_owned()
        } else {
            sorted_incomplete.join(", ")
        };
        activity.push(format!(
            "Non-WP runtime is not complete; remaining steps: {remaining}"
        ));
    }

    let mut required: BTreeSet<String> = BTreeSet::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for step_id in &recorded.step_ids {
        for spec in required_specs(manifest, step_id) {
            required.insert(spec);
        }
        for failure in plan_guard_failures(step_id, &present) {
            match failure.strip_prefix(MISSING_ARTIFACT_PREFIX) {
                Some(name) => {
                    missing.insert(name.to_owned());
                }
                None => activity.push(format!("{step_id}: {failure}")),
            }
        }
    }
    if required.is_empty() {
        activity.push(
            "Non-WP runtime has no declared blocking artifacts; acceptance refused".to_owned(),
        );
    }

    Ok(Some(NonWpAcceptanceFacts {
        activity_issues: activity,
        required_artifacts: required.into_iter().collect(),
        missing_artifacts: missing.into_iter().collect(),
        required_artifacts_source: loaded.map(|loaded| loaded.origin),
    }))
}

/// The Matrix-gate skip reason a non-WP summary carries (Source
/// `LaneGateOutcome(skip_reason=PLANNING_ARTIFACT_ONLY_SKIP_REASON)`).
pub fn non_wp_skip_reason() -> &'static str {
    PLANNING_ARTIFACT_ONLY_SKIP_REASON
}

/// The mission type's `expected-artifacts.yaml`, resolved through the canonical
/// tier chain for the ORG tier too (Source passes `repo_root` here, unlike the
/// software-dev trio read on the WP path).
fn load_plan_manifest(
    request: NonWpRuntimeRequest<'_>,
) -> Result<Option<LoadedExpectedArtifacts>, WorkspaceError> {
    let mission_type = request.mission_type.unwrap_or(ARTIFACT_ONLY_MISSION_TYPE);
    let project_root = project_root_for_feature_dir(request.feature_dir)?;
    let org_roots = match &project_root {
        Some(root) => request.providers.org_pack_roots(root),
        None => Vec::new(),
    };
    let packaged = match &project_root {
        Some(root) => request.providers.packaged_missions_root(root),
        None => None,
    };
    load_expected_artifacts_with_origin(
        mission_type,
        ExpectedArtifactRoots {
            org_roots: &org_roots,
            packaged_missions_root: packaged.as_deref(),
        },
        request.decoder,
    )
    .map_err(|error| WorkspaceError::InvalidArtifact {
        artifact: "expected-artifacts.yaml",
        detail: error.to_string(),
    })
}

/// Source `_presence_filenames_for` (`project_artifact_name_set(manifest).values()`)
/// plus the two unconditional tasks tags `gather_artifact_presence` always adds.
fn presence_vocabulary(manifest: Option<&ExpectedArtifactManifest>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    if let Some(manifest) = manifest {
        for (_, pattern) in manifest.artifact_names() {
            names.insert(pattern);
        }
    }
    names.insert(TASKS_ARTIFACT.to_owned());
    names.insert(TASKS_INDEX_ARTIFACT.to_owned());
    names
}

/// The presence set Source's `gather_artifact_presence` builds: `is_file()` on
/// every vocabulary name, read from the Mission's PRIMARY planning surface.
fn gather_presence(
    feature_dir: &Path,
    vocabulary: &BTreeSet<String>,
) -> Result<BTreeSet<String>, WorkspaceError> {
    let mut present = BTreeSet::new();
    for name in vocabulary {
        let candidate = feature_dir.join(name);
        let exists = sk_kernel::paths::source_path_exists(&candidate).map_err(|source| {
            WorkspaceError::Kernel(sk_kernel::KernelError::Io {
                action: "probe an expected artifact",
                path: candidate.clone(),
                source,
            })
        })?;
        if exists && candidate.is_file() {
            present.insert(name.clone());
        }
    }
    Ok(present)
}

/// Source `required_artifacts_for(step, family)`: `required_always` +
/// `required_by_step[step]`, `blocking` rows only, as `path_pattern`s.
fn required_specs(manifest: Option<&ExpectedArtifactManifest>, step: &str) -> Vec<String> {
    let Some(manifest) = manifest else {
        return Vec::new();
    };
    manifest
        .required_artifacts_for_step(step)
        .into_iter()
        .map(|spec| spec.path_pattern.clone())
        .collect()
}

/// Source `_evaluate_plan_guards` + `_check_artifact_present`.
fn plan_guard_failures(step: &str, present: &BTreeSet<String>) -> Vec<String> {
    let artifact = match step {
        "specify" => SPEC_ARTIFACT,
        "research" => RESEARCH_ARTIFACT,
        "plan" => PLAN_ARTIFACT,
        // Terminal status-commit step: the publish gate is sufficient.
        "review" => return Vec::new(),
        other => return vec![format!("No guard registered for plan action: {other}")],
    };
    if present.contains(artifact) {
        return Vec::new();
    }
    vec![format!("{MISSING_ARTIFACT_PREFIX}{artifact}")]
}

/// Source's `any(next((home / "tasks").glob("WP*.md"), None) is not None ...)`.
fn wp_task_files_present(homes: &[PathBuf]) -> Result<bool, WorkspaceError> {
    for home in homes {
        let tasks = home.join("tasks");
        let entries = match std::fs::read_dir(&tasks) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(WorkspaceError::Kernel(sk_kernel::KernelError::Io {
                    action: "list the work-package tasks directory",
                    path: tasks,
                    source,
                }));
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with("WP") && name.ends_with(".md") && entry.path().is_file() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
