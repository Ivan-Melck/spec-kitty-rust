//! Native governance execution for Spec Kitty.
//!
//! This crate owns the executable half of the doctrine domain:
//!
//! * the typed Doctrine Reference Graph (nodes, edges, URNs), strict loading,
//!   additive layer merge, three-layer org/project overlay and validation;
//! * the canonical `scope → requires → suggests → vocabulary` action traversal
//!   and both reachability channels;
//! * org-pack fragment loading, `enhances`/`overrides`/lineage projection and
//!   step/IO-preserving topology merges;
//! * the activation ledger (three-state allow, identity-spaced deny, deny-change
//!   guard), pure activation/deactivation planning, and one locked,
//!   comment-preserving, rollback-safe write;
//! * scoped, shared-reference-safe cascade activation/deactivation;
//! * action-scoped context and agent-profile lineage resolution, including the
//!   versioned `charter context --json` top-level contract.
//!
//! Boundaries (contract mirrors under `src/charter/**`, never invoked at runtime):
//!
//! * `sk-charter` remains the read-only owner of the artifact-kind taxonomy,
//!   project config and pack descriptors; this crate consumes its public API and
//!   never edits it.
//! * User-authored YAML is never re-serialised wholesale: mutations go through a
//!   lossless syntax tree and preserve comments, quoting and key order.
//! * A missing pinned pack is an error. No built-in content is invented.
//! * Org-charter policy import, progressive-disclosure body rendering and the
//!   text context renderers remain explicit gaps; no fake body is substituted.
//!
//! ```no_run
//! use std::path::Path;
//! use sk_doctrine::{load_graph_or_dir, validate_graph};
//!
//! let graph = load_graph_or_dir(Path::new("packs/built-in")).expect("graph");
//! let errors = validate_graph(&graph);
//! assert!(errors.is_empty(), "{errors:?}");
//! ```

mod activation;
mod cascade;
mod context;
mod error;
mod graph;
mod identity;
mod ledger;
mod loader;
mod lock;
mod merge;
mod org;
mod plan;
mod profile;
mod query;
mod validate;
mod yaml;

pub use activation::{
    ActivationState, deactivated_urns, filter_graph_by_activation, is_mission_scope_edge,
    reference_edges,
};
pub use cascade::{
    CascadeActivationResult, CascadeScope, DeactivationPlan, NO_CASCADE_HINT, NoCascadeReport,
    REFERENCE_RELATIONS, ReferencedArtifact, SharedSkip, cascade_activation_targets,
    deactivation_plan, referenced_but_not_cascaded,
};
pub use context::{
    ArtifactEntry, BOOTSTRAP_ACTIONS, CONTEXT_CONTRACT_TOP_LEVEL_KEYS, CONTEXT_SCHEMA_VERSION,
    CharterContext, ContextInputs, ContextMode, ContextRequest, Diagnostic, DiagnosticCode,
    OrgCharterBlock, ProjectCharterBlock, build_action_context, context_roots, resolve_action_urn,
    typed_kinds,
};
pub use error::DoctrineError;
pub use graph::{DrgEdge, DrgGraph, DrgNode, NodeKind, Relation, is_valid_urn, split_urn};
pub use identity::{IdentityIndex, IdentityLookup, scan_all};
pub use ledger::{
    ACTIVATION_LEDGER_REL, ActivationLedger, ActivationSource, CONFIG_YAML_REL, DenyListChoice,
    DenySet, EffectiveWhenAbsent, RequiringPack, ResolvedActivationSource, activatable_kinds,
    activation_ledger_path, allow_key, apply_ledger_write, check_deactivation_permitted, deny_key,
    effective_deny, ledger_keys, load_activation_ledger, prepare_ledger_write,
    refuse_unmigrated_deny_write, resolve_activation_source, resolve_deny,
};
pub use loader::{has_graph_files, load_graph, load_graph_or_dir, load_graph_str, merge_layers};
pub use lock::{DEFAULT_LOCK_TIMEOUT, FileLock, lock_path_for};
pub use merge::{
    MergeConflict, MergeOptions, MergeReport, bridge_org_edge, merge_three_layers,
    merge_three_layers_with_options, merge_topology_artifact, resolve_edge_endpoint,
};
pub use org::{
    ORG_FRAGMENT, OrgEdge, OrgFragment, OrgNode, PACK_DESCRIPTOR, ProjectArtifact, artifact_to_urn,
    canonical_org_kind, load_org_pack, normalize_directive_id, project_layer,
    scan_project_artifacts, singular_urn_kind, walk_project_agent_profile_nodes,
};
pub use plan::{
    ALREADY_IN_FORCE_WARNING, ActivationInputs, ActivationPlan, DeactivationInputs, EffectiveSet,
    FROZEN_LIST_NOTE, activation_key, apply_plan, commit_plan, kind_deny_key, plan_activation,
    plan_deactivation,
};
pub use profile::{AgentProfile, ProfileDelivery, ProfileRepository, UNION_LIST_FIELDS};
pub use query::{
    ActionDelivery, ResolvedContext, ResolvedRefs, action_channel_reachable, action_seed_urns,
    agent_profile_seed_urns, deliver_action_context, profile_channel_reachable,
    profile_channel_relations, resolve_context, resolve_transitive_refs, walk_edges,
};
pub use validate::{assert_valid, dangling_endpoints, duplicate_edge_triples, validate_graph};
pub use yaml::{PreparedWrite, WriteOutcome, YamlDocument, ensure_mapping};
