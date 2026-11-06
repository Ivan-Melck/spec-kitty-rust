mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_mission::{
    DependencyFinalizePlan, MissionWriteObserver, TasksAuthoringError, TasksOutline,
    parse_dependencies_from_tasks_md,
};
use support::{temp_project, write};

fn oracle_json(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/oracle")
        .join(name);
    serde_json::from_str(
        &fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .expect("oracle json")
}

fn tasks_root() -> tempfile::TempDir {
    let temp = temp_project();
    let dir = temp.path().join("kitty-specs/demo-01KZ6G6H");
    fs::create_dir_all(dir.join("tasks")).expect("tasks");
    temp
}

fn mission_dir(temp: &tempfile::TempDir) -> PathBuf {
    temp.path().join("kitty-specs/demo-01KZ6G6H")
}

fn wp(id: &str, deps_yaml: &str, extra: &str, body: &str) -> String {
    format!(
        "---\nwork_package_id: {id}\ntitle: Fixture {id}\ndependencies: {deps_yaml}\nphase: \"Phase 1\"\n{extra}---\n\n{body}"
    )
}

fn three_wp_fixture(temp: &tempfile::TempDir) {
    let dir = mission_dir(temp);
    write(
        &dir.join("tasks.md"),
        "## WP01 - Foundation\n\nDepends on nothing yet.\n\n## WP02 - API\n\n**Dependencies**: WP01\n\n## WP03 - UI\n\n### Dependencies\n- WP01\n- WP02\n",
    );
    write(
        &dir.join("tasks/WP01-foundation.md"),
        &wp("WP01", "[]", "", "# WP01\n\nFoundation body.\n"),
    );
    write(
        &dir.join("tasks/WP02-api.md"),
        &wp(
            "WP02",
            "[]",
            "custom_field: \"preserve me\"  # operator note\n",
            "# WP02\n\nAPI body with **markdown**.\n\n- [ ] T001\n",
        ),
    );
    write(
        &dir.join("tasks/WP03-ui.md"),
        &wp("WP03", "[]", "", "# WP03\n\nUI body.\n"),
    );
}

#[test]
fn dependency_parser_matches_every_python_oracle_case() {
    let cases = oracle_json("tasks/dependency_cases.json");
    for case in cases.as_array().expect("cases") {
        let name = case["name"].as_str().expect("name");
        let content = case["tasks_md"].as_str().expect("tasks_md");
        let expected = case["expected"].as_object().expect("expected");
        let actual = parse_dependencies_from_tasks_md(content).expect("parse");

        assert_eq!(actual.len(), expected.len(), "case {name}");
        for (wp, dependencies) in expected {
            let expected: Vec<String> = dependencies
                .as_array()
                .expect("deps array")
                .iter()
                .map(|value| value.as_str().expect("wp id").to_owned())
                .collect();
            let actual: Vec<String> = actual
                .get(wp)
                .unwrap_or_else(|| panic!("case {name}: missing {wp}"))
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect();
            assert_eq!(actual, expected, "case {name}, {wp}");
        }
    }
}

#[test]
fn outline_parses_sections_subtasks_and_structured_task_owners() {
    let outline = TasksOutline::parse(
        "## WP01 - Foundation\n\nSubtasks: T001, T002\n\n- [ ] T003 deliver\n\n| Task | Owner |\n|---|---|\n| T004 | WP01 |\n\n## Work Package 2 - API\n\n- [x] T005\n\n## Notes\n\nT006 is prose and belongs to nobody.\n",
    )
    .expect("outline");

    let ids: Vec<&str> = outline
        .sections()
        .iter()
        .map(|section| section.wp_id().as_str())
        .collect();
    assert_eq!(ids, vec!["WP01", "WP02"]);
    assert_eq!(outline.owners_of_task("T001"), vec!["WP01".to_owned()]);
    assert_eq!(
        outline.owners_of_task("T003"),
        vec!["WP01".to_owned()],
        "checkbox rows are structured"
    );
    assert_eq!(
        outline.owners_of_task("T004"),
        vec!["WP01".to_owned()],
        "pipe-table rows are structured"
    );
    assert_eq!(outline.owners_of_task("T005"), vec!["WP02".to_owned()]);
    assert!(outline.owners_of_task("T006").is_empty());
}

#[test]
fn duplicate_outline_sections_are_refused() {
    let error = TasksOutline::parse("## WP01 - A\n\n## WP01 - B\n").expect_err("duplicate");
    assert!(matches!(
        error,
        TasksAuthoringError::DuplicateSection { .. }
    ));
}

#[test]
fn finalize_plan_computes_the_effective_dependency_graph_without_writing() {
    let temp = tasks_root();
    three_wp_fixture(&temp);
    let before = snapshot(&mission_dir(&temp));

    let plan = DependencyFinalizePlan::compute(
        &mission_dir(&temp).join("tasks.md"),
        &mission_dir(&temp).join("tasks"),
    )
    .expect("plan");

    assert_eq!(
        plan.effective_dependencies()
            .iter()
            .map(|(wp, deps)| (
                wp.to_string(),
                deps.iter().map(ToString::to_string).collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("WP01".to_owned(), vec![]),
            ("WP02".to_owned(), vec!["WP01".to_owned()]),
            (
                "WP03".to_owned(),
                vec!["WP01".to_owned(), "WP02".to_owned()]
            ),
        ]
    );
    assert_eq!(plan.updated_wps(), vec!["WP02", "WP03"]);
    assert_eq!(plan.unchanged_wps(), vec!["WP01"]);
    assert_eq!(
        snapshot(&mission_dir(&temp)),
        before,
        "planning writes nothing"
    );

    let receipt = plan
        .apply(&mission_dir(&temp).join("tasks"))
        .expect("apply");
    assert_eq!(receipt.updated().len(), 2);
    assert_eq!(
        fs::read_to_string(mission_dir(&temp).join("tasks/WP02-api.md")).unwrap(),
        "---\nwork_package_id: WP02\ntitle: Fixture WP02\ndependencies: [WP01]\nphase: \"Phase 1\"\ncustom_field: \"preserve me\"  # operator note\n---\n\n# WP02\n\nAPI body with **markdown**.\n\n- [ ] T001\n",
        "only the dependencies value changes; comments and body stay byte-identical"
    );
    assert_eq!(
        fs::read_to_string(mission_dir(&temp).join("tasks/WP03-ui.md")).unwrap(),
        "---\nwork_package_id: WP03\ntitle: Fixture WP03\ndependencies: [WP01, WP02]\nphase: \"Phase 1\"\n---\n\n# WP03\n\nUI body.\n"
    );
    assert_eq!(
        fs::read_to_string(mission_dir(&temp).join("tasks.md")).unwrap(),
        "## WP01 - Foundation\n\nDepends on nothing yet.\n\n## WP02 - API\n\n**Dependencies**: WP01\n\n## WP03 - UI\n\n### Dependencies\n- WP01\n- WP02\n",
        "user-authored tasks.md is never rewritten"
    );

    let again = DependencyFinalizePlan::compute(
        &mission_dir(&temp).join("tasks.md"),
        &mission_dir(&temp).join("tasks"),
    )
    .expect("re-plan");
    assert!(again.updated_wps().is_empty(), "idempotent repair");
    let receipt = again
        .apply(&mission_dir(&temp).join("tasks"))
        .expect("apply noop");
    assert!(receipt.updated().is_empty());
}

#[test]
fn a_missing_dependency_field_is_inserted_without_disturbing_comments_or_formatting() {
    let temp = tasks_root();
    let dir = mission_dir(&temp);
    write(
        &dir.join("tasks.md"),
        "## WP01 - A\n\n## WP02 - B\n\n**Dependencies**: WP01\n",
    );
    write(
        &dir.join("tasks/WP01-a.md"),
        &wp("WP01", "[]", "", "# WP01\n"),
    );
    let original = "---\n# leading comment\nwork_package_id: WP02\ntitle: Actual\ncustom_note: \"keep me\"  # operator note\nphase: \"Phase 1\"\n---\n\nBody stays.\n";
    write(&dir.join("tasks/WP02-b.md"), original);

    let plan =
        DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks")).expect("plan");
    assert_eq!(plan.updated_wps(), vec!["WP02"]);
    plan.apply(&dir.join("tasks")).expect("apply");
    let updated = fs::read_to_string(dir.join("tasks/WP02-b.md")).unwrap();
    assert!(updated.contains("# leading comment\n"));
    assert!(updated.contains("custom_note: \"keep me\"  # operator note\n"));
    assert!(updated.contains("dependencies: [WP01]\n"));
    assert!(updated.ends_with("---\n\nBody stays.\n"));
}

#[test]
fn a_dependency_disagreement_refuses_and_writes_nothing() {
    let temp = tasks_root();
    three_wp_fixture(&temp);
    write(
        &mission_dir(&temp).join("tasks/WP02-api.md"),
        &wp("WP02", "[WP03]", "", "# WP02\n"),
    );
    let before = snapshot(&mission_dir(&temp));

    let error = DependencyFinalizePlan::compute(
        &mission_dir(&temp).join("tasks.md"),
        &mission_dir(&temp).join("tasks"),
    )
    .expect_err("disagreement");
    assert!(matches!(
        error,
        TasksAuthoringError::DependencyConflict { .. }
    ));
    assert_eq!(snapshot(&mission_dir(&temp)), before);
}

#[test]
fn cycles_self_dependencies_and_unknown_ids_refuse_before_writing() {
    let cases = [
        (
            "cycle",
            "## WP01 - A\nDepends on WP02\n\n## WP02 - B\nDepends on WP01\n",
        ),
        ("self", "## WP01 - A\nDepends on WP01\n\n## WP02 - B\n"),
        ("unknown", "## WP01 - A\nDepends on WP09\n\n## WP02 - B\n"),
    ];
    for (name, tasks_md) in cases {
        let temp = tasks_root();
        let dir = mission_dir(&temp);
        write(&dir.join("tasks.md"), tasks_md);
        write(
            &dir.join("tasks/WP01-a.md"),
            &wp("WP01", "[]", "", "# WP01\n"),
        );
        write(
            &dir.join("tasks/WP02-b.md"),
            &wp("WP02", "[]", "", "# WP02\n"),
        );
        let before = snapshot(&dir);

        let error =
            DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks")).unwrap_err();
        match name {
            "unknown" => assert!(
                matches!(error, TasksAuthoringError::UnknownDependency { .. }),
                "{error:?}"
            ),
            "self" => assert!(
                matches!(error, TasksAuthoringError::SelfDependency { .. }),
                "{error:?}"
            ),
            _ => assert!(
                matches!(error, TasksAuthoringError::DependencyCycle { .. }),
                "{error:?}"
            ),
        }
        assert_eq!(snapshot(&dir), before, "{name} refusal writes nothing");
    }
}

#[test]
fn wp_coverage_must_match_between_tasks_md_and_the_tasks_directory() {
    let temp = tasks_root();
    let dir = mission_dir(&temp);
    write(&dir.join("tasks.md"), "## WP01 - A\n");
    write(
        &dir.join("tasks/WP01-a.md"),
        &wp("WP01", "[]", "", "# WP01\n"),
    );
    write(
        &dir.join("tasks/WP02-b.md"),
        &wp("WP02", "[]", "", "# WP02\n"),
    );

    let error = DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks"))
        .expect_err("missing section");
    assert!(matches!(error, TasksAuthoringError::Coverage { .. }));

    write(&dir.join("tasks.md"), "## WP01 - A\n\n## WP03 - C\n");
    fs::remove_file(dir.join("tasks/WP02-b.md")).unwrap();
    let error = DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks"))
        .expect_err("extra section");
    assert!(matches!(error, TasksAuthoringError::Coverage { .. }));
}

#[test]
fn corrupt_work_package_frontmatter_refuses_the_whole_finalize() {
    let temp = tasks_root();
    three_wp_fixture(&temp);
    write(
        &mission_dir(&temp).join("tasks/WP03-ui.md"),
        "---\nwork_package_id: WP03\ndependencies: [\n---\n\nbody\n",
    );
    let before = snapshot(&mission_dir(&temp));

    let error = DependencyFinalizePlan::compute(
        &mission_dir(&temp).join("tasks.md"),
        &mission_dir(&temp).join("tasks"),
    )
    .expect_err("corrupt frontmatter");
    assert!(matches!(
        error,
        TasksAuthoringError::InvalidFrontmatter { .. } | TasksAuthoringError::Yaml { .. }
    ));
    assert_eq!(snapshot(&mission_dir(&temp)), before);
}

#[test]
fn legacy_quoted_empty_dependencies_are_preserved_without_rewrite() {
    let temp = tasks_root();
    let dir = mission_dir(&temp);
    write(&dir.join("tasks.md"), "## WP01 - A\n");
    write(
        &dir.join("tasks/WP01-a.md"),
        &wp("WP01", "\"[]\"", "", "# WP01\n"),
    );

    let plan =
        DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks")).expect("plan");
    assert!(plan.updated_wps().is_empty());
    assert_eq!(plan.unchanged_wps(), vec!["WP01"]);
}

#[test]
fn parsed_dependencies_preserve_frontmatter_deps_when_tasks_md_declares_none() {
    let temp = tasks_root();
    let dir = mission_dir(&temp);
    write(&dir.join("tasks.md"), "## WP01 - A\n\n## WP02 - B\n");
    write(
        &dir.join("tasks/WP01-a.md"),
        &wp("WP01", "[]", "", "# WP01\n"),
    );
    write(
        &dir.join("tasks/WP02-b.md"),
        &wp("WP02", "[WP01]", "", "# WP02\n"),
    );

    let plan =
        DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks")).expect("plan");
    assert!(plan.updated_wps().is_empty());
    assert_eq!(plan.preserved_wps(), vec!["WP02"]);
    assert_eq!(
        plan.effective_dependencies()
            .get(&"WP02".parse().expect("wp"))
            .expect("deps")
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["WP01"]
    );
}

#[test]
fn a_partial_finalize_failure_restores_every_prior_byte() {
    struct FailOnUi;
    impl MissionWriteObserver for FailOnUi {
        fn before_write(&mut self, path: &Path) -> std::io::Result<()> {
            if path.ends_with("WP03-ui.md") {
                return Err(std::io::Error::other("injected failure"));
            }
            Ok(())
        }
    }

    let temp = tasks_root();
    three_wp_fixture(&temp);
    let dir = mission_dir(&temp);
    let before = snapshot(&dir);
    let plan =
        DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks")).expect("plan");
    let mut observer = FailOnUi;
    let error = plan
        .apply_with_observer(&dir.join("tasks"), &mut observer)
        .expect_err("injected failure");
    assert!(matches!(error, TasksAuthoringError::Write { .. }));
    assert_eq!(
        snapshot(&dir),
        before,
        "WP02's update was rolled back with WP03"
    );
}

#[test]
fn duplicate_yaml_keys_are_refused_as_corruption() {
    let temp = tasks_root();
    let dir = mission_dir(&temp);
    write(&dir.join("tasks.md"), "## WP01 - A\n");
    write(
        &dir.join("tasks/WP01-a.md"),
        "---\nwork_package_id: WP01\ndependencies: []\ndependencies: [WP01]\n---\nbody\n",
    );
    let before = snapshot(&dir);
    let error = DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks"))
        .expect_err("duplicate key");
    assert!(matches!(
        error,
        TasksAuthoringError::InvalidFrontmatter { .. }
    ));
    assert_eq!(snapshot(&dir), before);
}

#[test]
fn missing_tasks_md_or_tasks_dir_is_a_typed_refusal() {
    let temp = tasks_root();
    let dir = mission_dir(&temp);
    assert!(matches!(
        DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks")).unwrap_err(),
        TasksAuthoringError::MissingTasksFile { .. }
    ));
    write(&dir.join("tasks.md"), "## WP01 - A\n");
    fs::remove_dir(dir.join("tasks")).unwrap();
    assert!(matches!(
        DependencyFinalizePlan::compute(&dir.join("tasks.md"), &dir.join("tasks")).unwrap_err(),
        TasksAuthoringError::MissingTasksDirectory { .. }
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
