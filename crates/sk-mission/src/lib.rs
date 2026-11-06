//! Mission planning: inventory, dependency planning/readiness, typed creation,
//! canonical-template authoring, and `tasks.md` finalization.
//!
//! # Scope and boundaries
//!
//! The read side inspects on-disk planning artifacts without rewriting user
//! Markdown or YAML. The write side is deliberately narrow:
//! [`MissionCreatePlan`] creates a Mission's planning scaffold,
//! [`MissionArtifactPlan`] authors the remaining template-derived planning
//! files, and [`DependencyFinalizePlan`] persists `tasks.md` dependency
//! declarations into WP frontmatter. All three are plan → validate → commit,
//! and every commit is failure-atomic.
//!
//! A committed plan is **not** an allocated or executable Mission. Lanes,
//! branches, worktrees, commits, placement, ownership, and status-event writes
//! are owned by other crates. [`MissionAllocationState::PlanningArtifactsOnly`]
//! makes that boundary explicit in the creation receipt.
//!
//! [`list_missions`] reads `kitty-specs/*/meta.json` with a bounded kernel
//! read, preserving every field (including extension fields) exactly as
//! decoded. [`WorkPackage`] parses the YAML frontmatter of `WP*.md` prompt
//! files without rewriting the Markdown. [`WorkPackageGraph`] validates the
//! dependency DAG (unknown ids, self-dependencies, duplicates, cycles) and
//! produces a deterministic topological order.
//!
//! Authoritative work-package *state* is **not** reconstructed here: there is
//! no event-log replay, no `status.json` reading, and no use of the retired
//! frontmatter `lane`/`history` values. [`dependency_readiness`] evaluates only
//! an explicit [`WpStateMap`] supplied by the caller, and distinguishes a
//! dependency with no entry ([`UnsatisfiedReason::MissingState`]) from one in a
//! non-acceptable lane.
//!
//! Canonical spec/plan/tasks/work-package prose is never invented or vendored:
//! callers inject [`CanonicalTemplate`]s resolved by the charter/template
//! chain. The canonical `tasks/README.md` is the one rendered constant, pinned
//! byte-for-byte by an oracle fixture.
//!
//! # Selection
//!
//! [`select_mission`] accepts a full canonical mission id (ULID), an 8-character
//! `mid8`, or an exact slug. It has no numeric-prefix, slug-tail, or sole-mission
//! fallback, and refuses an ambiguous match rather than guessing.
//!
//! # Limits
//!
//! `meta.json` reads default to [`DEFAULT_META_BYTE_LIMIT`],
//! work-package reads to [`DEFAULT_WORK_PACKAGE_BYTE_LIMIT`], and
//! `tasks.md` reads to [`DEFAULT_TASKS_MD_BYTE_LIMIT`]. Corrupt,
//! oversized, symlinked, duplicate-key, or duplicate-identity input fails
//! closed.

mod artifacts;
mod create;
pub mod definition;
mod error;
pub mod expected_artifacts;
mod frontmatter;
mod graph;
mod identity;
mod issue_matrix;
mod meta;
pub mod migration_rescue;
mod mission;
pub mod path_conventions;
mod paths;
mod readiness;
mod record;
mod review;
mod tasks;
mod template;
mod topology;
mod work_package;
mod write_lock;
mod yaml_patch;

pub use artifacts::{
    ArtifactFile, MissionArtifactPlan, MissionArtifactReceipt, MissionArtifactRequest,
    TaskArtifactError, WorkPackageDraft,
};
pub use create::{
    DEFAULT_MISSION_TYPE, MissionAllocationState, MissionCreateError, MissionCreateOptions,
    MissionCreatePlan, MissionCreateReceipt, MissionCreateRequest, MissionCreateSlug,
    MissionIdentity, MissionRetention, MissionWriteObserver, PlannedFile, PlannedFileKind,
    PlannedFileMode, PriorMissionActivity,
};
pub use definition::{
    CheckoutProviders, CoreSchemaYaml, DefinitionError, DefinitionOrigin, DefinitionProviders,
    DialectFidelity, MissionArtifacts, MissionCommand, MissionDefinition, MissionDomain,
    MissionMcpTools, MissionTaskMetadata, MissionTaskType, MissionValidation, MissionWorkflow,
    OrderedMap, ProjectOnlyProviders, PyYamlSafe, RegisteredMissionType, ResolutionTier,
    ResolvedMissionDefinition, RuamelSafe, SafeDocumentDecoder, SourceValue, WorkflowPhase,
    available_missions, load_mission_definition, recorded_mission_type, require_source_faithful,
    resolve_mission_definition, resolve_mission_definition_for_mission_dir,
};
pub use error::MissionError;
pub use frontmatter::{Frontmatter, FrontmatterError, split_frontmatter};
pub use graph::WorkPackageGraph;
pub use identity::{MID8_LENGTH, MISSION_ID_LENGTH, MissionId, MissionIdError};
pub use issue_matrix::{
    GatingClass, ISSUE_MATRIX_PLACEHOLDER_VERDICT, ISSUE_MATRIX_VERDICTS, IssueReference,
    OWN_REPO_SLUG, Occurrence, classify_occurrence, classify_occurrences,
    detect_issue_references_text, discover_issue_references, gating_issue_numbers,
    validate_issue_matrix_json, validate_issue_matrix_json_text,
};
pub use meta::{
    ACCEPTANCE_HISTORY_CAP, DEFAULT_META_BYTE_LIMIT, META_FILE_NAME, SPECS_DIR_NAME,
    record_acceptance, record_baseline_merge_commit, record_baseline_merge_commit_clear,
    record_consolidation_metadata, record_mission_number, record_pr_merge_baseline,
    stamp_accept_commit,
};
pub use mission::{Mission, list_missions, select_mission};
pub use path_conventions::{
    ContractsWaiver, MissionPathFacts, PathConventionError, PathConventionEvaluation,
    PathValidationInputs, PathValidationResult, ProjectPathConventions, build_warnings,
    evaluate_path_conventions, load_project_path_conventions, mission_path_facts,
    read_contracts_waiver_from_meta, validate_mission_paths,
};
pub use paths::{
    CHECKLISTS_DIR_NAME, MissionPathError, MissionPaths, PLAN_FILE_NAME, RESEARCH_DIR_NAME,
    SPEC_FILE_NAME, STATUS_EVENTS_FILE_NAME, TASKS_DIR_NAME, TASKS_FILE_NAME, TASKS_README_NAME,
};
pub use readiness::{
    DependencyReadiness, UnsatisfiedDependency, UnsatisfiedReason, WpState, WpStateMap,
    dependency_readiness, readiness_for_dependencies, ready_work_packages,
};
pub use record::{MissionRecord, list_mission_records, load_mission_record, select_mission_record};
pub use review::{
    BLE001_REMEDIATION, Ble001Finding, DeadCodeEvidence, GateRecord, MODE_MISMATCH_BODY,
    MissionReviewError, MissionReviewMode, MissionReviewReport, ReviewFinding, ReviewInputs,
    audit_ble001_line, collect_ble001_findings, diagnostic, is_test_only_path, resolve_mode,
    review_mission, validate_issue_matrix, validate_issue_matrix_text,
};
pub use tasks::{
    DEFAULT_TASKS_MD_BYTE_LIMIT, DependencyApplyReceipt, DependencyFinalizePlan, DependencyUpdate,
    TasksAuthoringError, TasksOutline, TasksSection, parse_dependencies_from_tasks_md,
};
pub use template::{
    CanonicalTemplate, MissionTemplateKind, MissionTemplates, TemplateError,
    WorkPackageTemplateContext, render_tasks_readme,
};
pub use topology::{MissionTopology, classify_topology, routes_through_coordination};
pub use work_package::{DEFAULT_WORK_PACKAGE_BYTE_LIMIT, WorkPackage};
pub use write_lock::{MISSION_WRITE_LOCK_TIMEOUT, lock_mission, lock_mission_birth};

// Re-exported so callers can build selectors and state maps without depending
// on the kernel crate directly.
pub use sk_kernel::{MissionSlug, WpId};
