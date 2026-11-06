//! CF34 — Source preset publication composition in `sk-doctrine`.
//!
//! The reference's `apply_preset_plan` is one run-wide-locked transaction: it
//! prepares the write from the authority's *current* bytes, clears a forced
//! exclusion against the *current* deny list, publishes the activation file and
//! then re-stamps the synthesis manifest. These are bounded functional smoke
//! tests for exactly that: fresh temporary projects, the repository's real
//! fetched built-in presets, and a real second process for lock ownership.
//!
//! Outstanding deep proof (recorded in the lane report, not claimed here): a
//! differential run of the same fixtures through the pinned Python
//! `apply_preset_plan`, Unicode ids, and Windows lock behaviour.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sk_charter::compute_bundle_content_hash;
use sk_charter::synthesis::SynthesisManifest;
use sk_doctrine::preset_application::{
    PresetApplicationError, apply_preset_plan, apply_preset_plan_observed, plan_preset_application,
};
use sk_doctrine::{DoctrineError, YamlDocument, acquire_synthesis_lock};

const ACTIVATION_REL: &str = ".kittify/charter/activation.yaml";
const CHARTER_REL: &str = ".kittify/charter/charter.yaml";
const MANIFEST_REL: &str = ".kittify/charter/synthesis-manifest.yaml";

struct Fixture {
    _directory: tempfile::TempDir,
    project: PathBuf,
}

impl Fixture {
    fn new(activation: &str) -> Self {
        let directory = tempfile::tempdir().expect("fixture root");
        let project = directory.path().to_path_buf();
        let directory = directory;
        write(
            &project.join(".kittify/config.yaml"),
            "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: acme-pack\n",
        );
        write(&project.join(ACTIVATION_REL), activation);
        write(&project.join(CHARTER_REL), "catalog: {}\n");
        let fixture = Self {
            _directory: directory,
            project,
        };
        fixture.artifact(
            "directives/900-fixture.directive.yaml",
            "id: DIRECTIVE_900\ntitle: Fixture directive\n",
        );
        fixture
    }

    fn path(&self) -> &Path {
        &self.project
    }

    fn artifact(&self, relative: &str, text: &str) {
        write(&self.project.join("acme-pack").join(relative), text);
    }

    fn preset(&self, name: &str, body: &str) {
        write(
            &self
                .project
                .join("acme-pack/presets")
                .join(format!("{name}.yaml")),
            body,
        );
    }

    fn seed_manifest(&self, extra: &str) {
        write(
            &self.project.join(MANIFEST_REL),
            &format!(
                "created_at: '2026-01-01T00:00:00+00:00'\n\
                 run_id: '01J00000000000000000000000'\n\
                 adapter_id: a\n\
                 adapter_version: '1'\n\
                 synthesizer_version: 4.0.0\n\
                 manifest_hash: '{}'\n\
                 artifacts: []\n{extra}",
                "0".repeat(64)
            ),
        );
    }

    fn activation_text(&self) -> String {
        fs::read_to_string(self.project.join(ACTIVATION_REL)).expect("the activation file")
    }

    fn manifest_text(&self) -> String {
        fs::read_to_string(self.project.join(MANIFEST_REL)).expect("the manifest")
    }

    fn activation(&self) -> YamlDocument {
        YamlDocument::open(&self.project.join(ACTIVATION_REL)).expect("the activation document")
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, text).expect("fixture file");
}

fn ids(document: &YamlDocument, key: &str) -> Option<Vec<String>> {
    document.get_string_list(key).expect("a string list")
}

fn one_directive_preset(fixture: &Fixture) {
    fixture.preset(
        "one",
        "name: one\ndescription: One directive.\nactivated_directives:\n  - DIRECTIVE_900\n",
    );
}

#[test]
fn the_activation_write_closes_with_the_manifest_restamp() {
    let fixture = Fixture::new("");
    one_directive_preset(&fixture);
    fixture.seed_manifest("bundle_content_hash: sha256:stale\n");
    let plan = plan_preset_application(fixture.path(), "acme", "one").expect("the plan");

    let outcome = apply_preset_plan(fixture.path(), &plan, false).expect("the write");
    let restamp = outcome
        .restamp
        .as_ref()
        .expect("a restamp followed the write");
    assert!(restamp.is_restamped(), "{restamp:?}");
    assert_eq!(restamp.warning_line(), None);

    // The activation file landed.
    assert_eq!(
        ids(&fixture.activation(), "activated_directives"),
        Some(vec!["900-fixture".to_owned()])
    );

    // The manifest stamp now describes the activated charter, and only the
    // stamp and its derived self-hash changed value.
    let manifest = SynthesisManifest::load(&fixture.path().join(MANIFEST_REL)).expect("loads");
    let expected = compute_bundle_content_hash(fixture.path()).expect("a hashable bundle");
    assert_eq!(
        manifest.bundle_content_hash.as_deref(),
        Some(expected.as_str())
    );
    assert!(manifest.verify_manifest_hash().is_ok());
    assert_eq!(manifest.schema_version, "3", "the model's own default");
    assert_eq!(manifest.artifacts.len(), 0);
    assert_eq!(manifest.run_id, "01J00000000000000000000000");
}

#[test]
fn a_true_noop_touches_neither_the_activation_file_nor_the_manifest() {
    let fixture = Fixture::new("activated_directives:\n  - 900-fixture\n");
    fixture.preset(
        "same",
        "name: same\ndescription: Already in force.\nactivated_directives:\n  - DIRECTIVE_900\n",
    );
    fixture.seed_manifest("bundle_content_hash: sha256:stale\n");
    let activation_before = fixture.activation_text();
    let manifest_before = fixture.manifest_text();

    let plan = plan_preset_application(fixture.path(), "acme", "same").expect("the plan");
    assert!(plan.is_noop(), "{:?}", plan.changes);
    let outcome = apply_preset_plan(fixture.path(), &plan, false).expect("the no-op");
    assert!(
        outcome.restamp.is_none(),
        "the reference does not re-stamp a true no-op"
    );
    assert_eq!(fixture.activation_text(), activation_before);
    assert_eq!(fixture.manifest_text(), manifest_before);
}

#[test]
fn force_reports_the_cleared_ids_from_the_current_deny_list() {
    let fixture = Fixture::new("deactivated_directives:\n  - 900-fixture\n");
    fixture.preset(
        "include",
        "name: include\ndescription: Lists the deactivated directive.\nactivated_directives:\n  - DIRECTIVE_900\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "include").expect("the plan");
    assert_eq!(
        plan.denied_listed.get("activated_directives"),
        Some(&vec!["900-fixture".to_owned()])
    );

    // A concurrent writer adds an exclusion after the plan was computed. The
    // reference re-reads the deny list under the lock and reports the real delta.
    write(
        &fixture.path().join(ACTIVATION_REL),
        "deactivated_directives:\n  - 900-fixture\n  - 901-other\n",
    );
    let outcome = apply_preset_plan(fixture.path(), &plan, true).expect("the forced write");
    assert_eq!(
        outcome.cleared.get("deactivated_directives"),
        Some(&vec!["900-fixture".to_owned(), "901-other".to_owned()]),
        "the cleared report comes from the CURRENT deny list, as the reference does"
    );
    assert_eq!(
        ids(&fixture.activation(), "activated_directives"),
        Some(vec!["900-fixture".to_owned()])
    );
    assert_eq!(
        ids(&fixture.activation(), "deactivated_directives"),
        None,
        "the forced write replaces the list with the plan's post-state (an empty list removes the key)"
    );
}

#[test]
fn the_write_re_reads_the_authority_instead_of_refusing_a_stale_plan() {
    let fixture = Fixture::new("");
    one_directive_preset(&fixture);
    let plan = plan_preset_application(fixture.path(), "acme", "one").expect("the plan");
    write(
        &fixture.path().join(ACTIVATION_REL),
        "# a concurrent writer got here first\n",
    );

    // The stricter Native capability keeps the old refusal...
    let error = apply_preset_plan_observed(fixture.path(), &plan, false)
        .expect_err("the plan's observation is stale");
    assert!(
        matches!(
            error,
            PresetApplicationError::Doctrine(DoctrineError::PreconditionChanged { .. })
        ),
        "{error:?}"
    );
    assert_eq!(
        fixture.activation_text(),
        "# a concurrent writer got here first\n"
    );

    // ...and the Source composition re-reads, applies and keeps that writer's line.
    let outcome = apply_preset_plan(fixture.path(), &plan, false).expect("re-read and apply");
    assert_eq!(
        ids(&fixture.activation(), "activated_directives"),
        Some(vec!["900-fixture".to_owned()])
    );
    assert!(
        fixture
            .activation_text()
            .contains("# a concurrent writer got here first")
    );
    assert!(outcome.restamp.is_some());
}

#[test]
fn an_unreadable_manifest_refuses_after_the_write_with_its_own_code() {
    let fixture = Fixture::new("");
    one_directive_preset(&fixture);
    fixture.seed_manifest("bundle_content_hash: sha256:stale\nextra_field: 1\n");
    let plan = plan_preset_application(fixture.path(), "acme", "one").expect("the plan");
    let error = apply_preset_plan(fixture.path(), &plan, false).expect_err("refused");
    assert_eq!(error.code(), "RESTAMP_FAILED");
    assert!(
        error.to_string().contains("bundle hash was not refreshed"),
        "{error}"
    );
    assert!(
        ids(&fixture.activation(), "activated_directives").is_some(),
        "the activation change is already saved when the re-stamp refuses"
    );
}

// ---------------------------------------------------------------------------
// Run-wide lock ownership, against a REAL second process
// ---------------------------------------------------------------------------

const HOLD_ENV: &str = "SK_CF34_HOLD_LOCK";
const RELEASE_ENV: &str = "SK_CF34_RELEASE";

fn hold_lock_child(root: &Path, release: &Path) {
    let lock = acquire_synthesis_lock(root, Duration::from_secs(10)).expect("the child lock");
    println!("CF34 acquired {}", lock.path().display());
    let started = Instant::now();
    while !release.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the parent never released the child"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("CF34 releasing");
}

/// Spawn this test binary as a second process that holds the run-wide lock.
fn hold_lock_elsewhere(
    test_name: &str,
    root: &Path,
) -> (Child, PathBuf, BufReader<std::process::ChildStdout>) {
    let release = root.join("cf34-release");
    let executable = std::env::current_exe().expect("the test binary");
    let mut child = Command::new(executable)
        .args(["--exact", test_name, "--nocapture"])
        .env(HOLD_ENV, root)
        .env(RELEASE_ENV, &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the lock holder");
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).expect("child line");
        assert_ne!(read, 0, "the lock holder exited without acquiring");
        if line.starts_with("CF34 acquired ") {
            break;
        }
    }
    (child, release, reader)
}

fn finish_holder(mut child: Child, release: &Path, reader: BufReader<std::process::ChildStdout>) {
    fs::write(release, b"go").expect("release the holder");
    let status = child.wait().expect("wait for the holder");
    drop(reader);
    assert!(status.success(), "the lock holder failed: {status}");
}

#[test]
fn a_customised_refusal_lands_before_the_run_wide_lock() {
    if std::env::var_os(HOLD_ENV).is_some() {
        let root = PathBuf::from(std::env::var_os(HOLD_ENV).expect("root"));
        let release = PathBuf::from(std::env::var_os(RELEASE_ENV).expect("release"));
        hold_lock_child(&root, &release);
        return;
    }
    let fixture = Fixture::new("activated_directives:\n  - 024-locality-of-change\n");
    let plan = plan_preset_application(fixture.path(), "built-in", "minimal").expect("the plan");
    assert_eq!(
        plan.customised_changes,
        vec!["activated_directives".to_owned()]
    );
    let (child, release, reader) = hold_lock_elsewhere(
        "a_customised_refusal_lands_before_the_run_wide_lock",
        fixture.path(),
    );
    let started = Instant::now();
    let error = apply_preset_plan(fixture.path(), &plan, false).expect_err("customised");
    let elapsed = started.elapsed();
    finish_holder(child, &release, reader);
    assert_eq!(error.code(), "PRESET_WOULD_OVERWRITE");
    assert!(
        elapsed < Duration::from_secs(3),
        "the refusal must not wait on the run-wide lock (waited {elapsed:?})"
    );
    assert_eq!(
        fixture.activation_text(),
        "activated_directives:\n  - 024-locality-of-change\n",
        "nothing was written"
    );
}

#[test]
fn the_publish_and_restamp_transaction_owns_the_run_wide_lock() {
    if std::env::var_os(HOLD_ENV).is_some() {
        let root = PathBuf::from(std::env::var_os(HOLD_ENV).expect("root"));
        let release = PathBuf::from(std::env::var_os(RELEASE_ENV).expect("release"));
        hold_lock_child(&root, &release);
        return;
    }
    let fixture = Fixture::new("");
    one_directive_preset(&fixture);
    fixture.seed_manifest("bundle_content_hash: sha256:stale\n");
    let plan = plan_preset_application(fixture.path(), "acme", "one").expect("the plan");
    let before = fixture.activation_text();
    let (child, release, reader) = hold_lock_elsewhere(
        "the_publish_and_restamp_transaction_owns_the_run_wide_lock",
        fixture.path(),
    );
    let started = Instant::now();
    let error = apply_preset_plan(fixture.path(), &plan, false).expect_err("lock held");
    let elapsed = started.elapsed();
    finish_holder(child, &release, reader);
    assert!(
        matches!(
            error,
            PresetApplicationError::Doctrine(DoctrineError::SynthesisLockHeld { .. })
        ),
        "{error:?}"
    );
    assert!(
        elapsed >= Duration::from_secs(9),
        "the write waits the bounded run-wide lock timeout (waited {elapsed:?})"
    );
    assert_eq!(fixture.activation_text(), before, "nothing was written");

    // With the lock free the same plan lands and restamps.
    let outcome = apply_preset_plan(fixture.path(), &plan, false).expect("the write");
    assert!(outcome.restamp.as_ref().is_some_and(|r| r.is_restamped()));
}

#[test]
fn a_plain_sibling_project_never_blocks_the_lock() {
    if std::env::var_os(HOLD_ENV).is_some() {
        let root = PathBuf::from(std::env::var_os(HOLD_ENV).expect("root"));
        let release = PathBuf::from(std::env::var_os(RELEASE_ENV).expect("release"));
        hold_lock_child(&root, &release);
        return;
    }
    let fixture = Fixture::new("");
    let sibling = Fixture::new("");
    one_directive_preset(&sibling);
    sibling.seed_manifest("bundle_content_hash: sha256:stale\n");
    // A second process holds THIS project's lock; the sibling's write is independent.
    let (child, release, reader) = hold_lock_elsewhere(
        "a_plain_sibling_project_never_blocks_the_lock",
        fixture.path(),
    );
    let plan = plan_preset_application(sibling.path(), "acme", "one").expect("the sibling plan");
    let started = Instant::now();
    let outcome = apply_preset_plan(sibling.path(), &plan, false).expect("the sibling write");
    let elapsed = started.elapsed();
    finish_holder(child, &release, reader);
    assert!(
        outcome
            .restamp
            .as_ref()
            .is_some_and(|restamp| restamp.is_restamped()),
        "another worktree's lock namespace never blocks this one"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the sibling never waited for the other namespace (waited {elapsed:?})"
    );
}
