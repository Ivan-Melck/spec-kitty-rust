//! CF13: non-WP (`plan`-family) runtime acceptance facts and their policy verdict.
//!
//! These drive the REAL collector ([`collect_source_acceptance_facts_with`]) and
//! the REAL Source-compatible policy
//! ([`sk_status::acceptance::source::evaluate_source_acceptance`]) over a fresh
//! Git fixture. The only injected piece is the read-only runtime-facts port
//! (Source's `read_runtime_acceptance_facts`), because the run store lives in
//! `sk-run-store`; the CLI composition implements that port over the canonical
//! `RunStore` (see `sk-cli`'s `compat::acceptance::providers`).
//!
//! Native functional contracts; the pinned-Source differential run over the same
//! fixtures is listed as outstanding in the task report.

mod support;

use std::path::{Path, PathBuf};

use sk_mission::{DefinitionProviders, Mission, RegisteredMissionType, list_missions};
use sk_status::acceptance::source::{
    LanesManifestFacts, MatrixEvidence, PathConventionFacts, SourceAcceptanceOptions,
    evaluate_source_acceptance,
};
use sk_workspace::acceptance_source::{
    AcceptanceCollectionOptions, MatrixInput, NonWpRuntimeFacts, NonWpRuntimeFactsProvider,
    collect_source_acceptance_facts_with,
};
use sk_workspace::error::WorkspaceError;

use support::{commit_all, init_repo, mission_id, write};
const SLUG: &str = "013-plan-mission";

/// The shipped `plan` mission files, compiled in so the fixture is hermetic.
const PLAN_MISSION_YAML: &str = include_str!("../../../packs/built-in/missions/plan/mission.yaml");
const PLAN_EXPECTED_ARTIFACTS: &str =
    include_str!("../../../packs/built-in/missions/plan/expected-artifacts.yaml");

/// A provider set with an isolated packaged tree (no org packs, no global home).
struct TestProviders {
    packaged: PathBuf,
}

impl DefinitionProviders for TestProviders {
    fn org_pack_roots(&self, _project_root: &Path) -> Vec<PathBuf> {
        Vec::new()
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

/// A recorded runtime, returned verbatim by the fake port.
struct FakeRuntime {
    facts: Option<NonWpRuntimeFacts>,
}

impl NonWpRuntimeFactsProvider for FakeRuntime {
    fn runtime_acceptance_facts(
        &self,
        _mission_slug: &str,
    ) -> Result<Option<NonWpRuntimeFacts>, WorkspaceError> {
        Ok(self.facts.clone())
    }
}

struct Fixture {
    _repo: support::Fixture,
    root: PathBuf,
    packaged: PathBuf,
}

impl Fixture {
    fn new(manifest: &str) -> Self {
        let repo = init_repo();
        let root = repo.root().to_path_buf();
        // Source finds the project root by walking up to `.kittify`.
        std::fs::create_dir_all(root.join(".kittify")).expect("kittify dir");
        let packaged = root.join("packaged-missions");
        write(&packaged.join("plan/mission.yaml"), PLAN_MISSION_YAML);
        write(&packaged.join("plan/expected-artifacts.yaml"), manifest);
        let feature_dir = root.join("kitty-specs").join(SLUG);
        std::fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
        write(
            &feature_dir.join("meta.json"),
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({
                    "mission_id": mission_id(13),
                    "mission_slug": SLUG,
                    "topology": "lanes",
                    "target_branch": "main",
                    "mission_type": "plan",
                }))
                .unwrap()
            ),
        );
        commit_all(&root, "plan mission");
        Self {
            _repo: repo,
            root,
            packaged,
        }
    }

    fn mission(&self) -> Mission {
        list_missions(&self.root)
            .expect("mission inventory")
            .into_iter()
            .find(|mission| mission.slug().as_str() == SLUG)
            .expect("mission present")
    }

    fn feature_dir(&self) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG)
    }

    fn write_artifact(&self, name: &str) {
        write(&self.feature_dir().join(name), "# artifact\n");
    }

    fn facts(
        &self,
        runtime: Option<NonWpRuntimeFacts>,
    ) -> sk_status::acceptance::source::SourceAcceptanceFacts {
        let providers = TestProviders {
            packaged: self.packaged.clone(),
        };
        let runtime_port = FakeRuntime { facts: runtime };
        collect_source_acceptance_facts_with(
            &self.root,
            &self.mission(),
            MatrixInput::NotRequired,
            AcceptanceCollectionOptions {
                acceptance: SourceAcceptanceOptions::default(),
                providers: Some(&providers),
                decoder: None,
                required_artifacts: None,
                non_wp_runtime: Some(&runtime_port),
            },
        )
        .expect("facts")
    }
}

fn complete_runtime() -> NonWpRuntimeFacts {
    NonWpRuntimeFacts {
        mission_family: "plan".to_owned(),
        step_ids: vec![
            "specify".to_owned(),
            "research".to_owned(),
            "plan".to_owned(),
            "review".to_owned(),
        ],
        completed_step_ids: vec![
            "specify".to_owned(),
            "research".to_owned(),
            "plan".to_owned(),
            "review".to_owned(),
        ],
        issued_step_id: None,
        blocked_reason: None,
        pending_decision_ids: Vec::new(),
    }
}

#[test]
fn a_complete_artifact_only_runtime_is_judged_on_its_declared_artifacts() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    fixture.write_artifact("spec.md");
    fixture.write_artifact("research.md");
    fixture.write_artifact("plan.md");
    commit_all(&fixture.root, "planning artifacts");

    let facts = fixture.facts(Some(complete_runtime()));
    // The lane gates are NOT run: no lanes.json is read or required, and the
    // summary carries Source's one legitimate skip reason.
    match &facts.lanes_manifest {
        LanesManifestFacts::NotRequired { skip_reason, .. } => {
            assert_eq!(skip_reason, "planning_artifact_only")
        }
        other => panic!("expected the non-WP skip, got {other:?}"),
    }
    assert!(facts.work_packages.is_empty());
    assert!(facts.missing_required_artifacts.is_empty());
    assert_eq!(facts.matrix, MatrixEvidence::NotRequired);

    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert!(
        summary.activity_issues.is_empty(),
        "a complete runtime has no activity issues: {:?}",
        summary.activity_issues
    );
    assert!(
        summary.blocked_checks.is_empty(),
        "{:?}",
        summary.blocked_checks
    );
    assert!(
        summary.skipped_checks.is_empty(),
        "the non-WP branch records no skipped matrix checks: {:?}",
        summary.skipped_checks
    );
    assert_eq!(
        summary.acceptance_matrix_gate_skip_reason.as_deref(),
        Some("planning_artifact_only")
    );
    assert_eq!(summary.matrix_verdict, None);
    assert!(summary.missing_artifacts.is_empty());
    assert_eq!(
        summary
            .lanes
            .iter()
            .map(|(lane, ids)| (lane.as_str(), ids.len()))
            .collect::<Vec<_>>(),
        vec![
            ("planned", 0),
            ("claimed", 0),
            ("in_progress", 0),
            ("for_review", 0),
            ("in_review", 0),
            ("approved", 0),
            ("done", 0),
            ("blocked", 0),
            ("canceled", 0),
        ]
    );
}

#[test]
fn an_incomplete_runtime_blocks_and_names_the_remaining_steps() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    fixture.write_artifact("spec.md");
    fixture.write_artifact("research.md");
    fixture.write_artifact("plan.md");
    let mut runtime = complete_runtime();
    runtime.completed_step_ids = vec!["specify".to_owned()];
    let facts = fixture.facts(Some(runtime));

    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert!(
        summary.activity_issues.iter().any(|issue| issue
            == "Non-WP runtime is not complete; remaining steps: plan, research, review"),
        "{:?}",
        summary.activity_issues
    );
    assert!(!summary.ok());
}

#[test]
fn an_outstanding_decision_or_blocked_runtime_is_not_complete() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    fixture.write_artifact("spec.md");
    fixture.write_artifact("research.md");
    fixture.write_artifact("plan.md");
    let mut runtime = complete_runtime();
    runtime.pending_decision_ids = vec!["DM-0001".to_owned()];
    let facts = fixture.facts(Some(runtime));
    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert!(
        summary.activity_issues.iter().any(|issue| issue
            == "Non-WP runtime is not complete; remaining steps: pending runtime decision/result"),
        "{:?}",
        summary.activity_issues
    );
}

#[test]
fn a_runtime_without_a_review_action_refuses_acceptance() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    fixture.write_artifact("spec.md");
    fixture.write_artifact("research.md");
    fixture.write_artifact("plan.md");
    let mut runtime = complete_runtime();
    runtime.step_ids = vec![
        "specify".to_owned(),
        "research".to_owned(),
        "plan".to_owned(),
    ];
    runtime.completed_step_ids = runtime.step_ids.clone();
    let facts = fixture.facts(Some(runtime));
    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert!(
        summary
            .activity_issues
            .iter()
            .any(|issue| issue == "Plan runtime has no review action; acceptance refused"),
        "{:?}",
        summary.activity_issues
    );
}

#[test]
fn a_declared_artifact_missing_from_the_mission_dir_is_reported_missing() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    fixture.write_artifact("spec.md");
    fixture.write_artifact("plan.md");
    // `research.md` is a declared blocking artifact of the `research` step.
    let facts = fixture.facts(Some(complete_runtime()));
    assert_eq!(
        facts.missing_required_artifacts,
        vec!["research.md".to_owned()]
    );
    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert_eq!(summary.missing_artifacts, vec!["research.md".to_owned()]);
    assert!(!summary.ok());
    assert!(
        summary
            .recommended_fix_order
            .iter()
            .any(|fix| fix.contains("Restore required mission artifacts")),
        "{:?}",
        summary.recommended_fix_order
    );
}

#[test]
fn a_manifest_with_no_blocking_artifact_rows_refuses_acceptance() {
    // The step keys stay the runtime's action ids, but no row is blocking.
    let manifest = "schema_version: '1.0'\nmission_type: 'plan'\nmanifest_version: '1'\nrequired_always: []\nrequired_by_step:\n  specify: []\n  research: []\n  plan: []\n  review: []\noptional_always: []\n";
    let fixture = Fixture::new(manifest);
    let facts = fixture.facts(Some(complete_runtime()));
    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert!(
        summary.activity_issues.iter().any(|issue| issue
            == "Non-WP runtime has no declared blocking artifacts; acceptance refused"),
        "{:?}",
        summary.activity_issues
    );
}

#[test]
fn a_work_package_task_file_keeps_the_work_package_path() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    write(
        &fixture.feature_dir().join("tasks/WP01-a.md"),
        "---\nwork_package_id: WP01\ntitle: A\n---\n",
    );
    let facts = fixture.facts(Some(complete_runtime()));
    // The non-WP branch did not answer, so the collector read lanes.json as the
    // WP path does — and a MISSING manifest is the WP path's own blocking fact.
    match &facts.lanes_manifest {
        LanesManifestFacts::Missing { message } => {
            assert!(message.contains("lanes.json is required"))
        }
        other => panic!("expected the work-package path's missing manifest, got {other:?}"),
    }
}

#[test]
fn a_runtime_with_wp_iteration_keeps_the_work_package_path() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    let mut runtime = complete_runtime();
    runtime.step_ids.push("implement".to_owned());
    runtime.completed_step_ids.push("implement".to_owned());
    let facts = fixture.facts(Some(runtime));
    assert!(matches!(
        facts.lanes_manifest,
        LanesManifestFacts::Missing { .. }
    ));
}

#[test]
fn no_recorded_runtime_keeps_the_work_package_path() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    let facts = fixture.facts(None);
    assert!(matches!(
        facts.lanes_manifest,
        LanesManifestFacts::Missing { .. }
    ));
}

#[test]
fn a_runtime_for_another_mission_family_is_a_typed_refusal() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    let mut runtime = complete_runtime();
    runtime.mission_family = "research".to_owned();
    let providers = TestProviders {
        packaged: fixture.packaged.clone(),
    };
    let runtime_port = FakeRuntime {
        facts: Some(runtime),
    };
    let error = collect_source_acceptance_facts_with(
        &fixture.root,
        &fixture.mission(),
        MatrixInput::NotRequired,
        AcceptanceCollectionOptions {
            acceptance: SourceAcceptanceOptions::default(),
            providers: Some(&providers),
            decoder: None,
            required_artifacts: None,
            non_wp_runtime: Some(&runtime_port),
        },
    )
    .expect_err("a mismatched runtime refuses");
    assert!(
        matches!(
            &error,
            WorkspaceError::InvalidArtifact { artifact, detail }
                if *artifact == "mission runtime"
                    && detail.contains("Mission type does not match its recorded runtime")
        ),
        "{error:?}"
    );
}

#[test]
fn a_software_dev_mission_never_takes_the_non_wp_branch() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    let feature_dir = fixture.feature_dir();
    write(
        &feature_dir.join("meta.json"),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "mission_id": mission_id(14),
                "mission_slug": SLUG,
                "topology": "lanes",
                "target_branch": "main",
                "mission_type": "software-dev",
            }))
            .unwrap()
        ),
    );
    let facts = fixture.facts(Some(complete_runtime()));
    assert!(matches!(
        facts.lanes_manifest,
        LanesManifestFacts::Missing { .. }
    ));
    // The mission type is still reported as recorded.
    assert_eq!(facts.mission_type.as_deref(), Some("software-dev"));
}

#[test]
fn the_non_wp_branch_still_resolves_path_conventions_from_the_definition() {
    let fixture = Fixture::new(PLAN_EXPECTED_ARTIFACTS);
    fixture.write_artifact("spec.md");
    fixture.write_artifact("research.md");
    fixture.write_artifact("plan.md");
    let facts = fixture.facts(Some(complete_runtime()));
    // The plan mission declares `paths: {workspace: docs/, deliverables: docs/}`,
    // so the definition really resolved — the path fact is evaluated, not
    // "not declared", and a fresh mission without those directories is a real
    // (strict) violation rather than a silent pass.
    assert!(matches!(
        facts.path_conventions,
        PathConventionFacts::Evaluated { .. }
    ));
}
