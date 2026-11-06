//! Permanent regressions promoted from the independent Mission-lock review.
//! Authoring must use the coordination key (not the directory-name key), and
//! the create's birth key must converge with its recorded post-birth identity.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use sk_kernel::locks::LockWait;
use sk_mission::{
    CanonicalTemplate, DependencyFinalizePlan, MissionArtifactPlan, MissionArtifactRequest,
    MissionCreateOptions, MissionCreatePlan, MissionCreateRequest, MissionIdentity,
    MissionTemplateKind, MissionTemplates, MissionTopology, WorkPackageDraft,
};
use sk_status::mutation::{acquire_mission_lock, mission_lock_key};
use support::{temp_project, write};

const SLUG: &str = "060-test";
const MID8: &str = "01KZ6G6H";
const COORD: &str = "060-test-01KZ6G6H";
const ID: &str = "01KZ6G6H4R8Y9J2N4Q7V3B6M1F";
const CREATED_AT: &str = "2026-11-02T10:00:00Z";

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

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
}

fn coord_mission(root: &Path) -> PathBuf {
    let mission = root.join("kitty-specs").join(SLUG);
    fs::create_dir_all(mission.join("tasks")).unwrap();
    fs::write(
        mission.join("meta.json"),
        serde_json::json!({
            "mission_slug": SLUG,
            "mission_id": ID,
            "mid8": MID8,
            "coordination_branch": format!("kitty/mission-{COORD}"),
        })
        .to_string(),
    )
    .unwrap();
    mission
}

fn lock_files(root: &Path) -> Vec<String> {
    let dir = root.join(".git/spec-kitty-locks");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".status.lock"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn hold_elsewhere(
    mission_dir: &Path,
    root: &Path,
) -> (mpsc::Sender<()>, std::thread::JoinHandle<String>) {
    let mission_dir = mission_dir.to_path_buf();
    let root = root.to_path_buf();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let lock = acquire_mission_lock(
            &mission_dir,
            Some(&root),
            LockWait::Bounded(Duration::from_secs(5)),
        )
        .expect("holder");
        let path = lock.path().display().to_string();
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
        path
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    (release_tx, handle)
}

#[test]
fn artifact_authoring_takes_the_coordination_key() {
    let temp = temp_project();
    let root = temp.path();
    init_repo(root);
    let mission = coord_mission(root);
    assert_eq!(mission_lock_key(&mission, root).unwrap(), COORD);

    let request = MissionArtifactRequest::new("main", "main", CREATED_AT)
        .with_work_package(WorkPackageDraft::new("WP01", "Foundation").with_subtasks(["T001"]));
    let plan =
        MissionArtifactPlan::plan(root, SLUG.parse().unwrap(), &templates(), &request).unwrap();

    let (release, holder) = hold_elsewhere(&mission, root);
    let error = plan
        .commit(root)
        .expect_err("coordination hold must exclude");
    release.send(()).unwrap();
    let held_path = holder.join().unwrap();
    println!("artifacts held path: {held_path}");
    println!("artifacts error: {error}");
    assert!(matches!(error, sk_mission::TaskArtifactError::Lock { .. }));
    assert!(!mission.join("plan.md").exists());
    assert!(
        held_path.ends_with(&format!("{COORD}.status.lock")),
        "{held_path}"
    );
    println!("artifacts lock files: {:?}", lock_files(root));
    assert_eq!(lock_files(root), vec![format!("{COORD}.status.lock")]);

    // Negative control: a hold on the bare directory-name key does NOT exclude
    // this coordination-routed Mission, which is why the key must come from meta.
    let common = sk_kernel::lock_families::status_lock_common_dir(root).unwrap();
    let bare_key = SLUG.to_owned();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release2, release_rx) = mpsc::channel::<()>();
    let bare_holder = std::thread::spawn(move || {
        let _lock = sk_kernel::lock_families::acquire_feature_status_lock(
            &common,
            &bare_key,
            LockWait::Bounded(Duration::from_secs(5)),
        )
        .expect("bare key holder");
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = plan
        .commit(root)
        .expect("a bare-name hold must not exclude");
    release2.send(()).unwrap();
    bare_holder.join().unwrap();
    assert!(
        second
            .created()
            .iter()
            .any(|path| path.ends_with("plan.md"))
    );
    println!("artifacts lock files after: {:?}", lock_files(root));
}

#[test]
fn dependency_finalization_takes_the_coordination_key() {
    let temp = temp_project();
    let root = temp.path();
    init_repo(root);
    let mission = coord_mission(root);
    write(
        &mission.join("tasks.md"),
        "## WP01 - Foundation\n\n## WP02 - API\n\n**Dependencies**: WP01\n",
    );
    write(
        &mission.join("tasks/WP01-foundation.md"),
        "---\nwork_package_id: WP01\ntitle: Foundation\ndependencies: []\n---\n\n# WP01\n",
    );
    write(
        &mission.join("tasks/WP02-api.md"),
        "---\nwork_package_id: WP02\ntitle: API\ndependencies: []\n---\n\n# WP02\n",
    );
    let tasks_dir = mission.join("tasks");
    let plan = DependencyFinalizePlan::compute(&mission.join("tasks.md"), &tasks_dir).unwrap();
    assert_eq!(plan.updated_wps(), vec!["WP02".to_owned()]);

    let (release, holder) = hold_elsewhere(&mission, root);
    let error = plan
        .apply(&tasks_dir)
        .expect_err("coordination hold must exclude");
    release.send(()).unwrap();
    let held_path = holder.join().unwrap();
    println!("tasks held path: {held_path}");
    println!("tasks error: {error}");
    assert!(matches!(
        error,
        sk_mission::TasksAuthoringError::Lock { .. }
    ));
    assert!(
        held_path.ends_with(&format!("{COORD}.status.lock")),
        "{held_path}"
    );
    assert_eq!(
        fs::read_to_string(tasks_dir.join("WP02-api.md")).unwrap(),
        "---\nwork_package_id: WP02\ntitle: API\ndependencies: []\n---\n\n# WP02\n"
    );
    println!("tasks lock files: {:?}", lock_files(root));
}

#[test]
fn create_birth_key_converges_with_the_created_mission_key() {
    let temp = temp_project();
    let root = temp.path();
    init_repo(root);
    let request = MissionCreateRequest::new("demo-mission", "main")
        .unwrap()
        .with_topology(MissionTopology::Coord)
        .with_coordination_branch("kitty/mission-demo-mission-01KZ6G6H");
    let identity = MissionIdentity::new(ID, CREATED_AT).unwrap();
    let options = MissionCreateOptions::new().with_lock_timeout(Duration::from_millis(100));
    let create_templates = MissionTemplates::new(template(
        MissionTemplateKind::Spec,
        "research/spec-template.md",
        "spec-template.md",
    ));
    let plan =
        MissionCreatePlan::plan(root, request, identity, &create_templates, &options).unwrap();
    let spec_dir = plan.spec_dir().to_path_buf();
    println!("create dir: {}", spec_dir.display());

    let (release, holder) = hold_elsewhere(&spec_dir, root);
    let error = plan
        .commit(root)
        .expect_err("a birth-key hold must exclude a create");
    release.send(()).unwrap();
    let held_path = holder.join().unwrap();
    println!("create held path: {held_path}");
    println!("create error: {error}");
    assert!(matches!(error, sk_mission::MissionCreateError::Lock { .. }));

    let receipt = plan.commit(root).expect("create after release");
    println!("created: {:?}", receipt.created().len());
    let post_birth = mission_lock_key(&spec_dir, root).unwrap();
    println!(
        "post-birth key: {post_birth} vs dir name {}",
        spec_dir.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(
        post_birth,
        spec_dir.file_name().unwrap().to_string_lossy(),
        "the birth key must converge with the key the created meta records"
    );
    println!("create lock files: {:?}", lock_files(root));
}
