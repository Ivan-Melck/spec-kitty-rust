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
pub mod activation_context;
pub mod activation_reporting;
pub mod activation_selection;
mod body;
mod cascade;
mod catalog_diagnosis;
mod charter_sync;
mod context;
pub mod context_include;
pub mod context_render;
mod context_state;
mod effective_set;
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
mod offering;
mod org;
mod org_charter;
pub mod org_status;
pub mod pack_skill_repository;
mod plan;
pub mod preset_application;
mod profile;
mod profile_construct;
mod profile_model;
mod profile_schema;
mod profile_sections;
mod profile_text;
mod project_governance;
mod python_set;
mod query;
mod reference_catalog;
pub mod reference_pointers;
mod role_capabilities;
pub mod selection_block;
mod source_denied;
pub mod template_nodes;
pub mod text_budget;
mod validate;
mod yaml;

pub use activation::{
    ActivationState, deactivated_urns, deactivated_urns_with_floor, filter_graph_by_activation,
    is_mission_scope_edge, project_activation_state, reference_edges,
    try_filter_graph_by_activation,
};
pub use activation_context::{
    ACTION_WILDCARDS, ALLOWED_ACTIONS, ActivationContextRejection, ActivationSelectorRegistry,
    FINE_GRAINED_TRIGGERS, REGISTERED_TRIGGERS, admits_action, canonical_built_in_root,
    matches_context,
};
pub use activation_reporting::{
    ActivationReportError, ActivationReportSink, SourceStderrSink, StrayReport, legacy_deny_keys,
    report_legacy_deny_keys, report_stray_legacy_keys, warn_once,
};
pub use activation_selection::{
    SPDD_DIRECTIVE_ID, SPDD_DIRECTIVE_NUMERIC_HINT, SPDD_PARADIGM_ID, SPDD_TACTIC_FILL_ID,
    SPDD_TACTIC_REVIEW_ID, SpddActivationError, SpddActivationSelection, is_spdd_reasons_active,
    spdd_activation_selection,
};
pub use body::{
    PayloadError, PayloadFailure, canonical_artifact_body, canonical_artifact_body_exact,
    canonical_artifact_body_exact_path, canonical_artifact_body_exact_value, exact_json_value,
    exact_payload, exact_payload_value, selected_text_field,
};
pub use cascade::{
    CascadeActivationResult, CascadeScope, DeactivationPlan, NO_CASCADE_HINT, NoCascadeReport,
    REFERENCE_RELATIONS, ReferencedArtifact, SharedSkip, cascade_activation_targets,
    cascade_activation_targets_for_state, deactivation_plan, referenced_but_not_cascaded,
    referenced_but_not_cascaded_for_state,
};
pub use catalog_diagnosis::{
    CatalogMissCause, CatalogMissDiagnosis, classify_catalog_miss, classify_scope_filtered_miss,
    diagnose_catalog_miss, emit_catalog_miss_warning, format_catalog_miss_stanza,
    reset_emitted_warnings,
};
pub use charter_sync::{
    CANONICAL_DERIVED_FILES, CHARTER_DIR_RELATIVE, CHARTER_HASH_KEY, CharterStaleness,
    CharterSyncError, CharterSyncResult, METADATA_FILENAME, charter_bundle_staleness,
    charter_directory, charter_markdown_path, charter_read_root, ensure_charter_bundle_fresh,
    resolve_canonical_repo_root, source_cwd_journal, sync_charter_bundle,
};
pub use context::{
    ArtifactEntry, BOOTSTRAP_ACTIONS, CONTEXT_CONTRACT_TOP_LEVEL_KEYS, CONTEXT_SCHEMA_VERSION,
    CharterContext, ContextInputs, ContextMode, ContextPhase, ContextRequest, Delivery, Diagnostic,
    DiagnosticCode, DirectiveEntry, EdgeReference, GovernanceReferenceStatus, OrgCharterBlock,
    OrgCharterPack, OrgGovernancePolicy, ProgressiveDeliveryError, ProjectCharterBlock,
    STATED_DEFAULT_WHEN, TextRail, TextStageBuild, build_action_context,
    build_action_context_structured, build_action_context_text, build_text_stage,
    build_text_stage_with_presence, charter_present, context_roots, context_roots_with_packs_root,
    normalize_context_action, org_charter_block, resolve_action_urn, select_text_rail, typed_kinds,
};
pub use context_state::{
    CONTEXT_STATE_FILE, CONTEXT_STATE_SCHEMA_VERSION, ContextStateBundle, ContextStateError,
    ContextStateValue, KITTIFY_DIRNAME, MIN_EFFECTIVE_DEPTH, load_context_state,
    prepare_context_state, write_context_state,
};
pub use effective_set::{
    ActivationContextError, DIRECTIVE_TOKEN, EffectiveSetError, EffectiveSetService, LEDGER_REASON,
    NATIVE_OFFERING_SERVICE, NativeOfferingService, OfferingContext, OfferingRoots,
    OfferingServiceFactory, REQUIRED_ONLY_REASON, SCAN_FAILURE_PREFIX, SERVICE_BUILD_PREFIX,
    ServiceKeys, UnknownActivationKey, resolve_effective_sets, token_for, valid_activation_keys,
    yaml_key_for_token,
};
pub use error::{DoctrineError, RETIRED_GOVERNANCE_RUNBOOK};
pub use graph::{DrgEdge, DrgGraph, DrgNode, NodeKind, Relation, is_valid_urn, split_urn};
pub use identity::{IdentityFailure, IdentityIndex, IdentityLayer, IdentityLookup, scan_all};
pub use language_scope::{
    BUILD_TOOLS, BuildToolSpec, COMPOUND_PREFIX_GUARD, CONFIG_LANGUAGES_KEY, CONFIG_RELATIVE_PATH,
    INTERVIEW_ANSWERS_RELATIVE_PATH, InferRequest, InferredLanguages, InterviewAnswers, JVM_FAMILY,
    LANGUAGES, LANGUAGES_ANSWER_KEY, LanguageScopeService, LanguageSpec, RESERVED_LANGUAGE_TOKENS,
    SENTINEL_LANGUAGE_TOKENS, SHIPPED_DEFAULT_ANSWERS, SHIPPED_DEFAULT_LANGUAGES_ANSWER,
    UNKNOWN_LANGUAGE, configured_languages, declared_languages, expand_language_families,
    extract_declared_languages, infer_repo_languages, is_catalog_language, is_language_family,
    is_placeholder_language_answer, lacks_specialist_guidance, language_scoped_kinds,
    language_tokens, python_value_repr, read_interview_answers, shipped_default_answer,
};
pub use language_vocabulary::{
    CatalogueRoots, DEFAULT_CACHE_CAPACITY, LANGUAGE_SCOPED_KINDS, LanguageVocabularyCache,
    SCOPE_FIELD, catalogue_roots, scoped_language_vocabulary,
};
pub use ledger::{
    ACTIVATION_LEDGER_REL, ActivationLedger, ActivationReadError, ActivationSource,
    CONFIG_YAML_REL, DenyListChoice, DenySet, EffectiveWhenAbsent, ExactActivationLedger,
    ExactDenySet, ExactIdentityLookup, LedgerData, LedgerReadOptions, LedgerToken,
    ResolvedActivationSource, ScalarProjectionUnavailable, activatable_kinds,
    activation_ledger_path, allow_key, apply_ledger_write, check_deactivation_permitted, deny_key,
    effective_deny, effective_deny_exact, ledger_keys, ledger_may_hold_deny,
    load_activation_ledger, load_activation_ledger_exact, load_activation_ledger_exact_inspection,
    load_activation_ledger_exact_with, load_activation_ledger_inspection,
    load_activation_ledger_with, non_kind_allow_keys, prepare_ledger_write,
    refuse_unmigrated_deny_write, resolve_activation_source, resolve_deny, resolve_deny_exact,
};
pub use loader::{
    GraphProvenanceError, has_graph_files, load_charter_document, load_document_text, load_graph,
    load_graph_or_dir, load_graph_str, load_graph_with_provenance, merge_layers,
};
pub use lock::{
    DEFAULT_LOCK_TIMEOUT, FileLock, SYNTHESIS_LOCK_SUBDIRECTORY, acquire_synthesis_lock,
    lock_path_for, synthesis_lock_path,
};
pub use merge::{
    MergeConflict, MergeOptions, MergeReport, bridge_org_edge, merge_three_layers,
    merge_three_layers_with_options, merge_topology_artifact, resolve_edge_endpoint,
};
pub use offering::{
    ActivationGate, CanonicalOfferingService, ConstructorAbort, GateFailure, NativeOfferingFailure,
    ORG_MISSION_TYPES_SUBDIR, OfferingLoadError, PROJECT_MISSION_TYPES_RELATIVE_TO_KITTIFY,
    WarningLog, availability_entries, availability_stems, mission_type_fallback,
    scan_mission_types_dir,
};
pub use org::{
    ORG_FRAGMENT, OrgEdge, OrgFragment, OrgNode, PACK_DESCRIPTOR, ProjectArtifact, artifact_to_urn,
    canonical_org_kind, load_org_pack, normalize_directive_id, normalize_directive_id_exact,
    project_layer, scan_project_artifacts, singular_urn_kind, walk_project_agent_profile_nodes,
};
pub use org_charter::{
    ActivationEntry, AuthoredDefaults, AuthoredScalar, ExactText, GovernancePolicy, ORG_CHARTER,
    ORG_CHARTER_SCHEMA_VERSION, OrgCharterPolicy, OrgCharterPolicyFold, OrgCharterRawDocument,
    OrgPackContext, PythonInt, RequiredFloor, RequiringPack, configured_org_roots,
    configured_org_roots_with_packs_root, declared_policies, fold_org_charter_policies,
    fold_org_charter_policies_with_context, load_org_charter_policy, normalize_packs_root,
    org_charter_documents, org_charter_documents_with_packs_root, org_charter_raw_documents,
    org_requirable_kinds, org_required_stems, packs_root_override, read_org_required_ids,
    read_org_required_ids_exact, read_org_required_provenance, read_org_required_provenance_exact,
    read_org_required_provenance_with_packs_root, read_org_required_selections,
    read_org_required_selections_exact, read_org_required_tokens, read_org_required_tokens_exact,
    read_org_required_tokens_exact_with_packs_root, read_org_required_tokens_with_packs_root,
    read_org_skill_namespace, read_org_skill_namespace_exact, required_floor,
    required_floor_with_packs_root, resolve_org_expected_artifacts, resolve_org_requirement_kinds,
    selection_overlayable_kinds, union_exact_tokens, union_required_tokens,
    validate_org_required_directive_stems,
};
pub use pack_skill_repository::{
    BUILTIN_TARGET_PREFIX, CLI_ALLOWED_PROGRAM, CLI_TARGET_PREFIX, MAX_SKILL_DESCRIPTION_LENGTH,
    MAX_SKILL_NAME_LENGTH, PACK_SKILL_SUFFIX, PackSkill, PackSkillLoadError, PackSkillRepository,
    PackSkillViolation, RESERVED_PREFIXES, SKILL_ID_MAX_LENGTH, SKILL_NAMESPACE_MAX_LENGTH,
    SkillExpansion, SkillForm, SkillInvocation, SkillParameter, apply_enhancement,
    exact_mapping_graph, rendered_description, rendered_name, rendered_name_exact,
    skill_limit_violation, skill_namespace_violation, skill_namespace_violation_exact,
    validate_pack_skill,
};
pub use plan::{
    ALREADY_IN_FORCE_WARNING, ActivationInputs, ActivationObservation, ActivationPlan,
    ActivationTransaction, DeactivationInputs, EffectiveSet, FROZEN_LIST_NOTE, activation_key,
    activation_write_target, apply_plan, commit_plan_observed, kind_deny_key, plan_activation,
    plan_deactivation,
};
pub use profile::{
    AgentProfile, DEFAULT_MAX_CONCURRENT_TASKS, DEFAULT_ROUTING_PRIORITY, DEFAULT_SCHEMA_VERSION,
    ExactInteger, ProfileBody, ProfileDelivery, ProfileLayer, ProfileLoadAbort,
    ProfileLoadAbortCause, ProfileRepository, SkippedProfile, UNION_LIST_FIELDS,
    applies_to_languages_match, applies_to_languages_match_exact, normalize_languages,
    profile_id_label,
};
pub use profile_sections::{
    ProfileSectionsError, ProfileSectionsInputs, format_profile_directive_code,
    render_profile_sections,
};
pub use project_governance::{
    DEFAULT_TEMPLATE_SET, DEFAULT_TOOL_REGISTRY, GovernanceCatalogue, GovernanceCharterSelection,
    GovernanceResolution, GovernanceResolutionError, ProjectGovernanceError,
    ProjectGovernanceOptions, compact_governance_summary, compact_governance_summary_with,
    governance_charter_selection, resolve_project_governance, resolve_project_governance_with,
};
pub use python_set::{HASH_SEED_FATAL, HashSeedError, validate_hash_seed_from_env};
pub use query::{
    ActionDelivery, ResolvedContext, ResolvedRefs, action_channel_reachable, action_seed_urns,
    agent_profile_seed_urns, deliver_action_context, profile_channel_reachable,
    profile_channel_relations, resolve_context, resolve_transitive_refs, walk_edges,
};
pub use reference_catalog::{
    ReferenceCatalogError, ReferenceEntry, denied_by_kind, denied_kind_of, load_reference_catalog,
    reference_admitted,
};
pub use reference_pointers::{
    REFERENCE_KINDS, REFERENCE_POINTER_LIMIT, ReferencePointerError, ReferenceSourceIndex,
    SelectedReference, distribute_references_across_kinds, filter_references_for_action,
    reference_source_index, resolve_reference_source, select_reference_pointers,
    select_reference_pointers_with_limit,
};
pub use role_capabilities::{
    ROLE_CAPABILITIES, RoleCapabilities, role_capabilities, role_default_verb,
};
pub use selection_block::{
    SelectionBlockError, SelectionBlockInputs, provenance_suffix, render_selection_block,
};
pub use sk_kernel::PythonString;
pub use source_denied::{LoadDeniedOptions, load_denied};
pub use validate::{assert_valid, dangling_endpoints, duplicate_edge_triples, validate_graph};
pub use yaml::{LockScope, PreparedWrite, WriteOutcome, YamlDocument, ensure_mapping, observe};
