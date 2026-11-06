//! Full-delivery init: recovery, refresh completion and clone-local seeding.
//!
//! These tests use the real checkout assets (template tree, charter skills,
//! agent profiles and the built-in preset). When the assets are absent (a
//! crate-only build) each test returns early rather than inventing a fixture.
//! Everything runs in temporary directories with a temporary `HOME`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_install::{
    InitOutcome, InitRequest, PendingDelivery, ProjectAssets, initialize_with_details,
};

const VERSION: &str = "4.0.0rc8";

fn checkout_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn checkout_assets(root: &Path) -> Option<ProjectAssets> {
    if !root.join("packs/built-in/missions").is_dir()
        || !root.join("src/charter/offering/templates").is_dir()
        || !root.join("src/charter/offering/skills").is_dir()
        || !root.join("packs/built-in/agent_profiles").is_dir()
    {
        return None;
    }
    Some(ProjectAssets::discover(root).expect("assets"))
}

fn request(project: &Path, agents: &[&str], assets: ProjectAssets) -> InitRequest {
    InitRequest::new(
        project,
        agents.iter().map(|agent| (*agent).to_string()).collect(),
        VERSION,
    )
    .with_project_assets(assets)
    .with_core_version(VERSION)
    .with_lock_wait(Duration::from_secs(10))
}

#[test]
fn full_init_delivers_profiles_sessions_and_leaves_no_lock_residue() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    let (_report, details) = initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        assets,
    ))
    .expect("full init");

    // Template tree, charter skills and profile projection.
    assert!(project.path().join(".kittify/missions/README.md").is_file());
    assert!(project.path().join(".kittify/memory/README.md").is_file());
    assert_eq!(details.ledger_seeded.len(), 124);
    assert!(details.activation_provisioned);
    assert!(details.event_written);
    assert!(details.identity.as_ref().is_some_and(|i| i.is_complete()));
    assert_eq!(details.profiles_written.len(), 50);
    assert!(
        details
            .orientation_written
            .contains(&"AGENTS.md".to_string())
    );
    assert!(
        details
            .orientation_written
            .contains(&".claude/settings.json".to_string())
    );
    assert!(
        project
            .path()
            .join(".claude/agents/analyst-annie.md")
            .is_file()
    );
    assert!(
        project
            .path()
            .join(".codex/agents/analyst-annie.toml")
            .is_file()
    );
    assert!(project.path().join("AGENTS.md").is_file());
    assert!(
        project
            .path()
            .join(".kittify/agent_profiles_manifest.json")
            .is_file()
    );
    // No legacy slash-command surface and no checkout-scoped lock residue.
    assert!(!project.path().join(".claude/commands").exists());
    assert!(!project.path().join(".kittify/.install.lock").exists());
    assert!(!project.path().join(".claudeignore").exists());
    // Provenance on the skills manifest is the running CLI version.
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(project.path().join(".kittify/skills-manifest.json")).expect("read"),
    )
    .expect("json");
    assert_eq!(manifest["spec_kitty_version"], VERSION);
    assert_eq!(manifest["entries"].as_array().expect("entries").len(), 222);
}

#[test]
fn a_required_failure_keeps_the_pending_record_and_resume_completes_it() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    let mut broken = assets;
    broken.activation_preset = Some(project.path().join("missing-default-preset.yaml"));
    let error = initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        broken,
    ))
    .expect_err("a missing preset must fail closed");
    assert_eq!(error.code(), "DEFAULT_PRESET_MISSING");
    // The interruption is recorded; nothing reports success.
    assert!(
        PendingDelivery::load(project.path())
            .expect("load")
            .is_some()
    );
    assert!(
        !project
            .path()
            .join(".kittify/charter/activation.yaml")
            .exists()
    );
    assert!(
        !project
            .path()
            .join(".kittify/canonical-events.jsonl")
            .exists()
    );
    assert!(
        !project
            .path()
            .join(".kittify/migration-ledger.jsonl")
            .exists()
    );

    // The operator repairs the preset and re-runs the same command.
    let (_report, details) = initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        checkout_assets(&root).expect("assets"),
    ))
    .expect("resume");
    assert!(
        PendingDelivery::load(project.path())
            .expect("load")
            .is_none()
    );
    assert!(details.activation_provisioned);
    assert!(details.event_written);
    assert_eq!(details.ledger_seeded.len(), 124);
    assert!(
        project
            .path()
            .join(".kittify/charter/activation.yaml")
            .is_file()
    );
}

#[test]
fn a_full_asset_set_without_the_default_preset_fails_closed() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    let mut incomplete = assets;
    incomplete.activation_preset = None;
    let error = initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        incomplete,
    ))
    .expect_err("a full asset set without the activation preset must fail closed");
    assert_eq!(error.code(), "DEFAULT_PRESET_MISSING");
    // The interruption is recorded and no later required effect claims success.
    assert!(
        PendingDelivery::load(project.path())
            .expect("load")
            .is_some()
    );
    // The preset is resolved *before* the first managed write, so a fresh
    // failure leaves no template tree, config, metadata or skill manifest.
    assert!(!project.path().join(".kittify/config.yaml").exists());
    assert!(!project.path().join(".kittify/metadata.yaml").exists());
    assert!(!project.path().join(".kittify/missions").exists());
    assert!(!project.path().join(".kittify/memory/README.md").exists());
    assert!(
        !project
            .path()
            .join(".kittify/skills-manifest.json")
            .exists()
    );
    assert!(
        !project
            .path()
            .join(".kittify/canonical-events.jsonl")
            .exists()
    );
    assert!(
        !project
            .path()
            .join(".kittify/migration-ledger.jsonl")
            .exists()
    );
    assert!(
        !project
            .path()
            .join(".kittify/charter/activation.yaml")
            .exists()
    );
}

#[test]
fn init_waits_on_the_canonical_state_dir_project_lock() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    let lock_path = sk_install::project_lock_path(project.path()).expect("lock path");
    let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _lock = sk_install::ProjectLock::acquire(&lock_path, Duration::from_secs(2))
            .expect("holder lock");
        ready_sender.send(()).expect("ready");
        release_receiver.recv().expect("release");
    });
    ready_receiver.recv().expect("ready");
    let mut request = request(project.path(), &["codex"], assets);
    request.lock_wait = Duration::from_millis(100);
    // Release the holder even when the contention expectation fails, so a red
    // run reports the assertion rather than a deadlocked helper thread.
    let result = initialize_with_details(&request);
    release_sender.send(()).expect("release");
    holder.join().expect("join");
    let error = result.expect_err("canonical lock is contended");
    assert_eq!(error.code(), "lock_unavailable");
    assert!(!project.path().join(".kittify/config.yaml").exists());
}

#[test]
fn a_refresh_of_an_activated_project_needs_no_preset() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        assets,
    ))
    .expect("full init");
    let activation = project.path().join(".kittify/charter/activation.yaml");
    let before = std::fs::read(&activation).expect("activation");

    // The already-activated project is refreshed from an asset set whose preset
    // source is gone: no preset is needed because provisioning is a no-op.
    let mut without_preset = checkout_assets(&root).expect("assets");
    without_preset.activation_preset = None;
    let (report, details) = initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        without_preset,
    ))
    .expect("refresh without a preset");
    assert_eq!(report.outcome, InitOutcome::AlreadyInitialized);
    assert!(!details.activation_provisioned);
    assert_eq!(std::fs::read(&activation).expect("activation"), before);
    assert!(
        PendingDelivery::load(project.path())
            .expect("load")
            .is_none()
    );
}

#[test]
fn an_agent_set_without_a_profile_renderer_writes_no_profile_manifest() {
    let root = checkout_root();
    if checkout_assets(&root).is_none() {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    }
    for agent in ["gemini", "windsurf", "pi"] {
        let project = tempfile::tempdir().expect("project");
        initialize_with_details(&request(
            project.path(),
            &[agent],
            checkout_assets(&root).expect("assets"),
        ))
        .unwrap_or_else(|error| panic!("{agent}: {error}"));
        assert!(
            !project
                .path()
                .join(".kittify/agent_profiles_manifest.json")
                .exists(),
            "{agent} has no project-local profile renderer, so no manifest is written"
        );
    }
    // Control: an agent with a renderer does record ownership.
    let project = tempfile::tempdir().expect("project");
    initialize_with_details(&request(
        project.path(),
        &["claude"],
        checkout_assets(&root).expect("assets"),
    ))
    .expect("claude init");
    assert!(
        project
            .path()
            .join(".kittify/agent_profiles_manifest.json")
            .is_file()
    );
}

#[test]
fn copilot_only_init_does_not_write_orientation_before_the_harness_root_exists() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    let (_report, details) =
        initialize_with_details(&request(project.path(), &["copilot"], assets))
            .expect("copilot init");
    // Upstream guards copilot orientation on `.github` existing, and session
    // presence runs before profile projection creates it.
    assert!(
        !project
            .path()
            .join(".github/copilot-instructions.md")
            .exists()
    );
    assert!(
        project
            .path()
            .join(".github/agents/analyst-annie.agent.md")
            .is_file()
    );
    assert!(
        !details
            .orientation_written
            .iter()
            .any(|path| path.contains("copilot"))
    );
}

#[cfg(unix)]
#[test]
fn symlinked_optional_surfaces_are_preserved_with_warnings_instead_of_aborting() {
    use std::os::unix::fs::symlink;

    let root = checkout_root();
    if checkout_assets(&root).is_none() {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    }
    let outside = tempfile::tempdir().expect("outside");

    // 1) A root `AGENTS.md` symlink (codex) is preserved; init completes.
    let project = tempfile::tempdir().expect("project");
    let shared_agents = outside.path().join("AGENTS.md");
    std::fs::write(&shared_agents, "shared agents\n").expect("shared agents");
    symlink(&shared_agents, project.path().join("AGENTS.md")).expect("symlink AGENTS.md");
    let (report, _details) = initialize_with_details(&request(
        project.path(),
        &["codex"],
        checkout_assets(&root).expect("assets"),
    ))
    .expect("init with a symlinked AGENTS.md");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("symlink")),
        "warnings: {:?}",
        report.warnings
    );
    assert_eq!(
        std::fs::read_to_string(&shared_agents).expect("shared agents"),
        "shared agents\n"
    );

    // 2) A symlinked `.claude/CLAUDE.md` is preserved; init completes.
    let project = tempfile::tempdir().expect("project");
    std::fs::create_dir_all(project.path().join(".claude")).expect("claude dir");
    let shared_claude = outside.path().join("CLAUDE.md");
    std::fs::write(&shared_claude, "team claude\n").expect("shared claude");
    symlink(&shared_claude, project.path().join(".claude/CLAUDE.md")).expect("symlink CLAUDE.md");
    let (report, _details) = initialize_with_details(&request(
        project.path(),
        &["claude"],
        checkout_assets(&root).expect("assets"),
    ))
    .expect("init with a symlinked CLAUDE.md");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("symlink")),
        "warnings: {:?}",
        report.warnings
    );
    assert_eq!(
        std::fs::read_to_string(&shared_claude).expect("shared claude"),
        "team claude\n"
    );

    // 3) A symlinked `.claude` root is refused for every claude surface while the
    //    delivery still completes and writes nothing outside the project.
    let project = tempfile::tempdir().expect("project");
    let outside_claude = tempfile::tempdir().expect("outside claude");
    symlink(outside_claude.path(), project.path().join(".claude")).expect("symlink .claude");
    let (report, _details) = initialize_with_details(&request(
        project.path(),
        &["claude"],
        checkout_assets(&root).expect("assets"),
    ))
    .expect("init with a symlinked .claude root");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("symlink")),
        "warnings: {:?}",
        report.warnings
    );
    assert_eq!(
        std::fs::read_dir(outside_claude.path())
            .expect("outside")
            .count(),
        0,
        "no byte may be written through a symlinked agent root"
    );
    assert!(
        !project
            .path()
            .join(".kittify/agent_profiles_manifest.json")
            .exists()
    );
    assert!(
        !project
            .path()
            .join(".kittify/skills-manifest.json")
            .exists()
    );
}

#[test]
fn resume_restores_every_missing_required_effect() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        assets,
    ))
    .expect("full init");
    for relative in [
        ".kittify/charter",
        ".kittify/canonical-events.jsonl",
        ".kittify/migration-ledger.jsonl",
        ".kittify/skills-manifest.json",
        ".kittify/agent_profiles_manifest.json",
        ".claude/skills",
        ".claude/agents",
        ".claude/settings.json",
        ".claude/CLAUDE.md",
        "AGENTS.md",
    ] {
        let path = project.path().join(relative);
        if path.is_dir() {
            std::fs::remove_dir_all(&path).expect("remove dir");
        } else if path.exists() {
            std::fs::remove_file(&path).expect("remove file");
        }
    }
    PendingDelivery::write(project.path(), &["codex".to_string(), "vibe".to_string()])
        .expect("pending");

    let (report, details) = initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        checkout_assets(&root).expect("assets"),
    ))
    .expect("resume");
    assert_eq!(report.outcome, InitOutcome::ResumedDelivery);
    assert!(
        PendingDelivery::load(project.path())
            .expect("load")
            .is_none()
    );
    assert!(details.event_written);
    assert!(details.ledger_seeded.len() >= 124);
    assert!(
        project
            .path()
            .join(".kittify/charter/activation.yaml")
            .is_file()
    );
    assert!(
        project
            .path()
            .join(".claude/agents/analyst-annie.md")
            .is_file()
    );
    assert!(project.path().join(".claude/skills").is_dir());
}

#[test]
fn refresh_completes_missing_effects_without_re_scaffolding() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    initialize_with_details(&request(project.path(), &["claude", "codex"], assets))
        .expect("full init");
    // Operator content under memory/ must survive a refresh.
    std::fs::write(project.path().join(".kittify/memory/operator.md"), "keep\n").unwrap();
    for relative in [
        ".kittify/charter",
        ".kittify/canonical-events.jsonl",
        ".kittify/migration-ledger.jsonl",
    ] {
        let path = project.path().join(relative);
        if path.is_dir() {
            std::fs::remove_dir_all(&path).expect("remove dir");
        } else if path.exists() {
            std::fs::remove_file(&path).expect("remove file");
        }
    }
    let mut options = sk_install::PlanOptions::new();
    options.now = Some("2026-11-02T20:00:00+00:00".to_string());
    let (report, details) = initialize_with_details(
        &request(
            project.path(),
            &["claude", "codex"],
            checkout_assets(&root).expect("assets"),
        )
        .with_options(options),
    )
    .expect("refresh");
    assert_eq!(report.outcome, InitOutcome::AlreadyInitialized);
    assert!(details.activation_provisioned);
    assert!(details.event_written);
    assert_eq!(details.ledger_seeded.len(), 124);
    assert_eq!(
        std::fs::read_to_string(project.path().join(".kittify/memory/operator.md")).unwrap(),
        "keep\n"
    );
    assert!(project.path().join(".kittify/missions/README.md").is_file());
}

#[test]
fn init_in_a_git_checkout_seeds_the_proven_clone_local_effects() {
    let root = checkout_root();
    let Some(assets) = checkout_assets(&root) else {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    };
    let project = tempfile::tempdir().expect("project");
    let status = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(project.path())
        .status()
        .expect("git");
    assert!(status.success());
    let (_report, details) = initialize_with_details(&request(
        project.path(),
        &["claude", "codex", "vibe"],
        assets,
    ))
    .expect("full init");
    assert_eq!(details.ledger_seeded.len(), 132);
    assert_eq!(
        std::fs::read_to_string(project.path().join(".kittify/migration-ledger.jsonl"))
            .expect("tracked ledger")
            .lines()
            .count(),
        124
    );
    let clone_local =
        std::fs::read_to_string(project.path().join(".kittify/migrations/clone-local.jsonl"))
            .expect("clone-local ledger");
    let ids: Vec<String> = clone_local
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).expect("row")["id"]
                .as_str()
                .expect("id")
                .to_string()
        })
        .collect();
    assert_eq!(ids.len(), 8);
    for expected in [
        "3.1.1_event_log_merge_driver",
        "3.2.6_decisions_event_log_merge_driver",
        "3.2.6_issue_matrix_driver_repoint",
        "3.2.6_meta_traces_merge_drivers",
        "3.2.7_review_cycle_merge_driver",
        "4.0.0rc5_decision_index_merge_driver",
        "4.0.0rc8_command_skill_hyphen_names",
        "4.0.0rc8_retire_antigravity_workflows",
    ] {
        assert!(ids.iter().any(|id| id == expected), "missing {expected}");
    }
    // The historical gate-driver migration is NOT settled: its frozen
    // filled-side issue-matrix name is superseded by the repoint migration.
    assert!(
        !ids.iter()
            .any(|id| id == "3.2.6_gate_artifact_merge_drivers")
    );
    // The clone-local half of the merge-driver wiring is actually present.
    let config = std::fs::read_to_string(project.path().join(".git/config")).expect("config");
    assert!(config.contains("[merge \"spec-kitty-event-log\"]"));
    assert!(config.contains("driver = spec-kitty merge-driver-event-log %O %A %B"));
}
