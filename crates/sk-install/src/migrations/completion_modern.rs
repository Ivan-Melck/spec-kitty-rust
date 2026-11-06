//! The modern (`target_version` major `>= 3`) completion effects.
//!
//! This module is the *implementation side* of the declared catalog entries
//! whose actual catalog `target_version` major is 3 or more and that the first
//! native tranche ([`crate::upgrade::NATIVE_MIGRATIONS`]) does not cover. The
//! partition is file ownership only: runtime *selection* stays declared-minus-
//! applied against the two-partition ledger, capped by `--target`, in catalog
//! order ([`crate::upgrade::select_candidates`]). Nothing here adds a second
//! planner, ordering authority or version window.
//!
//! | id | effect | family |
//! | --- | --- | --- |
//! | `0_9_3_surface_repair_wiring` | sentinel marker + read-only surface probe | [`surface`] |
//! | `3.2.0rc35_strip_selection_config` | drop the orphaned `selection` block | [`surface`] |
//! | `3.2.0rc35_repository_root_checkout_terminology` | proven generated command files only | [`surface`] |
//! | `3_3_0_session_presence_claude_code` | `.claude/CLAUDE.md` + both hooks | [`presence`] |
//! | `3_3_0_session_presence_all_harnesses` | every other configured harness | [`presence`] |
//! | `3_2_0rc39_refresh_orientation_block` | replace a stale installed block | [`presence`] |
//! | `migrate_lifecycle_envelope` | legacy 9-key → strict envelope | [`status_corpus`] |
//! | `3.1.1_residue_safe_normalize_status_json` | deterministic `materialized_at`, residue untouched | [`status_corpus`] |
//! | `3.2.8_provision_kitty_env` | `.kittify/.kitty.env` + pointer + ignore coverage | [`env_kitty`] |
//!
//! The remaining declared ids in this partition are **not** implemented here and
//! are deliberately absent from [`IMPLEMENTATIONS`]: an unimplemented selection
//! is a typed pre-write refusal in [`crate::upgrade`], which is the honest state
//! while the authority they need is missing. `/tmp/sk-cont-modern2-migrations-report.md`
//! records each one with the exact, re-verified reason.
//!
//! Further effects, by family:
//!
//! * [`retired_activation`] / [`context_sources`] — the shared consumer
//!   retirement engine (`3.2.6_retire_rtk_search_tooling`,
//!   `4.0.0rc5_retire_single_owner_doctrine_ids`) and the profile
//!   `context-sources` consolidation, both through the round-trip CST editor.
//! * [`commands_install`] — the command/skill installer family, through the one
//!   renderer and the `InstallPlan` ownership manifest.
//! * [`op_record`] — legacy `kitty-ops/*.jsonl` → the v2 `sk_invocation` models.
//! * [`activation_ledger`] — legacy activation keys → the one ledger, through
//!   the doctrine writer + synthesis-manifest hash restamp.
//! * [`mission_spine`] — verdict-provenance backfill and the single-branch
//!   topology restamp, through [`sk_status::migration_backfill`].
//!
//! ## The still-missing modern ids, by missing authority
//!
//! The charter-bundle family ([`charter_bundle`]) now owns the canonical YAML
//! writer/hash (`sk_charter::canonical_yaml`, verified byte-for-byte against the
//! pinned Source), the v1→v2 migration, the v2 defaults repair and the unified
//! bundle seal. [`skills_global`] owns the user-global skill relink through the
//! installer's symlink delivery mode, and [`crate::render::shim`] owns the thin
//! canonical command shims.
//!
//! * **Remaining charter-synthesis effects**: `charter_pack_cutover`,
//!   `consolidate_charter_bundle_fold`, `3.2.7_heal_provenance_paths`,
//!   `4_0_0rc5_heal_template_set_provenance`,
//!   `4.0.0rc8_project_intent_directive_rename`,
//!   `4.0.0rc8_promote_interview_selections` (see the F3 report for the exact
//!   remaining service each one needs).
//! * **Pack-skill catalog**: `3.0.3_globalize_skill_pack` resolves the canonical
//!   offering catalog; a project's charter-pack skills need the Source
//!   preparation/render chain, which is not ported yet.
//! * **Mission rescue spine**: `3.0.0_canonical_context` and
//!   `3.1.1_charter_rename` call the Source `migration.runner`;
//!   `3.2.0a4_normalize_mission_lifecycle` needs identity/derived-view writers;
//!   `runtime_state_backfill` needs the full `backfill_runtime_state` +
//!   `runtime_state_cutover` seed/verify subsystem (~2.9k Source lines). The
//!   read-only [`sk_status::migration_backfill::read_legacy_runtime`] half is
//!   ported and tested; the seed/verify writer is not.
//!
//! Every effect reuses an existing canonical authority: the session-presence
//! writer ([`crate::session`]), the round-trip YAML editor
//! ([`crate::yaml_edit`]), the installer renderer and ownership manifest
//! ([`crate::render`], [`crate::plan`], [`crate::skills`]), the v2 invocation
//! models ([`sk_invocation`]), the doctrine ledger writer ([`sk_doctrine`]), the
//! status envelope/reducer/event writers ([`sk_status`]), and the kernel's
//! no-follow file writers.

pub mod activation_ledger;
pub mod charter_bundle;
pub mod charter_synthesis;
pub mod commands_install;
pub mod context_sources;
pub mod env_kitty;
pub mod global_runtime;
pub mod mission_spine;
pub mod op_record;
pub mod presence;
pub mod rescue_spine;
pub mod retired_activation;
pub mod skills_global;
pub mod status_corpus;
pub mod support;
pub mod surface;

use crate::upgrade::NativeMigration;

/// Compatibility version for Source orientation/ledger protocols, through the
/// single Kernel authority (not the Native Cargo package version).
pub const COMPAT_CLI_VERSION: &str = sk_kernel::COMPAT_CORE_VERSION;

/// Every modern-completion migration implemented by this module.
///
/// The parent registry folds this slice into the one canonical
/// [`crate::upgrade::NATIVE_MIGRATIONS`] implementation list; the ids here must
/// never be duplicated there.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[
    surface::IMPLEMENTATIONS[0],
    surface::IMPLEMENTATIONS[1],
    surface::IMPLEMENTATIONS[2],
    presence::IMPLEMENTATIONS[0],
    presence::IMPLEMENTATIONS[1],
    presence::IMPLEMENTATIONS[2],
    status_corpus::IMPLEMENTATIONS[0],
    status_corpus::IMPLEMENTATIONS[1],
    env_kitty::IMPLEMENTATIONS[0],
    commands_install::IMPLEMENTATIONS[0],
    commands_install::IMPLEMENTATIONS[1],
    commands_install::IMPLEMENTATIONS[2],
    commands_install::IMPLEMENTATIONS[3],
    commands_install::IMPLEMENTATIONS[4],
    commands_install::IMPLEMENTATIONS[5],
    commands_install::IMPLEMENTATIONS[6],
    commands_install::IMPLEMENTATIONS[7],
    commands_install::IMPLEMENTATIONS[8],
    commands_install::IMPLEMENTATIONS[9],
    commands_install::IMPLEMENTATIONS[10],
    commands_install::IMPLEMENTATIONS[11],
    NativeMigration {
        id: "3_3_1_context_sources_consolidation",
        detect: context_sources::detect,
        can_apply: crate::upgrade::always_applicable,
        apply: context_sources::apply,
    },
    NativeMigration {
        id: "3.2.6_retire_rtk_search_tooling",
        detect: detect_rtk_search_tooling,
        can_apply: can_apply_rtk_search_tooling,
        apply: apply_rtk_search_tooling,
    },
    mission_spine::IMPLEMENTATIONS[0],
    mission_spine::IMPLEMENTATIONS[1],
    NativeMigration {
        id: "3_3_0_op_record_schema_v2",
        detect: op_record::detect,
        can_apply: op_record::can_apply,
        apply: op_record::apply,
    },
    NativeMigration {
        id: "4.0.0rc8_activation_ledger",
        detect: activation_ledger::detect,
        can_apply: crate::upgrade::always_applicable,
        apply: activation_ledger::apply,
    },
    NativeMigration {
        id: "4.0.0rc5_retire_single_owner_doctrine_ids",
        detect: detect_single_owner_doctrine_ids,
        can_apply: can_apply_single_owner_doctrine_ids,
        apply: apply_single_owner_doctrine_ids,
    },
    NativeMigration {
        id: "3.2.0rc35_charter_bundle_v2",
        detect: charter_bundle::detect_charter_bundle_v2,
        can_apply: charter_bundle::can_apply_charter_bundle_v2,
        apply: apply_charter_bundle_v2,
    },
    NativeMigration {
        id: "3.2.0rc35_charter_manifest_defaults_repair",
        detect: charter_bundle::detect_charter_manifest_defaults_repair,
        can_apply: charter_bundle::can_apply_charter_manifest_defaults_repair,
        apply: apply_charter_manifest_defaults_repair,
    },
    NativeMigration {
        id: "3.2.0rc35_unified_bundle",
        detect: charter_bundle::detect_unified_bundle,
        can_apply: charter_bundle::can_apply_unified_bundle,
        apply: apply_unified_bundle_migration,
    },
    NativeMigration {
        id: skills_global::MIGRATION_ID,
        detect: skills_global::detect,
        can_apply: skills_global::can_apply,
        apply: apply_skills_global,
    },
    NativeMigration {
        id: crate::render::shim::MIGRATION_ID,
        detect: crate::render::shim::detect_direct_canonical_commands,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_direct_canonical_commands,
    },
];

fn apply_unified_bundle_migration(
    project_root: &std::path::Path,
) -> Result<crate::upgrade::MigrationOutcome, crate::error::InstallError> {
    let (_report, outcome) = charter_bundle::apply_unified_bundle(project_root, false)?;
    Ok(outcome)
}

fn apply_charter_bundle_v2(
    project_root: &std::path::Path,
) -> Result<crate::upgrade::MigrationOutcome, crate::error::InstallError> {
    charter_bundle::apply_charter_bundle_v2(project_root, false)
}

fn apply_charter_manifest_defaults_repair(
    project_root: &std::path::Path,
) -> Result<crate::upgrade::MigrationOutcome, crate::error::InstallError> {
    charter_bundle::apply_charter_manifest_defaults_repair(project_root, false)
}

fn apply_skills_global(
    project_root: &std::path::Path,
) -> Result<crate::upgrade::MigrationOutcome, crate::error::InstallError> {
    skills_global::apply(project_root, false)
}

fn apply_direct_canonical_commands(
    project_root: &std::path::Path,
) -> Result<crate::upgrade::MigrationOutcome, crate::error::InstallError> {
    crate::render::shim::apply_direct_canonical_commands(project_root, false)
}

/// The retired `rtk-search-tooling` toolguide (Source `RETIRED_TOOLGUIDE_STEM`).
const RTK_RETIREMENT: retired_activation::Retirement = retired_activation::Retirement {
    kind_key: "activated_toolguides",
    stem: "rtk-search-tooling",
    reference_prefix: "TOOLGUIDE",
    successors: &[],
};

/// The single-owner doctrine retirement table (Source `RETIREMENTS`).
const SINGLE_OWNER_RETIREMENTS: [retired_activation::Retirement; 8] = [
    retired_activation::Retirement {
        kind_key: "activated_styleguides",
        stem: "adversarial-squad-cadence",
        reference_prefix: "STYLEGUIDE",
        successors: &[],
    },
    retired_activation::Retirement {
        kind_key: "activated_tactics",
        stem: "bug-fixing-checklist",
        reference_prefix: "TACTIC",
        successors: &[("activated_procedures", "test-first-bug-fixing")],
    },
    retired_activation::Retirement {
        kind_key: "activated_tactics",
        stem: "locality-of-change",
        reference_prefix: "TACTIC",
        successors: &[("activated_tactics", "avoid-gold-plating")],
    },
    retired_activation::Retirement {
        kind_key: "activated_tactics",
        stem: "common-docs-curation",
        reference_prefix: "TACTIC",
        successors: &[
            ("activated_tactics", "common-docs-scaffold"),
            ("activated_tactics", "common-docs-write"),
            ("activated_tactics", "common-docs-find"),
        ],
    },
    retired_activation::Retirement {
        kind_key: "activated_tactics",
        stem: "boring-code-review",
        reference_prefix: "TACTIC",
        successors: &[("activated_styleguides", "boring-code-review")],
    },
    retired_activation::Retirement {
        kind_key: "activated_tactics",
        stem: "behavior-driven-development",
        reference_prefix: "TACTIC",
        successors: &[("activated_tactics", "bdd-scenario-formulation")],
    },
    retired_activation::Retirement {
        kind_key: "activated_tactics",
        stem: "iterative-deepening-review",
        reference_prefix: "TACTIC",
        successors: &[],
    },
    retired_activation::Retirement {
        kind_key: "activated_procedures",
        stem: "tracker-organisation-workflow",
        reference_prefix: "PROCEDURE",
        successors: &[],
    },
];

fn detect_rtk_search_tooling(
    project_root: &std::path::Path,
) -> Result<bool, crate::error::InstallError> {
    retired_activation::detect_retirements(project_root, &[RTK_RETIREMENT], false)
}

fn can_apply_rtk_search_tooling(
    project_root: &std::path::Path,
) -> Result<Option<String>, crate::error::InstallError> {
    if detect_rtk_search_tooling(project_root)? {
        Ok(None)
    } else {
        Ok(Some(
            "rtk-search-tooling is not activated in this project".to_string(),
        ))
    }
}

fn apply_rtk_search_tooling(
    project_root: &std::path::Path,
) -> Result<crate::upgrade::MigrationOutcome, crate::error::InstallError> {
    let mut outcome =
        retired_activation::apply_retirements(project_root, &[RTK_RETIREMENT], false)?;
    if outcome.changes.is_empty() && outcome.warnings.is_empty() {
        outcome
            .changes
            .push("rtk-search-tooling already absent; nothing to remove".to_string());
    }
    Ok(outcome)
}

fn detect_single_owner_doctrine_ids(
    project_root: &std::path::Path,
) -> Result<bool, crate::error::InstallError> {
    retired_activation::detect_retirements(project_root, &SINGLE_OWNER_RETIREMENTS, true)
}

fn can_apply_single_owner_doctrine_ids(
    project_root: &std::path::Path,
) -> Result<Option<String>, crate::error::InstallError> {
    if detect_single_owner_doctrine_ids(project_root)? {
        Ok(None)
    } else {
        Ok(Some(
            "no retired single-owner doctrine id is activated in this project".to_string(),
        ))
    }
}

fn apply_single_owner_doctrine_ids(
    project_root: &std::path::Path,
) -> Result<crate::upgrade::MigrationOutcome, crate::error::InstallError> {
    let mut outcome =
        retired_activation::apply_retirements(project_root, &SINGLE_OWNER_RETIREMENTS, true)?;
    if outcome.changes.is_empty() && outcome.warnings.is_empty() {
        outcome.changes.push(
            "no retired single-owner doctrine id is activated in this project; nothing to remove"
                .to_string(),
        );
    }
    Ok(outcome)
}

/// The owned id list, in catalog order, as the parent's accounting reads it.
pub const OWNED_IDS: [&str; 33] = [
    "0_9_3_surface_repair_wiring",
    "3.0.3_globalize_skill_pack",
    "3_3_1_context_sources_consolidation",
    "3.1.1_direct_canonical_commands",
    "3.1.1_residue_safe_normalize_status_json",
    "3.1.2_globalize_commands",
    "3.2.0rc30_fix_runtime_next_result_default",
    "3.2.0a4_safe_globalize_commands",
    "3.2.0rc35_charter_bundle_v2",
    "3.2.0rc35_charter_manifest_defaults_repair",
    "3.2.0rc35_activate_builtin_mission_types",
    "3.2.0rc35_codex_to_skills",
    "3.2.0rc35_fix_prompt_file_workaround",
    "3.2.0rc35_kittify_profile_handoff",
    "3.2.0rc35_pi_letta_agent_backfill",
    "3.2.0rc35_repository_root_checkout_terminology",
    "3.2.0rc35_spk_skill_pack",
    "3.2.0rc35_strip_selection_config",
    "3.2.0rc35_unified_bundle",
    "3.2.0rc35_update_planning_templates",
    "3.2.6_retire_rtk_search_tooling",
    "3.2.8_provision_kitty_env",
    "3_2_0rc39_refresh_orientation_block",
    "3_3_0_session_presence_all_harnesses",
    "3_3_0_session_presence_claude_code",
    "3_3_0_op_record_schema_v2",
    "4_0_0rc5_single_branch_code_lanes_restamp",
    "4.0.0rc5_retire_single_owner_doctrine_ids",
    "4.0.0rc8_activation_ledger",
    "4.0.0rc8_command_skill_hyphen_names",
    "4.0.0rc8_retire_antigravity_workflows",
    "migrate_lifecycle_envelope",
    "verdict_provenance_backfill",
];
