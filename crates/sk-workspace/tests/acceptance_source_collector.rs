//! Collector tests for `sk_workspace::acceptance_source`.
//!
//! These assert the *evidence*, not the verdict: every fact is read through the
//! canonical service (lanes store, status snapshot, mission frontmatter, Git
//! seam, matrix model), and the genuinely-missing Source inputs are reported as
//! explicit gaps rather than guessed.
//!
//! Source receipts for the shapes asserted here are the oracle runs recorded in
//! the task report (`collect_feature_summary` over fresh Git fixtures under
//! CPython 3.11.15).

mod support;

use std::path::Path;

use sk_mission::{Mission, list_missions};
use sk_status::acceptance::source::{
    LanesManifestFacts, MatrixEvidence, MetaTargetBranch, PathConventionFacts,
};
use sk_workspace::acceptance_source::{
    FALLBACK_OPTIONAL_ARTIFACTS, MatrixInput, collect_source_acceptance_facts,
};
use sk_workspace::lanes::{ExecutionLane, LanesManifest, LanesStore};

use support::{commit_all, init_repo, mission_id, write, write_mission};

const SLUG: &str = "001-accept-collector";

fn load(root: &Path) -> Mission {
    list_missions(root)
        .expect("mission inventory")
        .into_iter()
        .find(|mission| mission.slug().as_str() == SLUG)
        .expect("mission present")
}

fn write_lanes(feature_dir: &Path, lane_id: &str, wp_ids: &[&str]) {
    let manifest = LanesManifest {
        version: LanesManifest::VERSION,
        mission_slug: SLUG.to_owned(),
        mission_id: Some(mission_id(7)),
        mission_branch: format!("kitty/mission-{SLUG}"),
        target_branch: "main".to_owned(),
        lanes: vec![ExecutionLane {
            lane_id: lane_id.to_owned(),
            wp_ids: wp_ids
                .iter()
                .map(|wp| wp.parse::<sk_kernel::WpId>().expect("wp id"))
                .collect(),
            write_scope: vec!["src/**".to_owned()],
            predicted_surfaces: vec!["test".to_owned()],
            depends_on_lanes: Vec::new(),
            parallel_group: 0,
        }],
        computed_at: "2026-01-01T00:00:00Z".to_owned(),
        computed_from: "collector test".to_owned(),
        planning_artifact_wps: Vec::new(),
        planning_commit_sha: None,
        collapse_report: None,
    };
    LanesStore::new(feature_dir)
        .write(&manifest)
        .expect("write lanes.json");
}

fn write_wp(feature_dir: &Path, body: &str) {
    write(&feature_dir.join("tasks").join("WP01.md"), body);
}

fn base_mission(root: &Path) -> std::path::PathBuf {
    let feature_dir = write_mission(root, SLUG, &mission_id(7), "lanes", "main");
    std::fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
    write(
        &feature_dir.join("spec.md"),
        "# Spec\n\n- FR-001: do the thing\n",
    );
    write(&feature_dir.join("plan.md"), "# Plan\n");
    write(&feature_dir.join("tasks.md"), "- [x] T001 done\n");
    feature_dir
}

#[test]
fn collects_source_facts_from_the_canonical_services() {
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    write_wp(
        &feature_dir,
        "---\nwork_package_id: WP01\ntitle: \"WP01 probe\"\nexecution_mode: code_change\nsubtasks: []\n---\n\n# WP01\n",
    );
    write_lanes(&feature_dir, "lane-a", &["WP01"]);
    commit_all(fixture.root(), "fixture");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::ReadOnly {
            directory: &feature_dir,
        },
    )
    .expect("facts");

    assert_eq!(facts.mission_slug, SLUG);
    assert_eq!(facts.mission_number, None);
    assert_eq!(
        facts.mission_type.as_deref(),
        Some("software-dev"),
        "mission type comes from meta.json"
    );
    assert_eq!(facts.branch.as_deref(), Some("main"));
    match &facts.lanes_manifest {
        LanesManifestFacts::Readable {
            target_branch,
            mission_branch,
            mission_has_code,
        } => {
            assert_eq!(target_branch, "main");
            assert_eq!(mission_branch, &format!("kitty/mission-{SLUG}"));
            assert!(*mission_has_code);
        }
        other => panic!("expected a readable manifest, got {other:?}"),
    }
    assert_eq!(
        facts.meta_target_branch,
        MetaTargetBranch::Declared("main".to_owned())
    );
    assert_eq!(facts.work_packages.len(), 1);
    let wp = &facts.work_packages[0];
    assert_eq!(wp.wp_id, "WP01");
    // The collector parses the authored YAML, so the surrounding quotes are
    // already gone here; the policy's `strip('"')` stays a no-op for this value
    // and still covers raw-text callers (Source receipt S1: "WP01 probe").
    assert_eq!(wp.title, "WP01 probe");
    assert_eq!(wp.path, format!("kitty-specs/{SLUG}/tasks/WP01.md"));
    // No status log in this fixture: the snapshot facts stay false (a genuine
    // absence the policy reports), never a fabricated empty-but-present state.
    assert!(!facts.status_events_present);
    assert!(!facts.status_snapshot_present);
    assert!(wp.lane.is_none());
    assert!(!wp.has_operator_provenance);
    assert_eq!(
        facts.tasks_md.as_deref(),
        Some("- [x] T001 done\n"),
        "the authored tasks.md text is carried verbatim"
    );
    // Every optional artifact Source's fallback lists and this fixture lacks.
    assert_eq!(
        facts.missing_optional_artifacts,
        FALLBACK_OPTIONAL_ARTIFACTS
            .iter()
            .map(|token| (*token).to_owned())
            .collect::<Vec<_>>()
    );
    assert!(facts.missing_required_artifacts.is_empty());
    // No `.kittify/` anywhere under this fixture, so Source's project-root walk
    // finds none and `get_mission_for_feature` raises `MissionNotFoundError`,
    // which Source catches (`mission = None`) — the path check is then a no-op.
    // The declaration is read from `mission.yaml` through the canonical resolver
    // (see `cf1_acceptance_providers.rs` for the resolved-definition cases).
    assert_eq!(facts.path_conventions, PathConventionFacts::NotDeclared);
    assert!(facts.git_dirty.is_empty(), "{:?}", facts.git_dirty);
    // The read-only leg found no matrix, so it reports the Source message.
    match &facts.matrix {
        MatrixEvidence::Missing { message, directory } => {
            assert!(message.contains("Acceptance matrix (acceptance-matrix.json) is required"));
            assert!(message.contains(&format!("--mission {SLUG}")));
            assert_eq!(
                directory.as_deref(),
                Some(feature_dir.display().to_string().as_str())
            );
        }
        other => panic!("expected a missing matrix, got {other:?}"),
    }
    // No `decisions/index.json` here: Source decodes a missing ledger as an EMPTY
    // index, so the statuses ARE available and every marker id is unknown
    // (blocking). Only a PRESENT ledger is undecodable in this build.
    assert!(facts.decision_statuses_available);
    assert!(facts.decision_statuses.is_empty());
}

#[test]
fn a_meta_json_artifact_declaration_is_not_a_source_input() {
    // Source resolves the optional-artifact set from the mission's `mission.yaml`
    // (`Mission.get_optional_artifacts`), never from `meta.json` — the key below
    // does not exist in any real mission's metadata. A present `quickstart.md`
    // therefore still leaves the OTHER Source fallback tokens missing.
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(feature_dir.join("meta.json")).unwrap())
            .expect("meta json");
    meta["artifacts"] = serde_json::json!({"optional": ["quickstart.md"]});
    write(
        &feature_dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).unwrap()),
    );
    write(&feature_dir.join("quickstart.md"), "# Quickstart\n");
    commit_all(fixture.root(), "fixture");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    assert_eq!(
        facts.missing_optional_artifacts,
        vec![
            "data-model.md".to_owned(),
            "research.md".to_owned(),
            "contracts".to_owned()
        ],
        "meta.json is not a Source authority for the declared artifact set"
    );
    assert_eq!(MatrixEvidence::NotRequired, facts.matrix);
}

#[test]
fn an_explicit_planning_execution_mode_keeps_a_planning_mission_code_free() {
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    write_wp(
        &feature_dir,
        "---\nwork_package_id: WP01\ntitle: \"planning\"\nexecution_mode: planning_artifact\n---\n",
    );
    write_lanes(&feature_dir, "lane-planning", &["WP01"]);
    commit_all(fixture.root(), "fixture");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    match facts.lanes_manifest {
        LanesManifestFacts::Readable {
            mission_has_code, ..
        } => assert!(
            !mission_has_code,
            "a planning-only mission must not be classified as code"
        ),
        other => panic!("expected a readable manifest, got {other:?}"),
    }
}

#[test]
fn corrupt_lanes_json_is_reported_as_corrupt_and_never_as_missing() {
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    write(&feature_dir.join("lanes.json"), "{not json");
    commit_all(fixture.root(), "fixture");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    match facts.lanes_manifest {
        LanesManifestFacts::Corrupt { message } => {
            assert!(message.starts_with("lanes.json at "));
            assert!(message.contains("is corrupt or malformed"));
            assert!(!message.contains("is required for"));
        }
        other => panic!("expected corruption, got {other:?}"),
    }
}

#[test]
fn an_absent_lanes_json_reports_the_source_remedy_message() {
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    commit_all(fixture.root(), "fixture");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    match facts.lanes_manifest {
        LanesManifestFacts::Missing { message } => {
            assert!(message.starts_with(&format!(
                "lanes.json is required for {}",
                feature_dir.display()
            )));
            assert!(message.contains("spec-kitty agent mission finalize-tasks"));
            assert!(message.contains("spec-kitty doctor mission-state --fix --mission <slug>"));
        }
        other => panic!("expected absence, got {other:?}"),
    }
}

#[test]
fn a_meta_json_paths_declaration_is_not_the_path_convention_authority() {
    // Source reads `mission.config.paths` from the resolved `mission.yaml`; the
    // `meta.json` keys below are invisible to it. The project-tier
    // `.kittify/missions/<type>/mission.yaml` is the resolver tier this fixture
    // can supply without an installed pack tree.
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    write(
        &fixture
            .path(".kittify")
            .join("missions")
            .join("software-dev")
            .join("mission.yaml"),
        "name: \"Software Dev Kitty\"\ndescription: \"fixture\"\nversion: \"1.0.0\"\ndomain: \"software\"\nworkflow:\n  phases:\n    - name: \"implement\"\n      description: \"implement\"\nartifacts:\n  required: [spec.md, plan.md, tasks.md]\n  optional: [notes.md]\npaths:\n  workspace: \"src/\"\n",
    );
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(feature_dir.join("meta.json")).unwrap())
            .expect("meta json");
    meta["paths"] = serde_json::json!({"workspace": "internal/", "tests": "spec/"});
    write(
        &feature_dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).unwrap()),
    );
    commit_all(fixture.root(), "fixture");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    match facts.path_conventions {
        PathConventionFacts::Evaluated {
            satisfied,
            ref blocking_text,
            ..
        } => {
            assert!(!satisfied);
            let blocking = blocking_text.as_deref().expect("blocking text");
            // The authored `mission.yaml` value is checked; the meta.json
            // `internal/`/`spec/` guesses are never probed.
            assert!(blocking.contains("workspace path: src"), "{blocking}");
        }
        other => panic!("expected the resolved definition to evaluate, got {other:?}"),
    }
    // The declared optional set is the definition's, not `meta.json`'s.
    assert_eq!(
        facts.missing_optional_artifacts,
        vec!["notes.md".to_owned()],
        "meta.json is not a Source authority for the declared artifact set"
    );
}

#[test]
fn this_missions_own_accept_artifacts_are_excluded_from_the_dirty_set() {
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    commit_all(fixture.root(), "fixture");
    // The accept pipeline's own convergence writes, uncommitted.
    write(
        &feature_dir.join("acceptance-matrix.json"),
        "{\"mission_slug\": \"001-accept-collector\"}",
    );
    write(&feature_dir.join("status.json"), "{}");
    write(&feature_dir.join("decisions").join("index.json"), "{}");
    // A real, unrelated change must still block.
    write(&fixture.root().join("src").join("lib.rs"), "fn main() {}\n");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    assert_eq!(facts.git_dirty.len(), 1, "{:?}", facts.git_dirty);
    // Git reports a brand-new `src/` as one collapsed untracked-directory entry,
    // rendered exactly as Source's `StatusEntry.display()` renders it (`XY path/`).
    assert_eq!(facts.git_dirty, vec!["?? src/".to_owned()]);
}

#[test]
fn the_append_only_status_log_is_never_treated_as_an_accept_own_write() {
    // `status.events.jsonl` shares the coarse `StatusState` artifact kind with the
    // daemon's `status.json` snapshot, but accept only READS it — a dirty one is
    // real uncommitted lane state and must still block on a flat mission.
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    commit_all(fixture.root(), "fixture");
    // An EMPTY event log is valid (no rows) and exists, so only the dirty gate
    // is under test here; a malformed row would be a different (real) refusal.
    write(&feature_dir.join("status.events.jsonl"), "");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    assert_eq!(
        facts.git_dirty,
        vec![format!("?? kitty-specs/{SLUG}/status.events.jsonl")]
    );
}

#[test]
fn coordination_residue_is_excluded_only_under_a_coordination_topology() {
    // The same untracked `traces/t1.md` is coordination residue under `coord`
    // (Source `S-dirty-coord` receipt) and real primary dirt under `lanes`
    // (`S-dirty-lanes`). The collector asks the canonical residue authority with
    // the Mission's STORED topology, so both verdicts come from one rule.
    for topology in ["lanes", "coord"] {
        let fixture = init_repo();
        let feature_dir = write_mission(fixture.root(), SLUG, &mission_id(7), topology, "main");
        std::fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
        write(&feature_dir.join("spec.md"), "# Spec\n");
        write(&feature_dir.join("plan.md"), "# Plan\n");
        write(&feature_dir.join("tasks.md"), "- [x] T001\n");
        commit_all(fixture.root(), "fixture");
        write(&feature_dir.join("traces").join("t1.md"), "trace\n");

        let facts = collect_source_acceptance_facts(
            fixture.root(),
            &load(fixture.root()),
            MatrixInput::NotRequired,
        )
        .expect("facts");
        let expected: Vec<String> = if topology == "coord" {
            Vec::new()
        } else {
            vec![format!("?? kitty-specs/{SLUG}/traces/")]
        };
        assert_eq!(facts.git_dirty, expected, "topology {topology}");
    }
}

#[test]
fn the_read_only_leg_reads_the_canonical_matrix() {
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    write_lanes(&feature_dir, "lane-a", &["WP01"]);
    write(
        &feature_dir.join("acceptance-matrix.json"),
        &serde_json::to_string_pretty(&serde_json::json!({
            "mission_slug": "001-accept-collector",
            "criteria": [{
                "criterion_id": "FR-001",
                "description": "FR-001 is satisfied and verified",
                "proof_type": "code_review",
                "pass_fail": "pass",
                "evidence": "WP01:review:WP01"
            }],
            "negative_invariants": []
        }))
        .expect("matrix json"),
    );
    commit_all(fixture.root(), "fixture");

    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::ReadOnly {
            directory: &feature_dir,
        },
    )
    .expect("facts");
    match &facts.matrix {
        MatrixEvidence::Judged {
            matrix,
            directory,
            negative_invariants_executed,
        } => {
            assert_eq!(matrix.mission_slug, SLUG);
            assert_eq!(matrix.overall_verdict(), "pass");
            assert_eq!(
                directory.as_deref(),
                Some(feature_dir.display().to_string().as_str())
            );
            assert!(
                !negative_invariants_executed,
                "the read-only leg must never claim it executed invariants"
            );
        }
        other => panic!("expected a judged matrix, got {other:?}"),
    }
}

#[test]
fn work_package_scalars_are_stringified_the_way_source_does() {
    let fixture = init_repo();
    let feature_dir = base_mission(fixture.root());
    write_wp(
        &feature_dir,
        "---\nwork_package_id: WP01\ntitle: \"probe\"\n---\n",
    );
    commit_all(fixture.root(), "fixture");
    let facts = collect_source_acceptance_facts(
        fixture.root(),
        &load(fixture.root()),
        MatrixInput::NotRequired,
    )
    .expect("facts");
    // No status log: no invented runtime slots.
    let wp = &facts.work_packages[0];
    assert!(wp.agent.is_none() && wp.assignee.is_none() && wp.shell_pid.is_none());
    assert!(wp.implementer_of_record.is_none());
}
