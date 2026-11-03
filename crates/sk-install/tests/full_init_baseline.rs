//! Full-init differential baseline against the isolated Python oracle.
//!
//! The fixture `tests/fixtures/oracle_init_claude_codex_vibe.json` was produced
//! by `spec-kitty init --ai claude,codex,vibe --non-interactive` at the pinned
//! Python reference snapshot, with `HOME` and the project in temporary
//! directories. It records the surfaces a full init must reproduce byte-for-byte
//! for those agents: the charter skills manifest, the command-skill manifest,
//! `.gitattributes`, the activation authority, the Vibe pointer, the ledger ids
//! and the non-profile `.gitignore` block.
//!
//! The test discovers the real checkout assets from `CARGO_MANIFEST_DIR/../..`.
//! When those sources are absent (a crate-only build) it returns early rather
//! than inventing a baseline. Nothing here runs Python.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_install::{InitRequest, ProjectAssets, initialize_with_details};

const VERSION: &str = "4.0.0rc8";

fn checkout_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/oracle_init_claude_codex_vibe.json"))
        .expect("fixture json")
}

fn string_set(values: &Value) -> BTreeSet<String> {
    values
        .as_array()
        .expect("array")
        .iter()
        .map(|value| value.as_str().expect("string").to_string())
        .collect()
}

#[test]
fn full_init_reproduces_the_oracle_surfaces_for_claude_codex_vibe() {
    let root = checkout_root();
    if !root.join("packs/built-in/missions").is_dir()
        || !root.join("src/charter/offering/templates").is_dir()
        || !root.join("src/charter/offering/skills").is_dir()
    {
        eprintln!("skipping: checkpoint assets are absent");
        return;
    }
    let project = tempfile::tempdir().expect("project");
    let assets = ProjectAssets::discover(&root).expect("assets");
    let request = InitRequest::new(
        project.path(),
        vec!["claude".into(), "codex".into(), "vibe".into()],
        VERSION,
    )
    .with_project_assets(assets)
    .with_core_version(VERSION);
    let (_report, details) = initialize_with_details(&request).expect("full init");
    assert_eq!(details.ledger_seeded.len(), 124);
    assert!(details.activation_provisioned);
    assert!(details.event_written);
    assert!(details.template_mode.is_some());
    assert!(details.skills_drifted.is_empty());

    let oracle = fixture();

    // Full init no longer writes slash-command files; the command surface is
    // the command-skill manifest.
    assert!(!project.path().join(".claude/commands").exists());

    // Charter skills manifest: exact entry set (path, class, agent, hash).
    let ours: Value = serde_json::from_slice(
        &std::fs::read(project.path().join(".kittify/skills-manifest.json")).expect("read"),
    )
    .expect("json");
    let normalize = |manifest: &Value| -> BTreeSet<String> {
        manifest["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| {
                format!(
                    "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    entry["skill_name"].as_str().unwrap(),
                    entry["source_file"].as_str().unwrap(),
                    entry["installed_path"].as_str().unwrap(),
                    entry["installation_class"].as_str().unwrap(),
                    entry["agent_key"].as_str().unwrap(),
                    entry["content_hash"].as_str().unwrap(),
                )
            })
            .collect()
    };
    let oracle_entries: BTreeSet<String> = oracle["skill_manifest"]
        .as_array()
        .expect("array")
        .iter()
        .map(|entry| {
            let fields = entry.as_array().expect("tuple");
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                fields[0].as_str().unwrap(),
                fields[1].as_str().unwrap(),
                fields[2].as_str().unwrap(),
                fields[3].as_str().unwrap(),
                // Ownership is part of the oracle tuple; omitting it compared a
                // weaker projection than the fixture records.
                fields[4].as_str().unwrap(),
                fields[5].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(normalize(&ours), oracle_entries);

    // Command-skill manifest: the 14 shared command skills and their hashes.
    let commands: Value = serde_json::from_slice(
        &std::fs::read(project.path().join(".kittify/command-skills-manifest.json"))
            .expect("read command manifest"),
    )
    .expect("json");
    let command_entries: BTreeSet<String> = commands["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                entry["path"].as_str().unwrap(),
                entry["content_hash"].as_str().unwrap(),
                entry["agents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|agent| agent.as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join(","),
            )
        })
        .collect();
    let oracle_commands: BTreeSet<String> = oracle["command_manifest"]
        .as_array()
        .expect("array")
        .iter()
        .map(|entry| {
            let fields = entry.as_array().expect("tuple");
            fields
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>()
                .join("\u{1f}")
        })
        .collect();
    assert_eq!(command_entries, oracle_commands);

    // Tracked attribute seed and activation/vibe pointers.
    assert_eq!(
        std::fs::read_to_string(project.path().join(".gitattributes")).unwrap(),
        oracle["gitattributes"].as_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join(".kittify/charter/activation.yaml")).unwrap(),
        oracle["activation"].as_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join(".vibe/config.toml")).unwrap(),
        oracle["vibe"].as_str().unwrap()
    );

    // `.gitignore`: the runtime + agent pattern set plus every projected
    // agent-profile path (the fixture carries those in a separate field because
    // the original tranche did not project profiles yet).
    let oracle_lines: BTreeSet<String> = oracle["gitignore"]
        .as_str()
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .chain(
            oracle["gitignore_profile_lines"]
                .as_array()
                .expect("profile lines")
                .iter()
                .map(|line| line.as_str().expect("line").to_string()),
        )
        .collect();
    let ours_lines: BTreeSet<String> = std::fs::read_to_string(project.path().join(".gitignore"))
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect();
    assert_eq!(ours_lines, oracle_lines);
    assert!(
        !oracle["gitignore_profile_lines"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Ledger: tracked set (clone-local ones stay pending on a non-git project).
    let ledger: BTreeSet<String> =
        std::fs::read_to_string(project.path().join(".kittify/migration-ledger.jsonl"))
            .unwrap()
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
    assert_eq!(ledger, string_set(&oracle["ledger_ids"]));

    // Config/metadata: agent selection, schema and capability stamp.
    let config = std::fs::read_to_string(project.path().join(".kittify/config.yaml")).unwrap();
    assert!(config.contains("  - claude\n  - codex\n  - vibe\n"));
    assert!(config.contains("project:\n"));
    let metadata = std::fs::read_to_string(project.path().join(".kittify/metadata.yaml")).unwrap();
    assert!(metadata.contains("  schema_version: 3\n"));
    for capability in sk_install::init::CURRENT_SCHEMA_CAPABILITIES {
        assert!(metadata.contains(&format!("    {capability}: true\n")));
    }
    assert!(metadata.contains(&format!("  version: {VERSION}\n")));
}

#[test]
fn a_second_full_init_is_a_no_op_for_every_asset_surface() {
    let root = checkout_root();
    if !root.join("packs/built-in/missions").is_dir() {
        return;
    }
    let project = tempfile::tempdir().expect("project");
    let assets = ProjectAssets::discover(&root).expect("assets");
    let request = InitRequest::new(
        project.path(),
        vec!["claude".into(), "codex".into()],
        VERSION,
    )
    .with_project_assets(assets)
    .with_core_version(VERSION);
    initialize_with_details(&request).expect("first init");
    let snapshot = |path: &str| std::fs::read(project.path().join(path)).unwrap();
    let before_manifest = snapshot(".kittify/skills-manifest.json");
    let before_command = snapshot(".kittify/command-skills-manifest.json");
    let before_gitignore = snapshot(".gitignore");
    let before_attributes = snapshot(".gitattributes");
    let before_event = snapshot(".kittify/canonical-events.jsonl");
    let before_ledger = snapshot(".kittify/migration-ledger.jsonl");
    initialize_with_details(&request).expect("second init");
    assert_eq!(snapshot(".kittify/skills-manifest.json"), before_manifest);
    assert_eq!(
        snapshot(".kittify/command-skills-manifest.json"),
        before_command
    );
    assert_eq!(snapshot(".gitignore"), before_gitignore);
    assert_eq!(snapshot(".gitattributes"), before_attributes);
    assert_eq!(snapshot(".kittify/canonical-events.jsonl"), before_event);
    assert_eq!(snapshot(".kittify/migration-ledger.jsonl"), before_ledger);
}

#[test]
fn a_populated_global_runtime_uses_the_minimal_project_skeleton() {
    let root = checkout_root();
    if !root.join("src/charter/offering/skills").is_dir() {
        return;
    }
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("missions/software-dev")).unwrap();
    let project = tempfile::tempdir().expect("project");
    let assets = ProjectAssets::discover(&root)
        .expect("assets")
        .with_global_runtime_home(home.path());
    let request = InitRequest::new(project.path(), vec!["claude".into()], VERSION)
        .with_project_assets(assets)
        .with_core_version(VERSION);
    let (_report, details) = initialize_with_details(&request).expect("init");
    assert_eq!(
        details.template_mode,
        Some(sk_install::TemplateMode::GlobalRuntime)
    );
    assert!(project.path().join(".kittify/memory").is_dir());
    assert!(!project.path().join(".kittify/templates").exists());
    assert!(!project.path().join(".kittify/missions").exists());
    // The project still receives its skill surface.
    assert!(
        project
            .path()
            .join(".claude/skills/spec-kitty-git-workflow/SKILL.md")
            .is_file()
    );
}

#[test]
fn a_pending_delivery_resumes_and_restores_the_skill_surface() {
    let root = checkout_root();
    if !root.join("src/charter/offering/skills").is_dir() {
        return;
    }
    let project = tempfile::tempdir().expect("project");
    let assets = ProjectAssets::discover(&root).expect("assets");
    let request = InitRequest::new(
        project.path(),
        vec!["claude".into(), "codex".into()],
        VERSION,
    )
    .with_project_assets(assets)
    .with_core_version(VERSION);
    initialize_with_details(&request).expect("first init");
    // Simulate a crash after the pending record but before skill delivery was
    // complete: one managed native skill is missing and the record (which names
    // only the command-skill roster, as upstream) still exists.
    let missing = project
        .path()
        .join(".claude/skills/spec-kitty-git-workflow/SKILL.md");
    std::fs::remove_file(&missing).expect("remove managed skill");
    sk_install::PendingDelivery::write(project.path(), &["codex".to_string()])
        .expect("pending record");
    let (report, details) = initialize_with_details(&request).expect("resume");
    assert_eq!(report.outcome, sk_install::InitOutcome::ResumedDelivery);
    assert!(missing.is_file());
    assert!(
        details
            .surface
            .restored
            .iter()
            .any(|path| path == ".claude/skills/spec-kitty-git-workflow/SKILL.md")
    );
    assert!(
        sk_install::pending_delivery(project.path())
            .expect("pending read")
            .is_none()
    );
}

#[test]
fn an_unowned_skill_destination_is_preserved_and_reported() {
    let root = checkout_root();
    if !root.join("src/charter/offering/skills").is_dir() {
        return;
    }
    let project = tempfile::tempdir().expect("project");
    std::fs::create_dir_all(
        project
            .path()
            .join(".claude/skills/spec-kitty-git-workflow"),
    )
    .unwrap();
    std::fs::write(
        project
            .path()
            .join(".claude/skills/spec-kitty-git-workflow/SKILL.md"),
        "# operator skill\n",
    )
    .unwrap();
    let assets = ProjectAssets::discover(&root).expect("assets");
    let request = InitRequest::new(project.path(), vec!["claude".into()], VERSION)
        .with_project_assets(assets)
        .with_core_version(VERSION);
    let (_report, _details) = initialize_with_details(&request).expect("init continues");
    assert_eq!(
        std::fs::read_to_string(
            project
                .path()
                .join(".claude/skills/spec-kitty-git-workflow/SKILL.md")
        )
        .unwrap(),
        "# operator skill\n"
    );
}

#[test]
fn an_incompatible_requires_core_pin_refuses_before_any_write() {
    let root = checkout_root();
    if !root.join("packs/built-in/missions").is_dir() {
        return;
    }
    let project = tempfile::tempdir().expect("project");
    let pin_path = root.join("packs/built-in.source.yaml");
    let pin = sk_install::PackPin::from_parts(
        &pin_path,
        "https://example.invalid/packs.git".to_string(),
        "0123456789abcdef0123456789abcdef01234567".to_string(),
        "packs/built-in".to_string(),
        sk_install::pin::TreeDigest {
            algorithm: sk_install::TREE_DIGEST_ALGORITHM.to_string(),
            value: "0".repeat(64),
            level: "built-in".to_string(),
        },
        Some(">=4.0.0rc7,<5".to_string()),
        Some("0".repeat(64)),
    )
    .expect("pin");
    let assets = ProjectAssets::discover(&root)
        .expect("assets")
        .with_missions_pin(pin_path, pin);
    let request = InitRequest::new(project.path(), vec!["codex".into()], VERSION)
        .with_project_assets(assets)
        .with_core_version("5.0.0");
    let error = initialize_with_details(&request).expect_err("incompatible");
    assert_eq!(error.code(), "PACK_REQUIRES_CORE");
    assert!(!project.path().join(".kittify").exists());
}
