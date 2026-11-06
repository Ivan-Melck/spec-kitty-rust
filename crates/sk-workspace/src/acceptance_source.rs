//! Canonical **evidence collector** for the Source-compatible acceptance policy
//! ([`sk_status::acceptance::source`]).
//!
//! The acceptance policy is a pure function of collected facts; this module
//! produces those facts from the canonical domain services instead of letting a
//! CLI adapter hand-roll a second read of lanes, status, mission metadata or
//! Git. Nothing here decides acceptance: every judgement stays in the policy.
//!
//! # What is reused (never re-implemented)
//!
//! * **`lanes.json`** — [`crate::lanes::LanesStore::read`], so a corrupt document
//!   stays [`crate::error::WorkspaceError::CorruptLanes`] (Source's
//!   `CorruptLanesError`) instead of being downgraded to "no lanes".
//! * **Status evidence** — [`sk_status::snapshot::materialize_dir`], the full CLI
//!   snapshot (cancellation provenance + `implementer_of_record` projections
//!   included), and [`sk_status::mutation_readiness::has_operator_provenance`],
//!   the single provenance accessor.
//! * **Work-package frontmatter** — [`sk_mission::split_frontmatter`] over the
//!   authored file; user YAML is read, never rewritten.
//! * **Mission definition** — [`sk_mission::resolve_mission_definition_for_mission_dir`]
//!   through the canonical tier chain, with the installed resource roots supplied
//!   by [`mission_definition::InstalledMissionProviders`] (`sk_kernel`'s packaged
//!   missions tree and `.kittify` home, `sk_charter`'s org-pack chain).
//! * **Path conventions and declared artifacts** —
//!   [`sk_mission::evaluate_path_conventions`] (project remap, research prefix,
//!   approved candidate roots, contracts waiver, rendered texts) and the
//!   definition's own `paths` / `artifacts` blocks.
//! * **Dirty tree** — [`sk_git::GitRepository::status`] (the one Git seam).
//! * **Acceptance matrix** — [`sk_status::acceptance::AcceptanceMatrix`], the one
//!   canonical reader/writer; this module never parses the artifact itself.
//!
//! # Source facts this module collects
//!
//! Grounded by execution of the pinned Source (`9aed3a8012`, CPython 3.11.15):
//! the dirty-gate and decision-status semantics below are receipts
//! `S-dirty-lanes` / `S-dirty-coord` / `S-decisions` in the task report, taken
//! from `acceptance._accept_dirty_gate` and
//! `acceptance._has_blocking_clarification_marker` in an isolated sandbox.
//!
//! 1. **The dirty-tree exclusion set** is collected through the canonical Native
//!    artifact authority, not a private filename list:
//!    * accept-pipeline own writes — [`kind_for_mission_file`] classifies
//!      `acceptance-matrix.json` ([`MissionArtifactKind::AcceptanceMatrix`]) and
//!      the materialized `status.json` snapshot (a [`MissionArtifactKind::StatusState`]
//!      whose basename is the snapshot, never the append-only
//!      `status.events.jsonl`);
//!    * self-bookkeeping through [`crate::coherence::is_self_bookkeeping_churn`],
//!      including depth-exact metadata for any Mission, provenance/audit/derived
//!      views, dossier snapshots, decision locks and Op-record orphans;
//!    * this Mission's decision ledger ([`MissionArtifactKind::DecisionLedger`]),
//!      including a Git-collapsed untracked `decisions/` directory, expanded the
//!      way Source expands it;
//!    * the accept pipeline's own `--normalize-encoding` `.bak` recovery write;
//!    * coordination residue — [`kind_is_coordination_residue`] under the Mission's
//!      STORED topology, so a flat mission's real primary artifacts still block.
//! 2. **The decision ledger is never fabricated.** Source's absent `index.json`
//!    is an EMPTY index, so a marker naming an unknown id blocks — the collector
//!    reports exactly that (`decision_statuses_available: true`, empty map). A
//!    PRESENT ledger is not decoded here: no canonical typed read-only
//!    `DecisionIndex` loader is published to this crate, so the fact stays
//!    explicitly unavailable instead of being guessed from raw JSON.
//! 3. **Mission path conventions and declared artifacts** are now resolved from
//!    the actual Mission definition (see the submodules), which removes the
//!    earlier hardcoded `Unevaluated` path fact and the hardcoded optional-artifact
//!    fallback for a Mission that really resolves. A Mission that does not resolve
//!    is Source's own no-op (`except MissionError: mission = None`), so it reports
//!    [`sk_status::acceptance::source::PathConventionFacts::NotDeclared`] plus
//!    [`declared_artifacts::FALLBACK_OPTIONAL_ARTIFACTS`]; that is a collected
//!    fact, not a guessed verdict.
//!    [`sk_status::acceptance::source::PathConventionFacts::Unevaluated`] stays
//!    reachable in the carrier for callers that cannot evaluate at all.
//! 4. **Non-WP (`plan`-family) acceptance** is the [`non_wp_runtime`] branch.
//!    When the Mission's recorded runtime exists, does NOT declare work-package
//!    iteration, has no `tasks/WP*.md` file, and is complete, the work-package
//!    gathering, the `lanes.json` read and the required trio are replaced by the
//!    runtime's completeness facts and the mission type's DECLARED blocking
//!    artifacts, exactly as Source's `collect_non_wp_runtime_facts` does. The
//!    lane gates are then reported as
//!    [`sk_status::acceptance::source::LanesManifestFacts::NotRequired`] with
//!    Source's one legitimate skip reason, so no matrix is ever expected — and
//!    none is written — for such a Mission.
//!
//! # Known gaps (recorded in the task report)
//!
//! * An activated *sparse* org mission-type registration is resolved by the
//!   provider the composition root passes in (`registered_mission_type`); the
//!   default [`InstalledMissionProviders`] still answers `None` because the
//!   layered registration roster is an activation/doctrine read this crate has
//!   no port for.
//! * The non-WP presence probe reads the PRIMARY planning surface this collector
//!   already resolves (`mission.spec_dir()`). Source additionally maps classified
//!   names through the artifact-placement seam and probes the coordination READ
//!   home; that seam is not composed here.
//!
//! Matrix judgement is the caller's: on the mutating `accept` leg the caller
//! populates review evidence and enforces negative invariants through the
//! canonical writers and hands the judged matrix in; the read-only `--diagnose`
//! leg reads the artifact as-is and declares that the invariants were not
//! executed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_git::GitRepository;
use sk_kernel::KernelError;
use sk_mission::{DefinitionProviders, PyYamlSafe};
use sk_mission::{Mission, SafeDocumentDecoder, routes_through_coordination};
use sk_status::acceptance::AcceptanceMatrix;
use sk_status::acceptance::source::{
    ClarificationDecisionStatus, ClarificationScan, LanesManifestFacts, MatrixEvidence,
    MetaTargetBranch, SourceAcceptanceFacts, SourceAcceptanceOptions, SourceWpFacts,
    clarification_decision_ids,
};
use sk_status::mutation_readiness::has_operator_provenance;
use sk_status::snapshot::SnapshotError;
use sk_status::snapshot::{EVENTS_FILENAME, WorkPackageState, materialize_dir};

use crate::artifact::{MissionArtifactKind, kind_for_mission_file, kind_is_coordination_residue};
use crate::error::WorkspaceError;
use crate::lanes::{LanesManifest, LanesStore};

pub mod declared_artifacts;
pub mod mission_definition;
pub mod non_wp_runtime;
pub mod path_evaluation;

pub use declared_artifacts::FALLBACK_OPTIONAL_ARTIFACTS;
pub use mission_definition::{InstalledMissionProviders, resolve_acceptance_definition};
pub use non_wp_runtime::{
    NonWpAcceptanceFacts, NonWpRuntimeFacts, NonWpRuntimeFactsProvider, NonWpRuntimeRequest,
    collect_non_wp_runtime_facts, is_artifact_only_acceptance,
};
pub use path_evaluation::{approved_lane_source_roots, path_convention_facts};

/// Source `summary_core._ENCODING_BACKUP_SUFFIX` (the `--normalize-encoding`
/// recovery write).
const ENCODING_BACKUP_SUFFIX: &str = ".bak";

/// The daemon-materialized status snapshot basename.
///
/// Deliberately the BASENAME, not the coarse `StatusState` kind: that kind also
/// carries the append-only `status.events.jsonl`, which accept only reads and
/// which therefore still blocks when dirty (Source `_is_accept_pipeline_own_write`).
const STATUS_SNAPSHOT_BASENAME: &str = "status.json";

/// Source's required planning artifacts for `software-dev`
/// (`expected-artifacts.yaml`, resolved by `acceptance._missing_artifacts`).
pub const REQUIRED_ARTIFACTS: [&str; 3] = ["spec.md", "plan.md", "tasks.md"];

/// Source's clarification scan set (`collect_feature_summary`'s
/// `clarification_files` for a work-package mission), in Source's order.
pub const CLARIFICATION_ARTIFACTS: [&str; 6] = [
    "spec.md",
    "plan.md",
    "quickstart.md",
    "tasks.md",
    "research.md",
    "data-model.md",
];

/// Where the acceptance matrix comes from for this run.
///
/// The caller owns the judgement (population/enforcement/persistence) because
/// those are `accept` write-phase decisions; the collector only carries it.
pub enum MatrixInput<'a> {
    /// The caller already judged the matrix through the canonical writers.
    Judged {
        /// The judged matrix.
        matrix: &'a AcceptanceMatrix,
        /// The surface it was read from.
        directory: Option<&'a Path>,
        /// `false` for the read-only `--diagnose` leg.
        negative_invariants_executed: bool,
    },
    /// Read-only leg: read the artifact as-is from `directory`.
    ///
    /// Source's `mutate_matrix=False` (diagnose) path: negative invariants are
    /// NOT executed, and the policy discloses that.
    ReadOnly {
        /// The coordinate-aware matrix surface the caller resolved.
        directory: &'a Path,
    },
    /// The mission never produces a matrix (planning-artifact-only).
    NotRequired,
    /// The caller already ran the canonical matrix execution step and hands the
    /// resulting evidence over as-is (population/enforcement/persistence/
    /// diagnosis all happened in `sk_workspace::acceptance_execution`).
    Computed(&'a MatrixEvidence),
}

/// Everything the collector needs beyond the Mission and the matrix.
///
/// The defaults are the production composition: Source's own strictness
/// (`strict_metadata = True`), the installed Mission-definition providers, and
/// the PyYAML-safe decoder. A caller running `accept --lenient` must pass the
/// same [`SourceAcceptanceOptions`] here as it passes to the policy, because
/// Source selects a different path-convention text and a different artifact
/// dedup set in that mode.
#[derive(Clone, Copy, Default)]
pub struct AcceptanceCollectionOptions<'a> {
    /// Acceptance strictness (`--lenient`).
    pub acceptance: SourceAcceptanceOptions,
    /// Mission-definition providers; `None` uses [`InstalledMissionProviders`].
    pub providers: Option<&'a dyn DefinitionProviders>,
    /// YAML decoder for `mission.yaml`/`config.yaml`; `None` uses [`PyYamlSafe`].
    pub decoder: Option<&'a dyn SafeDocumentDecoder>,
    /// The required planning-artifact names to probe.
    ///
    /// Source resolves these from `expected-artifacts.yaml`
    /// (`resolve_configured_artifact_name`) rather than hardcoding the trio; the
    /// caller can pass the live names here. `None` keeps [`REQUIRED_ARTIFACTS`],
    /// the pinned `software-dev` value, for callers with no manifest port.
    pub required_artifacts: Option<&'a [String]>,
    /// The read-only runtime-facts port for a non-WP (`plan`-family) Mission.
    ///
    /// Source always has the runtime bridge; a caller that composes none leaves
    /// this `None`, which keeps the work-package path (and is disclosed by the
    /// caller) rather than pretending a runtime completed.
    pub non_wp_runtime: Option<&'a dyn NonWpRuntimeFactsProvider>,
}

/// Where the required planning artifacts of a run were read from.
///
/// Reported so a caller can disclose the manifest that actually governed the
/// required-artifact check instead of inferring it from a branch or skip reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequiredArtifactsOrigin {
    /// The work-package path: the caller's configured names were used.
    WorkPackage {
        /// The manifest origin the caller read them from, `None` for the pinned default.
        manifest_origin: Option<String>,
    },
    /// The non-WP artifact-only path: the mission type's DECLARED manifest was used.
    MissionType {
        /// The mission type manifest's origin, `None` when no tier had one.
        manifest_origin: Option<String>,
    },
}

impl RequiredArtifactsOrigin {
    /// The origin label to disclose, if any.
    pub fn manifest_origin(&self) -> Option<&str> {
        match self {
            Self::WorkPackage { manifest_origin } | Self::MissionType { manifest_origin } => {
                manifest_origin.as_deref()
            }
        }
    }

    /// True when the mission type's own declared manifest governed the check.
    pub fn is_mission_type_manifest(&self) -> bool {
        matches!(self, Self::MissionType { .. })
    }
}

/// The collected facts plus where the required artifacts were read from.
pub struct SourceAcceptanceCollection {
    /// The facts the policy judges.
    pub facts: SourceAcceptanceFacts,
    /// The required-artifact provenance (disclosure).
    pub required_artifacts_origin: RequiredArtifactsOrigin,
}

/// Collect the Source acceptance facts for `mission` rooted at `root`.
///
/// This is [`collect_source_acceptance_facts_with`] under the production
/// composition and strict (`not lenient`) Source semantics.
///
/// # Errors
///
/// See [`collect_source_acceptance_facts_with`].
pub fn collect_source_acceptance_facts(
    root: &Path,
    mission: &Mission,
    matrix: MatrixInput<'_>,
) -> Result<SourceAcceptanceFacts, WorkspaceError> {
    collect_source_acceptance_facts_with(
        root,
        mission,
        matrix,
        AcceptanceCollectionOptions::default(),
    )
}

/// Collect the Source acceptance facts for `mission` rooted at `root`.
///
/// # Errors
///
/// A genuinely unreadable input is returned as an error, never collapsed into a
/// benign-looking fact: a `lanes.json` that cannot be read for a reason other
/// than absence/corruption, an unreadable `status.events.jsonl`, an unreadable
/// work-package file, a Mission definition that exists but cannot be read, a
/// malformed `project.path_conventions` section, or a declared path/token that
/// cannot be analysed. Genuine absence is modelled by the facts (Source's
/// legitimate-absence cases) — a missing repository, a missing manifest, a
/// missing artifact, a Mission that does not resolve.
pub fn collect_source_acceptance_facts_with(
    root: &Path,
    mission: &Mission,
    matrix: MatrixInput<'_>,
    options: AcceptanceCollectionOptions<'_>,
) -> Result<SourceAcceptanceFacts, WorkspaceError> {
    collect_source_acceptance_facts_with_origin(root, mission, matrix, options, None)
        .map(|collection| collection.facts)
}

/// Collect the Source acceptance facts AND the required-artifact provenance.
///
/// This is [`collect_source_acceptance_facts_with`] plus the one disclosure the
/// caller cannot derive honestly: which manifest supplied the required
/// planning-artifact names. `package_manifest_origin` is the provenance of the
/// names in `options.required_artifacts` (the caller's `expected-artifacts.yaml`
/// origin, or `None` for the documented pinned default); the non-WP branch
/// reports the mission type's own manifest instead, because that is the manifest
/// it really read.
///
/// # Errors
///
/// See [`collect_source_acceptance_facts_with`].
pub fn collect_source_acceptance_facts_with_origin(
    root: &Path,
    mission: &Mission,
    matrix: MatrixInput<'_>,
    options: AcceptanceCollectionOptions<'_>,
    package_manifest_origin: Option<&str>,
) -> Result<SourceAcceptanceCollection, WorkspaceError> {
    let feature_dir = mission.spec_dir().to_path_buf();
    let repository = match GitRepository::open(root) {
        Ok(repository) => Some(repository),
        Err(sk_git::GitError::NotARepository { .. } | sk_git::GitError::BareRepository { .. }) => {
            None
        }
        Err(error) => return Err(WorkspaceError::Git(error)),
    };

    // The provider composition carries no I/O of its own: selecting it first
    // keeps the facts below on ONE composition while Source's own read order
    // (runtime probe → planning artifacts → definition → path conventions)
    // stays intact.
    let installed = InstalledMissionProviders;
    let providers: &dyn DefinitionProviders = match options.providers {
        Some(providers) => providers,
        None => &installed,
    };
    let pyyaml = PyYamlSafe;
    let decoder: &dyn SafeDocumentDecoder = match options.decoder {
        Some(decoder) => decoder,
        None => &pyyaml,
    };

    // Source `collect_feature_summary` asks the non-WP branch FIRST: a `plan`
    // Mission whose recorded runtime is complete and artifact-only is vouched
    // for by that runtime and its declared artifacts, never by fabricated
    // tasks, lanes or work-package state. `None` keeps every read below
    // unchanged.
    let wp_task_homes = [feature_dir.clone()];
    let non_wp =
        non_wp_runtime::collect_non_wp_runtime_facts(non_wp_runtime::NonWpRuntimeRequest {
            repo_root: root,
            feature_dir: &feature_dir,
            mission_slug: mission.slug().as_str(),
            mission_type: mission.mission_type(),
            wp_task_homes: &wp_task_homes,
            providers,
            decoder,
            runtime: options.non_wp_runtime,
        })?;

    // A non-WP Mission never gathers work packages, never reads `lanes.json`
    // and never materializes the WP snapshot (Source skips both entirely), so
    // those reads — and their error phases — do not happen on this branch.
    let snapshot = if non_wp.is_none() {
        read_snapshot(&feature_dir)?
    } else {
        None
    };
    let (work_packages, explicit_code_wp) = if non_wp.is_none() {
        collect_work_packages(root, &feature_dir, snapshot.as_ref())?
    } else {
        (Vec::new(), false)
    };
    let (lanes_manifest, manifest) = if non_wp.is_none() {
        collect_lanes_manifest(&feature_dir, explicit_code_wp)?
    } else {
        (
            LanesManifestFacts::NotRequired {
                skip_reason: non_wp_runtime::non_wp_skip_reason().to_owned(),
                activity_issues: non_wp
                    .as_ref()
                    .map(|facts| facts.activity_issues.clone())
                    .unwrap_or_default(),
            },
            None,
        )
    };
    let meta_target_branch = match mission.meta_field("target_branch").and_then(|v| v.as_str()) {
        Some(branch) if !branch.trim().is_empty() => MetaTargetBranch::Declared(branch.to_owned()),
        _ => MetaTargetBranch::Undeclared,
    };

    let tasks_md = if non_wp.is_none() {
        read_optional_text(&feature_dir.join("tasks.md"))?
    } else {
        None
    };
    // Source scans exactly the required artifacts for clarification markers on
    // the non-WP branch (`clarification_files = required_artifacts`), so a
    // marker in, say, `data-model.md` cannot block a `plan` Mission.
    let clarification_artifacts: Vec<String> = match &non_wp {
        Some(facts) => facts.required_artifacts.clone(),
        None => CLARIFICATION_ARTIFACTS.map(str::to_owned).to_vec(),
    };
    let (clarifications, decision_statuses) =
        collect_clarifications(&feature_dir, &clarification_artifacts)?;

    // Source reads the Mission definition AFTER the planning artifacts
    // (`get_mission_for_feature` follows `_check_needs_clarification`), so a
    // corrupt ledger or an undecodable artifact still wins the error race.
    let resolved = resolve_acceptance_definition(&feature_dir, providers, decoder)?;
    let definition = resolved.as_ref().map(|resolved| &resolved.definition);

    // Source replaces ONLY the required-artifact check on the non-WP branch
    // (`missing_required = non_wp.missing_artifacts`); the optional set still
    // comes from the Mission's declared `artifacts.optional` below.
    let missing_required: Vec<String> = match &non_wp {
        Some(facts) => facts.missing_artifacts.clone(),
        None => options
            .required_artifacts
            .unwrap_or(&REQUIRED_ARTIFACTS.map(str::to_owned))
            .iter()
            .filter(|name| !feature_dir.join(name.as_str()).exists())
            .cloned()
            .collect(),
    };
    let optional_tokens = declared_artifacts::optional_artifact_tokens(definition);
    let missing_optional =
        declared_artifacts::missing_artifact_texts(&feature_dir, &optional_tokens)?;
    let candidate_roots = approved_lane_source_roots(root, manifest.as_ref(), &work_packages);
    let path_conventions = path_convention_facts(path_evaluation::PathConventionRequest {
        definition,
        mission_type_slug: resolved
            .as_ref()
            .and_then(|resolved| resolved.mission_type_slug()),
        repo_root: root,
        feature_dir: &feature_dir,
        planning_read_dir: &feature_dir,
        mission_slug: mission.slug().as_str(),
        candidate_source_roots: &candidate_roots,
        strict_metadata: options.acceptance.strict_metadata(),
        decoder,
    })?;
    // No linked marker means Source never consults even a corrupt index.
    // Required reads above either completed or propagated their typed error.
    let decision_statuses_available = true;
    let git_dirty = collect_dirty_paths(repository.as_ref(), &feature_dir, mission)?;
    let branch = match &repository {
        Some(repository) => repository.current_branch().map_err(WorkspaceError::Git)?,
        None => None,
    };

    Ok(SourceAcceptanceCollection {
        facts: SourceAcceptanceFacts {
            mission_slug: mission.slug().as_str().to_owned(),
            mission_number: mission.mission_number_display(),
            mission_type: mission.mission_type().map(str::to_owned),
            repo_root: root.display().to_string(),
            feature_dir: feature_dir.display().to_string(),
            tasks_dir: feature_dir.join("tasks").display().to_string(),
            worktree_root: match &repository {
                Some(repository) => repository.root().display().to_string(),
                None => root.display().to_string(),
            },
            primary_repo_root: match &repository {
                Some(repository) => repository.root().display().to_string(),
                None => root.display().to_string(),
            },
            branch,
            lanes_manifest,
            meta_target_branch,
            work_packages,
            status_events_present: feature_dir.join(EVENTS_FILENAME).exists(),
            // The non-WP branch never materializes the work-package snapshot
            // (Source never reads the status diary there); the policy ignores this
            // fact under `LanesManifestFacts::NotRequired`.
            status_snapshot_present: snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.work_packages.is_empty()),
            tasks_md,
            clarifications,
            // Exact IDs from the last required read; each scan also retains its
            // own fresh lookup snapshot, preserving Source's per-artifact timing.
            decision_statuses,
            decision_statuses_available,
            missing_required_artifacts: missing_required,
            missing_optional_artifacts: missing_optional,
            path_conventions,
            git_dirty,
            matrix: matrix_evidence(matrix),
        },
        required_artifacts_origin: match &non_wp {
            Some(facts) => RequiredArtifactsOrigin::MissionType {
                manifest_origin: facts.required_artifacts_source.clone(),
            },
            None => RequiredArtifactsOrigin::WorkPackage {
                manifest_origin: package_manifest_origin.map(str::to_owned),
            },
        },
    })
}

/// Read `lanes.json` through the canonical store.
///
/// The raw manifest is returned alongside the facts because Source's approved
/// candidate-source-root probe re-reads the same document; one read here keeps
/// both answers on one snapshot.
fn collect_lanes_manifest(
    feature_dir: &Path,
    explicit_code_wp: bool,
) -> Result<(LanesManifestFacts, Option<LanesManifest>), WorkspaceError> {
    match LanesStore::new(feature_dir).read() {
        Ok(Some(manifest)) => Ok((
            LanesManifestFacts::Readable {
                target_branch: manifest.target_branch.clone(),
                mission_branch: manifest.mission_branch.clone(),
                mission_has_code: mission_has_code(&manifest, explicit_code_wp),
            },
            Some(manifest),
        )),
        Ok(None) => Ok((
            LanesManifestFacts::Missing {
                message: format!(
                    "lanes.json is required for {}. Run 'spec-kitty agent mission finalize-tasks' to compute execution lanes, or if the mission has already begun executing, run 'spec-kitty doctor mission-state --fix --mission <slug>' to rebuild lanes.json from the event log.",
                    feature_dir.display()
                ),
            },
            None,
        )),
        Err(WorkspaceError::CorruptLanes { path, detail }) => Ok((
            LanesManifestFacts::Corrupt {
                message: format!(
                    "lanes.json at {} is corrupt or malformed: {detail}",
                    path.display()
                ),
            },
            None,
        )),
        Err(error) => Err(error),
    }
}

/// Source `lanes.compute.mission_has_code`: a code lane, or a work package whose
/// frontmatter EXPLICITLY declares a non-planning execution mode.
///
/// Only an explicit `execution_mode` counts (Source's `mode_source ==
/// "frontmatter"` leg): a WP with no declared mode must never flip a
/// planning-only mission into a code mission by inference.
fn mission_has_code(manifest: &LanesManifest, explicit_code_wp: bool) -> bool {
    manifest.has_code_lanes() || explicit_code_wp
}

/// Materialize the canonical snapshot, when the event log is readable.
fn read_snapshot(
    feature_dir: &Path,
) -> Result<Option<sk_status::snapshot::Snapshot>, WorkspaceError> {
    match materialize_dir(feature_dir) {
        Ok(snapshot) => Ok(Some(snapshot)),
        // A missing diary is an empty stream in the snapshot reader; a corrupt
        // one must surface as a real failure rather than "no work packages".
        Err(SnapshotError::Io(source)) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(status_error(feature_dir, error)),
    }
}

/// Map a snapshot-read failure onto this crate's typed failures.
///
/// Parse/shape failures are invalid status artifacts; I/O failures stay I/O.
fn status_error(feature_dir: &Path, error: SnapshotError) -> WorkspaceError {
    match error {
        SnapshotError::InputRead { source, .. } => WorkspaceError::Kernel(source),
        SnapshotError::Io(source) => WorkspaceError::Kernel(KernelError::Io {
            action: "read the status surface",
            path: feature_dir.join(EVENTS_FILENAME),
            source,
        }),
        other => WorkspaceError::InvalidArtifact {
            artifact: "status.events.jsonl",
            detail: other.to_string(),
        },
    }
}

/// One authored work-package file's frontmatter facts.
struct WpAuthoring {
    wp_id: String,
    title: String,
    execution_mode: Option<String>,
}

/// Read every `tasks/*.md` (Source `_iter_work_packages`), sorted, skipping
/// `readme.md`, and pair it with its reduced snapshot state.
fn collect_work_packages(
    root: &Path,
    feature_dir: &Path,
    snapshot: Option<&sk_status::snapshot::Snapshot>,
) -> Result<(Vec<SourceWpFacts>, bool), WorkspaceError> {
    let tasks_dir = feature_dir.join("tasks");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&tasks_dir)
        .map_err(|source| io_error("list", &tasks_dir, source))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "md")
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("readme.md"))
        })
        .collect();
    paths.sort();

    let mut facts = Vec::new();
    let mut explicit_code_wp = false;
    for path in paths {
        let text = read_artifact_text(&path)?;
        let authoring = authoring_of(&path, &text)?;
        if authoring
            .execution_mode
            .as_deref()
            .is_some_and(|mode| mode != "planning_artifact")
        {
            explicit_code_wp = true;
        }
        let state = snapshot.and_then(|snapshot| snapshot.work_packages.get(&authoring.wp_id));
        let relative = path
            .strip_prefix(root)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.display().to_string());
        facts.push(SourceWpFacts {
            wp_id: authoring.wp_id,
            title: authoring.title,
            path: relative,
            lane: state.and_then(|state| state.lane_string().map(str::to_owned)),
            agent: state
                .and_then(|state| state.get("agent"))
                .and_then(scalar_text),
            implementer_of_record: state
                .and_then(WorkPackageState::implementer_of_record)
                .map(str::to_owned),
            assignee: state
                .and_then(|state| state.get("assignee"))
                .and_then(scalar_text),
            shell_pid: state
                .and_then(|state| state.get("shell_pid"))
                .and_then(scalar_text),
            has_operator_provenance: has_operator_provenance(state),
            cancellation_reason: state
                .and_then(WorkPackageState::cancellation_reason)
                .map(str::to_owned),
            cancellation_actor: state
                .and_then(|state| state.get("actor"))
                .and_then(scalar_text),
            cancellation_at: state
                .and_then(WorkPackageState::last_transition_at)
                .map(str::to_owned),
        });
    }
    Ok((facts, explicit_code_wp))
}

/// Source's `str(...)` rendering of a reduced-snapshot scalar.
///
/// The snapshot stores `shell_pid` as a number and the rest as strings; Source
/// stringifies both, so `4242` becomes `"4242"` (never `"4242.0"`).
fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(match number.as_i64() {
            Some(integer) => integer.to_string(),
            None => number.to_string(),
        }),
        serde_json::Value::Bool(flag) => Some(if *flag {
            "True".to_owned()
        } else {
            "False".to_owned()
        }),
        _ => None,
    }
}

/// Parse `work_package_id`, `title` and `execution_mode` from a work package.
fn authoring_of(path: &Path, text: &str) -> Result<WpAuthoring, WorkspaceError> {
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let Ok(frontmatter) = sk_mission::split_frontmatter(text) else {
        // A WP file without frontmatter is still a WP file (Source yields it and
        // lets the status/metadata gates judge it); only the authored fields are
        // then unavailable.
        return Ok(WpAuthoring {
            wp_id: stem,
            title: String::new(),
            execution_mode: None,
        });
    };
    let document: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(frontmatter.yaml).unwrap_or(serde_yaml_ng::Value::Null);
    let field = |name: &str| match document.get(name) {
        Some(serde_yaml_ng::Value::String(text)) => Some(text.clone()),
        Some(serde_yaml_ng::Value::Number(number)) => Some(number.to_string()),
        _ => None,
    };
    Ok(WpAuthoring {
        wp_id: field("work_package_id").unwrap_or(stem),
        title: field("title").unwrap_or_default(),
        execution_mode: field("execution_mode"),
    })
}

/// One filesystem failure, typed through the kernel's `Io` shape.
fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> WorkspaceError {
    WorkspaceError::Kernel(KernelError::Io {
        action,
        path: path.to_path_buf(),
        source,
    })
}

/// Read one optional UTF-8 artifact, tolerating a leading BOM (Source's
/// `encoding="utf-8-sig"`).
fn read_optional_text(path: &Path) -> Result<Option<String>, WorkspaceError> {
    let exists = sk_kernel::paths::source_path_exists(path)
        .map_err(|source| io_error("stat", path, source))?;
    if !exists {
        return Ok(None);
    }
    read_artifact_text(path).map(Some)
}

fn read_artifact_text(path: &Path) -> Result<String, WorkspaceError> {
    let bytes = sk_kernel::read_source_bytes(path)?;
    let text = sk_kernel::python_utf8::decode_utf8_sig(&bytes).map_err(|source| {
        WorkspaceError::ArtifactEncoding {
            path: path.to_path_buf(),
            byte: source.object()[source.start],
            offset: source.start,
            source: Box::new(source),
        }
    })?;
    Ok(sk_kernel::python_text::universal_newlines(text).into_owned())
}

/// Scan Source's clarification artifacts for `NEEDS CLARIFICATION` markers.
type DecisionStatusFacts = BTreeMap<sk_kernel::PythonString, ClarificationDecisionStatus>;

fn collect_clarifications(
    feature_dir: &Path,
    artifacts: &[String],
) -> Result<(Vec<ClarificationScan>, DecisionStatusFacts), WorkspaceError> {
    let mut scans = Vec::new();
    let mut last_statuses = BTreeMap::new();
    for name in artifacts {
        let path = feature_dir.join(name);
        if let Some(text) = read_optional_text(&path)? {
            // Source reads freshly HERE, before moving to the next artifact:
            // a corrupt index must precede a later artifact's encoding error.
            let observed_decision_statuses = if clarification_decision_ids(&text).is_empty() {
                None
            } else {
                let store =
                    crate::decision_store::load_index(path.parent().expect("artifact parent"))
                        .map_err(|error| WorkspaceError::DecisionIndex(Box::new(error)))?;
                let statuses: DecisionStatusFacts = store
                    .statuses()
                    .iter()
                    .map(|(id, status)| {
                        use crate::decision_model::DecisionStatus;
                        let status = match status {
                            DecisionStatus::Open => ClarificationDecisionStatus::Open,
                            DecisionStatus::Deferred => ClarificationDecisionStatus::Deferred,
                            DecisionStatus::Resolved => ClarificationDecisionStatus::Resolved,
                            DecisionStatus::Canceled => ClarificationDecisionStatus::Canceled,
                        };
                        (id.clone(), status)
                    })
                    .collect();
                last_statuses.clone_from(&statuses);
                Some(statuses)
            };
            scans.push(ClarificationScan {
                path: path.display().to_string(),
                text,
                observed_decision_statuses,
            });
        }
    }
    Ok((scans, last_statuses))
}

/// Dirty paths after Source's accept-owned exclusion set is applied.
///
/// Mirrors `acceptance._accept_dirty_gate` through the canonical artifact and
/// self-bookkeeping authorities. Coordination residue is still applied last,
/// only under the Mission's stored topology.
fn collect_dirty_paths(
    repository: Option<&GitRepository>,
    feature_dir: &Path,
    mission: &Mission,
) -> Result<Vec<String>, WorkspaceError> {
    let Some(repository) = repository else {
        return Ok(Vec::new());
    };
    let entries = repository.status().map_err(WorkspaceError::Git)?;
    let slug = mission.slug().as_str();
    let feature_prefix = feature_dir
        .strip_prefix(repository.root())
        .map(|relative| {
            crate::artifact::normalize_posix(&relative.to_string_lossy().replace('\\', "/"))
        })
        .unwrap_or_default();

    let mut remaining: Vec<&sk_git::StatusEntry> = Vec::new();
    for entry in &entries {
        let path = crate::artifact::normalize_posix(&entry.path().display_lossy());
        if crate::coherence::is_self_bookkeeping_churn(entry.path().as_os_str().as_encoded_bytes())
            || is_accept_pipeline_own_write(&path, slug)
            || is_own_encoding_backup_write(&path, &feature_prefix)
            || is_decision_ledger_entry(&path, entry, repository.root(), slug)
        {
            continue;
        }
        remaining.push(entry);
    }
    // Source applies the residue filter LAST, and only when the Mission's stored
    // topology routes through coordination (a flat mission's real primary
    // artifacts must still block).
    if mission.topology().is_some_and(routes_through_coordination) {
        remaining.retain(|entry| {
            let path = crate::artifact::normalize_posix(&entry.path().display_lossy());
            let kind = kind_for_mission_file(&path, Some(slug));
            match (kind, mission.topology()) {
                (Some(kind), Some(topology)) => !kind_is_coordination_residue(kind, topology),
                // Unclassifiable paths are never residue (fail-closed).
                _ => true,
            }
        });
    }
    Ok(remaining
        .into_iter()
        .map(sk_git::StatusEntry::display_lossy)
        .collect())
}

/// Source `_is_accept_pipeline_own_write`: `acceptance-matrix.json` plus the
/// materialized `status.json`, scoped to THIS mission. Never `status.events.jsonl`.
fn is_accept_pipeline_own_write(path: &str, slug: &str) -> bool {
    match kind_for_mission_file(path, Some(slug)) {
        Some(MissionArtifactKind::AcceptanceMatrix) => true,
        Some(MissionArtifactKind::StatusState) => basename(path) == STATUS_SNAPSHOT_BASENAME,
        _ => false,
    }
}

/// Source `_is_own_encoding_backup_write`: `<this mission dir>/…​.bak`.
fn is_own_encoding_backup_write(path: &str, feature_prefix: &str) -> bool {
    !feature_prefix.is_empty()
        && path.starts_with(&format!("{feature_prefix}/"))
        && path.ends_with(ENCODING_BACKUP_SUFFIX)
}

/// Source `ledger_dirt.is_mission_decision_ledger_entry`: this Mission's
/// decision-ledger files (`decisions/index.json` + `DM-*.md`).
///
/// A Git-collapsed untracked `decisions/` entry is expanded the way Source
/// expands it: every file under the directory must classify as the ledger, and
/// an empty directory is NOT ledger dirt (Source's `bool(files)` guard).
fn is_decision_ledger_entry(
    path: &str,
    entry: &sk_git::StatusEntry,
    repository_root: &Path,
    slug: &str,
) -> bool {
    if !entry.is_directory() {
        return kind_for_mission_file(path, Some(slug))
            == Some(MissionArtifactKind::DecisionLedger);
    }
    let directory = repository_root.join(entry.path().to_path_buf());
    let mut files = Vec::new();
    collect_files(&directory, &mut files);
    !files.is_empty()
        && files.iter().all(|file| {
            kind_for_mission_file(&file.to_string_lossy().replace('\\', "/"), Some(slug))
                == Some(MissionArtifactKind::DecisionLedger)
        })
}

/// Repository-relative regular files under `directory`, recursively (Source
/// `_expand_entry`'s `rglob("*")` + `is_file()`). An unreadable directory
/// contributes nothing, so the collapsed entry keeps blocking.
fn collect_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    out.sort();
}

/// The last path component (Source's `basename`).
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Map the caller's [`MatrixInput`] onto the policy's carrier.
fn matrix_evidence(input: MatrixInput<'_>) -> MatrixEvidence {
    match input {
        MatrixInput::Judged {
            matrix,
            directory,
            negative_invariants_executed,
        } => MatrixEvidence::Judged {
            matrix: matrix.clone(),
            directory: directory.map(|dir| dir.display().to_string()),
            negative_invariants_executed,
        },
        MatrixInput::ReadOnly { directory } => {
            let read =
                read_matrix(directory.join(sk_status::acceptance::ACCEPTANCE_MATRIX_FILE_NAME));
            match read {
                Some(matrix) => MatrixEvidence::Judged {
                    matrix,
                    directory: Some(directory.display().to_string()),
                    negative_invariants_executed: false,
                },
                None => MatrixEvidence::Missing {
                    message: matrix_missing_message(directory),
                    directory: Some(directory.display().to_string()),
                },
            }
        }
        MatrixInput::NotRequired => MatrixEvidence::NotRequired,
        MatrixInput::Computed(evidence) => evidence.clone(),
    }
}

/// The Source "matrix required but not found" message for `directory`.
pub fn matrix_missing_message(directory: &Path) -> String {
    let slug = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    format!(
        "Acceptance matrix (acceptance-matrix.json) is required for lane-based features but was not found. This file is normally scaffolded automatically. If it is missing, regenerate it: spec-kitty agent mission finalize-tasks --mission {slug}"
    )
}

/// Read a matrix through the canonical reader; `None` covers both absence and an
/// unreadable document (the caller's diagnosed case is reported by the policy's
/// matrix check).
/// Read the acceptance matrix from `directory` through the canonical reader.
///
/// `Ok(None)` covers genuine absence; a present-but-unreadable document is a
/// typed invalid-artifact failure (Source raises at load: a malformed matrix item
/// is reported, never silently treated as missing).
pub fn read_acceptance_matrix(
    directory: &Path,
) -> Result<Option<AcceptanceMatrix>, WorkspaceError> {
    let path = directory.join(sk_status::acceptance::ACCEPTANCE_MATRIX_FILE_NAME);
    if !sk_kernel::paths::source_path_exists(&path)
        .map_err(|source| io_error("stat", &path, source))?
    {
        return Ok(None);
    }
    let bytes = sk_kernel::read_source_bytes(&path)?;
    let document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| WorkspaceError::InvalidArtifact {
            artifact: "acceptance-matrix.json",
            detail: format!("{}: {error}", path.display()),
        })?;
    let Some(object) = document.as_object() else {
        return Err(WorkspaceError::InvalidArtifact {
            artifact: "acceptance-matrix.json",
            detail: format!("{}: the document is not an object", path.display()),
        });
    };
    Ok(Some(AcceptanceMatrix::from_json(object)))
}

/// Read the matrix for `directory` without surfacing errors (diagnose helper).
fn read_matrix(path: PathBuf) -> Option<AcceptanceMatrix> {
    let directory = path.parent()?.to_path_buf();
    read_acceptance_matrix(&directory).ok().flatten()
}
