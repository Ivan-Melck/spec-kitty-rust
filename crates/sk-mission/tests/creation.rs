mod support;

use std::fs;
use std::path::{Path, PathBuf};

use sk_mission::{
    CanonicalTemplate, MissionAllocationState, MissionCreateError, MissionCreateOptions,
    MissionCreatePlan, MissionCreateRequest, MissionCreateSlug, MissionIdentity, MissionRetention,
    MissionTemplateKind, MissionTemplates, MissionTopology, PriorMissionActivity, list_missions,
    select_mission,
};
use support::{meta_json, mission_dir, temp_project, write, write_mission};

const ID: &str = "01KZ6G6H4R8Y9J2N4Q7V3B6M1F";
const CREATED_AT: &str = "2026-11-02T10:00:00Z";

fn oracle(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/oracle")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn spec_template() -> CanonicalTemplate {
    CanonicalTemplate::new(
        MissionTemplateKind::Spec,
        "research/spec-template.md",
        PathBuf::from("tests/fixtures/oracle/templates/spec-template.md"),
        oracle("templates/spec-template.md"),
    )
    .expect("canonical spec template")
}

fn templates() -> MissionTemplates {
    MissionTemplates::new(spec_template())
}

fn default_request() -> MissionCreateRequest {
    MissionCreateRequest::new("demo-mission", "main")
        .expect("request")
        .with_friendly_name("Demo Mission")
        .with_purpose("Demo Mission", "A demonstration mission.")
        .with_topology(MissionTopology::SingleBranch)
}

fn identity() -> MissionIdentity {
    MissionIdentity::new(ID, CREATED_AT).expect("identity")
}

fn plan(root: &Path) -> MissionCreatePlan {
    MissionCreatePlan::plan(
        root,
        default_request(),
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect("plan")
}

#[test]
fn plan_matches_the_python_baseline_meta_and_names_the_directory_canonically() {
    let temp = temp_project();
    let plan = plan(temp.path());

    assert_eq!(plan.mission_id().as_str(), ID);
    assert_eq!(plan.mid8(), "01KZ6G6H");
    assert_eq!(plan.directory_name().as_str(), "demo-mission-01KZ6G6H");
    assert_eq!(
        plan.mission_number(),
        None,
        "pre-merge missions have no number"
    );
    assert_eq!(
        plan.meta_json(),
        oracle("create/meta_single_branch.json"),
        "meta.json must match the canonical Python serialization byte-for-byte"
    );
    assert!(
        fs::read_dir(temp.path().join("kitty-specs"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true),
        "planning a Mission must not write anything"
    );
}

#[test]
fn friendly_name_and_purpose_defaults_match_the_python_baseline() {
    let temp = temp_project();
    let request = MissionCreateRequest::new("demo-mission", "main")
        .expect("request")
        .with_topology(MissionTopology::SingleBranch);
    let plan = MissionCreatePlan::plan(
        temp.path(),
        request,
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect("plan");
    assert_eq!(plan.meta_json(), oracle("create/meta_defaults.json"));
}

#[test]
fn plan_is_pure_and_commit_creates_the_planning_scaffold() {
    let temp = temp_project();
    let plan = plan(temp.path());

    assert!(!plan.spec_dir().exists());
    let receipt = plan.commit(temp.path()).expect("commit");

    assert!(plan.spec_dir().join("meta.json").is_file());
    assert!(plan.spec_dir().join("spec.md").is_file());
    assert!(plan.spec_dir().join("tasks/README.md").is_file());
    assert!(plan.spec_dir().join("tasks/.gitkeep").is_file());
    assert!(plan.spec_dir().join("checklists").is_dir());
    assert!(plan.spec_dir().join("research").is_dir());
    assert!(
        plan.spec_dir().join("status.events.jsonl").is_file(),
        "non-coordination scaffolds carry an empty canonical status log"
    );
    assert_eq!(
        fs::read_to_string(plan.spec_dir().join("spec.md")).unwrap(),
        oracle("templates/spec-template.md"),
        "spec.md is copied from the injected canonical template"
    );
    assert_eq!(
        fs::read_to_string(plan.spec_dir().join("tasks/README.md")).unwrap(),
        oracle("scaffold/tasks-readme-main.md")
    );
    assert_eq!(
        receipt.allocation(),
        MissionAllocationState::PlanningArtifactsOnly,
        "a filesystem plan is never an allocated or executable Mission"
    );
    assert_eq!(receipt.created().len(), receipt.created().len());
    assert!(receipt.preserved().is_empty());

    // The created Mission is discoverable and selectable by identity, never by
    // its future display number.
    let missions = list_missions(temp.path()).expect("inventory");
    assert_eq!(missions.len(), 1);
    let mission = select_mission(&missions, ID).expect("full id");
    assert_eq!(mission.slug().as_str(), "demo-mission-01KZ6G6H");
    assert_eq!(mission.mid8(), "01KZ6G6H");
    assert!(select_mission(&missions, "demo-mission").is_err());
    assert!(select_mission(&missions, "1").is_err());
}

#[test]
fn committing_the_same_plan_twice_is_idempotent() {
    let temp = temp_project();
    let plan = plan(temp.path());
    plan.commit(temp.path()).expect("first commit");

    let second = plan
        .commit(temp.path())
        .expect("second commit is a repair no-op");
    assert!(second.created().is_empty());
    assert!(
        second
            .unchanged()
            .iter()
            .any(|path| path.ends_with("meta.json")),
        "identical meta.json is unchanged, not rewritten"
    );
    assert!(
        second
            .unchanged()
            .iter()
            .any(|path| path.ends_with("spec.md")),
        "existing identical template output is unchanged, not rewritten"
    );
}

#[test]
fn a_live_same_key_prior_refuses_without_writing() {
    let temp = temp_project();
    write_mission(
        temp.path(),
        "demo-mission-01AAAAAA",
        "01AAAAAAAAAAAAAAAAAAAAAAAA",
    );
    let before = snapshot(temp.path());

    let error = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("live duplicate");
    assert!(matches!(error, MissionCreateError::AlreadyExists { .. }));
    assert_eq!(snapshot(temp.path()), before, "refusal strands nothing");
}

#[test]
fn a_prior_of_a_different_mission_type_is_not_a_duplicate() {
    let temp = temp_project();
    let prior = mission_dir(temp.path(), "demo-mission-01AAAAAA");
    write(
        &prior.join("meta.json"),
        &meta_json(
            "01AAAAAAAAAAAAAAAAAAAAAAAA",
            "demo-mission-01AAAAAA",
            None,
            "single_branch",
        )
        .replace("\"software-dev\"", "\"research\""),
    );

    let plan = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect("different mission type is a different key");
    plan.commit(temp.path()).expect("commit");
}

#[test]
fn allow_duplicate_permits_a_second_live_same_key_mission() {
    let temp = temp_project();
    write_mission(
        temp.path(),
        "demo-mission-01AAAAAA",
        "01AAAAAAAAAAAAAAAAAAAAAAAA",
    );

    let options = MissionCreateOptions::new().allow_duplicate(true);
    let plan = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &options,
    )
    .expect("duplicate allowed");
    plan.commit(temp.path()).expect("commit");
    assert_eq!(list_missions(temp.path()).unwrap().len(), 2);
}

#[test]
fn an_identity_collision_is_refused_even_with_allow_duplicate() {
    let temp = temp_project();
    write_mission(temp.path(), "other-01KZ6G6H", ID);

    let options = MissionCreateOptions::new().allow_duplicate(true);
    let error = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &options,
    )
    .expect_err("mission id collision");
    assert!(matches!(
        error,
        MissionCreateError::MissionIdCollision { .. }
    ));
}

#[test]
fn an_existing_unrelated_directory_refuses_without_writing() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "demo-mission-01KZ6G6H");
    write(&dir.join("notes.md"), "operator notes\n");
    let before = snapshot(temp.path());

    let error = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("directory without a Mission is not repairable");
    assert!(matches!(error, MissionCreateError::DirectoryExists { .. }));
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn an_abandoned_prior_is_repaired_in_place_preserving_custom_metadata_and_files() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "demo-mission-01KZ6G6H");
    write(
        &dir.join("meta.json"),
        r#"{
  "mission_id": "01KZ6G6H4R8Y9J2N4Q7V3B6M1F",
  "mid8": "01KZ6G6H",
  "mission_number": 7,
  "slug": "legacy-demo",
  "mission_slug": "legacy-demo",
  "friendly_name": "Operator Title",
  "mission_type": "software-dev",
  "target_branch": "release/2",
  "created_at": "2025-01-01T00:00:00Z",
  "custom_extension": {"keep": ["verbatim", 42]},
  "review_playbook": {"notes": "preserve me"}
}
"#,
    );
    write(
        &dir.join("spec.md"),
        "# Operator authored spec\n\nDo not overwrite.\n",
    );
    write(&dir.join("tasks/README.md"), "# Custom tasks readme\n");
    write(&dir.join("tasks/notes.md"), "custom\n");

    let options = MissionCreateOptions::new().mark_abandoned([ID.parse().expect("mission id")]);
    let plan = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &options,
    )
    .expect("repair plan");

    assert_eq!(
        plan.meta_json(),
        oracle("create/meta_repair_preserves_existing.json")
    );
    let receipt = plan.commit(temp.path()).expect("repair commit");

    assert_eq!(
        fs::read_to_string(dir.join("spec.md")).unwrap(),
        "# Operator authored spec\n\nDo not overwrite.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("tasks/README.md")).unwrap(),
        "# Custom tasks readme\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("tasks/notes.md")).unwrap(),
        "custom\n"
    );
    assert!(
        receipt
            .preserved()
            .iter()
            .any(|path| path.ends_with("spec.md"))
    );

    let missions = list_missions(temp.path()).expect("inventory");
    assert_eq!(missions.len(), 1);
    assert_eq!(missions[0].mission_number(), Some(7));
}

#[test]
fn an_abandoned_mark_does_not_authorize_rewriting_recorded_routing() {
    for (field, changed) in [
        ("topology", "lanes"),
        ("coordination_branch", "kitty/coord-recorded"),
    ] {
        let temp = temp_project();
        let plan = plan(temp.path());
        plan.commit(temp.path()).unwrap();
        let path = plan.spec_dir().join("meta.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        meta[field] = serde_json::json!(changed);
        fs::write(&path, serde_json::to_vec_pretty(&meta).unwrap()).unwrap();
        let before = snapshot(temp.path());
        let options = MissionCreateOptions::new().mark_abandoned([ID.parse().unwrap()]);
        let error = MissionCreatePlan::plan(
            temp.path(),
            default_request(),
            identity(),
            &templates(),
            &options,
        )
        .unwrap_err();
        assert!(
            matches!(error, MissionCreateError::IdentityConflict { .. }),
            "{error:?}"
        );
        assert_eq!(snapshot(temp.path()), before);
    }
}

#[test]
fn a_live_target_directory_is_never_repaired_without_an_abandoned_mark() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "demo-mission-01KZ6G6H");
    write(
        &dir.join("meta.json"),
        &meta_json(ID, "demo-mission-01KZ6G6H", None, "single_branch"),
    );

    let error = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("live target");
    assert!(matches!(error, MissionCreateError::AlreadyExists { .. }));
}

#[test]
fn corrupt_prior_metadata_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "broken");
    write(&dir.join("meta.json"), "{not json");
    let before = snapshot(temp.path());

    let error = MissionCreatePlan::plan(
        temp.path(),
        default_request(),
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("corrupt inventory fails closed");
    assert!(matches!(error, MissionCreateError::Inventory { .. }));
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn coordination_topologies_require_the_workspace_owner_branch_and_never_seed_status_locally() {
    let temp = temp_project();
    let mut request = default_request().with_topology(MissionTopology::Coord);
    let error = MissionCreatePlan::plan(
        temp.path(),
        request.clone(),
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("coordination branch is required");
    assert!(matches!(
        error,
        MissionCreateError::CoordinationBranchRequired { .. }
    ));

    request = request.with_coordination_branch("kitty/mission-demo-mission-01KZ6G6H");
    let plan = MissionCreatePlan::plan(
        temp.path(),
        request,
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect("coord plan");
    plan.commit(temp.path()).expect("coord commit");

    assert!(
        !plan.spec_dir().join("status.events.jsonl").exists(),
        "coordination-routed status lives on the coordination surface"
    );
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(plan.spec_dir().join("meta.json")).unwrap())
            .expect("meta json");
    assert_eq!(
        meta["coordination_branch"],
        "kitty/mission-demo-mission-01KZ6G6H"
    );
    assert_eq!(meta["topology"], "coord");
}

#[test]
fn a_non_coordination_mission_refuses_a_supplied_coordination_branch() {
    let temp = temp_project();
    let request = default_request().with_coordination_branch("kitty/mission-unexpected");
    let error = MissionCreatePlan::plan(
        temp.path(),
        request,
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("unexpected coordination branch");
    assert!(matches!(
        error,
        MissionCreateError::UnexpectedCoordinationBranch { .. }
    ));
}

#[test]
fn retain_and_binding_flags_are_written_only_when_true_and_match_the_oracle() {
    let temp = temp_project();
    let request = MissionCreateRequest::new("demo-mission", "main")
        .expect("request")
        .with_friendly_name("Demo Mission")
        .with_purpose("Research Mission", "Research context.")
        .with_mission_type("research")
        .with_topology(MissionTopology::Lanes)
        .with_retention(MissionRetention {
            pr_bound: true,
            retain_branches: true,
            retain_worktrees: true,
            commit_to_target: false,
        });
    let plan = MissionCreatePlan::plan(
        temp.path(),
        request,
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect("flags plan");
    assert_eq!(
        plan.meta_json(),
        oracle("create/meta_lanes_with_flags.json")
    );
}

#[test]
fn commit_to_target_is_only_valid_for_single_branch() {
    let temp = temp_project();
    let request = default_request()
        .with_topology(MissionTopology::Lanes)
        .with_retention(MissionRetention {
            commit_to_target: true,
            ..MissionRetention::default()
        });
    let error = MissionCreatePlan::plan(
        temp.path(),
        request,
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("commit_to_target is single_branch-only");
    assert!(matches!(
        error,
        MissionCreateError::CommitToTargetRequiresSingleBranch { .. }
    ));
}

#[test]
fn invalid_slugs_branches_titles_and_purpose_are_refused_before_any_write() {
    let temp = temp_project();
    for slug in [
        "User-Auth",
        "user_auth",
        "-leading",
        "trailing-",
        "two--hyphens",
        "",
    ] {
        assert!(
            MissionCreateSlug::parse(slug).is_err(),
            "{slug:?} is not a kebab-case mission slug"
        );
    }
    assert!(MissionCreateSlug::parse("068-feature").is_ok());
    assert!(MissionCreateSlug::parse("069").is_ok());

    let before = snapshot(temp.path());
    let bad_requests = [
        default_request().with_friendly_name("   "),
        default_request().with_purpose("", "context"),
        default_request().with_purpose(" \t\n ", "context"),
        default_request().with_purpose("tldr", "   "),
        default_request().with_mission_type("../escape"),
    ];
    for request in bad_requests {
        assert!(
            MissionCreatePlan::plan(
                temp.path(),
                request,
                identity(),
                &templates(),
                &MissionCreateOptions::new(),
            )
            .is_err()
        );
    }
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn a_stale_plan_refuses_when_the_target_appears_before_commit() {
    let temp = temp_project();
    let plan = plan(temp.path());
    let dir = mission_dir(temp.path(), "demo-mission-01KZ6G6H");
    write(
        &dir.join("meta.json"),
        &meta_json(
            "01AAAAAAAAAAAAAAAAAAAAAAAA",
            "demo-mission-01KZ6G6H",
            None,
            "single_branch",
        ),
    );

    let error = plan.commit(temp.path()).expect_err("stale plan");
    assert!(matches!(error, MissionCreateError::StalePlan { .. }));
}

#[test]
fn a_partial_commit_failure_rolls_back_every_artifact_it_created() {
    use sk_mission::MissionWriteObserver;

    struct FailOnReadme;
    impl MissionWriteObserver for FailOnReadme {
        fn before_write(&mut self, path: &Path) -> std::io::Result<()> {
            if path.ends_with("tasks/README.md") {
                return Err(std::io::Error::other("injected failure"));
            }
            Ok(())
        }
    }

    let temp = temp_project();
    let plan = plan(temp.path());
    let mut observer = FailOnReadme;
    let error = plan
        .commit_with_observer(temp.path(), &mut observer)
        .expect_err("injected failure");

    assert!(matches!(error, MissionCreateError::Write { .. }));
    assert!(
        !plan.spec_dir().exists(),
        "rollback removes the mission directory created by this commit"
    );
    let specs = temp.path().join("kitty-specs");
    assert!(
        !specs.exists() || fs::read_dir(&specs).unwrap().next().is_none(),
        "no stranded mission artifact"
    );
}

#[test]
fn private_or_absolute_slug_components_are_rejected_as_requests() {
    for slug in ["../escape", "a/b", "a\\b", "..", "."] {
        assert!(MissionCreateSlug::parse(slug).is_err());
    }
}

#[test]
fn duplicate_activity_defaults_to_live_and_abandoned_is_opt_in() {
    assert_eq!(PriorMissionActivity::default(), PriorMissionActivity::Live);
}

#[test]
fn the_reference_topology_default_is_coord_and_requires_the_owner_branch() {
    let request = MissionCreateRequest::new("demo-mission", "main").expect("request");
    assert_eq!(request.topology(), MissionTopology::Coord);
    let temp = temp_project();
    let error = MissionCreatePlan::plan(
        temp.path(),
        request,
        identity(),
        &templates(),
        &MissionCreateOptions::new(),
    )
    .expect_err("coordination branch must be supplied by the workspace owner");
    assert!(matches!(
        error,
        MissionCreateError::CoordinationBranchRequired { .. }
    ));
}

fn snapshot(root: &Path) -> Vec<String> {
    fn walk(path: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries {
            let entry = entry.expect("entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("contained")
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                out.push(format!("dir {relative}"));
                walk(&path, root, out);
            } else {
                let bytes = fs::read(&path).expect("read");
                out.push(format!("file {relative} {} {:?}", bytes.len(), bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}
