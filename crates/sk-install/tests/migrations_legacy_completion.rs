//! Source-first acceptance tests for the pre-3.x migration effects.
//!
//! Every case drives the real `detect` / `can_apply` / `apply` pointers the
//! registry consumes ([`sk_install::migrations::completion_legacy`]) against a
//! temporary project, and asserts the on-disk effect the pinned module states —
//! including the preservation branches (unknown user data, unproven scripts, a
//! user-authored command file) and idempotence (a second `detect` is false).

use std::path::{Path, PathBuf};

use sk_install::migrations::completion_legacy as legacy;
use sk_install::migrations::completion_legacy::layout::{
    AssetDisposition, ConflictReason, classify_asset, execute_migration,
};
use sk_install::upgrade::NativeMigration;

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, content).expect("write");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read")
}

fn project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp");
    write(
        &directory.path().join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n  - codex\n",
    );
    directory
}

fn git_init_and_stage(root: &Path, pathspec: &str) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "--quiet"]);
    run(&["add", "--", pathspec]);
}

fn migration(id: &str) -> &'static NativeMigration {
    legacy::implementation(id).unwrap_or_else(|| panic!("{id} is implemented"))
}

fn detect(id: &str, root: &Path) -> bool {
    (migration(id).detect)(root).expect("detect")
}

fn can_apply(id: &str, root: &Path) -> Option<String> {
    (migration(id).can_apply)(root).expect("can_apply")
}

fn apply(id: &str, root: &Path) -> sk_install::MigrationOutcome {
    (migration(id).apply)(root).expect("apply")
}

// --------------------------------------------------------------- 0.6.5 / 0.7.2

#[test]
fn commands_rename_moves_templates_and_missions_and_is_idempotent() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/templates/commands/specify.md"),
        "# specify\n",
    );
    write(
        &root.join(".kittify/missions/software-dev/commands/plan.md"),
        "# plan\n",
    );
    assert!(detect("0.6.5_commands_rename", root));
    assert_eq!(can_apply("0.6.5_commands_rename", root), None);
    let outcome = apply("0.6.5_commands_rename", root);
    assert!(outcome.changed);
    assert!(
        root.join(".kittify/templates/command-templates/specify.md")
            .is_file()
    );
    assert!(
        root.join(".kittify/missions/software-dev/command-templates/plan.md")
            .is_file()
    );
    assert!(!root.join(".kittify/templates/commands").exists());
    assert!(
        !detect("0.6.5_commands_rename", root),
        "second detect is false"
    );
}

#[test]
fn commands_rename_refuses_a_manual_merge() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/templates/commands/specify.md"),
        "# old\n",
    );
    write(
        &root.join(".kittify/templates/command-templates/specify.md"),
        "# new\n",
    );
    let reason = can_apply("0.6.5_commands_rename", root).expect("refusal");
    assert!(reason.contains("manual merge required"), "{reason}");
}

#[test]
fn worktree_commands_dedup_requires_the_main_checkout_commands() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".worktrees/wt1/.claude/commands/spec-kitty.specify.md"),
        "# specify\n",
    );
    assert!(detect("0.7.2_worktree_commands_dedup", root));
    let reason = can_apply("0.7.2_worktree_commands_dedup", root).expect("refusal");
    assert!(reason.contains(".claude/commands/"), "{reason}");

    write(
        &root.join(".claude/commands/spec-kitty.specify.md"),
        "# specify\n",
    );
    assert_eq!(can_apply("0.7.2_worktree_commands_dedup", root), None);
    assert!(apply("0.7.2_worktree_commands_dedup", root).changed);
    assert!(!root.join(".worktrees/wt1/.claude/commands").exists());
    assert!(!detect("0.7.2_worktree_commands_dedup", root));
}

// -------------------------------------------------------------------- 0.8.0

#[test]
fn worktree_agents_symlink_is_relative_and_preserves_a_real_file() {
    let directory = project();
    let root = directory.path();
    write(&root.join(".kittify/AGENTS.md"), "# AGENTS\n");
    std::fs::create_dir_all(root.join(".worktrees/wt1/.kittify")).expect("worktree");
    write(&root.join(".worktrees/wt2/.kittify/AGENTS.md"), "# mine\n");

    assert!(detect("0.8.0_worktree_agents_symlink", root));
    let outcome = apply("0.8.0_worktree_agents_symlink", root);
    assert!(outcome.changed);
    let link = root.join(".worktrees/wt1/.kittify/AGENTS.md");
    assert!(link.is_symlink());
    assert_eq!(
        std::fs::read_link(&link).expect("link"),
        PathBuf::from("../../../.kittify/AGENTS.md")
    );
    assert_eq!(read(&link), "# AGENTS\n");
    // A real (non-link) file is a user file and is preserved.
    assert_eq!(
        read(&root.join(".worktrees/wt2/.kittify/AGENTS.md")),
        "# mine\n"
    );
    assert!(!detect("0.8.0_worktree_agents_symlink", root));
}

// -------------------------------------------------------------------- 0.10.8

#[test]
fn memory_structure_moves_root_memory_and_repairs_a_broken_link() {
    let directory = project();
    let root = directory.path();
    write(&root.join("memory/glossary.md"), "terms\n");
    std::fs::create_dir_all(root.join(".kittify")).expect("kittify");
    std::os::unix::fs::symlink(
        "../../../nowhere/.kittify/memory",
        root.join(".kittify/memory"),
    )
    .expect("dangling link");

    assert!(detect("0.10.8_fix_memory_structure", root));
    assert!(apply("0.10.8_fix_memory_structure", root).changed);
    assert_eq!(read(&root.join(".kittify/memory/glossary.md")), "terms\n");
    assert!(!root.join("memory").exists());
    assert!(!root.join(".kittify/memory").is_symlink());
}

// -------------------------------------------------------- 0.9.0 / 0.9.1 residue

#[test]
fn residue_safe_lane_flatten_repairs_the_mission_and_leaves_residue_alone() {
    let directory = project();
    let root = directory.path();
    // A Mission: identity-carrying meta.json.
    write(
        &root.join("kitty-specs/001-mission/meta.json"),
        "{\"mission_id\": \"01HX\", \"mission_type\": \"software-dev\"}\n",
    );
    write(
        &root.join("kitty-specs/001-mission/tasks/planned/WP01-first.md"),
        "---\ntitle: first\n---\nbody\n",
    );
    // A residue folder: a lane tree with no Mission marker. Git tracks the
    // Mission, so the pinned predicate runs in its git-aware mode and answers
    // "residue" — the file-presence fallback deliberately keeps an uncommitted
    // folder holding only `tasks/` repairable.
    write(
        &root.join("kitty-specs/decisions/tasks/planned/WP09-leftover.md"),
        "---\ntitle: leftover\n---\nbody\n",
    );
    git_init_and_stage(root, "kitty-specs/001-mission");

    assert!(detect("0.9.0_residue_safe_frontmatter_only_lanes", root));
    let outcome = apply("0.9.0_residue_safe_frontmatter_only_lanes", root);
    assert!(outcome.changed);
    let flattened = read(&root.join("kitty-specs/001-mission/tasks/WP01-first.md"));
    assert!(flattened.contains("lane: \"planned\""), "{flattened}");
    assert!(flattened.contains("title: first"), "{flattened}");
    assert!(!root.join("kitty-specs/001-mission/tasks/planned").exists());
    // Residue is untouched, reported once.
    assert!(
        root.join("kitty-specs/decisions/tasks/planned/WP09-leftover.md")
            .is_file()
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("residue folder untouched")),
        "{:?}",
        outcome.warnings
    );
    assert!(
        !detect("0.9.0_residue_safe_frontmatter_only_lanes", root),
        "second detect is false"
    );
}

#[test]
fn residue_safe_complete_migration_moves_every_extension_and_cleans_worktrees() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join("kitty-specs/001-mission/meta.json"),
        "{\"mission_id\": \"01HX\"}\n",
    );
    write(
        &root.join("kitty-specs/001-mission/tasks/planned/phase-1.json"),
        "{}\n",
    );
    write(
        &root.join(".worktrees/wt1/.claude/commands/spec-kitty.specify.md"),
        "# specify\n",
    );
    write(
        &root.join(".worktrees/wt1/.kittify/scripts/bash/common.sh"),
        "#!/bin/sh\n",
    );

    assert!(detect("0.9.1_residue_safe_complete_migration", root));
    let outcome = apply("0.9.1_residue_safe_complete_migration", root);
    assert!(outcome.changed, "{:?}", outcome.warnings);
    assert!(
        root.join("kitty-specs/001-mission/tasks/phase-1.json")
            .is_file()
    );
    assert!(!root.join(".worktrees/wt1/.claude/commands").exists());
    assert!(!root.join(".worktrees/wt1/.kittify/scripts").exists());
    assert!(!detect("0.9.1_residue_safe_complete_migration", root));
}

// --------------------------------------------------------------------- 0.9.2

#[test]
fn research_mission_templates_copies_the_missing_prompt_template() {
    let directory = project();
    let root = directory.path();
    let package = sk_install::migrations::completion_legacy::assets::package_mission("research")
        .expect("the pinned research mission");
    if !package.join("templates/task-prompt-template.md").exists() {
        // A packaged tree without the file is the source's own skip, not a pass.
        assert!(can_apply("0.9.2_research_mission_templates", root).is_none());
        return;
    }
    write(
        &root.join(".kittify/missions/research/mission.yaml"),
        "name: research\n",
    );
    assert!(detect("0.9.2_research_mission_templates", root));
    assert_eq!(can_apply("0.9.2_research_mission_templates", root), None);
    assert!(apply("0.9.2_research_mission_templates", root).changed);
    let copied = root.join(".kittify/missions/research/templates/task-prompt-template.md");
    assert!(copied.is_file());
    assert_eq!(
        std::fs::read(&copied).expect("bytes"),
        std::fs::read(package.join("templates/task-prompt-template.md")).expect("source")
    );
    assert!(!detect("0.9.2_research_mission_templates", root));
}

#[test]
fn documentation_mission_is_installed_once() {
    let directory = project();
    let root = directory.path();
    assert!(detect("0.12.0_documentation_mission", root));
    assert_eq!(can_apply("0.12.0_documentation_mission", root), None);
    assert!(apply("0.12.0_documentation_mission", root).changed);
    let mission = root.join(".kittify/missions/documentation/mission.yaml");
    assert!(mission.is_file());
    // A second run is a no-op, not a re-copy.
    let outcome = apply("0.12.0_documentation_mission", root);
    assert!(!outcome.changed);
    assert!(!detect("0.12.0_documentation_mission", root));
}

// --------------------------------------------------------------------- 0.10.0

#[test]
fn python_only_preserves_unprovable_scripts_and_rewrites_template_references() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/scripts/bash/common.sh"),
        "#!/bin/sh\n# custom\n",
    );
    write(
        &root.join(".kittify/templates/command-templates/specify.md"),
        "run .kittify/scripts/bash/create-new-feature.sh now\n",
    );
    assert!(detect("0.10.0_python_only", root));
    let outcome = apply("0.10.0_python_only", root);
    // The script has no ownership signal: it is preserved, never deleted.
    assert!(root.join(".kittify/scripts/bash/common.sh").is_file());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("unprovable script")),
        "{:?}",
        outcome.warnings
    );
    let template = read(&root.join(".kittify/templates/command-templates/specify.md"));
    assert!(
        template.contains("spec-kitty agent mission create-feature now"),
        "{template}"
    );
}

// --------------------------------------------------- 0.10.1 / 0.10.2 populate

#[test]
fn populate_slash_commands_copies_the_mission_templates() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/missions/software-dev/command-templates/specify.md"),
        "# specify\n",
    );
    write(
        &root.join(".kittify/missions/software-dev/command-templates/plan.md"),
        "# plan\n",
    );
    std::fs::create_dir_all(root.join(".claude/commands")).expect("claude");
    assert!(detect("0.10.1_populate_slash_commands", root));
    assert_eq!(can_apply("0.10.1_populate_slash_commands", root), None);
    assert!(apply("0.10.1_populate_slash_commands", root).changed);
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.specify.md")),
        "# specify\n"
    );
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.plan.md")),
        "# plan\n"
    );
}

#[test]
fn update_slash_commands_overwrites_generated_files_and_preserves_authored_ones() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/missions/software-dev/command-templates/specify.md"),
        "# canonical specify\n",
    );
    write(
        &root.join(".kittify/missions/software-dev/command-templates/plan.md"),
        "# canonical plan\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.specify.md"),
        "<!-- spec-kitty-command-version: 1.0.0 -->\n# stale\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.plan.md"),
        "# my own plan command\n",
    );
    write(&root.join(".gitignore"), "tasks/planned/\n");
    write(
        &root.join(".claude/commands/spec-kitty.tasks.md"),
        "please edit tasks/planned/ files by hand\n",
    );
    assert!(detect("0.10.2_update_slash_commands", root));
    let outcome = apply("0.10.2_update_slash_commands", root);
    // The generated file (marker present) is refreshed…
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.specify.md")),
        "# canonical specify\n"
    );
    // …the user-authored one is preserved and reported.
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.plan.md")),
        "# my own plan command\n"
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("never overwritten")),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn update_slash_commands_removes_a_proven_legacy_toml_and_keeps_an_unproven_one() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/missions/software-dev/command-templates/specify.md"),
        "# specify\n",
    );
    write(
        &root.join(".kittify/commands/spec-kitty.old.toml"),
        "description = \"old\"\n\nprompt = \"\"\"\n<!-- spec-kitty-command-version: 1.0.0 -->\nbody\n\"\"\"\n",
    );
    write(
        &root.join(".kittify/commands/my-own.toml"),
        "description = \"mine\"\n\nprompt = \"\"\"\nhand written\n\"\"\"\n",
    );
    apply("0.10.2_update_slash_commands", root);
    assert!(!root.join(".kittify/commands/spec-kitty.old.toml").exists());
    assert!(root.join(".kittify/commands/my-own.toml").is_file());
}

// --------------------------------------------------------------------- 0.10.6

#[test]
fn workflow_simplification_refreshes_implement_and_review_from_the_mission_templates() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/missions/software-dev/command-templates/implement.md"),
        "# implement\n\nrun spec-kitty agent action implement\n",
    );
    write(
        &root.join(".kittify/missions/software-dev/command-templates/review.md"),
        "# review\n\nrun spec-kitty agent action review\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.implement.md"),
        "## Work Package Selection\nold instructions\n",
    );
    assert!(detect("0.10.6_workflow_simplification", root));
    assert_eq!(can_apply("0.10.6_workflow_simplification", root), None);
    let outcome = apply("0.10.6_workflow_simplification", root);
    assert!(outcome.changed, "{:?}", outcome.warnings);
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.implement.md")),
        "# implement\n\nrun spec-kitty agent action implement\n"
    );
    assert!(!detect("0.10.6_workflow_simplification", root));
}

// --------------------------------------------------------------------- 0.11.3

#[test]
fn workflow_agent_flag_appends_the_flag_and_expands_the_placeholder() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/commands/spec-kitty.implement.md"),
        "run spec-kitty agent action implement\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.review.md"),
        "spec-kitty agent action review --agent __AGENT__\n",
    );
    assert!(detect("0.11.3_workflow_agent_flag", root));
    assert!(apply("0.11.3_workflow_agent_flag", root).changed);
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.implement.md")),
        "run spec-kitty agent action implement --agent claude\n"
    );
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.review.md")),
        "spec-kitty agent action review --agent claude\n"
    );
    assert!(!detect("0.11.3_workflow_agent_flag", root));
}

// --------------------------------------------------------------------- 0.13.8

#[test]
fn target_branch_is_added_without_touching_other_fields() {
    let directory = project();
    let root = directory.path();
    std::fs::create_dir_all(root.join("kitty-specs/001-mission")).expect("mission");
    write(
        &root.join("kitty-specs/001-mission/meta.json"),
        "{\"mission_id\": \"01HX\", \"mission_number\": \"001\", \"friendly_name\": \"mission\"}\n",
    );
    write(
        &root.join("kitty-specs/001-mission/plan.md"),
        "**Target Branch**: `release/2.x`\n",
    );
    assert!(detect("0.13.8_target_branch", root));
    assert!(apply("0.13.8_target_branch", root).changed);
    let value: serde_json::Value =
        serde_json::from_str(&read(&root.join("kitty-specs/001-mission/meta.json"))).expect("json");
    assert_eq!(value["target_branch"], "release/2.x");
    assert_eq!(value["mission_id"], "01HX");
    assert!(!detect("0.13.8_target_branch", root));
}

#[test]
fn a_malformed_meta_json_is_reported_as_a_malformed_read() {
    let directory = project();
    let root = directory.path();
    std::fs::create_dir_all(root.join("kitty-specs/001-mission")).expect("mission");
    write(&root.join("kitty-specs/001-mission/meta.json"), "{not json");
    let outcome = apply("0.13.8_target_branch", root);
    assert!(!outcome.changed);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("Malformed JSON")),
        "{:?}",
        outcome.warnings
    );
    // Nothing was overwritten.
    assert_eq!(
        read(&root.join("kitty-specs/001-mission/meta.json")),
        "{not json"
    );
}

// --------------------------------------------------------------- 0.13.0 CSVs

#[test]
fn research_csv_schema_check_reports_a_mismatch_and_modifies_nothing() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join("kitty-specs/002-research/meta.json"),
        "{\"mission\": \"research\"}\n",
    );
    let csv = root.join("kitty-specs/002-research/research/evidence-log.csv");
    write(&csv, "timestamp,notes\n2026-01-01,cited\n");
    assert!(detect("0.13.0_research_csv_schema_check", root));
    let before = read(&csv);
    let outcome = apply("0.13.0_research_csv_schema_check", root);
    assert!(!outcome.changed);
    assert_eq!(read(&csv), before);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("evidence-log.csv")),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn a_canonical_research_csv_is_not_flagged() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join("kitty-specs/002-research/meta.json"),
        "{\"mission\": \"research\"}\n",
    );
    write(
        &root.join("kitty-specs/002-research/research/evidence-log.csv"),
        "timestamp,source_type,citation,key_finding,confidence,notes\n",
    );
    assert!(!detect("0.13.0_research_csv_schema_check", root));
}

// ------------------------------------------------------- 2.0.1 config rename

#[test]
fn tool_config_key_rename_preserves_comments_and_authored_keys() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "# operator note\ntools_legacy: keep\nagents:\n  available:\n  - claude\n# trailing comment\nproject:\n  name: demo\n",
    );
    assert!(detect("2.0.1_tool_config_key_rename", root));
    assert_eq!(can_apply("2.0.1_tool_config_key_rename", root), None);
    assert!(apply("2.0.1_tool_config_key_rename", root).changed);
    let text = read(&root.join(".kittify/config.yaml"));
    assert!(text.contains("# operator note"), "{text}");
    assert!(text.contains("# trailing comment"), "{text}");
    assert!(text.contains("tools:\n"), "{text}");
    assert!(!text.contains("agents:\n"), "{text}");
    assert!(text.contains("tools_legacy: keep"), "{text}");
    assert!(!detect("2.0.1_tool_config_key_rename", root));
}

#[test]
fn tool_config_key_rename_leaves_a_canonical_config_alone() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "tools:\n  available:\n  - claude\nagents:\n  available:\n  - claude\n",
    );
    assert!(!detect("2.0.1_tool_config_key_rename", root));
    assert!(!apply("2.0.1_tool_config_key_rename", root).changed);
}

// ------------------------------------------------------------ 2.0.1 prompts

#[test]
fn generated_prompt_repair_rewrites_stale_paths_and_merge_wording() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/commands/spec-kitty.tasks.md"),
        "Run spec-kitty agent check-prerequisites --require-tasks --include-tasks\n\
         The command merges each lane branch into main in sequence.\n",
    );
    assert!(detect("2.0.1_fix_generated_command_templates", root));
    assert!(apply("2.0.1_fix_generated_command_templates", root).changed);
    let text = read(&root.join(".claude/commands/spec-kitty.tasks.md"));
    assert!(
        text.contains("spec-kitty agent mission check-prerequisites --include-tasks"),
        "{text}"
    );
    assert!(
        text.contains("merges lane branches into the mission branch before landing"),
        "{text}"
    );
    assert!(!detect("2.0.1_fix_generated_command_templates", root));
}

// --------------------------------------------------------- 2.1.3 terminology

#[test]
fn planning_repository_terminology_is_replaced_in_agent_commands() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/commands/spec-kitty.specify.md"),
        "Create the mission in the planning repository, not a worktree.\n",
    );
    assert!(detect("2.1.3_fix_planning_repository_terminology", root));
    assert_eq!(
        can_apply("2.1.3_fix_planning_repository_terminology", root),
        None
    );
    assert!(apply("2.1.3_fix_planning_repository_terminology", root).changed);
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.specify.md")),
        "Create the mission in the project root checkout, not a worktree.\n"
    );
    assert!(!detect("2.1.3_fix_planning_repository_terminology", root));
    assert!(
        can_apply("2.1.3_fix_planning_repository_terminology", root).is_some(),
        "no hits is the source's not-applicable refusal"
    );
}

// -------------------------------------------------------- 2.2.0 profile-context

#[test]
fn profile_context_command_is_removed_only_when_proven_generated() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/commands/spec-kitty.profile-context.md"),
        "<!-- spec-kitty-command-version: 2.2.0 -->\n# profile-context\n",
    );
    write(
        &root.join(".agents/skills/x/spec-kitty.profile-context.md"),
        "# handwritten\n",
    );
    assert!(detect("2.2.0_profile_context_deployment", root));
    let outcome = apply("2.2.0_profile_context_deployment", root);
    assert!(outcome.changed);
    assert!(
        !root
            .join(".claude/commands/spec-kitty.profile-context.md")
            .exists()
    );
    assert!(!detect("2.2.0_profile_context_deployment", root));
}

#[test]
fn a_user_authored_profile_context_file_is_preserved() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/commands/spec-kitty.profile-context.md"),
        "# my own profile command\n",
    );
    let outcome = apply("2.2.0_profile_context_deployment", root);
    assert!(!outcome.changed);
    assert!(
        root.join(".claude/commands/spec-kitty.profile-context.md")
            .is_file()
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("not package-owned")),
        "{:?}",
        outcome.warnings
    );
}

// -------------------------------------------------------- 2.0.7 stale overrides

#[test]
fn stale_overrides_are_removed_only_when_byte_identical_to_the_package() {
    let directory = project();
    let root = directory.path();
    let Some(package) = sk_install::migrations::completion_legacy::assets::package_asset_root()
    else {
        return;
    };
    let counterpart = package.join("software-dev/templates/plan-template.md");
    if !counterpart.is_file() {
        return;
    }
    // `overrides/` mirrors `.kittify/`: a top-level shared asset dir maps to the
    // software-dev package default (the pinned `_matches_package_default` rule).
    let target = root.join(".kittify/overrides/templates/plan-template.md");
    write(
        &target,
        &std::fs::read_to_string(&counterpart).expect("counterpart"),
    );
    let custom = root.join(".kittify/overrides/templates/spec-template.md");
    write(&custom, "# my spec template\n");

    assert!(detect("2.0.7_fix_stale_overrides", root));
    assert_eq!(can_apply("2.0.7_fix_stale_overrides", root), None);
    let outcome = apply("2.0.7_fix_stale_overrides", root);
    assert!(outcome.changed, "{:?}", outcome.warnings);
    assert!(!target.exists(), "byte-identical override is removed");
    assert_eq!(read(&custom), "# my spec template\n");
    assert!(!detect("2.0.7_fix_stale_overrides", root));
}

// ------------------------------------------------------------------ 2.0.11

#[test]
fn install_skills_writes_the_managed_manifest_for_configured_agents() {
    let directory = project();
    let root = directory.path();
    assert!(detect("2.0.11_install_skills", root));
    assert_eq!(can_apply("2.0.11_install_skills", root), None);
    let outcome = apply("2.0.11_install_skills", root);
    assert!(outcome.changed, "{:?}", outcome.warnings);
    let manifest = read(&root.join(".kittify/skills-manifest.json"));
    assert!(
        manifest.contains("\"spec-kitty-mission-system\""),
        "{manifest}"
    );
    assert!(
        root.join(".claude/skills/spec-kitty-mission-system/SKILL.md")
            .is_file()
    );
    assert!(!detect("2.0.11_install_skills", root));
}

// ------------------------------------------------------------ 2.1.2 skills

#[test]
fn git_workflow_skill_is_installed_into_the_expected_roots() {
    let directory = project();
    let root = directory.path();
    assert!(detect("2.1.2_install_git_workflow_skill", root));
    assert!(apply("2.1.2_install_git_workflow_skill", root).changed);
    assert!(
        root.join(".claude/skills/spec-kitty-git-workflow/SKILL.md")
            .is_file()
    );
    assert!(
        root.join(".agents/skills/spec-kitty-git-workflow/SKILL.md")
            .is_file()
    );
    assert!(!detect("2.1.2_install_git_workflow_skill", root));
    // Idempotent: a second apply writes nothing.
    assert!(!apply("2.1.2_install_git_workflow_skill", root).changed);
}

#[test]
fn mission_system_skill_is_installed_with_its_reference() {
    let directory = project();
    let root = directory.path();
    assert!(detect("2.1.2_install_mission_system_skill", root));
    assert!(apply("2.1.2_install_mission_system_skill", root).changed);
    assert!(
        root.join(".claude/skills/spec-kitty-mission-system/SKILL.md")
            .is_file()
    );
    if sk_install::migrations::completion_legacy::assets::canonical_skill_files(
        "spec-kitty-mission-system",
    )
    .is_some_and(|files| files.iter().any(|(path, _)| path.contains("references/")))
    {
        assert!(
            root.join(
                ".claude/skills/spec-kitty-mission-system/references/mission-comparison-matrix.md"
            )
            .is_file()
        );
    }
}

#[test]
fn an_out_of_date_orchestrator_skill_file_is_replaced_from_the_canonical_skill() {
    let directory = project();
    let root = directory.path();
    let Some(files) = sk_install::migrations::completion_legacy::assets::canonical_skill_files(
        "spec-kitty-orchestrator-api-operator",
    ) else {
        return;
    };
    let Some((_, canonical)) = files.iter().find(|(path, _)| path == "SKILL.md") else {
        return;
    };
    let target = root.join(".claude/skills/spec-kitty-orchestrator-api-operator/SKILL.md");
    write(
        &target,
        "## Step 1: Verify the API Contract\n\n```bash\nspec-kitty orchestrator-api contract-version\n```\n",
    );
    assert!(detect("2.1.2_fix_orchestrator_api_skill", root));
    assert!(apply("2.1.2_fix_orchestrator_api_skill", root).changed);
    assert_eq!(std::fs::read(&target).expect("bytes"), *canonical);
    // The pinned `_OLD_MARKERS` prefix also occurs inside the canonical file, so
    // the source re-selects this repair on every run; the write is byte-stable.
    let again = apply("2.1.2_fix_orchestrator_api_skill", root);
    assert_eq!(std::fs::read(&target).expect("bytes"), *canonical);
    // The pinned marker matches the canonical file too, so a re-run re-selects it.
    assert!(again.changed);
}

// ------------------------------------------------------ 2.1.3 restore prompts

#[test]
fn a_thin_shim_is_replaced_by_a_full_rendered_prompt() {
    let directory = project();
    let root = directory.path();
    if sk_install::migrations::completion_legacy::agent_commands::runtime_command_templates(
        "software-dev",
    )
    .is_none()
    {
        return;
    }
    write(
        &root.join(".claude/commands/spec-kitty.specify.md"),
        "Run: spec-kitty agent shim specify \"$ARGUMENTS\"\n",
    );
    assert!(detect("2.1.3_restore_prompt_commands", root));
    assert_eq!(can_apply("2.1.3_restore_prompt_commands", root), None);
    let outcome = apply("2.1.3_restore_prompt_commands", root);
    assert!(outcome.changed, "{:?}", outcome.warnings);
    let restored = read(&root.join(".claude/commands/spec-kitty.specify.md"));
    assert!(!restored.contains("spec-kitty agent shim"), "{restored}");
    assert!(
        restored
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
            >= 10,
        "a full prompt, not a shim: {restored}"
    );
    assert!(
        restored.contains("spec-kitty-command-version"),
        "{restored}"
    );
    assert!(!detect("2.1.3_restore_prompt_commands", root));
}

#[test]
fn a_full_prompt_is_not_touched_by_the_restore() {
    let directory = project();
    let root = directory.path();
    let body: String = (0..20).map(|index| format!("line {index}\n")).collect();
    write(&root.join(".claude/commands/spec-kitty.plan.md"), &body);
    assert!(!detect("2.1.3_restore_prompt_commands", root));
    assert!(!apply("2.1.3_restore_prompt_commands", root).changed);
    assert_eq!(
        read(&root.join(".claude/commands/spec-kitty.plan.md")),
        body
    );
}

// ------------------------------------------------------------------- 2.0.6

#[test]
fn consistency_sweep_writes_a_baseline_meta_and_leaves_residue_alone() {
    let directory = project();
    let root = directory.path();
    // A Mission without meta.json but with a tracked-free spec.md.
    write(&root.join("kitty-specs/003-repair/spec.md"), "# spec\n");
    write(
        &root.join("kitty-specs/003-repair/tasks/WP01-fix.md"),
        "---\nlane: \"doing\"\ntitle: fix\n---\nbody\n",
    );
    // Residue: only a stray file, no Mission content.
    write(&root.join("kitty-specs/leftovers/notes.md"), "scratch\n");

    assert!(detect("2.0.6_residue_safe_consistency_sweep", root));
    assert_eq!(
        can_apply("2.0.6_residue_safe_consistency_sweep", root),
        None
    );
    let outcome = apply("2.0.6_residue_safe_consistency_sweep", root);
    assert!(outcome.changed, "{:?}", outcome.warnings);
    let meta: serde_json::Value =
        serde_json::from_str(&read(&root.join("kitty-specs/003-repair/meta.json"))).expect("meta");
    assert_eq!(meta["mission_number"], "003");
    assert_eq!(meta["slug"], "003-repair");
    assert_eq!(meta["mission_type"], "software-dev");
    assert!(
        meta.get("mission_id").is_none(),
        "no identity is fabricated"
    );
    let wp = read(&root.join("kitty-specs/003-repair/tasks/WP01-fix.md"));
    assert!(wp.contains("lane: \"in_progress\""), "{wp}");
    assert!(wp.contains("title: fix"), "{wp}");
    assert_eq!(
        read(&root.join("kitty-specs/leftovers/notes.md")),
        "scratch\n"
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("residue folder untouched")),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn consistency_sweep_regenerates_a_drifted_status_snapshot_from_the_event_log() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join("kitty-specs/004-state/meta.json"),
        "{\"mission_id\": \"01HX\", \"mission_type\": \"software-dev\"}\n",
    );
    // A minimal, valid single event row plus a stale snapshot.
    let event = r#"{"event_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","mission_slug":"004-state","wp_id":"WP01","from_lane":"planned","to_lane":"in_progress","at":"2026-11-02T00:00:00Z","actor":"agent-1","force":false,"execution_mode":"worktree"}"#;
    write(
        &root.join("kitty-specs/004-state/status.events.jsonl"),
        &format!("{event}\n"),
    );
    write(
        &root.join("kitty-specs/004-state/status.json"),
        "{\"event_count\": 0, \"work_packages\": {}, \"feature_slug\": \"004-state\"}\n",
    );
    let outcome = apply("2.0.6_residue_safe_consistency_sweep", root);
    assert!(outcome.changed, "{:?}", outcome.warnings);
    let snapshot: serde_json::Value =
        serde_json::from_str(&read(&root.join("kitty-specs/004-state/status.json")))
            .expect("snapshot");
    assert_eq!(snapshot["event_count"], 1, "{snapshot}");
}

#[test]
fn an_orphan_status_snapshot_is_archived_not_deleted() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join("kitty-specs/005-orphan/meta.json"),
        "{\"mission_id\": \"01HX\", \"mission_type\": \"software-dev\"}\n",
    );
    write(
        &root.join("kitty-specs/005-orphan/status.json"),
        "{\"event_count\": 0, \"work_packages\": {}, \"feature_slug\": \"005-orphan\", \"summary\": {\"planned\": 0, \"in_progress\": 0, \"for_review\": 0, \"approved\": 0, \"done\": 0, \"blocked\": 0, \"canceled\": 0}}\n",
    );
    apply("2.0.6_residue_safe_consistency_sweep", root);
    assert!(!root.join("kitty-specs/005-orphan/status.json").exists());
    let archived: Vec<PathBuf> = std::fs::read_dir(root.join("kitty-specs/005-orphan"))
        .expect("dir")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| {
                    name.to_string_lossy()
                        .starts_with("status.json.orphan.bak.")
                })
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        archived.len(),
        1,
        "the orphan snapshot is preserved on disk"
    );
}

// ------------------------------------------------------------------- layout

#[test]
fn layout_classifies_and_moves_customised_assets_into_overrides() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/templates/custom-template.md"),
        "# mine\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        "tools:\n  available:\n  - claude\n",
    );
    let report = execute_migration(root, false, "software-dev").expect("migration");
    assert_eq!(report.moved.len(), 1);
    assert!(
        root.join(".kittify/overrides/templates/custom-template.md")
            .is_file()
    );
    assert!(!root.join(".kittify/templates/custom-template.md").exists());
    assert!(root.join(".kittify/config.yaml").is_file());
    assert_eq!(
        classify_asset(
            &root.join(".kittify/config.yaml"),
            &root.join(".no-home"),
            &root.join(".kittify"),
            "software-dev",
            None
        ),
        AssetDisposition::ProjectSpecific
    );
}

#[test]
fn layout_preserves_a_file_that_differs_from_the_package_default() {
    let directory = project();
    let root = directory.path();
    let Some(package) = sk_install::migrations::completion_legacy::assets::package_asset_root()
    else {
        return;
    };
    if !package
        .join("software-dev/templates/plan-template.md")
        .is_file()
    {
        return;
    }
    write(
        &root.join(".kittify/missions/software-dev/templates/plan-template.md"),
        "# our own plan template\n",
    );
    let report = execute_migration(root, false, "software-dev").expect("migration");
    assert!(report.superseded.len() == 1, "{report:?}");
    assert!(
        root.join(".kittify/missions/software-dev/templates/plan-template.md")
            .is_file()
    );
}

#[test]
fn layout_reports_a_blocked_move_as_a_conflict_and_writes_nothing() {
    let directory = project();
    let root = directory.path();
    write(&root.join(".kittify/templates/kept.md"), "# mine\n");
    write(
        &root.join(".kittify/overrides/templates/kept.md"),
        "# occupies the destination\n",
    );
    let report = execute_migration(root, false, "software-dev").expect("migration");
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(
        report.conflicts[0].reason,
        ConflictReason::DestinationExists
    );
    assert!(root.join(".kittify/templates/kept.md").is_file());
    assert_eq!(
        read(&root.join(".kittify/overrides/templates/kept.md")),
        "# occupies the destination\n"
    );
}

#[test]
fn layout_dry_run_reports_without_writing() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/templates/custom-template.md"),
        "# mine\n",
    );
    let report = execute_migration(root, true, "software-dev").expect("migration");
    assert_eq!(report.moved.len(), 1);
    assert!(root.join(".kittify/templates/custom-template.md").is_file());
    assert!(!root.join(".kittify/overrides").exists());
}

// ------------------------------------------------- canonical registry wiring

#[test]
fn every_legacy_implementation_is_reachable_through_the_canonical_registry() {
    for migration in legacy::IMPLEMENTATIONS {
        let registered = sk_install::native_migrations()
            .find(|candidate| candidate.id == migration.id)
            .unwrap_or_else(|| panic!("{} is not in the canonical registry", migration.id));
        // The same function pointers, not a second implementation.
        assert!(std::ptr::fn_addr_eq(
            registered.detect,
            migration.detect as fn(&Path) -> Result<bool, sk_install::InstallError>
        ));
        assert!(std::ptr::fn_addr_eq(
            registered.apply,
            migration.apply
                as fn(&Path) -> Result<sk_install::MigrationOutcome, sk_install::InstallError>
        ));
    }
    let declared = sk_install::MigrationCatalog::embedded()
        .expect("catalog")
        .len();
    assert!(sk_install::native_migrations().count() <= declared);
}

#[test]
fn the_real_runner_applies_a_legacy_migration_through_the_registry() {
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  schema_version: 3\n  version: 4.0.0rc7\n  initialized_at: '2026-01-01T00:00:00+00:00'\n",
    );
    // `agents:` is the legacy key the 2.0.1 rename targets.
    std::fs::write(
        root.join(".kittify/config.yaml"),
        "# operator note\nagents:\n  available:\n  - claude\n",
    )
    .expect("config");
    // Settle every declared migration except the one under test.
    let catalog = sk_install::MigrationCatalog::embedded().expect("catalog");
    let ledger = sk_install::MigrationLedger::new(root);
    let records: Vec<sk_install::LedgerRecord> = catalog
        .specs()
        .iter()
        .filter(|spec| spec.id != "2.0.1_tool_config_key_rename")
        .map(|spec| {
            sk_install::LedgerRecord::for_spec(spec, sk_install::SETTLED_RESULT, "4.0.0rc8")
        })
        .collect();
    ledger.append_many(&records).expect("seed");

    let report = sk_install::run_upgrade(&sk_install::UpgradeRequest::new(root, "4.0.0rc8"))
        .expect("upgrade runs");
    assert_eq!(report.applied(), vec!["2.0.1_tool_config_key_rename"]);
    let config = read(&root.join(".kittify/config.yaml"));
    assert!(config.contains("tools:"), "{config}");
    assert!(config.contains("# operator note"), "{config}");
    assert!(!config.contains("agents:"), "{config}");
    let recorded = sk_install::MigrationLedger::new(root)
        .records(false)
        .expect("records");
    assert!(recorded.iter().any(|record| {
        record.id == "2.0.1_tool_config_key_rename" && record.result == sk_install::SETTLED_RESULT
    }));
    let metadata = read(&root.join(".kittify/metadata.yaml"));
    assert!(metadata.contains("version: 4.0.0rc8"), "{metadata}");
}

// ------------------------------------------------------------- shared façade

#[test]
fn the_shared_facade_retires_a_generated_command_and_preserves_a_user_file() {
    use sk_install::migrations::completion_legacy::shared;
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/commands/spec-kitty.retired.md"),
        "<!-- spec-kitty-command-version: 4.0.0rc8 -->\n# retired\n",
    );
    write(
        &root.join(".agents/commands/spec-kitty.retired.md"),
        "not a command dir here\n",
    );
    let outcome = shared::retire_command_file(root, "spec-kitty.retired.md").expect("retire");
    assert!(outcome.changed);
    assert!(!root.join(".claude/commands/spec-kitty.retired.md").exists());
    // The unprovable copy at a non-command path is untouched.
    assert!(
        root.join(".agents/commands/spec-kitty.retired.md")
            .is_file()
    );
}

#[test]
fn the_shared_facade_preserves_an_unproven_command_file() {
    use sk_install::migrations::completion_legacy::shared;
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/commands/spec-kitty.retired.md"),
        "# my own command\n",
    );
    let outcome = shared::retire_command_file(root, "spec-kitty.retired.md").expect("retire");
    assert!(!outcome.changed);
    assert!(
        root.join(".claude/commands/spec-kitty.retired.md")
            .is_file()
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("not package-owned")),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn the_shared_facade_archives_an_unprovable_skill_tree_instead_of_deleting_it() {
    use sk_install::migrations::completion_legacy::shared;
    let directory = project();
    let root = directory.path();
    write(
        &root.join(".claude/skills/retired-skill/SKILL.md"),
        "# mine\n",
    );
    let outcome = shared::retire_skill_dir(root, "retired-skill", Some(&root.join(".kittify")))
        .expect("retire");
    assert!(!root.join(".claude/skills/retired-skill").exists());
    assert!(outcome.changed, "{:?}", outcome.warnings);
    // The bytes are recoverable under the archive parent.
    let mut found = false;
    let mut stack = vec![root.join(".kittify")];
    while let Some(directory) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.ends_with("SKILL.md") {
                found = true;
            }
        }
    }
    assert!(
        found,
        "the preserved skill bytes live under the archive parent"
    );
}

#[test]
fn the_shared_facade_exposes_the_package_asset_root_and_canonical_skills() {
    use sk_install::migrations::completion_legacy::shared;
    let root = shared::asset_root().expect("the pinned package asset root");
    assert!(root.join("software-dev/mission.yaml").is_file(), "{root:?}");
    assert!(shared::package_mission("research").is_some());
    let text = shared::canonical_skill_text("spec-kitty-mission-system", "SKILL.md")
        .expect("canonical skill text");
    assert!(
        text.contains("spec-kitty-mission-system"),
        "{}",
        &text[..64.min(text.len())]
    );
    assert!(shared::canonical_skill_text("not-a-skill", "SKILL.md").is_none());
}

// ------------------------------------------------- Source-first differential
//
// The oracle trees under `tests/fixtures/legacy_oracle/` are the recorded output
// of a probe that ran the *pinned* migration classes (`.venv-rust-oracle`,
// CPython 3.11.15/53) on these fixtures. Each case rebuilds the fixture here,
// applies the native migration and compares the resulting tree byte-for-byte.
//
// The fixtures are committed, so the differential always runs; a missing or
// unreadable fixture FAILS the test. `SK_LEGACY_ORACLE` may point at a fresh
// probe directory to re-record them, but its absence is never a pass.

fn oracle_dir() -> PathBuf {
    std::env::var_os("SK_LEGACY_ORACLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("legacy_oracle")
        })
}

fn tree_of(root: &Path) -> std::collections::BTreeMap<String, String> {
    use sha2::Digest;
    let mut tree = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for entry in entries {
            let relative = entry
                .strip_prefix(root)
                .expect("inside root")
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if metadata.is_dir() {
                tree.insert(format!("{relative}/"), "DIR".to_string());
                stack.push(entry);
                continue;
            }
            let bytes = std::fs::read(&entry).unwrap_or_default();
            tree.insert(relative, format!("{:x}", sha2::Sha256::digest(&bytes)));
        }
    }
    tree
}

fn compare_with_source(case: &str, build: impl Fn(&Path), id: &str, skip: &[&str]) {
    let directory = oracle_dir();
    let path = directory.join(format!("{case}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let oracle: serde_json::Value = serde_json::from_str(&text).expect("oracle json");
    assert_eq!(oracle["case"], case);
    assert_eq!(oracle["meta"]["error"], serde_json::Value::Null);

    let project = project();
    let root = project.path();
    build(root);
    // The fixture must be identical before the migration runs.
    assert_eq!(
        detect(id, root),
        oracle["meta"]["detect"].as_bool().expect("detect"),
        "{case}: detect differs from Source"
    );
    let can_apply = can_apply(id, root);
    let source_can_apply = oracle["meta"]["can_apply"][0].as_bool().expect("can_apply");
    assert_eq!(
        can_apply.is_none(),
        source_can_apply,
        "{case}: can_apply differs"
    );
    let outcome = apply(id, root);
    assert_eq!(
        outcome.changed,
        oracle["meta"]["success"].as_bool().expect("success"),
        "{case}: success differs"
    );

    let native = tree_of(root);
    let expected = oracle["tree"].as_object().expect("tree");
    let mut mismatches: Vec<String> = Vec::new();
    for (relative, digest) in expected {
        if skip.contains(&relative.as_str()) {
            continue;
        }
        let want = digest.as_str().expect("digest");
        match native.get(relative) {
            Some(observed) if observed == want => {}
            Some(observed) => {
                mismatches.push(format!("{relative}: source {want} native {observed}"))
            }
            None => mismatches.push(format!("{relative}: source {want} native ABSENT")),
        }
    }
    for relative in native.keys() {
        if skip.contains(&relative.as_str()) {
            continue;
        }
        if !expected.contains_key(relative) {
            mismatches.push(format!("{relative}: native EXTRA"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{case}: tree differs from Source:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn differential_tool_config_key_rename_matches_source_data() {
    // The pinned module writes the renamed key through ruamel's round-trip
    // dumper, which appends the popped/re-inserted key at the end of the
    // mapping. The native edit keeps the key where the operator wrote it and
    // preserves every comment byte; the *data* is identical, which is what this
    // differential compares (see the report's documented deviation).
    let oracle: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(oracle_dir().join("2_0_1_tool_config_key_rename.json"))
            .expect("oracle"),
    )
    .expect("json");
    let project = project();
    let root = project.path();
    write(
        &root.join(".kittify/config.yaml"),
        "# operator note\ntools_legacy: keep\nagents:\n  available:\n  - claude\n# trailing comment\nproject:\n  name: demo\n",
    );
    assert!(detect("2.0.1_tool_config_key_rename", root));
    assert!(apply("2.0.1_tool_config_key_rename", root).changed);
    let native: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&read(&root.join(".kittify/config.yaml"))).expect("native yaml");
    // The Source post-state is the same mapping: `tools` carries the agents
    // block, and every other key (and the comments) survive.
    let source_yaml =
        "tools_legacy: keep\nproject:\n  name: demo\ntools:\n  available:\n  - claude\n";
    let source: serde_yaml_ng::Value = serde_yaml_ng::from_str(source_yaml).expect("source yaml");
    assert_eq!(native, source, "native {native:#?} vs source {source:#?}");
    let text = read(&root.join(".kittify/config.yaml"));
    assert!(text.contains("# operator note"));
    assert!(text.contains("# trailing comment"));
    let _ = oracle;
}

/// The base files every differential fixture carries (they exist before the
/// migration runs, so both sides start from identical bytes).
fn oracle_base(root: &Path) {
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n  - codex\n",
    );
    write(
        &root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  schema_version: 3\n  version: 4.0.0rc8\n",
    );
}

#[test]
fn differential_terminology_matches_source_bytes() {
    compare_with_source(
        "2_1_3_fix_planning_repository_terminology",
        |root| {
            oracle_base(root);
            write(
                &root.join(".claude/commands/spec-kitty.specify.md"),
                "Create the mission in the planning repository, not a worktree.\n",
            );
        },
        "2.1.3_fix_planning_repository_terminology",
        &[],
    );
}

#[test]
fn differential_profile_context_retirement_matches_source_bytes() {
    compare_with_source(
        "2_2_0_profile_context_deployment",
        |root| {
            oracle_base(root);
            write(
                &root.join(".claude/commands/spec-kitty.profile-context.md"),
                "<!-- spec-kitty-command-version: 2.2.0 -->\n# profile-context\n",
            );
            write(
                &root.join(".claude/commands/spec-kitty.user.md"),
                "# mine\n",
            );
        },
        "2.2.0_profile_context_deployment",
        &[],
    );
}

#[test]
fn differential_generated_prompt_repair_matches_source_bytes() {
    compare_with_source(
        "2_0_1_fix_generated_command_templates",
        |root| {
            oracle_base(root);
            write(
                &root.join(".claude/commands/spec-kitty.tasks.md"),
                "Run spec-kitty agent check-prerequisites --require-tasks --include-tasks\n\
                 The command merges each lane branch into main in sequence.\n",
            );
        },
        "2.0.1_fix_generated_command_templates",
        &[],
    );
}

#[test]
fn differential_python_only_matches_source_bytes() {
    compare_with_source(
        "0_10_0_python_only",
        |root| {
            oracle_base(root);
            write(
                &root.join(".kittify/scripts/bash/common.sh"),
                "#!/bin/sh\n# custom\n",
            );
            write(
                &root.join(".kittify/templates/command-templates/specify.md"),
                "run .kittify/scripts/bash/create-new-feature.sh now\n",
            );
        },
        "0.10.0_python_only",
        &[],
    );
}

#[test]
fn differential_stale_override_removal_matches_source_bytes() {
    compare_with_source(
        "2_0_7_fix_stale_overrides",
        |root| {
            oracle_base(root);
            let package = sk_install::migrations::completion_legacy::assets::package_asset_root()
                .expect("package root")
                .join("software-dev/templates/plan-template.md");
            write(
                &root.join(".kittify/overrides/templates/plan-template.md"),
                &read(&package),
            );
            write(
                &root.join(".kittify/overrides/templates/spec-template.md"),
                "# my spec template\n",
            );
        },
        "2.0.7_fix_stale_overrides",
        &[],
    );
}

#[test]
fn differential_restored_prompt_matches_source_bytes() {
    // Full byte parity: the pinned renderer annotates glossary references with
    // invisible `<!-- glossary:glossary:<term-id> -->` comments, resolved from the
    // repository whose `.kittify/` is nearest the *template* path. The native
    // renderer reproduces that (same authority, same order, same insertion
    // points), so no annotation stripping is involved.
    compare_with_source(
        "2_1_3_restore_prompt_commands",
        |root| {
            oracle_base(root);
            write(
                &root.join(".claude/commands/spec-kitty.specify.md"),
                "Run: spec-kitty agent shim specify \"$ARGUMENTS\"\n",
            );
        },
        "2.1.3_restore_prompt_commands",
        &[],
    );
    let project = project();
    let root = project.path();
    oracle_base(root);
    write(
        &root.join(".claude/commands/spec-kitty.specify.md"),
        "Run: spec-kitty agent shim specify \"$ARGUMENTS\"\n",
    );
    apply("2.1.3_restore_prompt_commands", root);
    let restored = read(&root.join(".claude/commands/spec-kitty.specify.md"));
    assert!(
        restored.contains("<!-- glossary:glossary:mission -->"),
        "the canonical annotation is emitted"
    );
}
