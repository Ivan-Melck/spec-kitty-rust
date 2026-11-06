//! CF6 — ordinary SPDD/REASONS content rendering through the canonical selector.
//!
//! Source truth (`charter/offering/spdd_reasons/template_renderer.py` +
//! `apply_spdd_blocks_for_project`): the template-materialisation seams pass the
//! project root, so the block's `active` flag is
//! `charter.offering.spdd_reasons.activation.is_spdd_reasons_active(repo_root)`.
//! The selector's verified rule is:
//!
//! * missing `.kittify/config.yaml` → `false`;
//! * present but empty `config.yaml` (no `activated_*` key) → `true`;
//! * a present empty allow list → that kind selects nothing;
//! * four selectors OR'd; a deny only from the ledger;
//! * malformed input raises instead of answering either boolean.
//!
//! These tests drive the **actual** selector through
//! [`sk_install::render::TemplateSource::for_project`] and the
//! `rewrite_agent_shims` project seam; they do not inject a boolean and are
//! Native-only (no Source child process is executed here).

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use sk_install::render::{
    REASONS_BLOCK_END, REASONS_BLOCK_START, TemplateSource, process_spdd_blocks,
    shim::rewrite_agent_shims,
};

/// The canonical author layout Source's inactive branch restores byte-for-byte.
const TEMPLATE: &str = "preceding line\n\n\
<!-- spdd:reasons-block:start -->\n\
\n\
### REASONS Guidance — fill the canvas\n\
R: requirements\n\
\n\
<!-- spdd:reasons-block:end -->\n\
\n\
following line\n";

/// The exact bytes the inactive branch produces: Source `_remove_blocks` deletes
/// the blank line that precedes the start marker, the block and both delimiters,
/// and keeps the blank line that follows the end marker. The result is therefore
/// anchored to the reference implementation (not to an assumed pre-feature file).
const PRE_FEATURE: &str = "preceding line\n\nfollowing line\n";

fn templates_root(base: &Path) -> PathBuf {
    let root = base.join("pack/missions");
    let step = root.join("mission-steps/software-dev/plan");
    std::fs::create_dir_all(&step).expect("template dirs");
    std::fs::write(step.join("prompt.md"), TEMPLATE).expect("template");
    root
}

fn project(base: &Path, config: Option<&str>) -> TempDir {
    let sandbox = tempfile::tempdir_in(base).expect("project tempdir");
    std::fs::create_dir_all(sandbox.path().join(".kittify")).expect("kittify");
    if let Some(config) = config {
        std::fs::write(sandbox.path().join(".kittify/config.yaml"), config).expect("config");
    }
    sandbox
}

fn resolved(project: &Path, templates: &Path) -> String {
    TemplateSource::new(templates)
        .for_project(project)
        .resolve("plan")
        .expect("resolve")
        .text
}

#[test]
fn a_missing_config_selects_inactive_and_restores_pre_feature_bytes() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());
    let sandbox = project(base.path(), None);
    let rendered = resolved(sandbox.path(), &templates);
    assert!(rendered.contains("preceding line"));
    assert!(!rendered.contains("REASONS"), "{rendered}");
    assert!(!rendered.contains(REASONS_BLOCK_START));
    assert_eq!(rendered, PRE_FEATURE);
}

#[test]
fn a_present_but_empty_config_selects_active_and_keeps_block_content() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());
    // Present and parseable, but no activation key at all: the allow-all state.
    let sandbox = project(base.path(), Some("# a real project config\nproject: {}\n"));
    let rendered = resolved(sandbox.path(), &templates);
    assert!(rendered.contains("### REASONS Guidance"), "{rendered}");
    assert!(rendered.contains("R: requirements"));
    assert!(!rendered.contains("spdd:reasons-block:start"));
    assert!(!rendered.contains("spdd:reasons-block:end"));
}

#[test]
fn explicit_empty_allow_lists_select_inactive() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());
    let sandbox = project(
        base.path(),
        Some("activated_paradigms: []\nactivated_tactics: []\nactivated_directives: []\n"),
    );
    let rendered = resolved(sandbox.path(), &templates);
    assert_eq!(rendered, PRE_FEATURE);
}

#[test]
fn one_named_selector_selects_active() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());
    let sandbox = project(
        base.path(),
        Some("activated_paradigms:\n  - structured-prompt-driven-development\n"),
    );
    let rendered = resolved(sandbox.path(), &templates);
    assert!(rendered.contains("### REASONS Guidance"), "{rendered}");
}

#[test]
fn a_pure_template_source_keeps_the_rendering_capability() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());
    // No project binding: the pure renderer keeps content and strips markers.
    let rendered = TemplateSource::new(&templates)
        .resolve("plan")
        .expect("resolve")
        .text;
    assert!(rendered.contains("### REASONS Guidance"));
    assert!(!rendered.contains("spdd:reasons-block"));
    // An explicit decision wins over a project binding.
    let sandbox = project(base.path(), None);
    let explicit = TemplateSource::new(&templates)
        .for_project(sandbox.path())
        .with_spdd_active(true)
        .resolve("plan")
        .expect("resolve")
        .text;
    assert!(explicit.contains("### REASONS Guidance"));
}

#[test]
fn a_marker_free_template_never_reads_the_activation_source() {
    let base = tempfile::tempdir().expect("base");
    let templates_root = base.path().join("pack/missions");
    let step = templates_root.join("mission-steps/software-dev/plan");
    std::fs::create_dir_all(&step).expect("dirs");
    std::fs::write(step.join("prompt.md"), "plain template\n").expect("template");
    // A malformed activation source would raise for a marker-bearing template;
    // the reference fast-path skips the read entirely, so this resolves.
    let sandbox = project(base.path(), Some("activated_paradigms: not-a-list\n"));
    let rendered = resolved(sandbox.path(), &templates_root);
    assert_eq!(rendered, "plain template\n");
}

#[test]
fn a_malformed_activation_source_raises_instead_of_answering() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());
    let sandbox = project(base.path(), Some("activated_paradigms: not-a-list\n"));
    let error = TemplateSource::new(&templates)
        .for_project(sandbox.path())
        .resolve("plan")
        .expect_err("malformed activation must not be a silent boolean");
    let message = error.to_string();
    assert!(
        message.contains("must be a list"),
        "unexpected error: {message}"
    );
}

#[test]
fn marker_validation_stays_a_typed_failure() {
    let error = process_spdd_blocks("x\n<!-- spdd:reasons-block:start -->\ny\n", true)
        .expect_err("unbalanced");
    assert!(error.to_string().contains("no matching end"), "{error}");
    let nested = format!(
        "{REASONS_BLOCK_START}\n{REASONS_BLOCK_START}\n{REASONS_BLOCK_END}\n{REASONS_BLOCK_END}\n"
    );
    let error = process_spdd_blocks(&nested, false).expect_err("nested");
    assert!(error.to_string().contains("nested"), "{error}");
}

/// `.kittify/config.yaml` naming one configured agent, plus the activation keys
/// (or none) that the selector reads.
fn shim_project(base: &Path, activation: &str) -> TempDir {
    let sandbox = tempfile::tempdir_in(base).expect("project tempdir");
    std::fs::create_dir_all(sandbox.path().join(".kittify")).expect("kittify");
    std::fs::write(
        sandbox.path().join(".kittify/config.yaml"),
        format!("agents:\n  available:\n  - claude\n{activation}"),
    )
    .expect("config");
    sandbox
}

fn plan_command(project: &Path) -> String {
    std::fs::read_to_string(project.join(".claude/commands/spec-kitty.plan.md")).expect("plan")
}

#[test]
fn the_shim_rewrite_renders_prompts_for_the_projects_activation_answer() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());

    let active = shim_project(base.path(), "");
    let result = rewrite_agent_shims(
        active.path(),
        "4.0.0rc8",
        Some(&TemplateSource::new(&templates)),
        false,
    )
    .expect("rewrite");
    assert!(
        result
            .warnings
            .iter()
            .all(|warning| !warning.contains("plan"))
    );
    let rendered = plan_command(active.path());
    assert!(rendered.contains("### REASONS Guidance"), "{rendered}");
    assert!(!rendered.contains("spdd:reasons-block"));

    let inactive = shim_project(
        base.path(),
        "activated_paradigms: []\nactivated_tactics: []\nactivated_directives: []\n",
    );
    rewrite_agent_shims(
        inactive.path(),
        "4.0.0rc8",
        Some(&TemplateSource::new(&templates)),
        false,
    )
    .expect("rewrite");
    let rendered = plan_command(inactive.path());
    assert!(rendered.contains("preceding line"));
    assert!(!rendered.contains("REASONS"), "{rendered}");
    assert!(!rendered.contains("spdd:reasons-block"));
}

#[test]
fn a_preview_rewrite_writes_nothing_in_either_activation_state() {
    let base = tempfile::tempdir().expect("base");
    let templates = templates_root(base.path());
    let sandbox = shim_project(base.path(), "");
    rewrite_agent_shims(
        sandbox.path(),
        "4.0.0rc8",
        Some(&TemplateSource::new(&templates)),
        true,
    )
    .expect("dry run");
    assert!(!sandbox.path().join(".claude/commands").exists());
}
