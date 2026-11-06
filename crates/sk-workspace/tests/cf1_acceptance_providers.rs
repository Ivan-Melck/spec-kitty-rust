//! CF1: the acceptance collector composes the **canonical Mission definition**
//! (resolved `mission.yaml`) into its facts.
//!
//! These are the basic valid cases plus the error smoke the CF1 order asks for:
//! a `software-dev` Mission, a `research` Mission with its deliverables prefix,
//! a Mission that declares no conventions, a project `path_conventions` remap,
//! the `meta.json` contracts waiver, the declared optional-artifact set, and a
//! malformed override section failing closed. Deep differential/adversarial
//! proof (custom expected-artifact manifests, registered sparse org mission
//! types, approval-window candidate roots through a real event log, Unicode
//! tokens) is enumerated as open verification debt in the task report.

mod support;

use std::path::{Path, PathBuf};

use sk_mission::{DefinitionProviders, RegisteredMissionType};
use sk_status::acceptance::source::{
    PathConventionFacts, SourceAcceptanceOptions, SourceWpFacts, evaluate_source_acceptance,
};
use sk_workspace::acceptance_source::declared_artifacts::FALLBACK_OPTIONAL_ARTIFACTS;
use sk_workspace::acceptance_source::{
    AcceptanceCollectionOptions, MatrixInput, approved_lane_source_roots,
    collect_source_acceptance_facts, collect_source_acceptance_facts_with,
};
use sk_workspace::lanes::{ExecutionLane, LanesManifest};

use sk_workspace::WorkspaceError;

use support::{Fixture, commit_all, init_repo, write};

const SLUG: &str = "001-cf1-accept";

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Mission-definition providers pinned to a temporary "packaged" tree, so a test
/// never depends on the installed pack tree or on the process environment.
struct TestProviders {
    packaged: PathBuf,
    org: Vec<PathBuf>,
}

impl DefinitionProviders for TestProviders {
    fn org_pack_roots(&self, _project_root: &Path) -> Vec<PathBuf> {
        self.org.clone()
    }

    fn registered_mission_type(
        &self,
        _project_root: &Path,
        _name: &str,
    ) -> Option<RegisteredMissionType> {
        None
    }

    fn packaged_missions_root(&self, _project_root: &Path) -> Option<PathBuf> {
        Some(self.packaged.clone())
    }

    fn kittify_home(&self, _project_root: &Path) -> Option<PathBuf> {
        None
    }
}

const SOFTWARE_DEV_MISSION_YAML: &str = r#"name: "Software Dev Kitty"
description: "software-dev acceptance fixture"
version: "1.0.0"
domain: "software"
workflow:
  phases:
    - name: "implement"
      description: "implement"
artifacts:
  required:
    - spec.md
    - plan.md
    - tasks.md
  optional:
    - data-model.md
    - contracts/
    - quickstart.md
    - research.md
    - checklists/
paths:
  workspace: "src/"
  tests: "tests/"
  deliverables: "contracts/"
  documentation: "docs/"
"#;

const RESEARCH_MISSION_YAML: &str = r#"name: "Deep Research Kitty"
description: "research acceptance fixture"
version: "1.0.0"
domain: "research"
workflow:
  phases:
    - name: "gather"
      description: "gather"
artifacts:
  required:
    - spec.md
    - plan.md
    - tasks.md
    - findings.md
  optional:
    - sources/
    - data/
    - literature-review.md
paths:
  workspace: "research/"
  data: "data/"
  deliverables: "findings/"
  documentation: "reports/"
"#;

const NO_CONVENTIONS_MISSION_YAML: &str = r#"name: "Bare Kitty"
description: "declares no path conventions"
version: "1.0.0"
domain: "other"
workflow:
  phases:
    - name: "write"
      description: "write"
artifacts:
  required: [spec.md, plan.md, tasks.md]
  optional: [notes.md]
paths: {}
"#;

/// A project root with `.kittify/`, a Mission directory and its planning trio.
fn project(fixture: &Fixture, mission_type: &str) -> PathBuf {
    write(&fixture.path(".kittify").join("keep"), "");
    let feature_dir = write_mission_meta(fixture.root(), SLUG, mission_type);
    std::fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
    write(&feature_dir.join("spec.md"), "# Spec\n");
    write(&feature_dir.join("plan.md"), "# Plan\n");
    write(&feature_dir.join("tasks.md"), "- [x] T001\n");
    feature_dir
}

/// `kitty-specs/<slug>/meta.json` with an explicit `mission_type`.
fn write_mission_meta(root: &Path, slug: &str, mission_type: &str) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    let meta = serde_json::json!({
        "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5011",
        "mission_slug": slug,
        "topology": "lanes",
        "target_branch": "main",
        "mission_type": mission_type,
    });
    write(
        &dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).expect("meta")),
    );
    dir
}

/// Write a packaged `missions/<type>/mission.yaml` and return the providers.
fn packaged(fixture: &Fixture, mission_type: &str, yaml: &str) -> TestProviders {
    let root = fixture.path("packaged-missions").join("missions");
    write(&root.join(mission_type).join("mission.yaml"), yaml);
    TestProviders {
        packaged: root,
        org: Vec::new(),
    }
}

fn load(root: &Path) -> sk_mission::Mission {
    sk_mission::list_missions(root)
        .expect("mission inventory")
        .into_iter()
        .find(|mission| mission.slug().as_str() == SLUG)
        .expect("mission present")
}

fn collect(
    fixture: &Fixture,
    providers: &TestProviders,
) -> sk_status::acceptance::source::SourceAcceptanceFacts {
    let mission = load(fixture.root());
    collect_source_acceptance_facts_with(
        fixture.root(),
        &mission,
        MatrixInput::NotRequired,
        AcceptanceCollectionOptions {
            providers: Some(providers),
            ..AcceptanceCollectionOptions::default()
        },
    )
    .expect("facts")
}

fn evaluated(
    facts: &sk_status::acceptance::source::SourceAcceptanceFacts,
) -> (
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Vec<String>,
) {
    match &facts.path_conventions {
        PathConventionFacts::Evaluated {
            satisfied,
            blocking_text,
            warning_text,
            notice,
            dedup_tokens,
        } => (
            *satisfied,
            blocking_text.clone(),
            warning_text.clone(),
            notice.clone(),
            dedup_tokens.clone(),
        ),
        other => panic!("expected an evaluated path-convention fact, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// software-dev: declared paths + declared optional artifacts
// ---------------------------------------------------------------------------

#[test]
fn software_dev_declared_paths_and_optional_artifacts_drive_the_facts() {
    let fixture = init_repo();
    let _feature_dir = project(&fixture, "software-dev");
    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    let (satisfied, blocking, warning, notice, dedup) = evaluated(&facts);
    assert!(!satisfied, "the declared build paths do not exist yet");
    assert!(warning.is_none());
    assert!(notice.is_none());
    assert_eq!(dedup, vec!["contracts".to_owned()]);
    let blocking = blocking.expect("blocking text");
    assert!(blocking.contains("workspace path: src"), "{blocking}");
    assert!(blocking.contains("tests path: tests"), "{blocking}");
    assert!(blocking.contains("documentation path: docs"), "{blocking}");
    // Required artifacts are Source's configured trio, not `artifacts.required`.
    assert!(facts.missing_required_artifacts.is_empty());
    // Optional artifacts come from the resolved definition, in authored order.
    assert_eq!(
        facts.missing_optional_artifacts,
        vec![
            "data-model.md".to_owned(),
            "contracts".to_owned(),
            "quickstart.md".to_owned(),
            "research.md".to_owned(),
            "checklists".to_owned(),
        ],
        "the declared optional set replaces Source's fallback list"
    );

    // The policy keeps the one strict-only relaxation and applies the dedup: the
    // waived/declared `contracts` token leaves the optional warning.
    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert_eq!(summary.path_violations.len(), 1, "{summary:?}");
    assert!(summary.path_violations[0].contains("src"), "{summary:?}");
    assert_eq!(
        summary.optional_missing,
        vec![
            "data-model.md".to_owned(),
            "quickstart.md".to_owned(),
            "research.md".to_owned(),
            "checklists".to_owned(),
        ]
    );
    assert!(
        summary
            .warnings
            .iter()
            .any(|warning| warning == "Path conventions not satisfied."),
        "{summary:?}"
    );
}

#[test]
fn creating_the_declared_paths_satisfies_the_check() {
    let fixture = init_repo();
    let feature_dir = project(&fixture, "software-dev");
    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    for path in ["src", "tests", "docs"] {
        write(&fixture.path(path).join("keep"), "");
    }
    // `deliverables: contracts/` is artifact-routed: it lives on the Mission
    // surface, never the repository root.
    write(&feature_dir.join("contracts").join("keep"), "");
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    let (satisfied, blocking, warning, notice, dedup) = evaluated(&facts);
    assert!(satisfied, "blocking: {blocking:?}");
    assert!(blocking.is_none());
    assert!(warning.is_none());
    assert!(notice.is_none());
    assert!(dedup.is_empty());

    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert!(summary.path_violations.is_empty(), "{summary:?}");
    assert_eq!(
        summary.optional_missing,
        vec![
            "data-model.md".to_owned(),
            "quickstart.md".to_owned(),
            "research.md".to_owned(),
            "checklists".to_owned(),
        ]
    );
}

// ---------------------------------------------------------------------------
// research: the deliverables prefix
// ---------------------------------------------------------------------------

#[test]
fn a_research_mission_evaluates_its_deliverables_prefix() {
    let fixture = init_repo();
    let _feature_dir = project(&fixture, "research");
    let providers = packaged(&fixture, "research", RESEARCH_MISSION_YAML);
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    let (satisfied, blocking, _, _, _) = evaluated(&facts);
    assert!(!satisfied);
    let blocking = blocking.expect("blocking text");
    let prefix = format!("docs/research/{SLUG}/");
    assert!(
        blocking.contains(&format!("{prefix}research")),
        "{blocking}"
    );
    assert!(blocking.contains(&format!("{prefix}data")), "{blocking}");
    assert!(
        blocking.contains(&format!("{prefix}findings")),
        "{blocking}"
    );
    assert!(blocking.contains(&format!("{prefix}reports")), "{blocking}");

    // The research Mission's own optional set, not the software-dev fallback.
    assert_eq!(
        facts.missing_optional_artifacts,
        vec![
            "sources".to_owned(),
            "data".to_owned(),
            "literature-review.md".to_owned()
        ]
    );

    // Create the prefixed deliverables and the check clears.
    for path in ["research", "data", "findings", "reports"] {
        write(
            &fixture
                .path(format!("docs/research/{SLUG}/{path}"))
                .join("keep"),
            "",
        );
    }
    let facts = collect(&fixture, &providers);
    let (satisfied, blocking, _, _, _) = evaluated(&facts);
    assert!(satisfied, "blocking: {blocking:?}");
}

// ---------------------------------------------------------------------------
// No declared conventions vs. no resolved Mission
// ---------------------------------------------------------------------------

#[test]
fn a_mission_declaring_no_paths_is_a_no_op_but_keeps_its_own_artifacts() {
    let fixture = init_repo();
    project(&fixture, "bare-type");
    let providers = packaged(&fixture, "bare-type", NO_CONVENTIONS_MISSION_YAML);
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    assert_eq!(facts.path_conventions, PathConventionFacts::NotDeclared);
    assert_eq!(
        facts.missing_optional_artifacts,
        vec!["notes.md".to_owned()],
        "a declaration of nothing is not the fallback list"
    );
}

#[test]
fn an_unresolvable_mission_type_reports_source_caught_no_op() {
    let fixture = init_repo();
    let _ = project(&fixture, "not-a-mission-type");
    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    // Source: `except MissionError: mission = None` -> path check is a no-op and
    // the optional set falls back to Source's documented list.
    assert_eq!(facts.path_conventions, PathConventionFacts::NotDeclared);
    assert_eq!(
        facts.missing_optional_artifacts,
        FALLBACK_OPTIONAL_ARTIFACTS
            .iter()
            .map(|token| (*token).to_owned())
            .collect::<Vec<_>>()
    );
}

#[test]
fn no_project_root_means_no_mission_and_the_fallback_artifacts() {
    let fixture = init_repo();
    // No `.kittify/` anywhere under the fixture: Source's ancestor walk finds no
    // project root and raises `MissionNotFoundError`, which it catches.
    let feature_dir = write_mission_meta(fixture.root(), SLUG, "software-dev");
    std::fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
    write(&feature_dir.join("spec.md"), "# Spec\n");
    commit_all(fixture.root(), "fixture");

    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    let facts = collect(&fixture, &providers);
    assert_eq!(facts.path_conventions, PathConventionFacts::NotDeclared);
    assert_eq!(
        facts.missing_optional_artifacts.len(),
        FALLBACK_OPTIONAL_ARTIFACTS.len()
    );
}

// ---------------------------------------------------------------------------
// Project override and contracts waiver
// ---------------------------------------------------------------------------

#[test]
fn a_project_path_convention_override_remaps_a_declared_path() {
    let fixture = init_repo();
    let feature_dir = project(&fixture, "software-dev");
    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    write(
        &fixture.path(".kittify").join("config.yaml"),
        "project:\n  path_conventions:\n    workspace: \"internal/\"\n",
    );
    for path in ["tests", "docs"] {
        write(&fixture.path(path).join("keep"), "");
    }
    write(&feature_dir.join("contracts").join("keep"), "");
    // Only the override target exists; the authored `src/` does not.
    write(&fixture.path("internal").join("keep"), "");
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    let (satisfied, blocking, _, _, _) = evaluated(&facts);
    assert!(satisfied, "blocking: {blocking:?}");

    // Removing the remapped directory blocks again, and the text names the
    // override, not the authored value.
    std::fs::remove_dir_all(fixture.path("internal")).expect("remove internal");
    let facts = collect(&fixture, &providers);
    let (satisfied, blocking, _, _, _) = evaluated(&facts);
    assert!(!satisfied);
    let blocking = blocking.expect("blocking text");
    assert!(blocking.contains("internal"), "{blocking}");
    assert!(!blocking.contains("path: src"), "{blocking}");
}

#[test]
fn the_meta_json_contracts_waiver_removes_the_declared_artifact() {
    let fixture = init_repo();
    let feature_dir = project(&fixture, "software-dev");
    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(feature_dir.join("meta.json")).unwrap())
            .expect("meta json");
    meta["contracts"] = serde_json::json!("none");
    meta["contracts_rationale"] = serde_json::json!("no HTTP surface");
    write(
        &feature_dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).unwrap()),
    );
    for path in ["src", "tests", "docs"] {
        write(&fixture.path(path).join("keep"), "");
    }
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    let (satisfied, blocking, _, notice, dedup) = evaluated(&facts);
    assert!(satisfied, "blocking: {blocking:?}");
    assert!(
        notice
            .as_deref()
            .is_some_and(|notice| notice.contains("contracts/ requirement waived by meta.json")),
        "{notice:?}"
    );
    assert_eq!(dedup, vec!["contracts".to_owned()]);

    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert!(
        !summary
            .optional_missing
            .iter()
            .any(|entry| entry == "contracts"),
        "{summary:?}"
    );
    assert!(
        summary
            .warnings
            .iter()
            .any(|warning| warning.contains("contracts/ requirement waived by meta.json")),
        "{summary:?}"
    );
}

// ---------------------------------------------------------------------------
// Required artifacts and failure paths
// ---------------------------------------------------------------------------

#[test]
fn missing_required_artifacts_are_the_pinned_software_dev_trio() {
    let fixture = init_repo();
    let feature_dir = project(&fixture, "software-dev");
    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    std::fs::remove_file(feature_dir.join("plan.md")).expect("remove plan.md");
    commit_all(fixture.root(), "fixture");

    let facts = collect(&fixture, &providers);
    assert_eq!(facts.missing_required_artifacts, vec!["plan.md".to_owned()]);
}

#[test]
fn a_malformed_project_override_section_fails_closed() {
    let fixture = init_repo();
    let _ = project(&fixture, "software-dev");
    let providers = packaged(&fixture, "software-dev", SOFTWARE_DEV_MISSION_YAML);
    write(
        &fixture.path(".kittify").join("config.yaml"),
        "project:\n  path_conventions:\n    not_a_path_key: \"elsewhere/\"\n",
    );
    commit_all(fixture.root(), "fixture");

    let mission = load(fixture.root());
    let error = collect_source_acceptance_facts_with(
        fixture.root(),
        &mission,
        MatrixInput::NotRequired,
        AcceptanceCollectionOptions {
            providers: Some(&providers),
            ..AcceptanceCollectionOptions::default()
        },
    )
    .expect_err("a malformed override section is a Source exception, not a verdict");
    match error {
        WorkspaceError::InvalidArtifact { artifact, detail } => {
            assert_eq!(artifact, "mission path conventions");
            assert!(detail.contains("Unknown path-convention key"), "{detail}");
        }
        other => panic!("expected a typed invalid-artifact refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Candidate source roots (approved lanes)
// ---------------------------------------------------------------------------

#[test]
fn approved_lane_worktrees_are_the_only_candidate_source_roots() {
    let fixture = init_repo();
    let manifest = LanesManifest {
        version: LanesManifest::VERSION,
        mission_slug: SLUG.to_owned(),
        mission_id: Some("01KNXQS9ATWWFXS3K5ZJ9E5011".to_owned()),
        mission_branch: format!("kitty/mission-{SLUG}"),
        target_branch: "main".to_owned(),
        lanes: vec![
            ExecutionLane {
                lane_id: "lane-approved".to_owned(),
                wp_ids: vec!["WP01".parse().expect("wp id")],
                write_scope: vec!["src/**".to_owned()],
                predicted_surfaces: vec!["test".to_owned()],
                depends_on_lanes: Vec::new(),
                parallel_group: 0,
            },
            ExecutionLane {
                lane_id: "lane-open".to_owned(),
                wp_ids: vec!["WP02".parse().expect("wp id")],
                write_scope: vec!["docs/**".to_owned()],
                predicted_surfaces: vec!["docs".to_owned()],
                depends_on_lanes: Vec::new(),
                parallel_group: 0,
            },
            ExecutionLane {
                lane_id: "lane-planning".to_owned(),
                wp_ids: Vec::new(),
                write_scope: Vec::new(),
                predicted_surfaces: Vec::new(),
                depends_on_lanes: Vec::new(),
                parallel_group: 0,
            },
        ],
        computed_at: "2026-01-01T00:00:00Z".to_owned(),
        computed_from: "cf1 fixture".to_owned(),
        planning_artifact_wps: Vec::new(),
        planning_commit_sha: None,
        collapse_report: None,
    };
    let approved_wt = fixture
        .path(".worktrees")
        .join(format!("{SLUG}-lane-approved"));
    std::fs::create_dir_all(&approved_wt).expect("approved worktree");
    let open_wt = fixture.path(".worktrees").join(format!("{SLUG}-lane-open"));
    std::fs::create_dir_all(&open_wt).expect("open worktree");

    let work_packages = vec![
        wp_fact("WP01", Some("approved")),
        wp_fact("WP02", Some("in_progress")),
    ];
    let roots = approved_lane_source_roots(fixture.root(), Some(&manifest), &work_packages);
    assert_eq!(roots, vec![approved_wt.clone()]);

    // No approved lane at all -> no roots (an unapproved lane can never satisfy
    // a declared build path).
    let roots = approved_lane_source_roots(
        fixture.root(),
        Some(&manifest),
        &[wp_fact("WP01", Some("for_review"))],
    );
    assert!(roots.is_empty());

    // A corrupt/missing manifest contributes nothing, exactly like Source's
    // `CorruptLanesError` guard.
    let roots = approved_lane_source_roots(fixture.root(), None, &work_packages);
    assert!(roots.is_empty());
}

fn wp_fact(wp_id: &str, lane: Option<&str>) -> SourceWpFacts {
    SourceWpFacts {
        wp_id: wp_id.to_owned(),
        title: String::new(),
        path: format!("kitty-specs/{SLUG}/tasks/{wp_id}.md"),
        lane: lane.map(str::to_owned),
        agent: None,
        implementer_of_record: None,
        assignee: None,
        shell_pid: None,
        has_operator_provenance: false,
        cancellation_reason: None,
        cancellation_actor: None,
        cancellation_at: None,
    }
}

// ---------------------------------------------------------------------------
// Production composition (installed pack tree)
// ---------------------------------------------------------------------------

#[test]
fn the_installed_composition_resolves_the_packaged_software_dev_definition() {
    // The shipped `packs/built-in/missions/software-dev/mission.yaml` is the
    // Native analogue of Source's `_packaged_missions_dir()`. This asserts the
    // real composition the CLI will use, not a test-only provider set.
    let packaged_root = sk_kernel::get_package_asset_root()
        .expect("the pinned built-in pack tree must be fetched (make sync-packs)");
    assert!(
        packaged_root
            .join("software-dev")
            .join("mission.yaml")
            .is_file()
    );

    let fixture = init_repo();
    project(&fixture, "software-dev");
    commit_all(fixture.root(), "fixture");

    let mission = load(fixture.root());
    let facts = collect_source_acceptance_facts(fixture.root(), &mission, MatrixInput::NotRequired)
        .expect("facts");
    match facts.path_conventions {
        PathConventionFacts::Evaluated {
            satisfied,
            ref blocking_text,
            ..
        } => {
            assert!(!satisfied, "{blocking_text:?}");
            let blocking = blocking_text.as_deref().expect("blocking text");
            assert!(blocking.contains("workspace path: src"), "{blocking}");
        }
        ref other => panic!("expected the packaged definition to evaluate, got {other:?}"),
    }
    assert_eq!(
        facts.missing_optional_artifacts,
        vec![
            "data-model.md".to_owned(),
            "contracts".to_owned(),
            "quickstart.md".to_owned(),
            "research.md".to_owned(),
            "checklists".to_owned(),
        ]
    );
}
