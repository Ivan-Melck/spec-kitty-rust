//! CF7: the canonical `expected-artifacts.yaml` reader and name projection.
//!
//! These are Native-side contracts against real files in the pinned pack tree
//! (and small synthetic manifests for the failure classes). They are NOT a
//! Source differential campaign: the Source oracle run is listed as outstanding
//! in the task report.

use std::fs;
use std::path::{Path, PathBuf};

use sk_mission::PyYamlSafe;
use sk_mission::expected_artifacts::{
    ExpectedArtifactRoots, ExpectedArtifactsError, acceptance_required_artifact_names,
    configured_artifact_name, load_expected_artifacts,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn packaged_missions() -> PathBuf {
    repo_root().join("packs").join("built-in").join("missions")
}

fn fixture(contents: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("org");
    let mission = root.join("missions").join("custom");
    fs::create_dir_all(&mission).expect("mission dir");
    fs::write(mission.join("expected-artifacts.yaml"), contents).expect("write manifest");
    (temp, root)
}

#[test]
fn shipped_software_dev_manifest_projects_the_acceptance_trio() {
    let decoder = PyYamlSafe;
    let path = packaged_missions();
    assert!(
        path.join("software-dev")
            .join("expected-artifacts.yaml")
            .is_file(),
        "the pinned pack tree must carry the software-dev manifest at {}",
        path.display()
    );

    let names = acceptance_required_artifact_names(
        ExpectedArtifactRoots {
            org_roots: &[],
            packaged_missions_root: Some(&path),
        },
        &decoder,
    )
    .expect("manifest loads")
    .expect("manifest present");
    assert_eq!(names, vec!["spec.md", "plan.md", "tasks.md"]);

    let manifest = load_expected_artifacts(
        "software-dev",
        ExpectedArtifactRoots {
            org_roots: &[],
            packaged_missions_root: Some(&path),
        },
        &decoder,
    )
    .expect("loads")
    .expect("present");
    assert_eq!(manifest.mission_type, "software-dev");
    assert!(manifest.step_ids().contains(&"specify"));
    // `output.tasks.list` is declared at `tasks_finalize`, not at `plan`.
    assert_eq!(
        manifest.artifact_name("output.tasks.list"),
        Some("tasks.md")
    );
    assert_eq!(manifest.artifact_name("input.spec.main"), Some("spec.md"));
    // Every optional row projects too (research.md/quickstart.md/...).
    assert_eq!(
        manifest.artifact_name("evidence.research"),
        Some("research.md")
    );
}

#[test]
fn org_tier_wins_whole_file_and_the_last_existing_match_wins() {
    let decoder = PyYamlSafe;
    let (_first_temp, first) = fixture(&manifest_text("custom", "spec.md"));
    let (_second_temp, second) = fixture(&manifest_text("custom", "custom-spec.md"));
    // A declared-but-absent root contributes nothing.
    let absent = PathBuf::from("/nonexistent-org-root-cf7");
    let org_roots = vec![first, absent, second];
    let manifest = load_expected_artifacts(
        "custom",
        ExpectedArtifactRoots {
            org_roots: &org_roots,
            packaged_missions_root: None,
        },
        &decoder,
    )
    .expect("loads")
    .expect("present");
    assert_eq!(
        manifest.artifact_name("input.spec.main"),
        Some("custom-spec.md")
    );
    // Whole-file precedence: the built-in tree is never consulted.
    let names = configured_artifact_name(
        "custom",
        "input.spec.main",
        ExpectedArtifactRoots {
            org_roots: &org_roots,
            packaged_missions_root: Some(&packaged_missions()),
        },
        &decoder,
    )
    .expect("resolves")
    .expect("present");
    assert_eq!(names, "custom-spec.md");
}

#[test]
fn absence_on_every_tier_is_none_not_an_error() {
    let decoder = PyYamlSafe;
    let loaded = load_expected_artifacts(
        "no-such-mission-type",
        ExpectedArtifactRoots {
            org_roots: &[],
            packaged_missions_root: Some(&packaged_missions()),
        },
        &decoder,
    )
    .expect("absence is not an error");
    assert!(loaded.is_none());
}

#[test]
fn a_schema_invalid_manifest_is_reported_not_treated_as_absent() {
    let decoder = PyYamlSafe;
    let (_temp, root) = fixture(
        "schema_version: '1.0'\nmission_type: custom\nrequired_always: []\nrequired_by_step: {}\noptional_always: []\ntypo_key: nope\n",
    );
    let error = load_expected_artifacts(
        "custom",
        ExpectedArtifactRoots {
            org_roots: std::slice::from_ref(&root),
            packaged_missions_root: None,
        },
        &decoder,
    )
    .expect_err("extra keys are forbidden");
    match error {
        ExpectedArtifactsError::Schema { detail, .. } => {
            assert!(detail.contains("typo_key"), "{detail}");
        }
        other => panic!("expected a schema failure, got {other:?}"),
    }
}

#[test]
fn a_bad_artifact_class_and_a_non_string_pattern_refuse() {
    let decoder = PyYamlSafe;
    let (_temp, root) = fixture(
        "schema_version: '1.0'\nmission_type: custom\nrequired_by_step:\n  specify:\n    - artifact_key: input.spec.main\n      artifact_class: nonsense\n      path_pattern: spec.md\n",
    );
    let error = load_expected_artifacts(
        "custom",
        ExpectedArtifactRoots {
            org_roots: std::slice::from_ref(&root),
            packaged_missions_root: None,
        },
        &decoder,
    )
    .expect_err("bad artifact_class");
    assert!(matches!(error, ExpectedArtifactsError::Schema { .. }));

    let (_temp2, root2) = fixture(
        "schema_version: '1.0'\nmission_type: custom\nrequired_by_step:\n  specify:\n    - artifact_key: input.spec.main\n      artifact_class: input\n      path_pattern: 7\n",
    );
    let error = load_expected_artifacts(
        "custom",
        ExpectedArtifactRoots {
            org_roots: std::slice::from_ref(&root2),
            packaged_missions_root: None,
        },
        &decoder,
    )
    .expect_err("non-string pattern");
    assert!(matches!(error, ExpectedArtifactsError::Schema { .. }));
}

#[test]
fn a_missing_projected_key_is_reported_with_the_key_name() {
    let decoder = PyYamlSafe;
    let (_temp, root) = fixture(&manifest_text("custom", "spec.md"));
    let error = configured_artifact_name(
        "custom",
        "output.plan.main",
        ExpectedArtifactRoots {
            org_roots: std::slice::from_ref(&root),
            packaged_missions_root: None,
        },
        &decoder,
    )
    .expect_err("the key is not projected");
    match error {
        ExpectedArtifactsError::MissingArtifactKey { artifact_key, .. } => {
            assert_eq!(artifact_key, "output.plan.main");
        }
        other => panic!("expected a missing-key failure, got {other:?}"),
    }
}

#[test]
fn an_unsafe_mission_type_refuses_before_any_read() {
    let decoder = PyYamlSafe;
    let error = load_expected_artifacts("../escape", ExpectedArtifactRoots::default(), &decoder)
        .expect_err("unsafe segment");
    assert!(matches!(
        error,
        ExpectedArtifactsError::UnsafeMissionType { .. }
    ));
}

fn manifest_text(mission_type: &str, spec_pattern: &str) -> String {
    format!(
        "schema_version: '1.0'\nmission_type: {mission_type}\nrequired_by_step:\n  specify:\n    - artifact_key: input.spec.main\n      artifact_class: input\n      path_pattern: {spec_pattern}\n      blocking: true\n"
    )
}
