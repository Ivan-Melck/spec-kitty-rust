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
//!   guard), pure activation/deactivation planning, and one authoritative
//!   transaction: plan-time observation, the canonical cross-language charter
//!   synthesis lock, comment-preserving atomic write and rollback;
//! * org-charter policy import (`required_<kind>` floors with provenance,
//!   `extends:` chains, org expected artifacts/requirement kinds);
//! * scoped, shared-reference-safe cascade activation/deactivation over the
//!   *effective* deny;
//! * action-scoped context and agent-profile lineage resolution, including the
//!   versioned `charter context --json` nested contract (real bodies, delivery,
//!   references, provenance labels and the governance blocks);
//! * the read-only language-scope service: the doctrine-derived scoped-language
//!   vocabulary (built-in, project overlay and configured org chain) and the
//!   configured → compiled → interview inference tiers, with the three-state
//!   no-signal / explicit-empty / declared contract. The runtime CLI profile
//!   gate consumes this service; full language/next parity remains unapproved.
//!
//! Boundaries (contract mirrors under `src/charter/**`, never invoked at runtime):
//!
//! * `sk-charter` remains the read-only owner of the artifact-kind taxonomy,
//!   project config and pack descriptors; this crate consumes its public API and
//!   never edits it.
//! * User-authored YAML is never re-serialised wholesale: mutations go through a
//!   lossless syntax tree and preserve comments, quoting and key order.
//! * A missing pinned pack is an error. No built-in content is invented.
//! * The text `charter context` renderers, mission-type action grains,
//!   interview/synthesis/compiler and pack lifecycle remain explicit gaps; no
//!   fake body, lock or successful stub is substituted for them.
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
mod body;
mod cascade;
mod context;
mod error;
mod graph;
mod identity;
mod language_scope;
mod language_vocabulary;
mod language_yaml;
mod ledger;
mod loader;
mod lock;
mod merge;
mod org;
mod org_charter;
mod plan;
mod profile;
mod query;
mod validate;
mod yaml;

pub use activation::{
    ActivationState, deactivated_urns, deactivated_urns_with_floor, filter_graph_by_activation,
    is_mission_scope_edge, reference_edges, try_filter_graph_by_activation,
};
pub use body::canonical_artifact_body;
pub use cascade::{
    CascadeActivationResult, CascadeScope, DeactivationPlan, NO_CASCADE_HINT, NoCascadeReport,
    REFERENCE_RELATIONS, ReferencedArtifact, SharedSkip, cascade_activation_targets,
    cascade_activation_targets_for_state, deactivation_plan, referenced_but_not_cascaded,
    referenced_but_not_cascaded_for_state,
};
pub use context::{
    ArtifactEntry, BOOTSTRAP_ACTIONS, CONTEXT_CONTRACT_TOP_LEVEL_KEYS, CONTEXT_SCHEMA_VERSION,
    CharterContext, ContextInputs, ContextMode, ContextRequest, Delivery, Diagnostic,
    DiagnosticCode, DirectiveEntry, EdgeReference, GovernanceReferenceStatus, OrgCharterBlock,
    OrgCharterPack, OrgGovernancePolicy, ProjectCharterBlock, STATED_DEFAULT_WHEN,
    build_action_context, context_roots, context_roots_with_packs_root, org_charter_block,
    resolve_action_urn, typed_kinds,
};
pub use error::{DoctrineError, RETIRED_GOVERNANCE_RUNBOOK};
pub use graph::{DrgEdge, DrgGraph, DrgNode, NodeKind, Relation, is_valid_urn, split_urn};
pub use identity::{IdentityIndex, IdentityLayer, IdentityLookup, scan_all};
pub use language_scope::{
    BUILD_TOOLS, BuildToolSpec, COMPOUND_PREFIX_GUARD, CONFIG_LANGUAGES_KEY, CONFIG_RELATIVE_PATH,
    INTERVIEW_ANSWERS_RELATIVE_PATH, InferRequest, InferredLanguages, InterviewAnswers, JVM_FAMILY,
    LANGUAGES, LANGUAGES_ANSWER_KEY, LanguageScopeService, LanguageSpec, RESERVED_LANGUAGE_TOKENS,
    SENTINEL_LANGUAGE_TOKENS, SHIPPED_DEFAULT_ANSWERS, SHIPPED_DEFAULT_LANGUAGES_ANSWER,
    UNKNOWN_LANGUAGE, configured_languages, declared_languages, expand_language_families,
    extract_declared_languages, infer_repo_languages, is_catalog_language, is_language_family,
    is_placeholder_language_answer, lacks_specialist_guidance, language_scoped_kinds,
    language_tokens, read_interview_answers, shipped_default_answer,
};
pub use language_vocabulary::{
    DEFAULT_CACHE_CAPACITY, LANGUAGE_SCOPED_KINDS, LanguageVocabularyCache, SCOPE_FIELD,
    scoped_language_vocabulary,
};
pub use ledger::{
    ACTIVATION_LEDGER_REL, ActivationLedger, ActivationSource, CONFIG_YAML_REL, DenyListChoice,
    DenySet, EffectiveWhenAbsent, ResolvedActivationSource, activatable_kinds,
    activation_ledger_path, allow_key, apply_ledger_write, check_deactivation_permitted, deny_key,
    effective_deny, ledger_keys, load_activation_ledger, prepare_ledger_write,
    refuse_unmigrated_deny_write, resolve_activation_source, resolve_deny,
};
pub use loader::{has_graph_files, load_graph, load_graph_or_dir, load_graph_str, merge_layers};
pub use lock::{
    DEFAULT_LOCK_TIMEOUT, FileLock, SYNTHESIS_LOCK_SUBDIRECTORY, acquire_synthesis_lock,
    lock_path_for, synthesis_lock_path,
};
pub use merge::{
    MergeConflict, MergeOptions, MergeReport, bridge_org_edge, merge_three_layers,
    merge_three_layers_with_options, merge_topology_artifact, resolve_edge_endpoint,
};
pub use org::{
    ORG_FRAGMENT, OrgEdge, OrgFragment, OrgNode, PACK_DESCRIPTOR, ProjectArtifact, artifact_to_urn,
    canonical_org_kind, load_org_pack, normalize_directive_id, project_layer,
    scan_project_artifacts, singular_urn_kind, walk_project_agent_profile_nodes,
};
pub use org_charter::{
    ActivationEntry, GovernancePolicy, ORG_CHARTER, ORG_CHARTER_SCHEMA_VERSION, OrgCharterPolicy,
    RequiredFloor, RequiringPack, configured_org_roots, configured_org_roots_with_packs_root,
    declared_policies, load_org_charter_policy, normalize_packs_root, org_charter_documents,
    org_charter_documents_with_packs_root, org_requirable_kinds, org_required_stems,
    packs_root_override, read_org_required_provenance,
    read_org_required_provenance_with_packs_root, read_org_required_tokens,
    read_org_required_tokens_with_packs_root, read_org_skill_namespace, required_floor,
    required_floor_with_packs_root, resolve_org_expected_artifacts, resolve_org_requirement_kinds,
    selection_overlayable_kinds, union_required_tokens, validate_org_required_directive_stems,
};
pub use plan::{
    ALREADY_IN_FORCE_WARNING, ActivationInputs, ActivationObservation, ActivationPlan,
    ActivationTransaction, DeactivationInputs, EffectiveSet, FROZEN_LIST_NOTE, activation_key,
    activation_write_target, apply_plan, commit_plan_observed, kind_deny_key, plan_activation,
    plan_deactivation,
};
pub use profile::{
    AgentProfile, ProfileDelivery, ProfileLayer, ProfileRepository, SkippedProfile,
    UNION_LIST_FIELDS, applies_to_languages_match, normalize_languages,
};
pub use query::{
    ActionDelivery, ResolvedContext, ResolvedRefs, action_channel_reachable, action_seed_urns,
    agent_profile_seed_urns, deliver_action_context, profile_channel_reachable,
    profile_channel_relations, resolve_context, resolve_transitive_refs, walk_edges,
};
pub use validate::{assert_valid, dangling_endpoints, duplicate_edge_triples, validate_graph};
pub use yaml::{LockScope, PreparedWrite, WriteOutcome, YamlDocument, ensure_mapping, observe};
