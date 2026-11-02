mod support;

use std::fs;
use std::path::{Path, PathBuf};

use sk_mission::{
    CanonicalTemplate, MissionArtifactPlan, MissionArtifactRequest, MissionTemplateKind,
    MissionTemplates, MissionWriteObserver, TaskArtifactError, WorkPackageDraft,
};
use support::{temp_project, write};

const CREATED_AT: &str = "2026-11-02T10:00:00Z";
const SLUG: &str = "demo-mission-01KZ6G6H";

fn fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/oracle/templates")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("read {name}: {error}"))
}

fn template(kind: MissionTemplateKind, id: &str, name: &str) -> CanonicalTemplate {
    CanonicalTemplate::new(
        kind,
        id,
        PathBuf::from("tests/fixtures/oracle/templates").join(name),
        fixture(name),
    )
    .expect("canonical template")
}

fn templates() -> MissionTemplates {
    MissionTemplates::new(template(
        MissionTemplateKind::Spec,
        "research/spec-template.md",
        "spec-template.md",
    ))
    .with_plan(template(
        MissionTemplateKind::Plan,
        "research/plan-template.md",
        "plan-template.md",
    ))
    .with_tasks(template(
        MissionTemplateKind::Tasks,
        "research/tasks-template.md",
        "tasks-template.md",
    ))
    .with_work_package(template(
        MissionTemplateKind::WorkPackage,
        "research/task-prompt-template.md",
        "task-prompt-template.md",
    ))
}

fn request() -> MissionArtifactRequest {
    MissionArtifactRequest::new("main", "main", CREATED_AT)
        .with_work_package(
            WorkPackageDraft::new("WP01", "Foundation")
                .with_phase("Phase 1 - Setup")
                .with_subtasks(["T001", "T002"]),
        )
        .with_work_package(
            WorkPackageDraft::new("WP02", "API & UI")
                .with_phase("Phase 2 - Delivery")
                .with_dependencies(["WP01"])
                .with_subtasks(["T003"]),
        )
}

fn mission_dir(temp: &tempfile::TempDir) -> PathBuf {
    temp.path().join("kitty-specs").join(SLUG)
}

fn prepare(temp: &tempfile::TempDir) {
    let dir = mission_dir(temp);
    fs::create_dir_all(dir.join("tasks")).expect("mission dir");
}

#[test]
fn plan_and_commit_scaffold_spec_plan_tasks_and_work_packages_from_injected_templates() {
    let temp = temp_project();
    prepare(&temp);
    let plan = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates(),
        &request(),
    )
    .expect("plan");

    assert!(
        !mission_dir(&temp).join("plan.md").exists(),
        "planning writes nothing"
    );
    let receipt = plan.commit(temp.path()).expect("commit");

    let dir = mission_dir(&temp);
    for file in ["spec.md", "plan.md", "tasks.md", "tasks/README.md"] {
        assert!(dir.join(file).is_file(), "{file} scaffolded");
    }
    assert_eq!(
        fs::read_to_string(dir.join("plan.md")).unwrap(),
        fixture("plan-template.md")
    );
    assert_eq!(
        fs::read_to_string(dir.join("tasks.md")).unwrap(),
        fixture("tasks-template.md")
    );
    let wp = fs::read_to_string(dir.join("tasks/WP01-foundation.md")).unwrap();
    assert!(wp.starts_with("---\nwork_package_id: \"WP01\"\ntitle: \"Foundation\"\n"));
    assert!(wp.contains("dependencies: []\n"));
    assert!(wp.contains("planning_base_branch: \"main\""));
    assert!(wp.contains("merge_target_branch: \"main\""));
    assert!(wp.contains("branch_strategy: \"Planning artifacts were generated on main; completed changes must merge back into main.\""));
    assert!(wp.contains("subtasks:\n  - T001\n  - T002\n"));
    assert!(wp.contains("phase: \"Phase 1 - Setup\"\n"));
    assert!(wp.contains(&format!("  - at: \"{CREATED_AT}\"\n")));
    assert!(wp.contains("# Research Work Package: WP01 – Foundation\n"));
    assert!(
        wp.contains("## Review Feedback"),
        "the injected canonical template body is preserved, not replaced with invented prose"
    );
    assert!(!wp.contains("{{"), "no template placeholder ships");
    assert_eq!(receipt.created().len(), 6);
    assert!(receipt.preserved().is_empty());
}

#[test]
fn a_second_run_preserves_every_existing_artifact_and_is_idempotent() {
    let temp = temp_project();
    prepare(&temp);
    let plan = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates(),
        &request(),
    )
    .expect("plan");
    plan.commit(temp.path()).expect("first commit");

    let dir = mission_dir(&temp);
    write(&dir.join("spec.md"), "# User spec\n\nDo not replace.\n");
    write(
        &dir.join("tasks.md"),
        "## WP01 - User authored\n\nKeep me.\n",
    );
    let wp_before = fs::read_to_string(dir.join("tasks/WP01-foundation.md")).expect("wp");

    let retry = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates(),
        &request(),
    )
    .expect("retry plan");
    let receipt = retry.commit(temp.path()).expect("retry commit");

    assert!(receipt.created().is_empty());
    assert_eq!(
        fs::read_to_string(dir.join("spec.md")).unwrap(),
        "# User spec\n\nDo not replace.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("tasks.md")).unwrap(),
        "## WP01 - User authored\n\nKeep me.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("tasks/WP01-foundation.md")).unwrap(),
        wp_before,
        "existing work-package prompt is preserved"
    );
}

#[test]
fn an_existing_work_package_id_under_another_filename_refuses_without_writing() {
    let temp = temp_project();
    prepare(&temp);
    write(
        &mission_dir(&temp).join("tasks/WP01-operator-name.md"),
        "---\nwork_package_id: WP01\ntitle: Operator\ndependencies: []\n---\n\nOperator body.\n",
    );
    write(&mission_dir(&temp).join("tasks.md"), "## WP01 - Operator\n");
    let before = snapshot(&mission_dir(&temp));

    let error = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates(),
        &request(),
    )
    .expect_err("conflicting wp filename");
    assert!(matches!(
        error,
        TaskArtifactError::WorkPackageIdConflict { .. }
    ));
    assert_eq!(snapshot(&mission_dir(&temp)), before);
}

#[test]
fn duplicate_drafts_and_self_dependencies_are_refused_before_writing() {
    let temp = temp_project();
    prepare(&temp);
    let duplicate = MissionArtifactRequest::new("main", "main", CREATED_AT)
        .with_work_package(WorkPackageDraft::new("WP01", "One"))
        .with_work_package(WorkPackageDraft::new("WP01", "Two"));
    let error = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates(),
        &duplicate,
    )
    .expect_err("duplicate draft");
    assert!(matches!(error, TaskArtifactError::DuplicateDraft { .. }));

    let self_dep = MissionArtifactRequest::new("main", "main", CREATED_AT)
        .with_work_package(WorkPackageDraft::new("WP01", "One").with_dependencies(["WP01"]));
    let error = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates(),
        &self_dep,
    )
    .expect_err("self dependency");
    assert!(matches!(error, TaskArtifactError::SelfDependency { .. }));
    assert!(!mission_dir(&temp).join("spec.md").exists());
}

#[test]
fn missing_or_empty_canonical_templates_are_refused() {
    let temp = temp_project();
    prepare(&temp);
    let only_spec = MissionTemplates::new(template(
        MissionTemplateKind::Spec,
        "research/spec-template.md",
        "spec-template.md",
    ));
    let error = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &only_spec,
        &request(),
    )
    .expect_err("missing plan/tasks/wp templates");
    assert!(matches!(error, TaskArtifactError::MissingTemplate { .. }));

    let empty = CanonicalTemplate::new(
        MissionTemplateKind::Spec,
        "empty",
        PathBuf::from("empty.md"),
        String::new(),
    )
    .expect_err("empty content is refused at construction");
    assert!(matches!(
        empty,
        sk_mission::TemplateError::EmptyContent { .. }
    ));
}

#[test]
fn unrendered_placeholders_are_refused_rather_than_shipped() {
    let temp = temp_project();
    prepare(&temp);
    let templates = templates().with_work_package(
        CanonicalTemplate::new(
            MissionTemplateKind::WorkPackage,
            "missing-context",
            PathBuf::from("wp.md"),
            "---\nwork_package_id: \"WPxx\"\n---\n\n{{unknown_placeholder}}\n",
        )
        .expect("template"),
    );
    let error = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates,
        &MissionArtifactRequest::new("main", "main", CREATED_AT)
            .with_work_package(WorkPackageDraft::new("WP01", "Foundation")),
    )
    .expect_err("unrendered placeholder");
    assert!(matches!(error, TaskArtifactError::Template { .. }));
    assert!(!mission_dir(&temp).join("spec.md").exists());
}

#[test]
fn a_partial_artifact_failure_rolls_back_every_created_file() {
    struct FailOnTasks;
    impl MissionWriteObserver for FailOnTasks {
        fn before_write(&mut self, path: &Path) -> std::io::Result<()> {
            if path.ends_with("tasks.md") {
                return Err(std::io::Error::other("injected failure"));
            }
            Ok(())
        }
    }

    let temp = temp_project();
    prepare(&temp);
    let dir = mission_dir(&temp);
    let plan = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &templates(),
        &request(),
    )
    .expect("plan");
    let mut observer = FailOnTasks;
    let error = plan
        .commit_with_observer(temp.path(), &mut observer)
        .expect_err("injected failure");
    assert!(matches!(error, TaskArtifactError::Write { .. }));
    assert!(!dir.join("spec.md").exists());
    assert!(!dir.join("plan.md").exists());
    assert!(!dir.join("tasks/WP01-foundation.md").exists());
}

#[test]
fn wp_filenames_are_canonical_and_reject_path_components() {
    let temp = temp_project();
    prepare(&temp);
    let draft = WorkPackageDraft::new("WP01", "API & UI: Endpoints / Auth");
    assert_eq!(draft.file_name().unwrap(), "WP01-api-ui-endpoints-auth.md");
    assert!(
        WorkPackageDraft::new("WP01", "x")
            .with_file_name("../escape.md")
            .is_err()
    );
    assert!(
        WorkPackageDraft::new("WP01", "x")
            .with_file_name("WP02-other.md")
            .is_err()
    );
    assert!(WorkPackageDraft::new("WP01", "").file_name().is_err());
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
                out.push(format!(
                    "file {relative} {:?}",
                    fs::read(&path).expect("read")
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}
