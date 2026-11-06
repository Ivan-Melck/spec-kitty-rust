//! Authoring consumers take the **shared** Mission-write lock.
//!
//! Create, plan/tasks authoring and dependency finalization used to take a
//! checkout-local `.kittify/spec-kitty-locks/mission-write.lock`. These tests
//! hold the canonical keyed lock first and prove each authoring commit refuses
//! (bounded timeout) and writes nothing, which is only observable if the
//! consumer actually went through the shared protocol.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use sk_kernel::locks::LockWait;
use sk_mission::{
    CanonicalTemplate, DependencyFinalizePlan, MissionArtifactPlan, MissionArtifactRequest,
    MissionCreateOptions, MissionCreatePlan, MissionCreateRequest, MissionIdentity,
    MissionTemplateKind, MissionTemplates, MissionTopology, TaskArtifactError, TasksAuthoringError,
    WorkPackageDraft,
};
use sk_status::mutation::{acquire_mission_birth_lock, acquire_mission_lock};
use support::{temp_project, write};

const ID: &str = "01KZ6G6H4R8Y9J2N4Q7V3B6M1F";
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

/// Hold the birth key on a different thread: same-thread takes are reentrant by
/// contract, so contention has to come from another thread.
fn hold_birth_elsewhere(
    mission_dir: &Path,
    root: &Path,
) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let mission_dir = mission_dir.to_path_buf();
    let root = root.to_path_buf();
    let handle = std::thread::spawn(move || {
        let _lock = acquire_mission_birth_lock(
            &mission_dir,
            Some(&root),
            LockWait::Bounded(Duration::from_secs(5)),
        )
        .expect("hold the shared birth key");
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    (release_tx, handle)
}

fn hold_mission_elsewhere(
    mission_dir: &Path,
    root: &Path,
) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let mission_dir = mission_dir.to_path_buf();
    let root = root.to_path_buf();
    let handle = std::thread::spawn(move || {
        let _lock = acquire_mission_lock(
            &mission_dir,
            Some(&root),
            LockWait::Bounded(Duration::from_secs(5)),
        )
        .expect("hold the shared Mission key");
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    (release_tx, handle)
}

fn create_templates() -> MissionTemplates {
    MissionTemplates::new(template(
        MissionTemplateKind::Spec,
        "research/spec-template.md",
        "spec-template.md",
    ))
}

fn authoring_templates() -> MissionTemplates {
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

#[test]
fn creation_refuses_while_the_birth_key_is_held_and_writes_nothing() {
    let temp = temp_project();
    let request = MissionCreateRequest::new("demo-mission", "main")
        .expect("request")
        .with_topology(MissionTopology::SingleBranch);
    let identity = MissionIdentity::new(ID, CREATED_AT).expect("identity");
    let options = MissionCreateOptions::new().with_lock_timeout(Duration::from_millis(100));
    let plan = MissionCreatePlan::plan(
        temp.path(),
        request,
        identity,
        &create_templates(),
        &options,
    )
    .expect("plan");

    let (release, holder) = hold_birth_elsewhere(plan.spec_dir(), temp.path());

    let error = plan
        .commit(temp.path())
        .expect_err("the birth key is held elsewhere");
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(matches!(error, sk_mission::MissionCreateError::Lock { .. }));
    assert!(!plan.spec_dir().join("meta.json").exists());
    assert!(!plan.spec_dir().exists());
    assert!(
        !temp
            .path()
            .join(".kittify/spec-kitty-locks/mission-write.lock")
            .exists(),
        "the retired checkout-local authoring lock must not be taken"
    );
}

#[test]
fn artifact_authoring_refuses_while_the_mission_key_is_held_and_writes_nothing() {
    let temp = temp_project();
    let mission_dir = temp.path().join("kitty-specs").join(SLUG);
    fs::create_dir_all(mission_dir.join("tasks")).expect("mission dir");
    let request = MissionArtifactRequest::new("main", "main", CREATED_AT)
        .with_work_package(WorkPackageDraft::new("WP01", "Foundation").with_subtasks(["T001"]));
    let plan = MissionArtifactPlan::plan(
        temp.path(),
        SLUG.parse().expect("slug"),
        &authoring_templates(),
        &request,
    )
    .expect("plan");

    let (release, holder) = hold_mission_elsewhere(&mission_dir, temp.path());

    let error = plan
        .commit(temp.path())
        .expect_err("the Mission key is held elsewhere");
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(matches!(error, TaskArtifactError::Lock { .. }));
    assert!(!mission_dir.join("plan.md").exists());
    assert!(!mission_dir.join("tasks.md").exists());
    assert!(
        !temp
            .path()
            .join(".kittify/spec-kitty-locks/mission-write.lock")
            .exists()
    );
}

#[test]
fn dependency_finalization_refuses_while_the_mission_key_is_held_and_writes_nothing() {
    let temp = temp_project();
    let mission_dir = temp.path().join("kitty-specs/demo-01KZ6G6H");
    fs::create_dir_all(mission_dir.join("tasks")).expect("tasks");
    write(
        &mission_dir.join("tasks.md"),
        "## WP01 - Foundation\n\n## WP02 - API\n\n**Dependencies**: WP01\n",
    );
    write(
        &mission_dir.join("tasks/WP01-foundation.md"),
        "---\nwork_package_id: WP01\ntitle: Foundation\ndependencies: []\n---\n\n# WP01\n",
    );
    write(
        &mission_dir.join("tasks/WP02-api.md"),
        "---\nwork_package_id: WP02\ntitle: API\ndependencies: []\n---\n\n# WP02\n",
    );
    let tasks_dir = mission_dir.join("tasks");
    let plan =
        DependencyFinalizePlan::compute(&mission_dir.join("tasks.md"), &tasks_dir).expect("plan");
    assert_eq!(plan.updated_wps(), vec!["WP02".to_owned()]);

    let (release, holder) = hold_mission_elsewhere(&mission_dir, temp.path());

    let error = plan
        .apply(&tasks_dir)
        .expect_err("the Mission key is held elsewhere");
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(matches!(error, TasksAuthoringError::Lock { .. }));
    assert_eq!(
        fs::read_to_string(tasks_dir.join("WP02-api.md")).unwrap(),
        "---\nwork_package_id: WP02\ntitle: API\ndependencies: []\n---\n\n# WP02\n"
    );
    assert!(
        !temp
            .path()
            .join(".kittify/spec-kitty-locks/mission-write.lock")
            .exists()
    );
}
