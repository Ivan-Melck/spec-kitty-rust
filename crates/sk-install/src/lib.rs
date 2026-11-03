//! Native initialization, managed-asset installation, pinned pack acquisition
//! and the upgrade/migration substrate.
//!
//! This crate replaces the Python init/installer/pack/metadata/identity/ledger
//! surface with one typed Rust domain:
//!
//! | Area | Owner module |
//! | --- | --- |
//! | Configured agent surfaces (17 keys, 12 slash families, 16 skill roots) | [`agent`] |
//! | Deterministic command/skill rendering + SPDD blocks | [`render`] |
//! | Command-skill ownership manifest (upstream wire format) | [`manifest`] |
//! | Charter skill catalog, shared projection, `skills-manifest.json`, repair | [`skills`] |
//! | Typed plan → validate → commit (indeterminate-commit readback) | [`plan`] |
//! | Fresh/full init, refresh, interrupted delivery, details | [`init`] |
//! | Template tree copy/backup + global-runtime minimal mode + scratch prover | [`template_tree`] |
//! | `.gitignore` runtime/agent block | [`gitignore`] |
//! | `.gitattributes` seed + merge-driver wiring plan | [`attributes`] |
//! | Mission-type activation provisioning | [`activation`] |
//! | Comment-preserving additive YAML editing | [`yaml_edit`] |
//! | Project metadata save/stamp | [`metadata`] |
//! | Project identity (UUIDv4/v5) and `ProjectInitialized` event | [`identity`], [`lifecycle`] |
//! | Vibe skill-path pointer | [`vibe`] |
//! | Two-partition migration ledger + declared catalog | [`ledger`] |
//! | Upgrade selection/execution/version and agent choices | [`upgrade`] |
//! | PEP 440 versions/specifiers and `requires_core` | [`version`] |
//! | Pin lockfile parsing/validation | [`pin`] |
//! | Pack tree digests (Python-compatible) | [`digest`] |
//! | Bounded archive extraction | [`archive`] |
//! | Fetch → verify → cache → place | [`pack`] |
//! | Project commit serialization | [`lock`] |
//!
//! Design commitments:
//!
//! - **Plan before writing.** Every mutation is a plan derived from observed
//!   filesystem state and the ownership manifest; nothing is written until the
//!   whole batch validates.
//! - **User content wins.** A file this tool did not write is never overwritten.
//!   A manifest-owned file with user edits is preserved everywhere except the
//!   derived, git-ignored shared skill projection.
//! - **Deterministic.** Same inputs, same bytes: rendered surfaces, manifest
//!   serialization, cache layout and placement order are all order-stable.
//! - **Fail closed.** Corrupt manifests, unsafe paths, unverified pins, archive
//!   escapes and interrupted swaps are typed refusals, never warnings or
//!   best-effort repairs.
//! - **No Python at runtime.** The Python tree is a reference and a test oracle
//!   only; no production path spawns an interpreter.
//! - **No bundled maintainer doctrine.** Pack content is fetched from the pinned
//!   external source and verified; this crate never ships or invents template
//!   copies, and never reads `packs/internal/`.
//!
//! Deliberate gaps (unimplemented migrations/transports, agent profiles, session
//! presence, global runtime provisioning, canonical event-lock ownership and
//! platform coverage) are listed in the crate README; a green test run here is
//! evidence for the contracts in this crate, not for full upstream parity.

pub mod activation;
pub mod agent;
pub mod archive;
pub mod attributes;
pub mod digest;
pub mod error;
pub mod gitignore;
pub mod identity;
pub mod init;
pub mod ledger;
pub mod lifecycle;
pub mod lock;
pub mod manifest;
pub mod metadata;
pub mod migrations;
pub mod pack;
pub mod pin;
pub mod plan;
pub mod profiles;
pub mod render;
pub mod session;
pub mod skills;
pub mod template_tree;
pub mod upgrade;
pub mod uuid_support;
pub mod version;
pub mod vibe;
pub mod yaml_edit;

pub use activation::{
    ACTIVATION_REL, ActivationTarget, default_preset_mission_types,
    provision_mission_type_activations, resolve_activation_target,
};
pub use agent::{
    AgentSurface, CANONICAL_COMMANDS, CLI_WRAPPER_COMMANDS, CommandSurface, PROMPT_BACKED_COMMANDS,
    RenderFamily, SkillClass, SkillSurface, agent_keys, agent_surface, agents,
    parse_agent_selection, valid_keys,
};
pub use archive::{ArchiveKind, ArchiveLimits, extract_archive};
pub use attributes::{
    GITHUB_DIFF_ATTRIBUTES, MERGE_DRIVERS, MergeDriver, MergeDriverConfigEntry,
    MergeDriverCustomization, MergeDriverWiringPlan, MergeDriverWiringState, command_on_path,
    ensure_attribute_lines, merge_attribute_seed_lines,
};
pub use digest::{TREE_DIGEST_ALGORITHM, clean_tree_digest, full_tree_digest, refuse_symlinks};
pub use error::InstallError;
pub use gitignore::{
    AGENT_BLOCK_BEGIN, AGENT_BLOCK_END, GITIGNORE_MARKER, ProtectionResult, agent_folder_notice,
    agent_ignore_entries, protect_all_agents, replace_agent_block,
};
pub use identity::{
    ProjectIdentity, derive_build_id, ensure_identity, generate_node_id, has_complete_identity,
    load_identity, resolve_identity,
};
pub use init::{
    CURRENT_SCHEMA_CAPABILITIES, CURRENT_SCHEMA_VERSION, InitDetails, InitOutcome, InitReport,
    InitRequest, PendingDelivery, ProjectAssets, ProjectEnvironment, initialize,
    initialize_with_details, is_initialized, pending_delivery, read_configured_agents,
};
pub use ledger::{
    CLONE_LOCAL_LEDGER_REL, LEGACY_MIGRATION_ID_MAP, LedgerRecord, MigrationCatalog,
    MigrationLedger, MigrationSpec, SETTLED_RESULT, TRACKED_LEDGER_REL,
    clone_local_effect_in_place, ledger_lock_path,
};
pub use lifecycle::{
    PROJECT_EVENTS_REL, PROJECT_INITIALIZED, ProjectEventAppendPlan, ProjectEventLine,
    ProjectInitializedPayload, emit_project_initialized, now_utc_z, payload_for_init,
    plan_project_initialized,
};
pub use lock::{ProjectLock, project_lock_path};
pub use manifest::{AssetManifest, MANIFEST_REL, ManifestEntry, content_hash};
pub use metadata::{ProjectMetadata, stamp_schema_metadata};
pub use pack::{
    AcquisitionReport, CacheEntry, FetchEvidence, FileTransport, HttpsArchiveTransport,
    PackAcquirer, PackTransport, PackTreeStatus, copy_tree_bounded, recover_interrupted_swap,
    remove_tree_bounded,
};
pub use pin::{PackPin, TreeDigest, validate_subtree};
pub use plan::{
    ApplicationReport, InstallPlan, Observed, PlanOptions, PlanSummary, PlannedAction,
    PlannedAsset, now_utc_iso, observe,
};
pub use profiles::{
    AgentProfile, PROFILES_MANIFEST_REL, PROJECTION_VERSION, ProfileCatalog, ProfileFormat,
    ProfileManifest, ProfileManifestEntry, ProfileProjectionReport, project_agent_profiles,
    render_codex_agent, render_markdown_agent, render_profile,
};
pub use render::{
    RenderedAsset, ResolvedTemplate, TemplateSource, command_file_rel_path, command_skill_rel_path,
    process_spdd_blocks, render_asset, render_command_template, render_skill_body,
};
pub use session::{
    CLAUDE_SETTINGS_REL, ORIENTATION_INVOCATION, OrientationContent, OrientationHealth,
    SECTION_CLOSE, SECTION_OPEN, SESSION_START_CMD, SESSION_STOP_CMD, SessionPresenceReport,
    apply_healthy_orientation, apply_session_presence,
};
pub use skills::{
    CanonicalSkill, ManagedFileEntry, ManagedSkillManifest, SKILLS_MANIFEST_REL, SkillCatalog,
    SkillInstallOptions, SkillInstallReport, SurfaceRepairSummary, install_skills_for_agent,
    install_skills_for_agents, restore_native_project_skills, run_surface_repair,
};
pub use template_tree::{
    RemovalVerdict, TemplateMode, TemplateTreeOptions, TemplateTreeReport, TemplateTreeSource,
    cleanup_init_scratch, discover_pack_agent_profiles, discover_pack_missions,
    discover_pack_presets, global_runtime_available, guard_destructive_removal,
    install_template_tree,
};
pub use upgrade::{
    AgentCheckInput, AgentCheckPayload, AgentUpgradeChoice, MigrationOutcome, MigrationPlanRow,
    MigrationResultRow, NATIVE_MIGRATIONS, NativeMigration, PlannedOutcome, UpgradePlan,
    UpgradeReport, UpgradeRequest, enforce_requires_core, plan_agent_check, plan_upgrade,
    run_upgrade,
};
pub use uuid_support::Uuid;
pub use version::{SpecifierSet, Version, check_requires_core, parse_requires_core};
pub use vibe::{
    VIBE_CONFIG_REL, VIBE_SKILL_PATH, ensure_project_skill_path, skill_path_configured,
};
pub use yaml_edit::{InsertPosition, MappingEntry, Scalar, YamlEditor};
