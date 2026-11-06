//! CF34 — the `charter activate --preset` run-wide lock window in the CLI.
//!
//! The reference holds the charter synthesis lock across plan + write + re-stamp
//! (`activation_write_window`), so a write that cannot take it refuses under its
//! own code and changes nothing. This drives the public
//! `run(CharterCommand::Activate(..))` entry in re-executed children whose CWD is
//! the fixture project (the root is discovered from the CWD, as the CLI does),
//! with a real second process holding the lock.
//!
//! Outstanding deep proof (lane report): Source-vs-Native message bytes for the
//! two lock codes, and a same-worktree `charter synthesize` interloper.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sk_cli::compat::charter::{ActivateArgs, CharterCommand, run};
use sk_doctrine::acquire_synthesis_lock;

const ACTIVATION_REL: &str = ".kittify/charter/activation.yaml";
const HOLD_ENV: &str = "SK_CF34_CLI_HOLD";
const RUN_ENV: &str = "SK_CF34_CLI_RUN";
const RELEASE_ENV: &str = "SK_CF34_CLI_RELEASE";

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, text).expect("fixture file");
}

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("fixture root");
    let project = directory.path();
    write(
        &project.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: acme-pack\n",
    );
    write(
        &project.join("acme-pack/directives/900-fixture.directive.yaml"),
        "id: DIRECTIVE_900\ntitle: Fixture directive\n",
    );
    write(
        &project.join("acme-pack/presets/one.yaml"),
        "name: one\ndescription: One fixture directive.\nactivated_directives:\n  - DIRECTIVE_900\n",
    );
    directory
}

fn args() -> ActivateArgs {
    ActivateArgs {
        kind: None,
        artifact_id: None,
        cascade: None,
        include_deactivated: false,
        only: false,
        preset: Some("one".to_owned()),
        pack: "acme".to_owned(),
        force: false,
        no_compile: false,
        compile: false,
        resynthesize: false,
        json: true,
    }
}

fn child_executable() -> PathBuf {
    std::env::current_exe().expect("the test binary")
}

/// The holder child: take the run-wide lock, announce it, wait for the release file.
fn hold_lock(root: &Path, release: &Path) {
    let lock = acquire_synthesis_lock(root, Duration::from_secs(10)).expect("the child lock");
    println!("CF34 acquired {}", lock.path().display());
    let started = Instant::now();
    while !release.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the parent never released the holder"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The runner child: invoke the CLI entry the way the dispatcher does.
fn run_entry() {
    let code = run(CharterCommand::Activate(args())).expect("the CLI renders its own refusal");
    println!("RECEIPT exit={code}");
}

fn spawn_holder(
    test_name: &str,
    root: &Path,
) -> (Child, PathBuf, BufReader<std::process::ChildStdout>) {
    let release = root.join("cf34-cli-release");
    let mut child = Command::new(child_executable())
        .args(["--exact", test_name, "--nocapture"])
        .env(HOLD_ENV, root)
        .env(RELEASE_ENV, &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the holder");
    let stdout = child.stdout.take().expect("holder stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).expect("holder line");
        assert_ne!(read, 0, "the holder exited without acquiring");
        if line.starts_with("CF34 acquired ") {
            break;
        }
    }
    (child, release, reader)
}

fn release_holder(mut child: Child, release: &Path, reader: BufReader<std::process::ChildStdout>) {
    fs::write(release, b"go").expect("release the holder");
    let status = child.wait().expect("wait for the holder");
    drop(reader);
    assert!(status.success(), "the holder failed: {status}");
}

fn run_elsewhere(test_name: &str, root: &Path) -> String {
    let output = Command::new(child_executable())
        .args(["--exact", test_name, "--nocapture"])
        .env(RUN_ENV, root)
        .current_dir(root)
        .output()
        .expect("run the CLI child");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "the CLI child failed: {stdout}\n{stderr}"
    );
    stdout
}

#[test]
fn the_preset_lock_window_refuses_and_writes_nothing() {
    if let Some(root) = std::env::var_os(HOLD_ENV) {
        let root = PathBuf::from(root);
        let release = PathBuf::from(std::env::var_os(RELEASE_ENV).expect("release"));
        hold_lock(&root, &release);
        return;
    }
    if std::env::var_os(RUN_ENV).is_some() {
        run_entry();
        return;
    }

    let directory = fixture();
    let project = directory.path();
    let before = fs::read_to_string(project.join(ACTIVATION_REL)).unwrap_or_default();
    let (child, release, reader) =
        spawn_holder("the_preset_lock_window_refuses_and_writes_nothing", project);
    let started = Instant::now();
    let stdout = run_elsewhere("the_preset_lock_window_refuses_and_writes_nothing", project);
    let elapsed = started.elapsed();
    release_holder(child, &release, reader);

    assert!(
        elapsed >= Duration::from_secs(9),
        "the CLI waits the bounded run-wide lock timeout (waited {elapsed:?})"
    );
    assert!(stdout.contains("RECEIPT exit=1"), "{stdout}");
    assert!(
        stdout.contains("\"code\": \"CHARTER_SYNTHESIS_LOCK_HELD\""),
        "the refusal carries the reference's lock code: {stdout}"
    );
    assert!(
        fs::read_to_string(project.join(ACTIVATION_REL)).unwrap_or_default() == before,
        "the refused run wrote nothing"
    );

    // With the lock free the same invocation applies the preset.
    let stdout = run_elsewhere("the_preset_lock_window_refuses_and_writes_nothing", project);
    assert!(stdout.contains("RECEIPT exit=0"), "{stdout}");
    assert!(stdout.contains("\"preset\": \"one\""), "{stdout}");
    let activation = fs::read_to_string(project.join(ACTIVATION_REL)).expect("the written file");
    assert!(
        activation.contains("activated_directives:") && activation.contains("900-fixture"),
        "{activation}"
    );
}
