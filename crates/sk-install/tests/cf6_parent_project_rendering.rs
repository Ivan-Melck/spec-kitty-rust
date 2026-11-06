//! Focused Native project/no-project composition; not Source-paired conformance.
//! No-assets fixtures exercise Native legacy prewrite rendering, NOT full init's
//! final command-skill phase after config publication.

mod support;

use sk_install::migrations::completion_legacy::agent_commands;
use sk_install::render::TemplateSource;
use sk_install::{InitRequest, InstallError, initialize};

const SENTINEL: &str = "CF6_PARENT_CONDITIONAL_SENTINEL";

fn template(root: &std::path::Path) -> TemplateSource {
    let body = format!(
        "{}\n<!-- spdd:reasons-block:start -->\n{SENTINEL}\n<!-- spdd:reasons-block:end -->\n",
        support::prompt_template("plan")
    );
    support::missions_root_with(root, "plan", &body)
}

fn initialized_output(explicit: Option<bool>) -> Vec<String> {
    let project = tempfile::tempdir().expect("project");
    let templates = tempfile::tempdir().expect("templates");
    let source = template(templates.path());
    let source = match explicit {
        Some(active) => source.with_spdd_active(active),
        None => source,
    };
    let request = InitRequest::new(
        project.path(),
        vec!["claude".to_string(), "codex".to_string()],
        "4.0.0rc8",
    )
    .with_templates(source);
    initialize(&request).expect("actual initialization");
    [
        ".claude/commands/spec-kitty.plan.md",
        ".agents/skills/spec-kitty-plan/SKILL.md",
    ]
    .into_iter()
    .map(|path| support::read(&project.path().join(path)))
    .collect()
}

#[test]
fn legacy_init_binds_even_an_explicit_template_source_to_its_actual_project() {
    for output in initialized_output(None) {
        assert!(!output.contains(SENTINEL), "missing config is inactive");
        assert!(!output.contains("spdd:reasons-block"));
        assert!(output.contains("Drive the plan workflow"));
    }
}

#[test]
fn legacy_init_preserves_an_explicit_activation_override() {
    for output in initialized_output(Some(true)) {
        assert!(output.contains(SENTINEL));
        assert!(!output.contains("spdd:reasons-block"));
    }
    for output in initialized_output(Some(false)) {
        assert!(!output.contains(SENTINEL));
    }
}

#[test]
fn full_init_renders_against_its_published_config_and_retries_without_churn() {
    let project = tempfile::tempdir().expect("project");
    let templates = tempfile::tempdir().expect("templates");
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("checkout");
    let assets = sk_install::ProjectAssets::discover(checkout).expect("actual assets");
    let request = InitRequest::new(project.path(), vec!["codex".to_string()], "4.0.0rc8")
        .with_project_assets(assets)
        .with_core_version("4.0.0rc8")
        .with_templates(template(templates.path()));
    assert!(!project.path().join(".kittify/config.yaml").exists());
    initialize(&request).expect("full init");
    assert!(sk_doctrine::is_spdd_reasons_active(project.path()).expect("actual selector"));
    let output = project
        .path()
        .join(".agents/skills/spec-kitty-plan/SKILL.md");
    let before = std::fs::read(&output).expect("published command");
    assert!(
        std::str::from_utf8(&before)
            .expect("UTF-8")
            .contains(SENTINEL)
    );
    let manifest = project.path().join(".kittify/command-skills-manifest.json");
    let manifest_before = std::fs::read(&manifest).expect("manifest");
    initialize(&request).expect("full init retry");
    assert_eq!(std::fs::read(&output).expect("retry output"), before);
    assert_eq!(
        std::fs::read(&manifest).expect("retry manifest"),
        manifest_before
    );
}

#[test]
fn activation_error_retains_the_selectors_original_type_and_body() {
    let project = tempfile::tempdir().expect("project");
    let templates = tempfile::tempdir().expect("templates");
    support::write(
        &project.path().join(".kittify/config.yaml"),
        "activated_paradigms: not-a-list\n",
    );
    let expected = sk_doctrine::is_spdd_reasons_active(project.path()).expect_err("selector error");
    let error = template(templates.path())
        .for_project(project.path())
        .resolve("plan")
        .expect_err("render must retain selector failure");
    assert_eq!(error.code(), "spdd_activation_failed");
    assert_eq!(error.to_string(), expected.body());
    let InstallError::SpddActivation(actual) = error else {
        panic!("not the canonical typed activation cause");
    };
    assert_eq!(*actual, expected);
}

#[test]
fn legacy_source_helper_distinguishes_omitted_project_from_actual_project() {
    let inactive = tempfile::tempdir().expect("missing config project");
    let active = tempfile::tempdir().expect("active project");
    support::write(&active.path().join(".kittify/config.yaml"), "{}\n");
    let omitted = agent_commands::render_prompt("claude", "plan").expect("omitted project");
    let missing = agent_commands::render_prompt_for_project("claude", "plan", inactive.path())
        .expect("missing config");
    let selected = agent_commands::render_prompt_for_project("claude", "plan", active.path())
        .expect("active config");
    assert_eq!(
        omitted, missing,
        "Source omitted project explicitly means inactive"
    );
    assert_ne!(
        omitted, selected,
        "the real pinned plan has conditional content"
    );
    assert!(!selected.contains("spdd:reasons-block"));
}
