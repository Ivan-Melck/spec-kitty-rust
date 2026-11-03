//! `4.0.0rc8_narrow_agent_gitignore` acceptance tests.

use std::path::Path;
use std::process::Command;

use sk_install::gitignore::{AGENT_BLOCK_BEGIN, AGENT_BLOCK_END, GITIGNORE_MARKER};
use sk_install::migrations::narrow_agent_gitignore;

const LEGACY_HEADER: &str = "# AI Agent directories (added by Spec Kitty CLI)\n# These contain auth tokens and should NEVER be committed\n";

fn git(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git")
}

fn project(gitignore: &str, agents: &[&str]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    assert!(git(root, &["init", "-q", "-b", "main"]).status.success());
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    let mut config = String::from("agents:\n  available:");
    if agents.is_empty() {
        config.push_str(" []\n");
    } else {
        config.push('\n');
        for agent in agents {
            config.push_str(&format!("  - {agent}\n"));
        }
    }
    std::fs::write(root.join(".kittify/config.yaml"), config).unwrap();
    std::fs::write(root.join(".gitignore"), gitignore).unwrap();
    directory
}

fn lines(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(".gitignore"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn ignored(root: &Path, path: &str) -> bool {
    git(root, &["check-ignore", "-q", path]).status.success()
}

#[test]
fn marker_run_lines_are_replaced_by_the_configured_block() {
    let directory = project(
        &format!(
            "node_modules/\n\n{GITIGNORE_MARKER}\n.claude/\n.codex/\n.gemini/\n.kittify/runtime/\n.agents/skills/\n"
        ),
        &["claude"],
    );
    let root = directory.path();
    for path in [
        ".claude/settings.json",
        ".claude/skills/spk-x/SKILL.md",
        ".claude/rules/team.md",
        ".agents/skills/team/SKILL.md",
    ] {
        let target = root.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, "x\n").unwrap();
    }
    assert!(narrow_agent_gitignore::detect(root).unwrap());
    let outcome = narrow_agent_gitignore::apply(root).unwrap();
    assert!(outcome.changed, "{:?}", outcome.warnings);
    let lines = lines(root);
    for entry in [".claude/", ".codex/", ".gemini/", ".agents/skills/"] {
        assert!(!lines.contains(&entry.to_string()), "{entry} remained");
    }
    assert!(lines.contains(&".kittify/runtime/".to_string()));
    assert!(lines.contains(&"node_modules/".to_string()));
    assert_eq!(
        lines
            .iter()
            .filter(|line| *line == AGENT_BLOCK_BEGIN)
            .count(),
        1
    );
    assert!(lines.contains(&".claude/commands/spec-kitty.*".to_string()));
    assert!(lines.contains(&".claude/skills/adversarial-squad/".to_string()));
    assert!(!ignored(root, ".claude/settings.json"));
    assert!(!ignored(root, ".claude/rules/team.md"));
    assert!(ignored(root, ".claude/skills/spk-x/SKILL.md"));
    assert!(ignored(root, ".codex/auth.json"));
    assert!(!ignored(root, ".agents/skills/team/SKILL.md"));
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains(".agents/skills/team/SKILL.md"))
    );
    assert!(!narrow_agent_gitignore::detect(root).unwrap());
}

#[test]
fn legacy_header_and_its_entries_are_removed() {
    let directory = project(
        &format!("{LEGACY_HEADER}.claude/\n.roo/\n.github/copilot/\n"),
        &["claude"],
    );
    narrow_agent_gitignore::apply(directory.path()).unwrap();
    let text = std::fs::read_to_string(directory.path().join(".gitignore")).unwrap();
    assert!(!text.contains("AI Agent directories"));
    assert!(!text.contains("auth tokens"));
    let lines = lines(directory.path());
    assert!(!lines.contains(&".roo/".to_string()));
    assert!(lines.contains(&".github/copilot/".to_string()));
}

#[test]
fn operator_rules_outside_a_spec_kitty_run_are_kept_and_named() {
    let directory = project(
        &format!(".kiro\n/.claude/\n\n{GITIGNORE_MARKER}\n.codex/\nmy-own/\n.gemini/\n"),
        &["claude"],
    );
    let outcome = narrow_agent_gitignore::apply(directory.path()).unwrap();
    let lines = lines(directory.path());
    assert!(lines.contains(&".kiro".to_string()));
    assert!(lines.contains(&"/.claude/".to_string()));
    assert!(lines.contains(&".gemini/".to_string()));
    assert!(!lines.contains(&".codex/".to_string()));
    let named = outcome.warnings.join(" ");
    assert!(named.contains("'.kiro'"), "{named}");
    assert!(named.contains("'/.claude/'"), "{named}");
    assert!(named.contains("'.gemini/'"), "{named}");
}

#[test]
fn backfill_rows_appended_after_operator_lines_are_still_spec_kittys() {
    let directory = project(
        &format!(
            "{GITIGNORE_MARKER}\n.claude/\n\n# mine\nbuild/\n.agents/skills/\n.cursor/commands/\n.cursor/skills/\n"
        ),
        &["codex"],
    );
    let outcome = narrow_agent_gitignore::apply(directory.path()).unwrap();
    let lines = lines(directory.path());
    for entry in [".agents/skills/", ".cursor/commands/", ".cursor/skills/"] {
        assert!(!lines.contains(&entry.to_string()), "{entry}");
    }
    assert!(lines.contains(&"build/".to_string()));
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("Kept your own"))
    );
}

#[test]
fn an_operator_cursor_row_is_kept_while_the_historical_neighbour_row_is_removed() {
    let directory = project(
        &format!("{GITIGNORE_MARKER}\n.claude/\n.kittify/runtime/\n.cursor/\n"),
        &["claude"],
    );
    let outcome = narrow_agent_gitignore::apply(directory.path()).unwrap();
    assert!(lines(directory.path()).contains(&".cursor/".to_string()));
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("'.cursor/'"))
    );

    let historical = project(
        &format!("{GITIGNORE_MARKER}\n.gemini/\n.cursor/\n.qwen/\n"),
        &["claude"],
    );
    narrow_agent_gitignore::apply(historical.path()).unwrap();
    assert!(!lines(historical.path()).contains(&".cursor/".to_string()));
}

#[test]
fn explicit_empty_agent_list_keeps_only_secrets_state_and_shared_reserved_names() {
    let directory = project(&format!("{GITIGNORE_MARKER}\n.claude/\n.vibe/\n"), &[]);
    narrow_agent_gitignore::apply(directory.path()).unwrap();
    let lines = lines(directory.path());
    let begin = lines
        .iter()
        .position(|line| line == AGENT_BLOCK_BEGIN)
        .unwrap();
    let end = lines
        .iter()
        .position(|line| line == AGENT_BLOCK_END)
        .unwrap();
    let block = &lines[begin + 1..end];
    assert_eq!(
        block,
        &[
            ".codex/auth.json",
            ".codex/history.jsonl",
            ".codex/sessions/",
            ".codex/log/",
            ".claude/settings.local.json",
            ".gemini/.env",
            ".qwen/.env",
            ".llxprt/.env",
            ".vibe/",
            ".pi/",
            ".letta/",
            ".github/copilot/",
            ".agents/skills/spec-kitty-*/",
            ".agents/skills/spec-kitty.*/",
            ".agents/skills/spec-kitty/",
            ".agents/skills/spk-*/",
        ]
    );
}

#[test]
fn missing_gitignore_gets_the_block_and_crlf_is_preserved() {
    let directory = project("", &["claude"]);
    std::fs::remove_file(directory.path().join(".gitignore")).unwrap();
    assert!(narrow_agent_gitignore::detect(directory.path()).unwrap());
    narrow_agent_gitignore::apply(directory.path()).unwrap();
    assert_eq!(lines(directory.path())[0], AGENT_BLOCK_BEGIN);

    let crlf = project(
        &format!("x\r\n{GITIGNORE_MARKER}\r\n.claude/\r\n"),
        &["claude"],
    );
    narrow_agent_gitignore::apply(crlf.path()).unwrap();
    let raw = std::fs::read(crlf.path().join(".gitignore")).unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("\r\n"));
    assert!(!text.replace("\r\n", "").contains('\n'));
}

#[test]
fn second_apply_changes_nothing() {
    let directory = project(&format!("{GITIGNORE_MARKER}\n.claude/\n"), &["claude"]);
    narrow_agent_gitignore::apply(directory.path()).unwrap();
    let before = std::fs::read_to_string(directory.path().join(".gitignore")).unwrap();
    let again = narrow_agent_gitignore::apply(directory.path()).unwrap();
    assert!(!again.changed);
    assert_eq!(again.changes, vec!["Agent ignore rules already narrow"]);
    assert_eq!(
        std::fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
        before
    );
}

#[test]
fn session_files_and_profiles_are_ignored_for_the_agents_that_write_them() {
    let directory = project(
        &format!("{GITIGNORE_MARKER}\n.cursor/\n.kiro/\n"),
        &["cursor", "kiro"],
    );
    let root = directory.path();
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    std::fs::write(
        root.join(".kittify/agent_profiles_manifest.json"),
        format!(
            "{{\"entries\":[{{\"output_path\":{}}}]}}",
            serde_json::to_string(
                &root
                    .join(".cursor/agents/reviewer.md")
                    .to_string_lossy()
                    .to_string()
            )
            .unwrap()
        ),
    )
    .unwrap();
    narrow_agent_gitignore::apply(root).unwrap();
    let lines = lines(root);
    for entry in [
        ".cursor/rules/spec-kitty.mdc",
        ".kiro/steering/spec-kitty.md",
        ".cursor/agents/reviewer.md",
    ] {
        assert!(lines.contains(&entry.to_string()), "{entry}");
    }
}

#[test]
fn unsafe_inputs_fail_closed_without_overwriting() {
    let directory = project(&format!("{GITIGNORE_MARKER}\n.claude/\n"), &["claude"]);
    std::fs::write(
        directory.path().join(".kittify/config.yaml"),
        "agents: [unclosed\n",
    )
    .unwrap();
    let before = std::fs::read_to_string(directory.path().join(".gitignore")).unwrap();
    assert!(narrow_agent_gitignore::detect(directory.path()).unwrap());
    assert!(
        narrow_agent_gitignore::can_apply(directory.path())
            .unwrap()
            .is_some()
    );
    assert!(narrow_agent_gitignore::apply(directory.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
        before
    );
}
