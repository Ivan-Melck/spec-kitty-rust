//! Renderer-family acceptance tests: all four families, determinism, refusals.

mod support;

use std::path::Path;

use sk_install::agent::{CANONICAL_COMMANDS, agent_surface, parse_agent_selection};
use sk_install::render::{
    REPLACEMENT_BLOCK, TemplateSource, render_asset, render_command_template,
    rewrite_command_invocations, rewrite_user_input_block,
};

const VERSION: &str = "4.0.0rc7";

#[test]
fn every_slash_family_renders_for_every_canonical_command() {
    let directory = tempfile::tempdir().expect("temp dir");
    let templates = support::missions_root(&directory.path().join("missions"));

    let discovered = templates.available_commands().expect("templates list");
    assert_eq!(
        discovered.len(),
        12,
        "the fixture ships the prompt-backed commands"
    );

    for surface in sk_install::agent::agents()
        .iter()
        .filter(|surface| surface.command.is_some())
    {
        for command in &discovered {
            let command = command.as_str();
            let asset = render_asset(surface, command, VERSION, Some(&templates))
                .unwrap_or_else(|error| panic!("{} {command}: {error}", surface.key));
            let text = String::from_utf8(asset.bytes.clone()).expect("utf-8 render");
            let command_surface = surface.command.expect("slash surface");

            // Path and family are the surface's own.
            assert_eq!(asset.family, command_surface.family);
            assert_eq!(
                asset.path,
                sk_install::command_file_rel_path(&command_surface, command)
            );
            assert!(asset.path.ends_with(&format!(
                "spec-kitty.{command}.{}",
                command_surface.family.extension()
            )));

            // The version marker is present in every family.
            assert!(
                text.contains(&format!("<!-- spec-kitty-command-version: {VERSION} -->")),
                "{} {command} missing version marker",
                surface.key
            );
            // No unsubstituted template variable leaks.
            assert!(
                !text.contains("{ARGS}"),
                "{} {command} leaked {{ARGS}}",
                surface.key
            );
            assert!(
                !text.contains("__AGENT__"),
                "{} {command} leaked __AGENT__",
                surface.key
            );
            assert!(
                text.ends_with('\n'),
                "{} {command} must end with a newline",
                surface.key
            );

            match command_surface.family {
                sk_install::RenderFamily::Toml => {
                    assert!(
                        text.starts_with(&format!(
                            "description = \"Run the {command} workflow\"\n\nprompt = \"\"\"\n"
                        )),
                        "TOML header for {} {command}: {text:?}",
                        surface.key
                    );
                    assert!(
                        text.contains(&format!("spec-kitty {command} {{{{args}}}}")),
                        "TOML placeholder for {} {command}",
                        surface.key
                    );
                    assert!(text.trim_end().ends_with("\"\"\""));
                }
                sk_install::RenderFamily::Markdown | sk_install::RenderFamily::PromptMarkdown => {
                    assert!(
                        text.starts_with(&format!(
                            "---\ndescription: Run the {command} workflow\n---\n"
                        )),
                        "markdown frontmatter for {} {command}: {text:?}",
                        surface.key
                    );
                    assert!(
                        text.contains(&format!("spec-kitty {command} $ARGUMENTS")),
                        "markdown placeholder for {} {command}",
                        surface.key
                    );
                    // The startup upgrade check is prepended to command files.
                    assert!(text.contains("## Startup Upgrade Check"));
                }
                sk_install::RenderFamily::Skill => {
                    unreachable!("slash surfaces never use the skill family")
                }
            }
        }
    }
}

#[test]
fn skill_family_renders_one_shared_body_for_all_command_skill_agents() {
    let directory = tempfile::tempdir().expect("temp dir");
    let templates = support::missions_root(&directory.path().join("missions"));

    let agents: Vec<_> = parse_agent_selection("codex,vibe,pi,letta,antigravity").expect("agents");
    assert_eq!(agents.len(), 5);

    for command in CANONICAL_COMMANDS {
        let mut renders = Vec::new();
        for agent in &agents {
            let asset = render_asset(agent, command, VERSION, Some(&templates))
                .unwrap_or_else(|error| panic!("{} {command}: {error}", agent.key));
            assert_eq!(
                asset.path,
                format!(".agents/skills/spec-kitty-{command}/SKILL.md")
            );
            assert_eq!(asset.family, sk_install::RenderFamily::Skill);
            renders.push(asset.bytes);
        }
        for render in &renders {
            assert_eq!(render, &renders[0], "one shared body for {command}");
            let text = String::from_utf8(render.clone()).expect("utf-8");
            assert!(
                !text.contains("$ARGUMENTS"),
                "no stray $ARGUMENTS in {command}"
            );
            if sk_install::PROMPT_BACKED_COMMANDS.contains(&command) {
                assert!(
                    text.contains(REPLACEMENT_BLOCK),
                    "user input block rewritten"
                );
                assert!(
                    !text.contains("## User Input\n\n```"),
                    "raw user input block replaced"
                );
            }
            assert!(text.starts_with(&format!("---\nname: spec-kitty-{command}\ndescription: ")));
            assert!(text.contains("metadata:\n  user-invocable: \"true\"\n---\n"));
            // The body shares one invocation rewrite for every agent: no dotted
            // token survives, and prompt-backed bodies reference the canonical name.
            assert!(
                !text.contains("/spec-kitty.plan"),
                "dotted invocation retired for {command}"
            );
            if sk_install::PROMPT_BACKED_COMMANDS.contains(&command) {
                assert!(
                    text.contains("/spec-kitty-plan"),
                    "dotted invocation rewritten for {command}"
                );
            }
        }
    }
}

#[test]
fn cli_wrapper_skills_render_without_a_template_source() {
    let directory = tempfile::tempdir().expect("temp dir");
    let missing = TemplateSource::new(directory.path().join("does-not-exist"));
    let codex = agent_surface("codex").expect("codex");

    for command in ["consolidate", "status"] {
        let asset = render_asset(codex, command, VERSION, Some(&missing)).expect("wrapper renders");
        let text = String::from_utf8(asset.bytes).expect("utf-8");
        assert!(text.contains(&format!("name: spec-kitty-{command}")));
        assert!(text.contains("## User Input"));
        assert!(text.contains("spec-kitty"));
        assert!(!text.contains("$ARGUMENTS"));
    }

    // A prompt-backed command genuinely needs a template.
    let error = render_asset(codex, "plan", VERSION, Some(&missing)).expect_err("no template");
    assert_eq!(error.code(), "template_not_found");
    let error = render_asset(codex, "plan", VERSION, None).expect_err("no source");
    assert_eq!(error.code(), "template_source_unavailable");
}

#[test]
fn rendering_is_byte_deterministic() {
    let directory = tempfile::tempdir().expect("temp dir");
    let templates = support::missions_root(&directory.path().join("missions"));

    for key in ["claude", "gemini", "copilot", "codex"] {
        let agent = agent_surface(key).expect("agent");
        let first = render_asset(agent, "implement", VERSION, Some(&templates)).expect("first");
        let second = render_asset(agent, "implement", VERSION, Some(&templates)).expect("second");
        assert_eq!(first.bytes, second.bytes, "{key} render is deterministic");
        assert_eq!(first.source_hash, second.source_hash);
    }
}

#[test]
fn skill_rendering_refuses_a_template_without_user_input() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path().join("missions");
    let templates =
        support::missions_root_with(&root, "plan", &support::prompt_without_user_input("plan"));
    let codex = agent_surface("codex").expect("codex");
    let error = render_asset(codex, "plan", VERSION, Some(&templates)).expect_err("missing block");
    assert_eq!(error.code(), "user_input_block_missing");
    assert!(error.to_string().contains("prompt.md"));
}

#[test]
fn skill_rendering_refuses_a_stray_arguments_token_outside_the_block() {
    let template = "---\ndescription: stray\n---\n\n## Purpose\n\nUse $ARGUMENTS here.\n\n\
## User Input\n\n```text\n$ARGUMENTS\n```\n\n## Steps\n\nGo.\n";
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path().join("missions");
    let templates = support::missions_root_with(&root, "plan", template);
    let codex = agent_surface("codex").expect("codex");
    let error = render_asset(codex, "plan", VERSION, Some(&templates)).expect_err("stray token");
    assert_eq!(error.code(), "stray_arguments_token");
    assert!(error.to_string().contains("line"));
}

#[test]
fn template_source_prefers_the_canonical_layout_then_the_legacy_one() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path().join("missions");
    // Legacy layout only.
    support::write(
        &root.join("software-dev/command-templates/plan.md"),
        support::prompt_template("plan"),
    );
    let source = TemplateSource::new(&root);
    let resolved = source.resolve("plan").expect("legacy layout resolves");
    assert!(
        resolved
            .path
            .ends_with("software-dev/command-templates/plan.md")
    );

    // Canonical layout wins when both exist.
    support::write(
        &root.join("mission-steps/software-dev/plan/prompt.md"),
        support::prompt_template("plan"),
    );
    let resolved = source.resolve("plan").expect("canonical layout resolves");
    assert!(
        resolved
            .path
            .ends_with("mission-steps/software-dev/plan/prompt.md")
    );

    assert_eq!(
        source.resolve("nope").expect_err("absent").code(),
        "template_not_found"
    );
    let unavailable = TemplateSource::new(directory.path().join("missing"));
    assert_eq!(
        unavailable
            .ensure_available()
            .expect_err("missing root")
            .code(),
        "template_source_unavailable"
    );
}

#[test]
fn user_input_block_and_invocation_rewrites_are_exact() {
    let body = "# Title\n\n## User Input\n\n$ARGUMENTS\n\n## Steps\n\nUse /spec-kitty.plan.md and /spec-kitty.plan.\n";
    let rewritten = rewrite_user_input_block(body).expect("block present");
    assert!(rewritten.contains(REPLACEMENT_BLOCK));
    assert!(!rewritten.contains("$ARGUMENTS"));
    let rewritten = rewrite_command_invocations(&rewritten);
    assert!(
        rewritten.contains("/spec-kitty.plan.md"),
        "file names untouched"
    );
    assert!(
        rewritten.contains("/spec-kitty-plan."),
        "sentence-period token rewritten"
    );
    assert!(rewrite_user_input_block("# no block\n").is_none());
}

#[test]
fn toml_family_escapes_triple_quotes_and_backslashes() {
    let surface = sk_install::CommandSurface {
        directory: ".gemini/commands",
        argument_placeholder: "{{args}}",
        family: sk_install::RenderFamily::Toml,
    };
    let template =
        "---\ndescription: \"quoted: desc\"\n---\n\n## Purpose\n\nUse \"\"\" and C:\\path.\n";
    let rendered = render_command_template(template, "gemini", &surface, VERSION).expect("render");
    assert!(rendered.contains("description = \"quoted: desc\""));
    assert!(
        rendered.contains("\"\"\\\""),
        "triple quotes escaped: {rendered}"
    );
    assert!(rendered.contains("C:\\\\path"), "backslash escaped");
    assert!(!rendered.contains("---"), "TOML has no YAML frontmatter");
}

#[test]
fn markdown_family_without_frontmatter_starts_at_the_version_marker() {
    let surface = sk_install::CommandSurface {
        directory: ".claude/commands",
        argument_placeholder: "$ARGUMENTS",
        family: sk_install::RenderFamily::Markdown,
    };
    let rendered = render_command_template(
        "# Plain\n\n## Purpose\n\nPlain body.\n",
        "claude",
        &surface,
        VERSION,
    )
    .expect("render");
    assert!(rendered.starts_with(&format!("<!-- spec-kitty-command-version: {VERSION} -->\n")));
}

#[test]
fn rendered_skill_never_contains_the_placeholder() {
    let directory = tempfile::tempdir().expect("temp dir");
    let templates = support::missions_root(&directory.path().join("missions"));
    let codex = agent_surface("codex").expect("codex");
    for command in CANONICAL_COMMANDS {
        let asset = render_asset(codex, command, VERSION, Some(&templates)).expect("render");
        let text = String::from_utf8(asset.bytes).expect("utf-8");
        for (index, line) in text.lines().enumerate() {
            assert!(
                !line.contains("$ARGUMENTS"),
                "{command} line {} still has $ARGUMENTS",
                index + 1
            );
        }
    }
    let _ = Path::new(".");
}
