//! Path-convention, artifact, waiver and project-remap authority for acceptance.
//!
//! Source's acceptance path asks two questions of a Mission's *declared* layout:
//! is every declared path present, and is the declared artifact set there? The
//! answers come from the resolved Mission definition (`mission.yaml`), the
//! project's `project.path_conventions` remap, the Mission's `contracts: none`
//! waiver, and — for a build path introduced by an approved lane that has not
//! landed yet — the caller's approved candidate source roots. They never come
//! from `meta.json`, which carries no path or artifact configuration.
//!
//! The pieces:
//!
//! * [`tokens`] — the Source token vocabulary: normalization, artifact tokens,
//!   the research prefix join, and directory-creation suggestions.
//! * [`result`] — the `PathValidationResult` contract and both rendered texts.
//! * [`project`] — the narrow, fail-closed `project.path_conventions` reader.
//! * [`waiver`] — the fail-closed `meta.json` contracts waiver.
//! * [`validate`] — `validate_mission_paths`: remap → prefix → routing →
//!   candidate roots → waiver → reporting.
//! * [`evaluate`] — `evaluate_path_conventions`/`build_warnings` and the
//!   read-only [`evaluate::MissionPathFacts`] the parent collector maps.
//!
//! # Boundaries
//!
//! This module decides *facts and text*, not policy and not wiring: the parent
//! owns when acceptance evaluates and how the facts reach Status, and Status owns
//! the acceptance verdict. Nothing here mutates a working tree, a status surface,
//! or a Mission; nothing here invents a "ready" root — a candidate root is only
//! used when the caller passes it, and a `feature_dir` only when supplied.

pub mod evaluate;
pub mod project;
pub mod result;
pub mod tokens;
pub mod validate;
pub mod waiver;

pub use evaluate::{
    CONTRACTS_ARTIFACT, ContractsWaiverEffect, EvaluatePathConventionsInputs, MissionPathFacts,
    PathConventionEvaluation, build_warnings, contracts_waiver_effect, evaluate_path_conventions,
    get_deliverables_path, mission_path_facts, research_path_prefix,
};
pub use project::{
    ARTIFACT_ROUTED_KEYS, PathConventionsConfigError, ProjectPathConventions,
    load_project_path_conventions,
};
pub use result::{PATH_CONVENTIONS_NOT_SATISFIED, PathConventionRefusal, PathValidationResult};
pub use tokens::{
    artifact_tokens_for, authored_path, contains_token, normalize_path_token, path_text,
    prefix_required_path, suggest_directory_creation,
};
pub use validate::{
    PathConventionError, PathValidationInputs, PathValidationOutcome, remappable_keys,
    validate_mission_paths,
};
pub use waiver::{
    CONTRACTS_FIELD, CONTRACTS_RATIONALE_FIELD, CONTRACTS_WAIVER_VALUE, ContractsWaiver,
    read_contracts_waiver_from_meta,
};
