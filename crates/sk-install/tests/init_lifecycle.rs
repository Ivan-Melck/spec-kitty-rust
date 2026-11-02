//! Initialization acceptance tests: fresh scaffold, idempotent re-run,
//! manifest-owned refresh, user-content preservation, modified managed assets,
//! missing/duplicate manifests, and interrupted-delivery recovery.
//!
//! Every test runs in its own temporary project. Nothing here touches the real
//! checkout, `~/.kittify`, or any agent directory outside the fixture root.

mod support;

use std::path::Path;

use sk_install::init::{
    CURRENT_SCHEMA_CAPABILITIES, InitOutcome, InitRequest, PendingDelivery, initialize,
    is_initialized, pending_delivery, read_configured_agents,
};
use sk_install::manifest::MANIFEST_REL;

const VERSION: &str = "4.0.0rc7";

fn project() -> (tempfile::TempDir, tempfile::TempDir) {
    (
        tempfile::tempdir().expect("project"),
        tempfile::tempdir().expect("templates"),
    )
}

fn request(project_root: &Path, templates_root: &Path, agents: &[&str]) -> InitRequest {
    let templates = support::missions_root(templates_root);
    InitRequest::new(
        project_root,
        agents.iter().map(|agent| agent.to_string()).collect(),
        VERSION,
    )
    .with_templates(templates)
}

#[test]
fn fresh_init_creates_the_scaffold_manifest_and_every_managed_surface() {
    let (project, templates) = project();
    let report = initialize(&request(
        project.path(),
        templates.path(),
        &["claude", "codex"],
    ))
    .expect("fresh init");

    assert_eq!(report.outcome, InitOutcome::Initialized);
    assert!(report.config_written);
    assert!(report.metadata_written);
    assert!(report.summary.created >= 20, "claude files + codex skills");

    // Config, metadata and memory scaffold.
    assert!(is_initialized(project.path()));
    let config = support::read(&project.path().join(".kittify/config.yaml"));
    assert!(config.contains("agents:\n  available:\n  - claude\n  - codex\n"));
    assert!(config.contains("  auto_commit: true"));
    assert!(config.contains("vcs:\n  type: git\n"));
    assert_eq!(
        read_configured_agents(project.path()).expect("configured agents"),
        vec!["claude".to_string(), "codex".to_string()]
    );
    let metadata = support::read(&project.path().join(".kittify/metadata.yaml"));
    assert!(metadata.contains("schema_version: 3"));
    for capability in CURRENT_SCHEMA_CAPABILITIES {
        assert!(metadata.contains(&format!("{capability}: true")));
    }
    assert!(project.path().join(".kittify/memory").is_dir());

    // Claude gets slash command files, codex gets shared skill files.
    assert!(
        project
            .path()
            .join(".claude/commands/spec-kitty.plan.md")
            .is_file()
    );
    assert!(
        project
            .path()
            .join(".agents/skills/spec-kitty-plan/SKILL.md")
            .is_file()
    );

    // The ownership manifest records both surfaces with their owners.
    let manifest = sk_install::AssetManifest::load(project.path())
        .expect("manifest loads")
        .expect("manifest exists");
    let claude_entry = manifest
        .find(".claude/commands/spec-kitty.plan.md")
        .expect("claude entry");
    assert_eq!(claude_entry.agents, vec!["claude"]);
    let codex_entry = manifest
        .find(".agents/skills/spec-kitty-plan/SKILL.md")
        .expect("codex entry");
    assert_eq!(codex_entry.agents, vec!["codex"]);
    assert_eq!(codex_entry.spec_kitty_version, VERSION);

    // No recovery record survives a completed delivery.
    assert!(pending_delivery(project.path()).expect("pending").is_none());
    assert_eq!(report.application.preserved.len(), 0);
}

#[test]
fn re_running_init_is_idempotent_and_writes_nothing() {
    let (project, templates) = project();
    let first = initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    assert_eq!(first.outcome, InitOutcome::Initialized);
    let manifest_before = support::file_bytes(&project.path().join(MANIFEST_REL));

    let second =
        initialize(&request(project.path(), templates.path(), &["codex"])).expect("re-init");
    assert_eq!(second.outcome, InitOutcome::AlreadyInitialized);
    assert!(
        second.application.is_noop(),
        "second run must not write: {second:?}"
    );
    assert!(!second.config_written);
    assert_eq!(
        second.summary.created + second.summary.updated + second.summary.regenerated,
        0
    );
    assert_eq!(
        support::file_bytes(&project.path().join(MANIFEST_REL)),
        manifest_before,
        "manifest bytes are stable across an idempotent run"
    );
}

#[test]
fn user_authored_files_and_custom_skills_are_never_touched() {
    let (project, templates) = project();
    // A team's own committed skill beside the generated ones, plus an authored
    // command file the installer must not claim.
    support::write(
        &project.path().join(".agents/skills/my-team-skill/SKILL.md"),
        "# my team skill\n\nHand-authored.\n",
    );
    support::write(
        &project.path().join(".claude/commands/my-custom.md"),
        "# custom\n",
    );
    let custom_before =
        support::file_bytes(&project.path().join(".agents/skills/my-team-skill/SKILL.md"));

    initialize(&request(
        project.path(),
        templates.path(),
        &["claude", "codex"],
    ))
    .expect("init");

    let custom_after =
        support::file_bytes(&project.path().join(".agents/skills/my-team-skill/SKILL.md"));
    assert_eq!(custom_after, custom_before, "custom skill bytes preserved");
    assert_eq!(
        support::read(&project.path().join(".claude/commands/my-custom.md")),
        "# custom\n"
    );
    let manifest = sk_install::AssetManifest::load(project.path())
        .expect("manifest")
        .expect("exists");
    assert!(
        manifest
            .find(".agents/skills/my-team-skill/SKILL.md")
            .is_none(),
        "a team skill is never claimed"
    );
    assert!(manifest.find(".claude/commands/my-custom.md").is_none());
}

#[test]
fn an_unowned_file_at_a_canonical_path_is_a_collision_and_is_preserved() {
    let (project, templates) = project();
    let target = project.path().join(".claude/commands/spec-kitty.plan.md");
    support::write(&target, "# my own plan command\n");

    let report = initialize(&request(project.path(), templates.path(), &["claude"])).expect("init");
    assert_eq!(report.outcome, InitOutcome::Initialized);
    assert_eq!(report.summary.collisions, 1);
    assert_eq!(
        support::read(&target),
        "# my own plan command\n",
        "unowned content is never overwritten"
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("Your own file blocks command skill")),
        "collision is reported: {:?}",
        report.warnings
    );
    let manifest = sk_install::AssetManifest::load(project.path())
        .expect("manifest")
        .expect("exists");
    assert!(
        manifest
            .find(".claude/commands/spec-kitty.plan.md")
            .is_none(),
        "a collision is not claimed"
    );
    // The rest of the batch still landed.
    assert!(
        project
            .path()
            .join(".claude/commands/spec-kitty.specify.md")
            .is_file()
    );
}

#[test]
fn a_modified_derived_shared_projection_is_regenerated() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    let target = project
        .path()
        .join(".agents/skills/spec-kitty-plan/SKILL.md");
    let canonical = support::file_bytes(&target);
    let mut edited = canonical.clone();
    edited.extend_from_slice(b"\nlocal edit\n");
    std::fs::write(&target, &edited).expect("edit projection");

    let report =
        initialize(&request(project.path(), templates.path(), &["codex"])).expect("refresh");
    assert_eq!(report.outcome, InitOutcome::AlreadyInitialized);
    assert_eq!(report.summary.regenerated, 1);
    assert_eq!(
        support::file_bytes(&target),
        canonical,
        "the derived projection is restored to the canonical render"
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("Regenerated drifted derived"))
    );
}

#[test]
fn a_modified_native_root_file_is_preserved_untouched() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["claude"])).expect("init");
    let target = project.path().join(".claude/commands/spec-kitty.plan.md");
    let mut edited = support::file_bytes(&target);
    edited.extend_from_slice(b"\nI edited this command myself.\n");
    std::fs::write(&target, &edited).expect("edit native command");

    let report =
        initialize(&request(project.path(), templates.path(), &["claude"])).expect("refresh");
    assert_eq!(report.outcome, InitOutcome::AlreadyInitialized);
    assert_eq!(report.summary.preserved, 1);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("has your edits"))
    );
    let after = support::file_bytes(&target);
    assert_eq!(after, edited, "a committed user edit is never overwritten");
    // The path stays owned: the user's edit does not silently drop the manifest entry.
    let manifest = sk_install::AssetManifest::load(project.path())
        .expect("manifest")
        .expect("exists");
    assert!(
        manifest
            .find(".claude/commands/spec-kitty.plan.md")
            .is_some()
    );
}

#[test]
fn a_missing_manifest_is_rebuilt_without_touching_files() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    let skill = project
        .path()
        .join(".agents/skills/spec-kitty-plan/SKILL.md");
    let bytes_before = support::file_bytes(&skill);
    std::fs::remove_file(project.path().join(MANIFEST_REL)).expect("remove manifest");

    let report =
        initialize(&request(project.path(), templates.path(), &["codex"])).expect("refresh");
    assert_eq!(report.outcome, InitOutcome::AlreadyInitialized);
    assert!(report.application.manifest_written, "manifest is rebuilt");
    assert_eq!(
        report.summary.adopted, 14,
        "identical bytes are adopted, not rewritten"
    );
    assert_eq!(report.summary.created + report.summary.updated, 0);
    assert_eq!(support::file_bytes(&skill), bytes_before);
    assert!(
        sk_install::AssetManifest::load(project.path())
            .expect("manifest")
            .is_some()
    );
}

#[test]
fn a_deleted_managed_file_is_restored() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    let skill = project
        .path()
        .join(".agents/skills/spec-kitty-plan/SKILL.md");
    let canonical = support::file_bytes(&skill);
    std::fs::remove_file(&skill).expect("delete managed file");

    let report =
        initialize(&request(project.path(), templates.path(), &["codex"])).expect("refresh");
    assert_eq!(report.summary.created, 1, "a gap is restored");
    assert_eq!(support::file_bytes(&skill), canonical);
}

#[test]
fn a_duplicate_manifest_blocks_refresh_with_a_typed_error() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    let duplicate = br#"{"schema_version":1,"entries":[
        {"path":".agents/skills/spec-kitty-plan/SKILL.md","content_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","agents":["codex"],"installed_at":"x","spec_kitty_version":"v"},
        {"path":".agents/skills/spec-kitty-plan/SKILL.md","content_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","agents":["vibe"],"installed_at":"y","spec_kitty_version":"v"}
    ]}"#;
    support::write(&project.path().join(MANIFEST_REL), duplicate);

    let error =
        initialize(&request(project.path(), templates.path(), &["codex"])).expect_err("refused");
    assert_eq!(error.code(), "manifest_duplicate_path");
    // Nothing was written while the manifest was untrusted.
    assert_eq!(
        support::file_bytes(&project.path().join(MANIFEST_REL)),
        duplicate,
        "a corrupt manifest is never rewritten"
    );
}

#[test]
fn a_corrupt_manifest_blocks_refresh() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    support::write(&project.path().join(MANIFEST_REL), "{not json");
    let error =
        initialize(&request(project.path(), templates.path(), &["codex"])).expect_err("refused");
    assert_eq!(error.code(), "manifest_corrupt_json");
}

#[test]
fn an_interrupted_delivery_is_resumed_and_cleared() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    // Simulate a crash mid-delivery: a pending record plus missing projections.
    let skill = project
        .path()
        .join(".agents/skills/spec-kitty-plan/SKILL.md");
    std::fs::remove_file(&skill).expect("delete delivered file");
    PendingDelivery::write(project.path(), &["codex".to_string()]).expect("pending record");
    assert!(
        pending_delivery(project.path())
            .expect("read pending")
            .is_some()
    );

    let report = initialize(&request(project.path(), templates.path(), &[])).expect("resume");
    assert_eq!(report.outcome, InitOutcome::ResumedDelivery);
    assert_eq!(report.summary.created, 1);
    assert!(skill.is_file(), "the interrupted delivery finished");
    assert!(
        pending_delivery(project.path())
            .expect("read pending")
            .is_none(),
        "the recovery record is cleared after completion"
    );
}

#[test]
fn resuming_with_a_different_agent_set_is_refused() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    PendingDelivery::write(project.path(), &["codex".to_string()]).expect("pending record");

    let error =
        initialize(&request(project.path(), templates.path(), &["vibe"])).expect_err("refused");
    assert_eq!(error.code(), "pending_delivery_invalid");
    assert!(pending_delivery(project.path()).expect("read").is_some());
}

#[test]
fn an_invalid_pending_record_is_refused() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    support::write(
        &project
            .path()
            .join(".kittify/init-command-skills.pending.json"),
        r#"{"schema_version":1,"agents":["ghost"]}"#,
    );
    let error = initialize(&request(project.path(), templates.path(), &[])).expect_err("refused");
    assert_eq!(error.code(), "pending_delivery_invalid");

    support::write(
        &project
            .path()
            .join(".kittify/init-command-skills.pending.json"),
        r#"{"schema_version":1,"agents":["codex","codex"]}"#,
    );
    let error = initialize(&request(project.path(), templates.path(), &[])).expect_err("refused");
    assert_eq!(error.code(), "pending_delivery_invalid");
}

#[test]
fn requesting_an_unconfigured_agent_on_an_initialized_project_is_refused() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    let error =
        initialize(&request(project.path(), templates.path(), &["claude"])).expect_err("refused");
    assert_eq!(error.code(), "already_initialized");
    assert!(error.to_string().contains("agent config add"));
}

#[test]
fn a_preexisting_metadata_file_is_merged_additively_never_replaced() {
    let (project, templates) = project();
    support::write(
        &project.path().join(".kittify/metadata.yaml"),
        "# operator authored\nspec_kitty:\n  schema_version: 3\n  custom: keep-me\n",
    );
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    let text = support::read(&project.path().join(".kittify/metadata.yaml"));
    // Operator bytes (header, comment, custom key, authored schema) survive; the
    // model-owned version fields and the missing capability map are added.
    assert!(text.starts_with("# operator authored\n"));
    assert!(text.contains("  schema_version: 3\n"));
    assert!(text.contains("  custom: keep-me\n"));
    assert!(text.contains("  version: 4.0.0rc7\n"));
    assert!(text.contains("  schema_capabilities:\n"));
}

#[test]
fn unknown_agents_and_empty_selections_fail_closed() {
    let (project, templates) = project();
    let error =
        initialize(&request(project.path(), templates.path(), &["ghost"])).expect_err("refused");
    assert_eq!(error.code(), "unknown_agent");
    let error = initialize(&request(project.path(), templates.path(), &[])).expect_err("refused");
    assert_eq!(error.code(), "no_agents_selected");
    assert!(
        !is_initialized(project.path()),
        "a refused init writes nothing"
    );
}

#[test]
fn a_missing_template_source_blocks_slash_and_skill_surfaces() {
    let project = tempfile::tempdir().expect("project");
    let templates = tempfile::tempdir().expect("templates");
    let missing = sk_install::TemplateSource::new(templates.path().join("absent"));
    // A slash-command surface needs the template tree itself, so the discovery
    // step refuses with `template_source_unavailable`; a command skill fails when
    // its own prompt is missing (`template_not_found`). Both are typed refusals.
    for (agent, expected) in [
        ("claude", "template_source_unavailable"),
        ("codex", "template_not_found"),
    ] {
        let request = InitRequest::new(project.path(), vec![agent.to_string()], VERSION)
            .with_templates(missing.clone());
        let error = initialize(&request).expect_err("no templates");
        assert_eq!(error.code(), expected, "agent {agent}");
    }
    assert!(!is_initialized(project.path()));
}

#[test]
fn agent_removal_releases_ownership_and_keeps_shared_files() {
    let (project, templates) = project();
    initialize(&request(
        project.path(),
        templates.path(),
        &["codex", "vibe"],
    ))
    .expect("init");
    let skill = project
        .path()
        .join(".agents/skills/spec-kitty-plan/SKILL.md");
    let bytes = support::file_bytes(&skill);

    let plan = sk_install::InstallPlan::plan_agent_removal(
        project.path(),
        VERSION,
        "codex",
        &sk_install::PlanOptions::new(),
    )
    .expect("removal plan");
    assert_eq!(plan.summary().deleted, 0, "vibe still owns the shared file");
    plan.commit(std::time::Duration::from_secs(1))
        .expect("commit");

    let manifest = sk_install::AssetManifest::load(project.path())
        .expect("manifest")
        .expect("exists");
    assert_eq!(
        manifest
            .find(".agents/skills/spec-kitty-plan/SKILL.md")
            .expect("entry")
            .agents,
        vec!["vibe"]
    );
    assert_eq!(support::file_bytes(&skill), bytes, "bytes untouched");

    // Removing the last owner deletes the file.
    let plan = sk_install::InstallPlan::plan_agent_removal(
        project.path(),
        VERSION,
        "vibe",
        &sk_install::PlanOptions::new(),
    )
    .expect("removal plan");
    plan.commit(std::time::Duration::from_secs(1))
        .expect("commit");
    assert!(!skill.exists(), "the last owner's file is removed");
    let manifest = sk_install::AssetManifest::load(project.path())
        .expect("manifest")
        .expect("exists");
    assert!(manifest.entries().is_empty());
}

#[test]
fn outcome_spelling_is_stable() {
    assert_eq!(InitOutcome::Initialized.as_str(), "initialized");
    assert_eq!(
        InitOutcome::AlreadyInitialized.as_str(),
        "already_initialized"
    );
    assert_eq!(InitOutcome::ResumedDelivery.as_str(), "resumed_delivery");
}

#[test]
fn a_legacy_config_without_an_agent_list_is_reported_not_guessed() {
    let (project, templates) = project();
    initialize(&request(project.path(), templates.path(), &["codex"])).expect("init");
    let config_rel = ".kittify/config.yaml";
    let legacy = "agents:\n  auto_commit: true\n";
    support::write(&project.path().join(config_rel), legacy);

    let error = initialize(&request(project.path(), templates.path(), &[])).expect_err("refused");
    assert_eq!(error.code(), "config_invalid");
    assert!(error.to_string().contains("agents.available"));
    assert_eq!(
        support::read(&project.path().join(config_rel)),
        legacy,
        "a config this crate cannot read is never rewritten"
    );
}
