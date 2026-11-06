//! CF29 — `charter activate --preset` through the compatibility CLI (the thin
//! adapter over the canonical preset planner/writer).
//!
//! The compat charter tree is registered by the parent root CLI, not here, so
//! this test drives the public `run(CharterCommand::Activate(..))` entry in a
//! re-executed child whose CWD is the fixture project (the project root is
//! discovered from the CWD, exactly as the CLI does it). The child asserts the
//! returned exit codes and prints receipts; the parent asserts the machine
//! payload, the human text and the on-disk result.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_cli::compat::charter::{ActivateArgs, CharterCommand, run};

const ACTIVATION_REL: &str = ".kittify/charter/activation.yaml";

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, text).expect("fixture file");
}

fn fixture_preset(presets: &str) -> tempfile::TempDir {
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
    write(&project.join("acme-pack/presets/one.yaml"), presets);
    directory
}

fn args(preset: &str, pack: &str) -> ActivateArgs {
    ActivateArgs {
        kind: None,
        artifact_id: None,
        cascade: None,
        include_deactivated: false,
        only: false,
        preset: Some(preset.to_owned()),
        pack: pack.to_owned(),
        force: false,
        no_compile: false,
        compile: false,
        resynthesize: false,
        json: false,
    }
}

/// The child's body: drive the four invocations and print receipts.
fn child() {
    let mut apply = args("one", "acme");
    apply.json = true;
    let code = run(CharterCommand::Activate(apply)).expect("the preset applies");
    println!("RECEIPT apply exit={code}");

    let noop = args("one", "acme");
    let code = run(CharterCommand::Activate(noop)).expect("the second run is a no-op");
    println!("RECEIPT noop exit={code}");

    let mut missing = args("nope", "built-in");
    missing.json = true;
    let code = run(CharterCommand::Activate(missing)).expect("the refusal renders itself");
    println!("RECEIPT missing exit={code}");

    let mut cascade = args("one", "acme");
    cascade.cascade = Some("all".to_owned());
    match run(CharterCommand::Activate(cascade)) {
        Ok(code) => panic!("--preset --cascade must be a usage refusal, got {code}"),
        Err(error) => println!(
            "RECEIPT cascade exit={} message={}",
            error.exit(),
            error.message()
        ),
    }

    let mut resynthesize = args("one", "acme");
    resynthesize.resynthesize = true;
    match run(CharterCommand::Activate(resynthesize)) {
        Ok(code) => panic!("--resynthesize must refuse, got {code}"),
        Err(error) => println!(
            "RECEIPT resynthesize exit={} message={}",
            error.exit(),
            error.message()
        ),
    }
}

fn run_child(project: &Path) -> String {
    let executable = std::env::current_exe().expect("the test binary");
    let output = Command::new(executable)
        .args([
            "--exact",
            "the_preset_surface_plans_writes_and_refuses",
            "--nocapture",
        ])
        .current_dir(project)
        .env("SK_CF29_CLI_CHILD", "1")
        .output()
        .expect("run the child");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "child failed: {stdout}\n{stderr}");
    stdout
}

#[test]
fn the_preset_surface_plans_writes_and_refuses() {
    if std::env::var_os("SK_CF29_CLI_CHILD").is_some() {
        child();
        return;
    }
    let directory = fixture_preset(
        "name: one\ndescription: One fixture directive.\nactivated_directives:\n  - DIRECTIVE_900\n",
    );
    let project: PathBuf = directory.path().to_path_buf();

    let stdout = run_child(&project);
    println!("--- child receipts ---\n{stdout}");

    assert!(stdout.contains("RECEIPT apply exit=0"), "{stdout}");
    assert!(
        stdout.contains("\"preset\": \"one\"") && stdout.contains("\"pack\": \"acme\""),
        "the JSON payload names the preset and its pack: {stdout}"
    );
    assert!(
        stdout.contains("\"900-fixture\""),
        "the declared id is stored as its stem: {stdout}"
    );
    assert!(
        stdout.contains("\"removed\": []"),
        "no key is removed: {stdout}"
    );
    assert!(
        stdout.contains(ACTIVATION_REL),
        "the payload names the ledger target: {stdout}"
    );
    assert!(stdout.contains("RECEIPT noop exit=0"), "{stdout}");
    assert!(
        stdout.contains("is already in force in") && stdout.contains("nothing changed."),
        "the second run reports the no-op: {stdout}"
    );
    assert!(stdout.contains("RECEIPT missing exit=1"), "{stdout}");
    assert!(
        stdout.contains("PRESET_NOT_FOUND"),
        "the refusal carries its machine code: {stdout}"
    );
    assert!(
        stdout.contains("RECEIPT cascade exit=2"),
        "the incompatible flag is a usage refusal: {stdout}"
    );
    assert!(
        stdout.contains("--cascade cannot be combined with --preset"),
        "{stdout}"
    );
    assert!(
        stdout.contains("RECEIPT resynthesize exit=2"),
        "--resynthesize is refused before any write: {stdout}"
    );

    let activation = fs::read_to_string(project.join(ACTIVATION_REL)).expect("the written ledger");
    assert!(
        activation.contains("activated_directives:") && activation.contains("900-fixture"),
        "{activation}"
    );
    assert!(
        !activation.contains("nope"),
        "the refused runs wrote nothing: {activation}"
    );
}
