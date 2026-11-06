//! The agent surface registry: every configured surface is reachable.
//!
//! These assertions are the contract behind "all configured agent surfaces and
//! argument placeholders": 17 configured keys, 12 slash-command families with
//! their own placeholders, 16 installable skill roots, 1 wrapper-only agent.

mod support;

use sk_install::agent::{
    ARGUMENTS_PLACEHOLDER, CANONICAL_COMMANDS, CommandSurface, RenderFamily, SkillClass,
    TOML_ARGS_PLACEHOLDER, agent_keys, agent_surface, agents, is_canonical_command,
    is_prompt_backed, parse_agent_selection, valid_keys,
};
use sk_install::render::{
    command_file_rel_path, command_skill_rel_path, legacy_command_skill_rel_path,
};

#[test]
fn registry_covers_every_configured_agent_once() {
    let surfaces = agents();
    assert_eq!(surfaces.len(), 17, "the configured agent key count");
    let mut keys: Vec<&str> = surfaces.iter().map(|surface| surface.key).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), surfaces.len(), "agent keys are unique");

    for expected in [
        "copilot",
        "claude",
        "gemini",
        "cursor",
        "qwen",
        "opencode",
        "codex",
        "windsurf",
        "kilocode",
        "auggie",
        "q",
        "kiro",
        "antigravity",
        "vibe",
        "pi",
        "letta",
        "llxprt",
    ] {
        let surface = agent_surface(expected).expect("configured agent resolves");
        assert_eq!(surface.key, expected);
        assert!(!surface.display.is_empty());
    }
    assert_eq!(agent_keys().count(), 17);
    assert!(valid_keys().contains("llxprt"));
}

#[test]
fn slash_command_surfaces_and_argument_placeholders_are_complete() {
    let slash: Vec<_> = agents()
        .iter()
        .filter(|surface| surface.command.is_some())
        .collect();
    assert_eq!(slash.len(), 12, "twelve slash-command harnesses");

    let mut families = std::collections::BTreeMap::new();
    for surface in &slash {
        let command = surface.command.expect("slash surface");
        assert!(
            matches!(
                command.family,
                RenderFamily::Markdown | RenderFamily::Toml | RenderFamily::PromptMarkdown
            ),
            "{} must not use the skill family",
            surface.key
        );
        let expected = match command.family {
            RenderFamily::Toml => TOML_ARGS_PLACEHOLDER,
            _ => ARGUMENTS_PLACEHOLDER,
        };
        assert_eq!(
            command.argument_placeholder, expected,
            "{} placeholder",
            surface.key
        );
        families
            .entry(command.family)
            .or_insert_with(Vec::new)
            .push(surface.key);
    }
    assert_eq!(families[&RenderFamily::Markdown].len(), 8);
    assert_eq!(families[&RenderFamily::Toml].len(), 3);
    assert_eq!(families[&RenderFamily::PromptMarkdown].len(), 1);
    assert_eq!(families[&RenderFamily::PromptMarkdown][0], "copilot");
    assert_eq!(
        families[&RenderFamily::Toml],
        vec!["gemini", "qwen", "llxprt"]
    );

    // The one canonical command-file path per family.
    let copilot = agent_surface("copilot").expect("copilot");
    assert_eq!(
        command_file_rel_path(&copilot.command.expect("surface"), "plan"),
        ".github/prompts/spec-kitty.plan.prompt.md"
    );
    let gemini = agent_surface("gemini").expect("gemini");
    assert_eq!(
        command_file_rel_path(&gemini.command.expect("surface"), "plan"),
        ".gemini/commands/spec-kitty.plan.toml"
    );
    let claude = agent_surface("claude").expect("claude");
    assert_eq!(
        command_file_rel_path(&claude.command.expect("surface"), "plan"),
        ".claude/commands/spec-kitty.plan.md"
    );
    let windsurf = agent_surface("windsurf").expect("windsurf");
    assert_eq!(
        command_file_rel_path(&windsurf.command.expect("surface"), "plan"),
        ".windsurf/workflows/spec-kitty.plan.md"
    );
    let opencode = agent_surface("opencode").expect("opencode");
    assert_eq!(
        command_file_rel_path(&opencode.command.expect("surface"), "plan"),
        ".opencode/command/spec-kitty.plan.md"
    );
}

#[test]
fn skill_surfaces_cover_sixteen_agents_and_one_wrapper() {
    let installable: Vec<_> = agents()
        .iter()
        .filter(|surface| surface.skill.primary_root().is_some())
        .collect();
    assert_eq!(installable.len(), 16, "sixteen installable skill surfaces");

    let wrappers: Vec<_> = agents()
        .iter()
        .filter(|surface| surface.skill.class == SkillClass::WrapperOnly)
        .collect();
    assert_eq!(wrappers.len(), 1);
    assert_eq!(wrappers[0].key, "q");
    assert!(wrappers[0].require_skill_root().is_err());
    assert!(
        wrappers[0].command.is_some(),
        "the wrapper still reads command files"
    );

    let mut native: Vec<&str> = agents()
        .iter()
        .filter(|surface| surface.skill.class == SkillClass::NativeRootRequired)
        .map(|surface| surface.key)
        .collect();
    native.sort_unstable();
    assert_eq!(native, vec!["claude", "kilocode", "qwen"]);

    let mut command_skill_agents: Vec<&str> = agents()
        .iter()
        .filter(|surface| surface.installs_command_skills())
        .map(|surface| surface.key)
        .collect();
    command_skill_agents.sort_unstable();
    assert_eq!(
        command_skill_agents,
        vec!["antigravity", "codex", "letta", "pi", "vibe"]
    );

    // Shared-root agents must actually declare the shared root first.
    for surface in agents()
        .iter()
        .filter(|surface| surface.skill.class == SkillClass::SharedRootCapable)
    {
        assert_eq!(
            surface.skill.primary_root(),
            Some(".agents/skills/"),
            "{} shares .agents/skills/",
            surface.key
        );
    }
}

#[test]
fn command_skill_paths_use_the_canonical_and_legacy_naming_authority() {
    assert_eq!(
        command_skill_rel_path("plan"),
        ".agents/skills/spec-kitty-plan/SKILL.md"
    );
    assert_eq!(
        legacy_command_skill_rel_path("plan"),
        ".agents/skills/spec-kitty.plan/SKILL.md"
    );
    assert_eq!(CANONICAL_COMMANDS.len(), 14);
    let mut sorted = CANONICAL_COMMANDS;
    sorted.sort_unstable();
    assert_eq!(sorted, CANONICAL_COMMANDS, "canonical order is sorted");
    assert!(is_canonical_command("status"));
    assert!(!is_canonical_command("checklist"), "checklist was retired");
    assert!(is_prompt_backed("tasks-outline"));
    assert!(!is_prompt_backed("consolidate"));
}

#[test]
fn agent_selection_is_ordered_deduplicated_and_fail_closed() {
    let selected = parse_agent_selection(" Claude , codex;claude ").expect("valid selection");
    let keys: Vec<&str> = selected.iter().map(|surface| surface.key).collect();
    assert_eq!(keys, vec!["claude", "codex"]);

    let error = parse_agent_selection("claude,nope").expect_err("unknown agent refused");
    assert_eq!(error.code(), "unknown_agent");
    assert!(error.to_string().contains("nope"));
    assert!(error.to_string().contains("llxprt"));

    assert_eq!(
        parse_agent_selection(" , ")
            .expect_err("empty selection")
            .code(),
        "no_agents_selected"
    );
    assert_eq!(
        agent_surface("Claude").expect("case-insensitive").key,
        "claude"
    );
}

#[test]
fn wrapper_agent_refuses_a_slash_surface_it_does_not_have() {
    let codex = agent_surface("codex").expect("codex");
    assert!(codex.command.is_none());
    let error = codex
        .require_command_surface()
        .expect_err("no slash surface");
    assert_eq!(error.code(), "agent_has_no_command_surface");
    assert_eq!(
        codex.require_skill_root().expect("skill root"),
        ".agents/skills/"
    );
}

#[test]
fn command_surface_is_copy_and_self_consistent() {
    let surface = CommandSurface {
        directory: ".claude/commands",
        argument_placeholder: ARGUMENTS_PLACEHOLDER,
        family: RenderFamily::Markdown,
    };
    let copy = surface;
    assert_eq!(copy, surface);
    assert_eq!(RenderFamily::PromptMarkdown.extension(), "prompt.md");
    assert_eq!(RenderFamily::Skill.extension(), "SKILL.md");
    let _ = support::canonical_commands();
}
