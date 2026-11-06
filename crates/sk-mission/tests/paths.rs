mod support;

use std::fs;
use std::path::{Path, PathBuf};

use sk_mission::{
    CanonicalTemplate, MissionPathError, MissionPaths, MissionTemplateKind, MissionTemplates,
    TemplateError, render_tasks_readme,
};
use support::{temp_project, write};

fn oracle(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/oracle")
            .join(name),
    )
    .expect("oracle fixture")
}

#[test]
fn canonical_paths_are_derived_from_the_project_root_and_validated_slug() {
    let temp = temp_project();
    let paths = MissionPaths::parse(temp.path(), "demo-mission-01KZ6G6H").expect("paths");

    assert_eq!(paths.root(), temp.path());
    assert_eq!(paths.specs_dir(), temp.path().join("kitty-specs"));
    assert_eq!(
        paths.spec_dir(),
        temp.path().join("kitty-specs/demo-mission-01KZ6G6H")
    );
    assert_eq!(paths.meta_file(), paths.spec_dir().join("meta.json"));
    assert_eq!(paths.spec_file(), paths.spec_dir().join("spec.md"));
    assert_eq!(paths.plan_file(), paths.spec_dir().join("plan.md"));
    assert_eq!(paths.tasks_file(), paths.spec_dir().join("tasks.md"));
    assert_eq!(paths.tasks_dir(), paths.spec_dir().join("tasks"));
    assert_eq!(
        paths.tasks_readme(),
        paths.spec_dir().join("tasks/README.md")
    );
    assert_eq!(
        paths.status_events_file(),
        paths.spec_dir().join("status.events.jsonl")
    );
    assert!(!paths.spec_dir().exists(), "composing paths writes nothing");
}

#[test]
fn invalid_slugs_and_missing_roots_are_refused() {
    let temp = temp_project();
    assert!(matches!(
        MissionPaths::parse(temp.path(), "../escape"),
        Err(MissionPathError::InvalidMissionSlug { .. })
    ));
    assert!(matches!(
        MissionPaths::parse(&temp.path().join("missing"), "demo"),
        Err(MissionPathError::RootNotDirectory { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_mission_directory_symlink_that_escapes_the_root_is_refused() {
    let temp = temp_project();
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), temp.path().join("kitty-specs/escape"))
        .expect("symlink");
    assert!(matches!(
        MissionPaths::parse(temp.path(), "escape"),
        Err(MissionPathError::Containment { .. })
    ));
}

#[test]
fn canonical_tasks_readme_rendering_matches_the_oracle_for_two_branches() {
    assert_eq!(
        render_tasks_readme("main"),
        oracle("scaffold/tasks-readme-main.md")
    );
    assert_eq!(
        render_tasks_readme("release/2"),
        oracle("scaffold/tasks-readme-release.md")
    );
}

#[test]
fn templates_expose_provenance_and_refuse_missing_kinds() {
    let spec = CanonicalTemplate::new(
        MissionTemplateKind::Spec,
        "research/spec-template.md",
        PathBuf::from("tests/fixtures/oracle/templates/spec-template.md"),
        oracle("templates/spec-template.md"),
    )
    .expect("template");
    assert_eq!(spec.id(), "research/spec-template.md");
    assert_eq!(spec.kind(), MissionTemplateKind::Spec);
    assert!(spec.content().starts_with("# Research Specification"));

    let templates = MissionTemplates::new(spec);
    assert!(matches!(
        templates.require(MissionTemplateKind::Plan),
        Err(TemplateError::Missing {
            kind: MissionTemplateKind::Plan
        })
    ));
}

#[test]
fn empty_templates_are_refused_at_construction() {
    let error = CanonicalTemplate::new(
        MissionTemplateKind::Plan,
        "empty",
        PathBuf::from("empty.md"),
        "   \n",
    )
    .expect_err("empty template");
    assert!(matches!(error, TemplateError::EmptyContent { .. }));

    let error = CanonicalTemplate::new(
        MissionTemplateKind::Plan,
        "  ",
        PathBuf::from("empty.md"),
        "content",
    )
    .expect_err("empty id");
    assert!(matches!(error, TemplateError::EmptyId { .. }));
}

#[test]
fn a_planning_artifact_is_never_confused_with_an_allocated_mission() {
    // The path type carries no lane/worktree/branch field at all; this test
    // pins the deliberate absence by composing only planning paths.
    let temp = temp_project();
    let paths = MissionPaths::parse(temp.path(), "demo-01KZ6G6H").expect("paths");
    assert!(!paths.spec_dir().join("lanes.json").exists());
    assert!(!paths.spec_dir().join("worktrees").exists());
    write(&paths.spec_file(), "spec");
    assert!(paths.spec_file().is_file());
}
