//! Differential tests for the F2 modern-migration slice.
//!
//! Every expectation is a *Source* observation recorded by
//! `/tmp/sk-cont-modern2-probe.py` / `-probe2.py` under the locked oracle
//! (CPython 3.11.15, 53 distributions, isolated HOME/XDG/TMP), run before these
//! native assertions.

use std::path::{Path, PathBuf};

use sk_install::migrations::completion_modern::{
    commands_install, context_sources, retired_activation,
};

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, text).expect("write");
}

fn project(root: &Path, agents: &[&str]) {
    let mut text = String::from("agents:\n  available:\n");
    for agent in agents {
        text.push_str(&format!("    - {agent}\n"));
    }
    write(&root.join(".kittify/config.yaml"), &text);
}

const MARKER: &str = "<!-- spec-kitty-command-version: 4.0.0rc8 -->\n";

#[test]
fn context_sources_merges_ids_and_preserves_authored_bytes() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let profile = root.join(".kittify/profiles/custom.agent.yaml");
    write(
        &profile,
        "# operator profile\nname: custom-implementer\ncontext-sources:\n  directives: [001-one, 002-two]      # inline note\n  tactics: [avoid-gold-plating]\n  toolguides: []\n  styleguides: []\n  doctrine-layers: [core]\n  additional: [notes.md]\ndirective-references:\n  - code: '001-one'\n    name: one                 # authored comment\n    rationale: keep me\n# trailing comment\nskill-references:\n  - id: spec-kitty\n",
    );
    let clean = root.join(".kittify/profiles/clean.agent.yaml");
    write(&clean, "name: clean\n");
    assert!(context_sources::detect(root).expect("detect"));
    let outcome = context_sources::apply(root).expect("apply");
    assert!(outcome.changed);
    assert_eq!(
        outcome.changes,
        vec![
            "Consolidated context-sources in .kittify/profiles/custom.agent.yaml (merged 2 refs, dropped 2 non-edge entries (additional:notes.md, doctrine-layers:core))"
        ]
    );
    let text = std::fs::read_to_string(&profile).expect("read");
    assert!(text.starts_with("# operator profile\n"), "{text}");
    assert!(text.contains("# authored comment"), "{text}");
    assert!(text.contains("# trailing comment"), "{text}");
    assert!(text.contains("code: '001-one'"), "{text}");
    assert!(!text.contains("doctrine-layers"), "{text}");
    assert!(!text.contains("notes.md"), "{text}");
    assert!(!text.contains("\ncontext-sources:"), "{text}");
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).expect("yaml");
    let directives = parsed["directive-references"].as_sequence().expect("seq");
    assert_eq!(directives.len(), 2);
    assert_eq!(directives[1]["code"].as_str(), Some("002-two"));
    assert_eq!(
        directives[1]["rationale"].as_str(),
        Some(
            "Migrated from the retired context-sources surface (mission doctrine-drg-silent-drop-boundary-01M0PE7E)."
        )
    );
    assert_eq!(
        parsed["tactic-references"][0]["id"].as_str(),
        Some("avoid-gold-plating")
    );
    assert_eq!(
        parsed["skill-references"][0]["id"].as_str(),
        Some("spec-kitty")
    );
    assert_eq!(
        std::fs::read_to_string(&clean).expect("clean"),
        "name: clean\n"
    );
    assert!(!context_sources::detect(root).expect("second detect"));
    let second = context_sources::apply(root).expect("second");
    assert_eq!(
        second.changes,
        vec!["No consumer profile authored context-sources"]
    );
}

#[test]
fn rtk_retirement_removes_members_and_catalog_blocks() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "# project config\nagents:\n  available:\n    - claude\nactivated_toolguides:\n  - other-guide          # keep\n  - rtk-search-tooling\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "activated_toolguides:\n  - rtk-search-tooling\ncatalog:\n  mission: software-dev\n  references:\n    - id: TOOLGUIDE:rtk-search-tooling\n      name: rtk\n    - id: TACTIC:keep-me\n      name: keep\n",
    );
    write(
        &root.join(".kittify/charter/references.yaml"),
        "references:\n  - id: TOOLGUIDE:rtk-search-tooling\n    source_path: gone.md\n  - id: TACTIC:keep-me\n",
    );
    // The Source retirement table is expressed as data so the shared engine is
    // exercised exactly as the pin does.
    let retirement = retired_activation::Retirement {
        kind_key: "activated_toolguides",
        stem: "rtk-search-tooling",
        reference_prefix: "TOOLGUIDE",
        successors: &[],
    };
    assert!(retired_activation::detect_retirements(root, &[retirement], false).expect("detect"));
    let outcome = retired_activation::apply_retirements(root, &[retirement], false).expect("apply");
    assert!(outcome.changed);
    assert_eq!(
        outcome.changes,
        vec![
            "Removed rtk-search-tooling from .kittify/config.yaml (activated_toolguides)",
            "Removed rtk-search-tooling from .kittify/charter/charter.yaml (activated_toolguides)",
            "Removed rtk-search-tooling from .kittify/charter/charter.yaml (catalog)",
            "Removed rtk-search-tooling from .kittify/charter/references.yaml (references)",
        ]
    );
    let config = std::fs::read_to_string(root.join(".kittify/config.yaml")).expect("config");
    assert!(config.contains("other-guide"), "{config}");
    assert!(config.contains("# keep"), "{config}");
    assert!(!config.contains("rtk-search-tooling"), "{config}");
    let charter =
        std::fs::read_to_string(root.join(".kittify/charter/charter.yaml")).expect("charter");
    assert!(!charter.contains("rtk-search-tooling"), "{charter}");
    assert!(charter.contains("TACTIC:keep-me"), "{charter}");
    assert!(charter.contains("mission: software-dev"), "{charter}");
    assert!(!retired_activation::detect_retirements(root, &[retirement], false).expect("second"));
}

#[test]
fn single_owner_successors_activate_in_the_same_vocabulary() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "activated_tactics:\n  - bug-fixing-checklist\n  - locality-of-change\n  - keep-me\nactivated_procedures:\n  - tracker-organisation-workflow\n  - test-first-bug-fixing\nactivated_directives:\n  - 024-locality-of-change\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "activated_tactics:\n  - common-docs-curation\ngovernance:\n  charter:\n    selected_tactics:\n      - locality-of-change\n      - keep-me\n    selected_styleguides:\n      - boring-code-review\ncatalog:\n  references:\n    - id: TACTIC:locality-of-change\n    - id: STYLEGUIDE:boring-code-review\n",
    );
    write(
        &root.join(".kittify/charter/interview/answers.yaml"),
        "selected_tactics:\n  - behavior-driven-development\n  - keep-me\nselected_directives:\n  - 024-locality-of-change\n",
    );
    let retirements = [
        retired_activation::Retirement {
            kind_key: "activated_styleguides",
            stem: "adversarial-squad-cadence",
            reference_prefix: "STYLEGUIDE",
            successors: &[],
        },
        retired_activation::Retirement {
            kind_key: "activated_tactics",
            stem: "bug-fixing-checklist",
            reference_prefix: "TACTIC",
            successors: &[("activated_procedures", "test-first-bug-fixing")],
        },
        retired_activation::Retirement {
            kind_key: "activated_tactics",
            stem: "locality-of-change",
            reference_prefix: "TACTIC",
            successors: &[("activated_tactics", "avoid-gold-plating")],
        },
        retired_activation::Retirement {
            kind_key: "activated_tactics",
            stem: "common-docs-curation",
            reference_prefix: "TACTIC",
            successors: &[
                ("activated_tactics", "common-docs-scaffold"),
                ("activated_tactics", "common-docs-write"),
                ("activated_tactics", "common-docs-find"),
            ],
        },
        retired_activation::Retirement {
            kind_key: "activated_tactics",
            stem: "boring-code-review",
            reference_prefix: "TACTIC",
            successors: &[("activated_styleguides", "boring-code-review")],
        },
        retired_activation::Retirement {
            kind_key: "activated_tactics",
            stem: "behavior-driven-development",
            reference_prefix: "TACTIC",
            successors: &[("activated_tactics", "bdd-scenario-formulation")],
        },
        retired_activation::Retirement {
            kind_key: "activated_tactics",
            stem: "iterative-deepening-review",
            reference_prefix: "TACTIC",
            successors: &[],
        },
        retired_activation::Retirement {
            kind_key: "activated_procedures",
            stem: "tracker-organisation-workflow",
            reference_prefix: "PROCEDURE",
            successors: &[],
        },
    ];
    assert!(retired_activation::detect_retirements(root, &retirements, true).expect("detect"));
    let outcome = retired_activation::apply_retirements(root, &retirements, true).expect("apply");
    assert!(outcome.changed);
    let config = std::fs::read_to_string(root.join(".kittify/config.yaml")).expect("config");
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&config).expect("yaml");
    assert_eq!(
        parsed["activated_tactics"]
            .as_sequence()
            .expect("tactics")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>(),
        vec!["keep-me", "avoid-gold-plating"]
    );
    assert_eq!(
        parsed["activated_procedures"][0].as_str(),
        Some("test-first-bug-fixing")
    );
    assert_eq!(
        parsed["activated_directives"][0].as_str(),
        Some("024-locality-of-change")
    );
    let charter =
        std::fs::read_to_string(root.join(".kittify/charter/charter.yaml")).expect("charter");
    assert!(charter.contains("common-docs-scaffold"), "{charter}");
    assert!(charter.contains("avoid-gold-plating"), "{charter}");
    assert!(!charter.contains("TACTIC:locality-of-change"), "{charter}");
    assert!(
        charter.contains("STYLEGUIDE:boring-code-review"),
        "{charter}"
    );
    let answers = std::fs::read_to_string(root.join(".kittify/charter/interview/answers.yaml"))
        .expect("answers");
    assert!(
        !answers.contains("behavior-driven-development"),
        "{answers}"
    );
    assert!(answers.contains("024-locality-of-change"), "{answers}");
}

#[test]
fn runtime_next_skill_fixes_are_source_marker_driven() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    project(root, &["claude"]);
    let skill = root.join(".claude/skills/spec-kitty-runtime-next/SKILL.md");
    write(
        &skill,
        "---\nname: spec-kitty-runtime-next\n---\n\nDefaults to `success` if omitted.\n",
    );
    assert!(commands_install::detect_runtime_next_result_default(root).expect("detect"));
    let outcome = commands_install::apply_runtime_next_result_default(root).expect("apply");
    assert_eq!(
        outcome.changes,
        vec!["Replaced .claude/skills/spec-kitty-runtime-next/SKILL.md"]
    );
    let text = std::fs::read_to_string(&skill).expect("read");
    assert!(
        !text.contains("Defaults to `success` if omitted."),
        "{text}"
    );
    assert!(!commands_install::detect_runtime_next_result_default(root).expect("second"));

    let root2 = tempfile::tempdir().expect("temp");
    let root2 = root2.path();
    project(root2, &["claude"]);
    let skill2 = root2.join(".claude/skills/spec-kitty-runtime-next/SKILL.md");
    write(
        &skill2,
        "---\nname: spec-kitty-runtime-next\n---\n\nCheck `prompt_file` is not null, then read and execute\n",
    );
    assert!(commands_install::detect_prompt_file_workaround(root2).expect("detect"));
    let outcome = commands_install::apply_prompt_file_workaround(root2).expect("apply");
    assert_eq!(
        outcome.changes,
        vec!["Replaced .claude/skills/spec-kitty-runtime-next/SKILL.md"]
    );
    let text = std::fs::read_to_string(&skill2).expect("read");
    assert!(
        !text.contains("Check `prompt_file` is not null, then read and execute"),
        "{text}"
    );
}

#[test]
fn builtin_mission_types_are_appended_without_losing_authored_config() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "# authored\nagents:\n  available:\n    - claude\n",
    );
    assert!(commands_install::detect_activate_builtin_mission_types(root).expect("detect"));
    let outcome = commands_install::apply_activate_builtin_mission_types(root).expect("apply");
    assert_eq!(
        outcome.changes,
        vec![
            "Added mission_type_activations: [\"documentation\", \"plan\", \"research\", \"software-dev\"]"
        ]
    );
    let text = std::fs::read_to_string(root.join(".kittify/config.yaml")).expect("read");
    assert!(text.starts_with("# authored\n"), "{text}");
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).expect("yaml");
    let values: Vec<&str> = parsed["mission_type_activations"]
        .as_sequence()
        .expect("seq")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert_eq!(
        values,
        vec!["documentation", "plan", "research", "software-dev"]
    );
    assert!(!commands_install::detect_activate_builtin_mission_types(root).expect("second"));
    let again = commands_install::apply_activate_builtin_mission_types(root).expect("again");
    assert_eq!(
        again.changes,
        vec!["mission_type_activations already present; no changes needed"]
    );
}

fn handoff_project(root: &Path) {
    project(root, &["claude"]);
    write(
        &root.join(".kittify/missions/software-dev/command-templates/implement.md"),
        "---\nprofile: x\n---\n\n- `profile`\n- `tool`\n\nIf `profile` is empty, ask.\n\n## Output\n",
    );
    write(
        &root.join(".kittify/missions/software-dev/command-templates/review.md"),
        "# Review\n\n## 2. Parse WP frontmatter\n\n### 3. Next\n",
    );
    write(
        &root.join(".kittify/overrides/missions/software-dev/templates/task-prompt-template.md"),
        "---\ntitle: x\n---\n\n# Task\n\nBody\n",
    );
    write(
        &root.join(".agents/skills/spec-kitty.implement/SKILL.md"),
        "# Implement\n\n- `profile`\n\n## Output\n",
    );
}

#[test]
fn profile_handoff_inserts_the_source_blocks_idempotently() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    handoff_project(root);
    assert!(commands_install::detect_kittify_profile_handoff(root).expect("detect"));
    let outcome = commands_install::apply_kittify_profile_handoff(root).expect("apply");
    assert!(outcome.changed);
    let implement = std::fs::read_to_string(
        root.join(".kittify/missions/software-dev/command-templates/implement.md"),
    )
    .expect("implement");
    assert!(implement.contains("- `agent_profile`"), "{implement}");
    assert!(implement.contains("- `agent`"), "{implement}");
    assert!(
        implement.contains("If `agent_profile` is empty"),
        "{implement}"
    );
    assert!(
        implement.contains("Prepare for Review Hand-off"),
        "{implement}"
    );
    assert!(
        implement.find("Prepare for Review Hand-off").unwrap()
            < implement.find("## Output").unwrap()
    );
    let template = std::fs::read_to_string(
        root.join(".kittify/overrides/missions/software-dev/templates/task-prompt-template.md"),
    )
    .expect("template");
    assert!(template.contains("agent_profile: \"\""), "{template}");
    assert!(template.contains("role: \"\""), "{template}");
    assert!(template.contains("⚡ Do This First"), "{template}");
    let skill = std::fs::read_to_string(root.join(".agents/skills/spec-kitty.implement/SKILL.md"))
        .expect("skill");
    assert!(skill.contains("Prepare for Review Hand-off"), "{skill}");
    assert!(!commands_install::detect_kittify_profile_handoff(root).expect("second"));
    let again = commands_install::apply_kittify_profile_handoff(root).expect("again");
    assert!(again.changes.is_empty());
}

#[test]
fn codex_prompts_are_preserved_and_command_skills_installed() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    project(root, &["codex"]);
    write(
        &root.join(".codex/prompts/spec-kitty.specify.md"),
        &format!("{MARKER}old\n"),
    );
    write(&root.join(".codex/prompts/custom.md"), "third party\n");
    assert!(commands_install::detect_codex_to_skills(root).expect("detect"));
    let outcome = commands_install::apply_codex_to_skills(root).expect("apply");
    assert!(
        root.join(".codex/prompts.superseded/spec-kitty.specify.md")
            .exists()
    );
    assert!(!root.join(".codex/prompts/spec-kitty.specify.md").exists());
    assert!(root.join(".codex/prompts/custom.md").exists());
    assert!(
        outcome
            .changes
            .iter()
            .any(|line| line.contains("Installed 11 Codex skill packages"))
    );
    assert!(
        root.join(".agents/skills/spec-kitty.specify/SKILL.md")
            .exists()
    );
    assert!(!commands_install::detect_codex_to_skills(root).expect("second"));
}

#[test]
fn spk_skill_pack_installs_only_the_spk_hierarchy() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    project(root, &["claude"]);
    assert!(commands_install::detect_spk_skill_pack(root).expect("detect"));
    let outcome = commands_install::apply_spk_skill_pack(root).expect("apply");
    assert!(outcome.changed);
    assert!(
        root.join(".claude/skills/spk-admin-upgrade/SKILL.md")
            .exists(),
        "{:?}",
        outcome.changes
    );
    assert!(
        !root
            .join(".claude/skills/spec-kitty-mission-review/SKILL.md")
            .exists()
    );
    assert!(root.join(".kittify/skills-manifest.json").exists());
    assert!(!commands_install::detect_spk_skill_pack(root).expect("second"));
}

#[test]
fn every_owned_id_is_implemented_once_and_absent_from_the_first_tranche() {
    use sk_install::migrations::completion_modern::{IMPLEMENTATIONS, OWNED_IDS};
    use sk_install::upgrade::NATIVE_MIGRATIONS;
    let mut owned = OWNED_IDS.to_vec();
    owned.sort_unstable();
    let before = owned.len();
    owned.dedup();
    assert_eq!(owned.len(), before, "OWNED_IDS contains duplicates");
    for id in OWNED_IDS {
        assert_eq!(
            IMPLEMENTATIONS
                .iter()
                .filter(|migration| migration.id == id)
                .count(),
            1,
            "{id} must be implemented exactly once in this slice"
        );
        assert!(
            !NATIVE_MIGRATIONS.iter().any(|migration| migration.id == id),
            "{id} must not be duplicated in the first tranche"
        );
    }
    let catalog = sk_install::ledger::MigrationCatalog::embedded().expect("catalog");
    for id in OWNED_IDS {
        assert!(catalog.get(id).is_some(), "{id} must be a real catalog id");
    }
}

fn _unused(_path: PathBuf) {}

#[test]
fn activation_ledger_moves_legacy_keys_and_preserves_authored_config() {
    use sk_install::migrations::completion_modern::activation_ledger;
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "# authored\nagents:\n  available:\n    - claude\nactivated_tactics:\n  - keep-me\nmission_type_activations:\n  - software-dev\nactivated_kinds:\n  - directive\n",
    );
    assert!(activation_ledger::detect(root).expect("detect"));
    let outcome = activation_ledger::apply(root).expect("apply");
    let config = std::fs::read_to_string(root.join(".kittify/config.yaml")).expect("config");
    let ledger =
        std::fs::read_to_string(root.join(".kittify/charter/activation.yaml")).expect("ledger");
    assert!(config.starts_with("# authored\n"), "{config}");
    assert!(!config.contains("activated_tactics"), "{config}");
    assert!(!config.contains("mission_type_activations"), "{config}");
    assert!(config.contains("- claude"), "{config}");
    assert!(ledger.contains("activated_tactics:\n- keep-me"), "{ledger}");
    assert!(ledger.contains("activated_kinds:\n- directive"), "{ledger}");
    assert_eq!(
        outcome.changes,
        vec![
            "Moved activated_kinds: .kittify/config.yaml -> .kittify/charter/activation.yaml",
            "Moved mission_type_activations: .kittify/config.yaml -> .kittify/charter/activation.yaml",
            "Moved activated_tactics: .kittify/config.yaml -> .kittify/charter/activation.yaml",
            "Wrote .kittify/charter/activation.yaml",
            "Wrote .kittify/config.yaml",
        ]
    );
    assert!(!activation_ledger::detect(root).expect("second"));
}

#[test]
fn activation_ledger_reads_the_pointer_authority_and_strips_it() {
    use sk_install::migrations::completion_modern::activation_ledger;
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "charter: .kittify/charter/charter.yaml\nagents:\n  available:\n    - claude\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "activated_directives:\n  - 001-one\n",
    );
    let outcome = activation_ledger::apply(root).expect("apply");
    let charter =
        std::fs::read_to_string(root.join(".kittify/charter/charter.yaml")).expect("charter");
    let ledger =
        std::fs::read_to_string(root.join(".kittify/charter/activation.yaml")).expect("ledger");
    assert!(!charter.contains("activated_directives"), "{charter}");
    assert_eq!(charter, "{}\n");
    assert!(
        ledger.contains("activated_directives:\n- 001-one"),
        "{ledger}"
    );
    assert!(
        outcome.changes.contains(
            &"Moved activated_directives: .kittify/charter/charter.yaml -> .kittify/charter/activation.yaml".to_string()
        ),
        "{:?}",
        outcome.changes
    );
}

#[test]
fn activation_ledger_refuses_divergence_and_deny_keys_without_writing() {
    use sk_install::migrations::completion_modern::activation_ledger;
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "charter: .kittify/charter/charter.yaml\nactivated_tactics:\n  - other\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "activated_tactics:\n  - keep\n",
    );
    let before = std::fs::read_to_string(root.join(".kittify/config.yaml")).expect("before");
    assert!(activation_ledger::detect(root).expect("detect"));
    let error = activation_ledger::apply(root).expect_err("refused");
    let message = error.to_string();
    assert!(
        message.contains(
            "Refused to move the activation keys to .kittify/charter/activation.yaml: activated_tactics differs between "
        ),
        "{message}"
    );
    assert!(
        message.contains(
            "Make the files agree, or remove the stale copy, then run `spec-kitty migrate activation-ledger` or `spec-kitty upgrade` again. Nothing was written."
        ),
        "{message}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".kittify/config.yaml")).expect("after"),
        before
    );
    assert!(!root.join(".kittify/charter/activation.yaml").exists());

    let root2 = tempfile::tempdir().expect("temp");
    let root2 = root2.path();
    write(
        &root2.join(".kittify/config.yaml"),
        "activated_tactics:\n  - keep\ndeactivated_tactics:\n  - old\n",
    );
    let error = activation_ledger::apply(root2).expect_err("refused deny");
    let message = error.to_string();
    assert!(message.contains("deactivated_tactics in "), "{message}");
    assert!(
        message.contains(
            "is a legacy deny-list key; it was never applied and is not moved. Remove it by hand"
        ),
        "{message}"
    );
}

#[test]
fn activation_ledger_strips_against_an_existing_equal_ledger() {
    use sk_install::migrations::completion_modern::activation_ledger;
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "activated_tactics:\n  - keep\nagents:\n  available:\n    - claude\n",
    );
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "activated_tactics:\n- keep\n",
    );
    let outcome = activation_ledger::apply(root).expect("apply");
    let config = std::fs::read_to_string(root.join(".kittify/config.yaml")).expect("config");
    let ledger =
        std::fs::read_to_string(root.join(".kittify/charter/activation.yaml")).expect("ledger");
    assert!(!config.contains("activated_tactics"), "{config}");
    assert_eq!(ledger, "activated_tactics:\n- keep\n");
    assert!(
        !outcome
            .changes
            .contains(&"Wrote .kittify/charter/activation.yaml".to_string()),
        "{:?}",
        outcome.changes
    );
}

#[test]
fn op_record_schema_v2_rewrites_salvageable_and_deletes_unrepresentable() {
    use sk_install::migrations::completion_modern::op_record;
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let ops = root.join("kitty-ops");
    let id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let legacy = ops.join(format!("op-{id}.jsonl"));
    write(
        &legacy,
        &format!(
            "{}\n{}\n{}\n",
            serde_json::json!({"event": "started", "invocation_id": id, "profile_id": "p", "request_text": "do", "started_at": "2026-01-01T00:00:00Z"}),
            serde_json::json!({"event": "completed", "outcome": null}),
            serde_json::json!({"event": "link", "target": "x"}),
        ),
    );
    write(&ops.join("ops-index.jsonl"), "{\"keep\": true}\n");
    write(&ops.join("lifecycle.jsonl"), "{\"keep\": true}\n");
    write(&ops.join("propagation-errors.jsonl"), "{\"keep\": true}\n");
    write(
        &ops.join("no-identity.jsonl"),
        &format!(
            "{}\n",
            serde_json::json!({"event": "started", "invocation_id": "", "profile_id": ""})
        ),
    );
    assert!(op_record::detect(root).expect("detect"));
    assert!(op_record::can_apply(root).expect("can_apply").is_none());
    let outcome = op_record::apply(root).expect("apply");
    assert_eq!(
        outcome.changes,
        vec![
            "Deleted unsalvageable no-identity.jsonl (started event lacks invocation_id/profile_id)".to_string(),
            format!("Rewrote op-{id}.jsonl to v2 schema"),
        ]
    );
    assert!(
        outcome.warnings.contains(&format!(
            "op-{id}.jsonl: completed_at missing; fell back to started_at"
        )),
        "{:?}",
        outcome.warnings
    );
    assert!(
        outcome
            .warnings
            .contains(&"Deleted 1 unsalvageable Op record file(s): no-identity.jsonl".to_string()),
        "{:?}",
        outcome.warnings
    );
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&legacy)
        .expect("read")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["action"], "unrecorded");
    assert_eq!(lines[0]["actor"], "unrecorded");
    assert_eq!(lines[0]["mode_of_work"], "task_execution");
    assert_eq!(lines[0]["governance_context_hash"], "");
    assert_eq!(lines[0]["governance_context_available"], true);
    assert_eq!(lines[1]["completed_at"], "2026-01-01T00:00:00Z");
    assert_eq!(lines[1]["outcome"], "abandoned");
    assert_eq!(lines[1]["closed_by"], "agent");
    assert_eq!(
        lines[2],
        serde_json::json!({"event": "link", "target": "x"})
    );
    assert_eq!(
        std::fs::read_to_string(ops.join("ops-index.jsonl")).expect("index"),
        "{\"keep\": true}\n"
    );
    assert!(!op_record::detect(root).expect("second"));
}

fn lane(lane_id: &str) -> serde_json::Value {
    serde_json::json!({"lane_id": lane_id, "wp_ids": [], "write_scope": [], "predicted_surfaces": [], "depends_on_lanes": [], "parallel_group": 1})
}

fn lanes_manifest(lanes: Vec<serde_json::Value>) -> String {
    serde_json::json!({"version": 1, "mission_slug": "m", "mission_id": null, "mission_branch": "kitty/mission-m", "target_branch": "main", "lanes": lanes, "computed_at": "2026-01-01T00:00:00+00:00", "computed_from": "test"}).to_string()
}

#[test]
fn verdict_provenance_backfill_appends_the_source_event() {
    use sk_install::migrations::completion_modern::mission_spine;
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let mission = root.join("kitty-specs/mission-alpha");
    write(
        &mission.join("meta.json"),
        &serde_json::json!({"mission_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "mission_slug": "mission-alpha"}).to_string(),
    );
    write(
        &mission.join("status.events.jsonl"),
        &format!(
            "{}\n",
            serde_json::json!({"event_id": "01ARZ3NDEKTSV4RRFFQ69G5FAA", "mission_slug": "mission-alpha", "wp_id": "WP01", "from_lane": "planned", "to_lane": "claimed", "at": "2026-01-01T00:00:00+00:00", "actor": "claude", "force": false, "execution_mode": "worktree", "reason": null, "review_ref": null})
        ),
    );
    write(
        &mission.join("status.json"),
        &serde_json::json!({"mission_slug": "mission-alpha", "work_packages": {}}).to_string(),
    );
    write(
        &mission.join("tasks/WP01-alpha/review-cycle-1.md"),
        "---\ncycle_number: 1\nwp_id: WP01\nmission_slug: mission-alpha\nreviewer_agent: reviewer-renata\nreviewed_at: '2026-01-02T00:00:00+00:00'\nverdict: rejected\n---\n\nrejected body\n",
    );
    write(
        &mission.join("tasks/WP01-alpha/review-cycle-2.md"),
        "---\ncycle_number: 2\nwp_id: WP01\nmission_slug: mission-alpha\nreviewer_agent: reviewer-renata\nreviewed_at: '2026-01-03T00:00:00+00:00'\nverdict: approved\n---\n\napproved body\n",
    );
    assert!(mission_spine::detect_verdict_provenance_backfill(root).expect("detect"));
    assert!(
        mission_spine::can_apply_verdict_provenance_backfill(root)
            .expect("can_apply")
            .is_none()
    );
    let outcome = mission_spine::apply_verdict_provenance_backfill(root).expect("apply");
    assert_eq!(
        outcome.changes,
        vec!["Backfilled 1 stranded verdict event(s) across 1 mission(s) scanned"]
    );
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(mission.join("status.events.jsonl"))
        .expect("rows")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    let reviews: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|row| {
            row.get("review_result")
                .is_some_and(|value| !value.is_null())
        })
        .collect();
    assert_eq!(reviews.len(), 1, "{rows:?}");
    let event = reviews[0];
    assert_eq!(event["event_id"], "1SRG7HQ9W7CAB2ED2F37Q0G6DY");
    assert_eq!(event["mission_id"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    assert_eq!(event["mission_slug"], "mission-alpha");
    assert_eq!(event["wp_id"], "WP01");
    assert_eq!(event["from_lane"], "in_review");
    assert_eq!(event["to_lane"], "approved");
    assert_eq!(event["at"], "2026-01-03T00:00:00+00:00");
    assert_eq!(event["force"], false);
    assert_eq!(event["execution_mode"], "worktree");
    assert_eq!(
        event["actor"],
        serde_json::json!({"role": "system", "profile": null, "model": null, "tool": "migration:verdict_provenance_backfill"})
    );
    assert_eq!(
        event["review_result"],
        serde_json::json!({"reviewer": "reviewer-renata", "verdict": "approved", "reference": "review-cycle://mission-alpha/WP01-alpha/review-cycle-2.md"})
    );
    assert!(!mission_spine::detect_verdict_provenance_backfill(root).expect("second"));
}

#[test]
fn single_branch_restamp_skips_terminal_and_planning_only_missions() {
    use sk_install::migrations::completion_modern::mission_spine;
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    let stale = root.join("kitty-specs/stale-mission");
    write(
        &stale.join("meta.json"),
        &serde_json::json!({"mission_slug": "stale-mission", "topology": "single_branch"})
            .to_string(),
    );
    write(
        &stale.join("lanes.json"),
        &lanes_manifest(vec![lane("lane-planning"), lane("lane-a")]),
    );
    let done = root.join("kitty-specs/done-mission");
    write(
        &done.join("meta.json"),
        &serde_json::json!({"mission_slug": "done-mission", "topology": "single_branch", "merged_at": "2026-01-01T00:00:00+00:00"}).to_string(),
    );
    write(
        &done.join("lanes.json"),
        &lanes_manifest(vec![lane("lane-a")]),
    );
    let planning = root.join("kitty-specs/planning-only");
    write(
        &planning.join("meta.json"),
        &serde_json::json!({"mission_slug": "planning-only", "topology": "single_branch"})
            .to_string(),
    );
    write(
        &planning.join("lanes.json"),
        &lanes_manifest(vec![lane("lane-planning")]),
    );
    assert!(mission_spine::detect_single_branch_restamp(root).expect("detect"));
    let outcome = mission_spine::apply_single_branch_restamp(root).expect("apply");
    assert_eq!(
        outcome.changes,
        vec!["stale-mission: topology single_branch -> lanes"]
    );
    let stale_meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(stale.join("meta.json")).expect("meta"))
            .expect("json");
    assert_eq!(stale_meta["topology"], "lanes");
    let done_meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(done.join("meta.json")).expect("meta"))
            .expect("json");
    assert_eq!(done_meta["topology"], "single_branch");
    let planning_meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(planning.join("meta.json")).expect("meta"))
            .expect("json");
    assert_eq!(planning_meta["topology"], "single_branch");
    assert!(!mission_spine::detect_single_branch_restamp(root).expect("second"));
}
