//! Modern-completion migration effects (`completion_modern`).
//!
//! Each test owns a fresh temporary project, home, XDG root and temp dir; the
//! corpus is *authored* (old state the pinned Source migrations see), never
//! copied from native output. Source-first pairing for every id here:
//!
//! | native id | pinned Source test module |
//! | --- | --- |
//! | `0_9_3_surface_repair_wiring` | `tests/specify_cli/tool_surface/test_surface_repair_wiring.py` |
//! | `3.2.0rc35_strip_selection_config` | `tests/specify_cli/upgrade/migrations/test_m_3_2_0rc35_strip_selection_config.py` |
//! | `3.2.0rc35_repository_root_checkout_terminology` | `tests/specify_cli/upgrade/migrations/test_m_3_2_0rc35_repository_root_checkout_terminology.py` |
//! | `3_3_0_session_presence_claude_code` | `tests/specify_cli/upgrade/migrations/test_m_session_presence_claude_code.py` |
//! | `3_3_0_session_presence_all_harnesses` | `tests/specify_cli/upgrade/migrations/test_m_session_presence_all_harnesses.py` |
//! | `3_2_0rc39_refresh_orientation_block` | `tests/specify_cli/upgrade/migrations/test_m_3_2_0rc39_refresh_orientation_block.py` |
//! | `migrate_lifecycle_envelope` | `tests/status/test_migrate_lifecycle_envelope.py` |
//! | `3.1.1_residue_safe_normalize_status_json` | `tests/specify_cli/upgrade/migrations/test_m_3_1_1_residue_safe_normalize_status_json.py` |
//! | `3.2.8_provision_kitty_env` | `tests/specify_cli/upgrade/migrations/test_provision_kitty_env.py` |
//!
//! The Source modules run (and must pass) *before* this file, through
//! `.venv-rust-oracle/bin/python -m pytest`; the evidence log for that ordering
//! lives beside this run's native evidence.

use std::path::{Path, PathBuf};
use std::process::Command;

use sk_install::migrations::completion_modern;
use sk_install::{MigrationCatalog, NATIVE_MIGRATIONS, NativeMigration};

const CLAUDE_ID: &str = "3_3_0_session_presence_claude_code";
const ALL_HARNESSES_ID: &str = "3_3_0_session_presence_all_harnesses";
const REFRESH_ID: &str = "3_2_0rc39_refresh_orientation_block";

fn native(id: &str) -> &'static NativeMigration {
    completion_modern::IMPLEMENTATIONS
        .iter()
        .find(|migration| migration.id == id)
        .unwrap_or_else(|| panic!("no modern implementation for {id}"))
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn project() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".kittify")).unwrap();
    temp
}

fn project_with_config(agents: &[&str]) -> tempfile::TempDir {
    let temp = project();
    let mut config = String::from("agents:\n  available:\n");
    for agent in agents {
        config.push_str(&format!("    - {agent}\n"));
    }
    config.push_str("  auto_commit: true\n");
    write(&temp.path().join(".kittify/config.yaml"), &config);
    temp
}

fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", root.join(".gitconfig-unused"))
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git available");
    assert!(
        status.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

// ---------------------------------------------------------------------------
// Registry surface
// ---------------------------------------------------------------------------

#[test]
fn every_owned_id_is_implemented_once_and_absent_from_the_first_tranche() {
    let catalog = MigrationCatalog::embedded().expect("catalog");
    // F2 expanded the slice to 28; F3 added the charter bundle/repair, the
    // unified bundle seal, the user-global skill relink and the canonical shim
    // rewrite (33).
    assert_eq!(completion_modern::OWNED_IDS.len(), 33);
    for id in completion_modern::OWNED_IDS {
        assert!(
            catalog.get(id).is_some(),
            "{id} is not a declared catalog migration"
        );
        assert!(
            completion_modern::IMPLEMENTATIONS
                .iter()
                .filter(|migration| migration.id == id)
                .count()
                == 1,
            "{id} must be implemented exactly once"
        );
        assert!(
            !NATIVE_MIGRATIONS.iter().any(|migration| migration.id == id),
            "{id} is already implemented by the first tranche"
        );
    }
    // Every entry carries the catalog id, never a spelling that would silently
    // never match.
    for migration in completion_modern::IMPLEMENTATIONS {
        assert!(
            catalog.get(migration.id).is_some(),
            "{} is not in the declared catalog",
            migration.id
        );
        assert!(completion_modern::OWNED_IDS.contains(&migration.id));
    }
}

// ---------------------------------------------------------------------------
// 0_9_3_surface_repair_wiring
// ---------------------------------------------------------------------------

#[test]
fn surface_wiring_writes_the_marker_once_and_reports_the_probe() {
    let temp = project_with_config(&["claude", "cursor"]);
    let migration = native("0_9_3_surface_repair_wiring");

    assert!(
        (migration.detect)(temp.path()).unwrap(),
        "the sentinel marker is absent, so the migration is needed"
    );
    assert_eq!((migration.can_apply)(temp.path()).unwrap(), None);
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        outcome.changes,
        vec!["surface_repair_wiring: probed 2 tool surface(s)".to_string()]
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".kittify/surface_repair_wired")).unwrap(),
        "wired\n"
    );
    // Idempotent: the marker settles detect.
    assert!(!(migration.detect)(temp.path()).unwrap());
}

#[test]
fn surface_wiring_refuses_an_uninitialized_project() {
    let temp = tempfile::tempdir().unwrap();
    let migration = native("0_9_3_surface_repair_wiring");
    assert!(!(migration.detect)(temp.path()).unwrap());
    assert_eq!(
        (migration.can_apply)(temp.path()).unwrap().as_deref(),
        Some("No .kittify directory found; project is not initialized")
    );
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_strip_selection_config
// ---------------------------------------------------------------------------

const SELECTION_CONFIG: &str = "# operator config\nvcs:\n  type: git\nagents:\n  available:\n    - claude\n  auto_commit: true\n  selection:\n    preferred_implementer: alice\n    preferred_reviewer: bob\n  lint_on_edit: false\ntools:\n  selection:\n    preferred_implementer: carol\n";

#[test]
fn strip_selection_removes_both_legacy_roots_and_preserves_everything_else() {
    let temp = project();
    let config = temp.path().join(".kittify/config.yaml");
    write(&config, SELECTION_CONFIG);
    let migration = native("3.2.0rc35_strip_selection_config");

    assert!((migration.detect)(temp.path()).unwrap());
    assert_eq!((migration.can_apply)(temp.path()).unwrap(), None);
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        outcome.changes,
        vec![
            "Removed: agents.selection".to_string(),
            "Removed: tools.selection".to_string()
        ]
    );

    let edited = std::fs::read_to_string(&config).unwrap();
    assert!(edited.starts_with("# operator config\nvcs:\n  type: git\n"));
    assert!(edited.contains("  auto_commit: true\n"));
    assert!(edited.contains("  lint_on_edit: false\n"));
    assert!(!edited.contains("selection"));
    assert!(!edited.contains("alice"));
    assert!(!edited.contains("carol"));

    // Idempotent, and the empty `tools:` mapping left behind is authored state
    // the source keeps too.
    assert!(!(migration.detect)(temp.path()).unwrap());
    let again = (migration.apply)(temp.path()).unwrap();
    assert!(!again.changed);
    assert_eq!(again.changes, vec!["No selection block found".to_string()]);
}

#[test]
fn strip_selection_tolerates_a_missing_or_unparseable_config() {
    let temp = project();
    let migration = native("3.2.0rc35_strip_selection_config");
    assert!(!(migration.detect)(temp.path()).unwrap());
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert_eq!(outcome.changes, vec!["No config.yaml found".to_string()]);

    write(
        &temp.path().join(".kittify/config.yaml"),
        "agents: [unclosed\n",
    );
    assert!(
        !(migration.detect)(temp.path()).unwrap(),
        "an unparseable config is not 'already clean'; the source reports False and retries later"
    );
}

#[test]
fn strip_selection_refuses_a_non_file_config_path() {
    let temp = project();
    std::fs::create_dir_all(temp.path().join(".kittify/config.yaml")).unwrap();
    let migration = native("3.2.0rc35_strip_selection_config");
    assert_eq!(
        (migration.can_apply)(temp.path()).unwrap().as_deref(),
        Some("config path exists but is not a file")
    );
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_repository_root_checkout_terminology
// ---------------------------------------------------------------------------

const GENERATED_COMMAND: &str = "<!-- spec-kitty-command-version: 4.0.0rc8 -->\n# specify\n\nPlanning happens in main for ALL missions.\nUse the main repo checkout.\n";
const CUSTOM_COMMAND: &str =
    "# my own command\nRun this in the main repo, per our team convention.\n";

#[test]
fn terminology_rewrites_only_proven_generated_command_files() {
    let temp = project_with_config(&["claude"]);
    let generated = temp.path().join(".claude/commands/spec-kitty.specify.md");
    let custom = temp.path().join(".claude/commands/spec-kitty.custom.md");
    write(&generated, GENERATED_COMMAND);
    write(&custom, CUSTOM_COMMAND);
    let migration = native("3.2.0rc35_repository_root_checkout_terminology");

    assert!((migration.detect)(temp.path()).unwrap());
    assert_eq!((migration.can_apply)(temp.path()).unwrap(), None);
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert!(outcome.changed);

    let rewritten = std::fs::read_to_string(&generated).unwrap();
    assert!(rewritten.contains(
        "Planning happens in the repository root checkout for all missions. The mission target branch comes from branch-context."
    ));
    // Pinned replacement order: "main repo" is rewritten before "main repo
    // checkout" can match again, so the source itself produces the doubled
    // "repository root checkout checkout". The native port keeps that quirk.
    assert!(
        rewritten.contains("Use the repository root checkout checkout."),
        "{rewritten}"
    );
    assert!(!rewritten.contains("main repo"));
    // The generated file stays read-only, exactly as the generation layer left it.
    let mode = std::fs::metadata(&generated).unwrap().permissions();
    assert!(
        mode.readonly(),
        "generated command file must stay read-only"
    );

    // The unproven custom command keeps its exact bytes and is reported.
    assert_eq!(std::fs::read_to_string(&custom).unwrap(), CUSTOM_COMMAND);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("not proven package-owned")),
        "warnings: {:?}",
        outcome.warnings
    );
    // The preserved unproven file still matches, so the migration stays
    // pending — exactly like the source, which re-evaluates it every run.
    assert!((migration.detect)(temp.path()).unwrap());
}

#[test]
fn terminology_ignores_agents_that_are_not_configured() {
    let temp = project_with_config(&["gemini"]);
    write(
        &temp.path().join(".claude/commands/spec-kitty.specify.md"),
        GENERATED_COMMAND,
    );
    let migration = native("3.2.0rc35_repository_root_checkout_terminology");
    assert!(!(migration.detect)(temp.path()).unwrap());
    assert_eq!(
        (migration.can_apply)(temp.path()).unwrap().as_deref(),
        Some("No generated command files need repository-root terminology updates")
    );
}

// ---------------------------------------------------------------------------
// session presence (claude, all harnesses, refresh)
// ---------------------------------------------------------------------------

fn section_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|text| text.matches("<!-- spec-kitty:orientation -->").count())
        .unwrap_or(0)
}

#[test]
fn claude_presence_writes_the_section_and_both_hooks_then_settles() {
    let temp = project_with_config(&["claude"]);
    std::fs::create_dir_all(temp.path().join(".claude")).unwrap();
    let migration = native(CLAUDE_ID);

    assert!((migration.detect)(temp.path()).unwrap());
    assert_eq!((migration.can_apply)(temp.path()).unwrap(), None);
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        outcome.changes,
        vec![
            "Wrote spec-kitty orientation to .claude/CLAUDE.md".to_string(),
            "Registered spec-kitty session-start SessionStart hook".to_string(),
            "Registered spec-kitty session-stop Stop hook".to_string(),
        ]
    );
    let claude_md = temp.path().join(".claude/CLAUDE.md");
    assert_eq!(section_count(&claude_md), 1);
    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert!(text.contains("<!-- /spec-kitty:orientation -->"));
    assert!(text.contains("**Spec Kitty v4.0.0rc8**"));
    assert!(text.contains("`spec-kitty dispatch \"<request verbatim>\"`"));

    let settings = std::fs::read_to_string(temp.path().join(".claude/settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert!(settings["hooks"]["SessionStart"].is_array());
    assert!(settings["hooks"]["Stop"].is_array());

    assert!(
        !(migration.detect)(temp.path()).unwrap(),
        "presence is complete"
    );
    // Idempotent: a second pass replaces in place, never duplicating the block.
    (migration.apply)(temp.path()).unwrap();
    assert_eq!(section_count(&claude_md), 1);
}

#[test]
fn claude_presence_is_skipped_when_claude_is_not_configured() {
    let temp = project_with_config(&["gemini"]);
    std::fs::create_dir_all(temp.path().join(".claude")).unwrap();
    let migration = native(CLAUDE_ID);
    assert!(!(migration.detect)(temp.path()).unwrap());
    assert!(!temp.path().join(".claude/CLAUDE.md").exists());
}

#[test]
fn claude_presence_detects_incomplete_hooks_even_when_the_section_exists() {
    let temp = project_with_config(&["claude"]);
    write(
        &temp.path().join(".claude/CLAUDE.md"),
        "# Project\n\n<!-- spec-kitty:orientation -->\nold\n<!-- /spec-kitty:orientation -->\n",
    );
    let migration = native(CLAUDE_ID);
    assert!(
        (migration.detect)(temp.path()).unwrap(),
        "a missing hook pair means presence is incomplete"
    );
}

#[test]
fn all_harnesses_writes_every_configured_non_claude_surface_once() {
    let temp = project_with_config(&["claude", "cursor", "gemini", "codex", "kiro"]);
    for directory in [".claude", ".cursor", ".kiro"] {
        std::fs::create_dir_all(temp.path().join(directory)).unwrap();
    }
    let migration = native(ALL_HARNESSES_ID);

    assert!((migration.detect)(temp.path()).unwrap());
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        outcome.changes,
        vec![
            "Wrote session presence orientation for harness: cursor".to_string(),
            "Wrote session presence orientation for harness: gemini".to_string(),
            "Wrote session presence orientation for harness: codex".to_string(),
            "Wrote session presence orientation for harness: kiro".to_string(),
        ]
    );
    assert_eq!(section_count(&temp.path().join("AGENTS.md")), 1);
    assert_eq!(section_count(&temp.path().join("GEMINI.md")), 1);
    assert_eq!(
        section_count(&temp.path().join(".cursor/rules/spec-kitty.mdc")),
        1
    );
    assert_eq!(
        section_count(&temp.path().join(".kiro/steering/spec-kitty.md")),
        1
    );
    // `claude` belongs to the Phase-1 effect and is untouched here.
    assert!(!temp.path().join(".claude/CLAUDE.md").exists());
    assert!(!(migration.detect)(temp.path()).unwrap());
    let again = (migration.apply)(temp.path()).unwrap();
    assert!(!again.changed);
}

#[test]
fn all_harnesses_only_reports_harnesses_whose_root_exists() {
    // `copilot` guards on `.github/`; without it the source skips the write and
    // leaves the migration pending.
    let temp = project_with_config(&["copilot"]);
    let migration = native(ALL_HARNESSES_ID);
    assert!(!(migration.detect)(temp.path()).unwrap());
    std::fs::create_dir_all(temp.path().join(".github")).unwrap();
    assert!((migration.detect)(temp.path()).unwrap());
    (migration.apply)(temp.path()).unwrap();
    assert_eq!(
        section_count(&temp.path().join(".github/copilot-instructions.md")),
        1
    );
}

#[test]
fn refresh_replaces_a_stale_installed_block_and_never_installs_an_absent_one() {
    let temp = project_with_config(&["gemini", "cursor"]);
    std::fs::create_dir_all(temp.path().join(".cursor")).unwrap();
    let stale_block = sk_install::OrientationContent::healthy("0.0.1", "stale").render();
    write(
        &temp.path().join("GEMINI.md"),
        &format!("# Team notes\n\n{stale_block}\n\nmore notes\n"),
    );
    let migration = native(REFRESH_ID);

    assert!((migration.detect)(temp.path()).unwrap());
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert_eq!(
        outcome.changes,
        vec!["Refreshed orientation block for harness: gemini".to_string()]
    );
    let text = std::fs::read_to_string(temp.path().join("GEMINI.md")).unwrap();
    assert!(text.starts_with("# Team notes\n\n"));
    assert!(text.contains("**Spec Kitty v4.0.0rc8**"));
    assert!(text.trim_end().ends_with("more notes"));
    assert_eq!(section_count(&temp.path().join("GEMINI.md")), 1);
    // The absent cursor block stays absent — the install effects own it.
    assert!(!temp.path().join(".cursor/rules/spec-kitty.mdc").exists());
    assert!(!(migration.detect)(temp.path()).unwrap());
}

#[test]
fn refresh_preserves_an_operator_edited_block_but_still_reports_the_attempt() {
    let temp = project_with_config(&["gemini"]);
    let edited = "<!-- spec-kitty:orientation -->\n**Spec Kitty v0.0.1** — project: stale (healthy)\nmy own note\n<!-- /spec-kitty:orientation -->\n";
    write(&temp.path().join("GEMINI.md"), edited);
    let migration = native(REFRESH_ID);
    assert!((migration.detect)(temp.path()).unwrap());
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert_eq!(
        outcome.changes,
        vec!["Refreshed orientation block for harness: gemini".to_string()]
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("GEMINI.md")).unwrap(),
        edited,
        "an operator-edited managed section is never rewritten"
    );
}

// ---------------------------------------------------------------------------
// migrate_lifecycle_envelope
// ---------------------------------------------------------------------------

const LEGACY_PROJECT_ROW: &str = r#"{"aggregate_id": "01K9M0R3W00000000000000000", "aggregate_type": "Project", "event_id": "01K9M0R3WJ8V6Q1Y4T7B2N5C0A", "event_type": "ProjectInitialized", "payload": {"project_slug": "demo", "project_uuid": "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d"}, "project_slug": "demo", "project_uuid": "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d", "schema_version": "5.0.0", "timestamp": "2026-11-02T09:00:00+00:00"}"#;

const LEGACY_MISSION_ROW: &str = r#"{"aggregate_id": "01K9M0R3W00000000000000000", "aggregate_type": "Mission", "event_id": "01K9M0R3WK9W7R2Z5V8C3P6D1B", "event_type": "MissionCreated", "payload": {"mission_id": "01K9M0R3W00000000000000000", "mission_slug": "demo", "wp_count": 1}, "project_slug": "demo", "project_uuid": "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d", "schema_version": "5.0.0", "timestamp": "2026-11-02T09:00:01+00:00"}"#;

#[test]
fn lifecycle_envelope_rewrites_the_project_and_every_mission_log() {
    let temp = project();
    write(
        &temp.path().join(".kittify/canonical-events.jsonl"),
        &format!("{LEGACY_PROJECT_ROW}\n"),
    );
    write(
        &temp.path().join("kitty-specs/001-demo/status.events.jsonl"),
        &format!("{LEGACY_MISSION_ROW}\n"),
    );
    let migration = native("migrate_lifecycle_envelope");

    assert!((migration.detect)(temp.path()).unwrap());
    assert_eq!((migration.can_apply)(temp.path()).unwrap(), None);
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        outcome.changes,
        vec![
            "Rewrote 2 legacy-shaped lifecycle envelope row(s) to F1's strict shape across 2 event log(s) scanned".to_string()
        ]
    );

    for path in [
        temp.path().join(".kittify/canonical-events.jsonl"),
        temp.path().join("kitty-specs/001-demo/status.events.jsonl"),
    ] {
        let text = std::fs::read_to_string(&path).unwrap();
        let row: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(row["schema_version"], "3.0.0", "path {}", path.display());
        for key in [
            "causation_id",
            "correlation_id",
            "data_tier",
            "node_id",
            "lamport_clock",
            "build_id",
        ] {
            assert!(
                row.get(key).is_some(),
                "{key} missing in {}",
                path.display()
            );
        }
        assert!(row.get("aggregate_type").is_none());
        // The event identity and payload survive the envelope rewrite.
        assert_eq!(
            row["event_type"],
            if path.ends_with("canonical-events.jsonl") {
                "ProjectInitialized"
            } else {
                "MissionCreated"
            }
        );
        let backup = path.with_file_name(format!(
            "{}.pre-migration.bak",
            path.file_name().unwrap().to_str().unwrap()
        ));
        assert!(backup.exists(), "the pre-migration snapshot is retained");
        assert!(std::fs::read_to_string(&backup).unwrap().contains("5.0.0"));
    }

    assert!(!(migration.detect)(temp.path()).unwrap());
    let again = (migration.apply)(temp.path()).unwrap();
    assert!(!again.changed);
}

#[test]
fn lifecycle_envelope_noops_without_any_event_log() {
    let temp = project();
    let migration = native("migrate_lifecycle_envelope");
    assert!(!(migration.detect)(temp.path()).unwrap());
    assert_eq!(
        (migration.can_apply)(temp.path()).unwrap().as_deref(),
        Some("no legacy-shaped lifecycle envelope rows found in any event log")
    );
    assert!(!(migration.apply)(temp.path()).unwrap().changed);
}

#[test]
fn lifecycle_envelope_fails_and_names_a_stale_snapshot_refusal() {
    let temp = project();
    let log = temp.path().join(".kittify/canonical-events.jsonl");
    write(&log, &format!("{LEGACY_PROJECT_ROW}\n"));
    std::fs::write(
        log.with_file_name("canonical-events.jsonl.pre-migration.bak"),
        "stale snapshot\n",
    )
    .unwrap();
    let migration = native("migrate_lifecycle_envelope");
    let error = (migration.apply)(temp.path()).expect_err("a stale snapshot is refused");
    assert!(
        error.to_string().contains("canonical-events.jsonl"),
        "error must name the path: {error}"
    );
    // The log itself is untouched by the refusal.
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("\"schema_version\": \"5.0.0\"")
    );
}

// ---------------------------------------------------------------------------
// 3.1.1_residue_safe_normalize_status_json
// ---------------------------------------------------------------------------

const STALE_STATUS: &str = "{\n  \"materialized_at\": \"2026-11-02T09:00:00+00:00\",\n  \"schema_version\": 1,\n  \"work_packages\": []\n}\n";

#[test]
fn residue_safe_status_normalises_materialized_at_and_keeps_other_fields() {
    let temp = project();
    write(
        &temp.path().join("kitty-specs/001-demo/status.events.jsonl"),
        "",
    );
    write(
        &temp.path().join("kitty-specs/001-demo/status.json"),
        STALE_STATUS,
    );
    let migration = native("3.1.1_residue_safe_normalize_status_json");

    assert!((migration.detect)(temp.path()).unwrap());
    assert_eq!((migration.can_apply)(temp.path()).unwrap(), None);
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert_eq!(
        outcome.changes,
        vec!["001-demo: normalised status.json".to_string()]
    );

    let text =
        std::fs::read_to_string(temp.path().join("kitty-specs/001-demo/status.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["materialized_at"], "");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["work_packages"], serde_json::json!([]));
    assert!(text.ends_with('\n'));
    // `json.dumps(sort_keys=True, indent=2)` byte shape: two-space indent and
    // sorted keys.
    assert!(text.starts_with("{\n  \"materialized_at\""), "{text}");

    assert!(!(migration.detect)(temp.path()).unwrap());
    assert!(!(migration.apply)(temp.path()).unwrap().changed);
}

#[test]
fn residue_safe_status_leaves_a_tracked_residue_folder_untouched() {
    let temp = project();
    write(
        &temp.path().join("kitty-specs/001-real/spec.md"),
        "# spec\n",
    );
    write(
        &temp.path().join("kitty-specs/001-real/status.json"),
        STALE_STATUS,
    );
    // A folder with no mission_id and no tracked marker: git-aware residue.
    write(
        &temp.path().join("kitty-specs/junk-status/status.json"),
        STALE_STATUS,
    );
    run_git(temp.path(), &["init", "-q"]);
    run_git(temp.path(), &["add", "kitty-specs/001-real/spec.md"]);

    let migration = native("3.1.1_residue_safe_normalize_status_json");
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert_eq!(
        outcome.changes,
        vec!["001-real: normalised status.json".to_string()]
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("kitty-specs/junk-status/status.json")).unwrap(),
        STALE_STATUS,
        "residue bytes are never rewritten"
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("junk-status") && warning.contains("not a Mission")),
        "warnings: {:?}",
        outcome.warnings
    );
}

#[test]
fn residue_safe_status_defers_to_a_settled_predecessor() {
    let temp = project();
    write(
        &temp.path().join("kitty-specs/001-demo/status.events.jsonl"),
        "",
    );
    write(
        &temp.path().join("kitty-specs/001-demo/status.json"),
        STALE_STATUS,
    );
    let ledger = temp.path().join(".kittify/migration-ledger.jsonl");
    write(
        &ledger,
        "{\"id\": \"3.1.1_normalize_status_json\", \"applied_at\": \"2026-11-01T00:00:00+00:00\", \"result\": \"success\", \"identity\": \"3.1.1_normalize_status_json\", \"content_hash\": \"sha256:0\", \"cli_version\": \"4.0.0rc8\"}\n",
    );
    let migration = native("3.1.1_residue_safe_normalize_status_json");
    assert!(
        !(migration.detect)(temp.path()).unwrap(),
        "a settled predecessor proves the repair already ran"
    );
}

// ---------------------------------------------------------------------------
// 3.2.8_provision_kitty_env
// ---------------------------------------------------------------------------

#[test]
fn provision_kitty_env_creates_the_scaffold_pointer_and_ignore_coverage() {
    let temp = project_with_config(&["claude"]);
    let migration = native("3.2.8_provision_kitty_env");

    assert!((migration.detect)(temp.path()).unwrap());
    assert_eq!((migration.can_apply)(temp.path()).unwrap(), None);
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert_eq!(
        outcome.changes,
        vec![
            "Created .kittify/.kitty.env".to_string(),
            "Registered env_file: ${SPEC_KITTY_HOME}/.kitty.env in config.yaml".to_string(),
            "Added .kittify/.kitty.env to .gitignore".to_string(),
            "Added .kittify/.kitty.env to .claudeignore".to_string(),
        ]
    );

    let env_file = std::fs::read_to_string(temp.path().join(".kittify/.kitty.env")).unwrap();
    assert!(env_file.contains("# Spec Kitty per-repo operator environment file."));
    assert!(env_file.contains("# SPEC_KITTY_ORG_TOKEN="));
    assert!(
        !env_file
            .lines()
            .any(|line| line.trim_start().starts_with("SPEC_KITTY_PACKS_ROOT=")),
        "the packs-root var is never assigned in the scaffold"
    );

    let config = std::fs::read_to_string(temp.path().join(".kittify/config.yaml")).unwrap();
    assert!(config.starts_with("agents:\n  available:\n    - claude\n"));
    assert!(config.contains("env_file: ${SPEC_KITTY_HOME}/.kitty.env\n"));
    assert!(config.contains("  auto_commit: true\n"));

    assert!(
        std::fs::read_to_string(temp.path().join(".gitignore"))
            .unwrap()
            .contains(".kittify/.kitty.env")
    );
    let claudeignore = std::fs::read_to_string(temp.path().join(".claudeignore")).unwrap();
    assert_eq!(claudeignore, ".kittify/.kitty.env\n");

    assert!(!(migration.detect)(temp.path()).unwrap());
    let again = (migration.apply)(temp.path()).unwrap();
    assert_eq!(again.changes, vec!["already provisioned".to_string()]);
}

#[test]
fn provision_kitty_env_appends_to_an_authored_claudeignore_and_gitignore() {
    let temp = project_with_config(&["claude"]);
    write(
        &temp.path().join(".claudeignore"),
        "# team ignores\n*.secret\n",
    );
    write(&temp.path().join(".gitignore"), "target/\n");
    let migration = native("3.2.8_provision_kitty_env");
    (migration.apply)(temp.path()).unwrap();

    let claudeignore = std::fs::read_to_string(temp.path().join(".claudeignore")).unwrap();
    assert_eq!(
        claudeignore, "# team ignores\n*.secret\n\n.kittify/.kitty.env\n",
        "authored lines survive; the entry is appended like the source"
    );
    let gitignore = std::fs::read_to_string(temp.path().join(".gitignore")).unwrap();
    assert!(gitignore.starts_with("target/\n"));
    assert!(gitignore.contains(".kittify/.kitty.env"));
}

#[test]
fn provision_kitty_env_refuses_a_symlinked_claudeignore() {
    let temp = project_with_config(&["claude"]);
    let target = temp.path().join("outside-ignore");
    write(&target, "the operator's file\n");
    std::os::unix::fs::symlink(&target, temp.path().join(".claudeignore")).unwrap();
    let migration = native("3.2.8_provision_kitty_env");
    let error = (migration.apply)(temp.path()).expect_err("a symlink is refused");
    assert_eq!(error.code(), "unsafe_path");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "the operator's file\n",
        "the link target is never written through"
    );
}

#[test]
fn provision_kitty_env_keeps_an_authored_env_file_pointer() {
    let temp = project();
    write(
        &temp.path().join(".kittify/config.yaml"),
        "# keep me\nagents:\n  available:\n    - claude\nenv_file: /custom/.kitty.env\n",
    );
    let migration = native("3.2.8_provision_kitty_env");
    assert_eq!(
        (migration.can_apply)(temp.path()).unwrap(),
        None,
        "the env file and ignore coverage are still missing"
    );
    let outcome = (migration.apply)(temp.path()).unwrap();
    assert!(
        !outcome
            .changes
            .iter()
            .any(|change| change.contains("Registered env_file")),
        "an authored pointer is never overwritten: {:?}",
        outcome.changes
    );
    let config = std::fs::read_to_string(temp.path().join(".kittify/config.yaml")).unwrap();
    assert!(config.contains("env_file: /custom/.kitty.env\n"));
    assert!(config.starts_with("# keep me\n"));
}

fn owned_ids_paths() -> Vec<PathBuf> {
    completion_modern::OWNED_IDS
        .iter()
        .map(PathBuf::from)
        .collect()
}

#[test]
fn owned_ids_are_unique() {
    let mut ids = owned_ids_paths();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), completion_modern::OWNED_IDS.len());
}
