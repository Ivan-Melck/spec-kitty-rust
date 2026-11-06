//! The remaining pre-3.x migration effects (`target_version` major < 3), as a
//! registry-ready slice of real `detect` / `can_apply` / `apply` pointers.
//!
//! This module owns the *ownership partition* of the pinned catalog, not a
//! second selection engine: the parent's canonical registry merges
//! [`IMPLEMENTATIONS`] into `crate::upgrade::NATIVE_MIGRATIONS`, and selection
//! stays declared-minus-applied over the ledger, capped by the run's target
//! (ADR 2026-10-17-1 §3). Nothing here decides *whether* a migration runs; each
//! entry decides only what it finds (`detect`), whether this project shape is
//! applicable (`can_apply`) and what it changes (`apply`).
//!
//! | family | ids |
//! | --- | --- |
//! | [`missions`] | `0.6.7`, `0.9.2`, `0.12.0` |
//! | [`worktrees`] | `0.6.5`, `0.7.2`, `0.8.0`, `0.10.8` |
//! | [`commands`] | `0.7.3`, `0.10.0`…`0.14.0` |
//! | [`meta`] | `0.13.0_research_csv_schema_check`, `0.13.8` |
//! | [`skills_pack`] | `2.0.11`, `2.1.1`, `2.1.2` (×4), `2.1.3_restore_prompt_commands` |
//! | [`surfaces`] | `2.0.1` (×2), `2.0.7`, `2.1.3_fix_planning_repository_terminology`, `2.2.0` |
//! | [`residue`] | `0.9.0`, `0.9.1`, `2.0.6` residue-safe successors |
//!
//! [`layout`] is the shared `.kittify/` centralised-layout migration those
//! effects and the `migrate` command both use; [`shared`] is the documented
//! façade over the installer primitives (asset roots, generated writes,
//! ownership guards, command-skill retirement) that every migration family —
//! including the `target_version` major >= 3 ones — may call. Neither registers
//! an id of its own.
//!
//! ## Accounting
//!
//! The pinned catalog declares 136 migrations. Before this module 55 had a
//! native implementation and **38** pre-3.x ids did not; [`IMPLEMENTATIONS`]
//! carries exactly those 38, and [`SOURCE_BINDINGS`] names the pinned module and
//! class each entry ports. The remaining 43 ids are `target_version` major >= 3
//! and belong to the modern migration track.

use std::path::Path;

use crate::error::InstallError;
use crate::upgrade::{MigrationOutcome, NativeMigration};

pub mod agent_commands;
pub mod assets;
pub mod commands;
pub mod layout;
pub mod meta;
pub mod missions;
pub mod paths;
pub mod residue;
pub mod shared;
pub mod skills_pack;
pub mod surfaces;
pub mod worktrees;

/// The version stamped into a rendered command file's marker by
/// `2.1.3_restore_prompt_commands`.
///
/// Source-authored marker protocols use the compatibility CLI version, not the
/// Native Cargo package version. The exported name remains a convenience alias.
pub const LEGACY_RENDER_VERSION: &str = sk_kernel::COMPAT_CORE_VERSION;

fn never(_project_root: &Path) -> Result<bool, InstallError> {
    Ok(false)
}

fn always(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

fn superseded(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(Some("Superseded by charter-rename migration".to_string()))
}

fn superseded_outcome(_project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    Ok(MigrationOutcome {
        changed: false,
        changes: Vec::new(),
        warnings: vec!["Superseded by charter-rename".to_string()],
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// One Source binding: the native id, the pinned module that defines it, and the
/// effect the port reproduces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceBinding {
    /// The declared catalog id.
    pub id: &'static str,
    /// The pinned Python module (module-relative under
    /// `src/specify_cli/upgrade/migrations/`).
    pub source_module: &'static str,
    /// The pinned class name.
    pub source_class: &'static str,
    /// One-line effect statement, in the source's own terms.
    pub effect: &'static str,
}

/// The 38 pre-3.x implementations, ready for the canonical registry.
pub const IMPLEMENTATIONS: [NativeMigration; 38] = [
    NativeMigration {
        id: "0.6.5_commands_rename",
        detect: worktrees::detect_commands_rename,
        can_apply: worktrees::can_apply_commands_rename,
        apply: worktrees::apply_commands_rename,
    },
    NativeMigration {
        id: "0.6.7_ensure_missions",
        detect: missions::detect_ensure_missions,
        can_apply: missions::can_apply_ensure_missions,
        apply: missions::apply_ensure_missions,
    },
    NativeMigration {
        id: "0.7.2_worktree_commands_dedup",
        detect: worktrees::detect_worktree_commands_dedup,
        can_apply: worktrees::can_apply_worktree_commands_dedup,
        apply: worktrees::apply_worktree_commands_dedup,
    },
    NativeMigration {
        id: "0.7.3_update_scripts",
        detect: commands::detect_update_scripts,
        can_apply: commands::can_apply_update_scripts,
        apply: commands::apply_update_scripts,
    },
    NativeMigration {
        id: "0.8.0_worktree_agents_symlink",
        detect: worktrees::detect_worktree_agents_symlink,
        can_apply: worktrees::can_apply_worktree_agents_symlink,
        apply: worktrees::apply_worktree_agents_symlink,
    },
    NativeMigration {
        id: "0.9.0_residue_safe_frontmatter_only_lanes",
        detect: residue::detect_residue_safe_frontmatter_lanes,
        can_apply: residue::can_apply_always,
        apply: residue::apply_residue_safe_frontmatter_lanes,
    },
    NativeMigration {
        id: "0.9.1_residue_safe_complete_migration",
        detect: residue::detect_residue_safe_complete_migration,
        can_apply: residue::can_apply_always,
        apply: residue::apply_residue_safe_complete_migration,
    },
    NativeMigration {
        id: "0.9.2_research_mission_templates",
        detect: missions::detect_research_mission_templates,
        can_apply: missions::can_apply_research_mission_templates,
        apply: missions::apply_research_mission_templates,
    },
    NativeMigration {
        id: "0.10.0_python_only",
        detect: commands::detect_python_only,
        can_apply: commands::can_apply_python_only,
        apply: commands::apply_python_only,
    },
    NativeMigration {
        id: "0.10.1_populate_slash_commands",
        detect: commands::detect_populate_slash_commands,
        can_apply: commands::can_apply_populate_slash_commands,
        apply: commands::apply_populate_slash_commands,
    },
    NativeMigration {
        id: "0.10.2_update_slash_commands",
        detect: commands::detect_update_slash_commands,
        can_apply: commands::can_apply_update_slash_commands,
        apply: commands::apply_update_slash_commands,
    },
    NativeMigration {
        id: "0.10.6_workflow_simplification",
        detect: commands::detect_workflow_simplification,
        can_apply: commands::can_apply_workflow_simplification,
        apply: commands::apply_workflow_simplification,
    },
    NativeMigration {
        id: "0.10.8_fix_memory_structure",
        detect: worktrees::detect_memory_structure,
        can_apply: worktrees::can_apply_memory_structure,
        apply: worktrees::apply_memory_structure,
    },
    NativeMigration {
        id: "0.10.14_update_implement_slash_command",
        detect: commands::detect_never,
        can_apply: commands::can_apply_update_implement_0_10_14,
        apply: commands::apply_update_implement_0_10_14,
    },
    NativeMigration {
        id: "0.11.1_improved_workflow_templates",
        detect: commands::detect_never,
        can_apply: commands::can_apply_inert,
        apply: commands::apply_improved_workflow_templates,
    },
    NativeMigration {
        id: "0.11.1_update_implement_slash_command",
        detect: commands::detect_never,
        can_apply: commands::can_apply_update_implement_0_11_1,
        apply: commands::apply_update_implement_0_11_1,
    },
    NativeMigration {
        id: "0.11.2_improved_workflow_templates",
        detect: commands::detect_never,
        can_apply: commands::can_apply_inert,
        apply: commands::apply_improved_workflow_templates,
    },
    NativeMigration {
        id: "0.11.3_workflow_agent_flag",
        detect: commands::detect_workflow_agent_flag,
        can_apply: commands::can_apply_workflow_agent_flag,
        apply: commands::apply_workflow_agent_flag,
    },
    NativeMigration {
        id: "0.12.0_documentation_mission",
        detect: missions::detect_documentation_mission,
        can_apply: missions::can_apply_documentation_mission,
        apply: missions::apply_documentation_mission,
    },
    NativeMigration {
        id: "0.13.0_research_csv_schema_check",
        detect: meta::detect_research_csv_schema_check,
        can_apply: meta::can_apply_research_csv_schema_check,
        apply: meta::apply_research_csv_schema_check,
    },
    NativeMigration {
        id: "0.13.0_update_charter_templates",
        detect: never,
        can_apply: superseded,
        apply: superseded_outcome,
    },
    NativeMigration {
        id: "0.13.0_update_research_implement_templates",
        detect: commands::detect_never,
        can_apply: commands::can_apply_inert,
        apply: commands::apply_update_research_implement,
    },
    NativeMigration {
        id: "0.13.5_add_commit_workflow_to_templates",
        detect: commands::detect_never,
        can_apply: commands::can_apply_inert,
        apply: commands::apply_add_commit_workflow,
    },
    NativeMigration {
        id: "0.13.8_target_branch",
        detect: meta::detect_target_branch,
        can_apply: meta::can_apply_target_branch,
        apply: meta::apply_target_branch,
    },
    NativeMigration {
        id: "0.14.0_centralized_feature_detection",
        detect: commands::detect_never,
        can_apply: commands::can_apply_inert,
        apply: commands::apply_centralized_feature_detection,
    },
    NativeMigration {
        id: "2.0.1_fix_generated_command_templates",
        detect: surfaces::detect_fix_generated_command_templates,
        can_apply: surfaces::can_apply_fix_generated_command_templates,
        apply: surfaces::apply_fix_generated_command_templates,
    },
    NativeMigration {
        id: "2.0.1_tool_config_key_rename",
        detect: surfaces::detect_tool_config_key_rename,
        can_apply: surfaces::can_apply_tool_config_key_rename,
        apply: surfaces::apply_tool_config_key_rename,
    },
    NativeMigration {
        id: "2.0.6_residue_safe_consistency_sweep",
        detect: residue::detect_residue_safe_consistency_sweep,
        can_apply: residue::can_apply_consistency_sweep,
        apply: residue::apply_residue_safe_consistency_sweep,
    },
    NativeMigration {
        id: "2.0.7_fix_stale_overrides",
        detect: surfaces::detect_fix_stale_overrides,
        can_apply: surfaces::can_apply_fix_stale_overrides,
        apply: surfaces::apply_fix_stale_overrides,
    },
    NativeMigration {
        id: "2.0.11_install_skills",
        detect: skills_pack::detect_install_skills,
        can_apply: skills_pack::can_apply_install_skills,
        apply: skills_pack::apply_install_skills,
    },
    NativeMigration {
        id: "2.1.1_repair_skill_pack",
        detect: skills_pack::detect_repair_skill_pack,
        can_apply: skills_pack::can_apply_repair_skill_pack,
        apply: skills_pack::apply_repair_skill_pack,
    },
    NativeMigration {
        id: "2.1.2_fix_orchestrator_api_skill",
        detect: skills_pack::detect_fix_orchestrator_api_skill,
        can_apply: always,
        apply: skills_pack::apply_fix_orchestrator_api_skill,
    },
    NativeMigration {
        id: "2.1.2_fix_runtime_next_skill",
        detect: skills_pack::detect_fix_runtime_next_skill,
        can_apply: always,
        apply: skills_pack::apply_fix_runtime_next_skill,
    },
    NativeMigration {
        id: "2.1.2_install_git_workflow_skill",
        detect: skills_pack::detect_install_git_workflow_skill,
        can_apply: always,
        apply: skills_pack::apply_install_git_workflow_skill,
    },
    NativeMigration {
        id: "2.1.2_install_mission_system_skill",
        detect: skills_pack::detect_install_mission_system_skill,
        can_apply: always,
        apply: skills_pack::apply_install_mission_system_skill,
    },
    NativeMigration {
        id: "2.1.3_fix_planning_repository_terminology",
        detect: surfaces::detect_planning_repository_terminology,
        can_apply: surfaces::can_apply_planning_repository_terminology,
        apply: surfaces::apply_planning_repository_terminology,
    },
    NativeMigration {
        id: "2.1.3_restore_prompt_commands",
        detect: skills_pack::detect_restore_prompt_commands,
        can_apply: skills_pack::can_apply_restore_prompt_commands,
        apply: skills_pack::apply_restore_prompt_commands,
    },
    NativeMigration {
        id: "2.2.0_profile_context_deployment",
        detect: surfaces::detect_profile_context_deployment,
        can_apply: surfaces::can_apply_profile_context_deployment,
        apply: surfaces::apply_profile_context_deployment,
    },
];

/// The Source binding of every [`IMPLEMENTATIONS`] entry.
pub const SOURCE_BINDINGS: [SourceBinding; 38] = [
    SourceBinding {
        id: "0.6.5_commands_rename",
        source_module: "m_0_6_5_commands_rename",
        source_class: "CommandsRenameMigration",
        effect: "rename commands/ to command-templates/ in templates and missions (worktree copies removed)",
    },
    SourceBinding {
        id: "0.6.7_ensure_missions",
        source_module: "m_0_6_7_ensure_missions",
        source_class: "EnsureMissionsMigration",
        effect: "copy missing software-dev/research missions from the package, archiving an incomplete one",
    },
    SourceBinding {
        id: "0.7.2_worktree_commands_dedup",
        source_module: "m_0_7_2_worktree_commands_dedup",
        source_class: "WorktreeCommandsDedupMigration",
        effect: "remove a worktree's own .claude/commands/ (it inherits the main checkout's)",
    },
    SourceBinding {
        id: "0.7.3_update_scripts",
        source_module: "m_0_7_3_update_scripts",
        source_class: "UpdateScriptsMigration",
        effect: "refresh .kittify/scripts/bash scripts from the package when it still ships them",
    },
    SourceBinding {
        id: "0.8.0_worktree_agents_symlink",
        source_module: "m_0_8_0_worktree_agents_symlink",
        source_class: "WorktreeAgentsSymlinkMigration",
        effect: "link each worktree's .kittify/AGENTS.md to the main checkout's (copy on failure)",
    },
    SourceBinding {
        id: "0.9.0_residue_safe_frontmatter_only_lanes",
        source_module: "m_0_9_0_residue_safe_frontmatter_only_lanes",
        source_class: "ResidueSafeFrontmatterOnlyLanesMigration",
        effect: "flatten tasks/<lane>/ to frontmatter-only lanes in Missions, never in a residue folder",
    },
    SourceBinding {
        id: "0.9.1_residue_safe_complete_migration",
        source_module: "m_0_9_1_residue_safe_complete_migration",
        source_class: "ResidueSafeCompleteLaneMigration",
        effect: "move leftover lane files, clean worktree agent/script dirs, normalise lane frontmatter",
    },
    SourceBinding {
        id: "0.9.2_research_mission_templates",
        source_module: "m_0_9_2_research_mission_templates",
        source_class: "ResearchMissionTemplatesMigration",
        effect: "add research templates/task-prompt-template.md (and legacy tasks command when packaged)",
    },
    SourceBinding {
        id: "0.10.0_python_only",
        source_module: "m_0_10_0_python_only",
        source_class: "PythonOnlyMigration",
        effect: "rewrite bash script references to `spec-kitty agent` commands; preserve unprovable scripts",
    },
    SourceBinding {
        id: "0.10.1_populate_slash_commands",
        source_module: "m_0_10_1_populate_slash_commands",
        source_class: "PopulateSlashCommandsMigration",
        effect: "copy mission command templates into empty configured agent command dirs",
    },
    SourceBinding {
        id: "0.10.2_update_slash_commands",
        source_module: "m_0_10_2_update_slash_commands",
        source_class: "UpdateSlashCommandsMigration",
        effect: "refresh generated commands from the mission templates; sweep legacy .kittify/commands/*.toml through the guard",
    },
    SourceBinding {
        id: "0.10.6_workflow_simplification",
        source_module: "m_0_10_6_workflow_simplification",
        source_class: "WorkflowSimplificationMigration",
        effect: "refresh implement/review commands from the mission templates",
    },
    SourceBinding {
        id: "0.10.8_fix_memory_structure",
        source_module: "m_0_10_8_fix_memory_structure",
        source_class: "FixMemoryStructureMigration",
        effect: "move root memory/ into .kittify/, repair broken memory/AGENTS.md symlinks",
    },
    SourceBinding {
        id: "0.10.14_update_implement_slash_command",
        source_module: "m_0_10_14_update_implement_slash_command",
        source_class: "UpdateImplementSlashCommandMigration_0_10_14",
        effect: "permanently inert (detect False) 0.10.x backport; can_apply still reads the template",
    },
    SourceBinding {
        id: "0.11.1_improved_workflow_templates",
        source_module: "m_0_11_1_improved_workflow_templates",
        source_class: "ImprovedWorkflowTemplatesMigration",
        effect: "permanently inert (detect False, can_apply refuses: templates removed in WP10)",
    },
    SourceBinding {
        id: "0.11.1_update_implement_slash_command",
        source_module: "m_0_11_1_update_implement_slash_command",
        source_class: "UpdateImplementSlashCommandMigration",
        effect: "permanently inert (detect False, can_apply refuses: templates removed in WP10)",
    },
    SourceBinding {
        id: "0.11.2_improved_workflow_templates",
        source_module: "m_0_11_2_improved_workflow_templates",
        source_class: "ImprovedWorkflowTemplatesMigration",
        effect: "permanently inert (detect False, can_apply refuses: templates removed in WP10)",
    },
    SourceBinding {
        id: "0.11.3_workflow_agent_flag",
        source_module: "m_0_11_3_workflow_agent_flag",
        source_class: "WorkflowAgentFlagMigration",
        effect: "append `--agent <name>` to action commands and expand __AGENT__ in agent prompts",
    },
    SourceBinding {
        id: "0.12.0_documentation_mission",
        source_module: "m_0_12_0_documentation_mission",
        source_class: "InstallDocumentationMission",
        effect: "copy the documentation mission into .kittify/missions/",
    },
    SourceBinding {
        id: "0.13.0_research_csv_schema_check",
        source_module: "m_0_13_0_research_csv_schema_check",
        source_class: "ResearchCSVSchemaCheckMigration",
        effect: "report research CSV header mismatches; modifies nothing",
    },
    SourceBinding {
        id: "0.13.0_update_charter_templates",
        source_module: "m_0_13_0_update_charter_templates",
        source_class: "UpdateCharterTemplatesMigration",
        effect: "recorded no-op superseded by 3.1.1_charter_rename",
    },
    SourceBinding {
        id: "0.13.0_update_research_implement_templates",
        source_module: "m_0_13_0_update_research_implement_templates",
        source_class: "UpdateResearchImplementTemplatesMigration",
        effect: "permanently inert (detect False); apply refreshes a research implement prompt",
    },
    SourceBinding {
        id: "0.13.5_add_commit_workflow_to_templates",
        source_module: "m_0_13_5_add_commit_workflow_to_templates",
        source_class: "AddCommitWorkflowToTemplatesMigration",
        effect: "permanently inert (detect False); apply copies the packaged implement template",
    },
    SourceBinding {
        id: "0.13.8_target_branch",
        source_module: "m_0_13_8_target_branch",
        source_class: "TargetBranchMigration",
        effect: "add target_branch to kitty-specs/NNN-*/meta.json",
    },
    SourceBinding {
        id: "0.14.0_centralized_feature_detection",
        source_module: "m_0_14_0_centralized_feature_detection",
        source_class: "CentralizedFeatureDetectionMigration",
        effect: "permanently inert (detect False); apply copies the packaged plan.md template",
    },
    SourceBinding {
        id: "2.0.1_fix_generated_command_templates",
        source_module: "m_2_0_1_fix_generated_command_templates",
        source_class: "FixGeneratedCommandTemplatesMigration",
        effect: "repair stale command paths, --require-tasks, script placeholders and sequential-merge wording",
    },
    SourceBinding {
        id: "2.0.1_tool_config_key_rename",
        source_module: "m_2_0_1_tool_config_key_rename",
        source_class: "ToolConfigKeyRenameMigration",
        effect: "rename config.yaml `agents:` key to `tools:` in place",
    },
    SourceBinding {
        id: "2.0.6_residue_safe_consistency_sweep",
        source_module: "m_2_0_6_residue_safe_consistency_sweep",
        source_class: "ResidueSafeConsistencySweepMigration",
        effect: "repair baseline meta.json, lane frontmatter, orphan/regenerated status.json, tasks.md prompt refs",
    },
    SourceBinding {
        id: "2.0.7_fix_stale_overrides",
        source_module: "m_2_0_7_fix_stale_overrides",
        source_class: "FixStaleOverridesMigration",
        effect: "remove .kittify/overrides/ files byte-identical to the package default",
    },
    SourceBinding {
        id: "2.0.11_install_skills",
        source_module: "m_2_0_11_install_skills",
        source_class: "InstallSkillsMigration",
        effect: "install the canonical skill pack for configured agents and write the managed manifest",
    },
    SourceBinding {
        id: "2.1.1_repair_skill_pack",
        source_module: "m_2_1_1_repair_skill_pack",
        source_class: "RepairSkillPackMigration",
        effect: "rebuild missing/drifted managed skills after the 2.1.0 wheel defect",
    },
    SourceBinding {
        id: "2.1.2_fix_orchestrator_api_skill",
        source_module: "m_2_1_2_fix_orchestrator_api_skill",
        source_class: "FixOrchestratorApiSkillMigration",
        effect: "replace an out-of-date orchestrator-api SKILL.md with the canonical one",
    },
    SourceBinding {
        id: "2.1.2_fix_runtime_next_skill",
        source_module: "m_2_1_2_fix_runtime_next_skill",
        source_class: "FixRuntimeNextSkillMigration",
        effect: "replace an out-of-date runtime-next SKILL.md with the canonical one",
    },
    SourceBinding {
        id: "2.1.2_install_git_workflow_skill",
        source_module: "m_2_1_2_install_git_workflow_skill",
        source_class: "InstallGitWorkflowSkillMigration",
        effect: "install the git-workflow skill into the skill roots",
    },
    SourceBinding {
        id: "2.1.2_install_mission_system_skill",
        source_module: "m_2_1_2_install_mission_system_skill",
        source_class: "InstallMissionSystemSkillMigration",
        effect: "install the mission-system skill into the skill roots",
    },
    SourceBinding {
        id: "2.1.3_fix_planning_repository_terminology",
        source_module: "m_2_1_3_fix_planning_repository_terminology",
        source_class: "FixPlanningRepositoryTerminology",
        effect: "replace 'planning repository' with 'project root checkout' in agent command files",
    },
    SourceBinding {
        id: "2.1.3_restore_prompt_commands",
        source_module: "m_2_1_3_restore_prompt_commands",
        source_class: "RestorePromptCommandsMigration",
        effect: "replace thin `spec-kitty agent shim` stubs with fully rendered command prompts",
    },
    SourceBinding {
        id: "2.2.0_profile_context_deployment",
        source_module: "m_2_2_0_profile_context_deployment",
        source_class: "ProfileContextDeploymentMigration",
        effect: "remove the retired spec-kitty.profile-context command file",
    },
];

/// The Source binding for `id`, when this module implements it.
pub fn binding(id: &str) -> Option<&'static SourceBinding> {
    SOURCE_BINDINGS.iter().find(|binding| binding.id == id)
}

/// The implementation for `id`, when this module provides one.
pub fn implementation(id: &str) -> Option<&'static NativeMigration> {
    IMPLEMENTATIONS.iter().find(|migration| migration.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::MigrationCatalog;
    use std::collections::BTreeSet;

    #[test]
    fn every_declared_pre_three_id_has_exactly_one_implementation() {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        let pre_three: Vec<&str> = catalog
            .specs()
            .iter()
            .filter(|spec| {
                spec.target_version
                    .split('.')
                    .next()
                    .and_then(|major| major.parse::<u32>().ok())
                    .is_some_and(|major| major < 3)
            })
            .map(|spec| spec.id.as_str())
            .collect();
        assert_eq!(pre_three.len(), 59);
        let mut implemented = BTreeSet::new();
        for migration in IMPLEMENTATIONS {
            assert!(
                implemented.insert(migration.id),
                "duplicate implementation {}",
                migration.id
            );
            assert!(
                catalog.get(migration.id).is_some(),
                "{} is not declared in the catalog",
                migration.id
            );
        }
        // The 21 pre-3.x ids the earlier tranche already implemented stay
        // canonical elsewhere; this module owns the other 38.
        assert_eq!(implemented.len(), 38);
        let elsewhere: Vec<&str> = pre_three
            .iter()
            .copied()
            .filter(|id| !implemented.contains(id))
            .collect();
        assert_eq!(elsewhere.len(), 21);
        for id in elsewhere {
            let migration = crate::upgrade::NATIVE_MIGRATIONS
                .iter()
                .find(|migration| migration.id == id);
            assert!(
                migration.is_some(),
                "{id} is neither in completion_legacy nor in NATIVE_MIGRATIONS"
            );
        }
    }

    #[test]
    fn every_implementation_has_a_source_binding() {
        for migration in IMPLEMENTATIONS {
            let binding = binding(migration.id)
                .unwrap_or_else(|| panic!("no Source binding for {}", migration.id));
            assert!(!binding.source_module.is_empty());
            assert!(!binding.source_class.is_empty());
            assert!(!binding.effect.is_empty());
        }
        for binding in SOURCE_BINDINGS {
            assert!(
                implementation(binding.id).is_some(),
                "binding for {} has no implementation",
                binding.id
            );
        }
    }

    #[test]
    fn only_the_pinned_inert_ids_report_themselves_not_needed() {
        // The pinned registry keeps these as permanently inert history; their
        // `detect()` is False for every project, so the runner records the same
        // recorded no-op the source records.
        let inert = [
            "0.10.14_update_implement_slash_command",
            "0.11.1_improved_workflow_templates",
            "0.11.1_update_implement_slash_command",
            "0.11.2_improved_workflow_templates",
            "0.13.0_update_charter_templates",
            "0.13.0_update_research_implement_templates",
            "0.13.5_add_commit_workflow_to_templates",
            "0.14.0_centralized_feature_detection",
        ];
        let directory = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(directory.path().join(".kittify")).expect("kittify");
        for id in inert {
            let migration = implementation(id).expect("implementation");
            assert!(
                !(migration.detect)(directory.path()).expect("detect"),
                "{id} must be inert"
            );
        }
    }
}
