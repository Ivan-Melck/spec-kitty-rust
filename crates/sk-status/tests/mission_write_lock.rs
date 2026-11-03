//! The shared Mission-write lock protocol: one key per Mission, canonical
//! primary-vs-coordination aliases, registered holds, rekey extension, birth and
//! fallback locks, refusal, timeout, and cross-language contention with the
//! pinned Python oracle.
//!
//! Every fixture is a temporary repository; nothing here touches a real
//! checkout, runtime directory, or the user's home.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use sk_kernel::KernelError;
use sk_kernel::locks::LockWait;
use sk_status::mutation::{
    MutationError, acquire_mission_birth_lock, acquire_mission_lock, acquire_mission_lock_with,
    extend_mission_lock_to_moved_key, mission_lock_key, mission_lock_key_with, mission_lock_path,
};

const SLUG: &str = "060-test";
const MID8: &str = "01COORD0";
const COORD_NAME: &str = "060-test-01COORD0";
const MISSION_ID: &str = "01TESTMISSIONIDXXXXXXXXXX";
const WAIT: Duration = Duration::from_secs(5);

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "lock@example.invalid"]);
    git(root, &["config", "user.name", "Lock Fixture"]);
}

fn write_meta(directory: &Path, meta: &serde_json::Value) -> PathBuf {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join("meta.json"), meta.to_string()).unwrap();
    directory.to_path_buf()
}

/// A repository with a legacy bare-directory coordination Mission: the primary
/// directory is `kitty-specs/060-test` and the coordination surface is named by
/// the composed key `060-test-01COORD0`.
struct CoordMission {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    primary: PathBuf,
    coord: PathBuf,
}

fn coord_mission() -> CoordMission {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let primary = write_meta(
        &repo.join("kitty-specs").join(SLUG),
        &serde_json::json!({
            "mission_slug": SLUG,
            "mission_id": MISSION_ID,
            "mid8": MID8,
            "coordination_branch": format!("kitty/mission-{COORD_NAME}"),
        }),
    );
    let coord = repo
        .join(".worktrees")
        .join(format!("{COORD_NAME}-coord"))
        .join("kitty-specs")
        .join(COORD_NAME);
    fs::create_dir_all(&coord).unwrap();
    CoordMission {
        _temp: temp,
        repo,
        primary,
        coord,
    }
}

#[test]
fn the_primary_and_coordination_directories_take_one_lock_file() {
    let mission = coord_mission();
    let via_primary = mission_lock_path(&mission.primary, Some(&mission.repo)).unwrap();
    let via_coord = mission_lock_path(&mission.coord, Some(&mission.repo)).unwrap();
    assert_eq!(via_primary, via_coord);
    assert!(via_primary.ends_with(format!("{COORD_NAME}.status.lock")));
    assert_eq!(
        mission_lock_key(&mission.primary, &mission.repo).unwrap(),
        COORD_NAME
    );
    assert_eq!(
        mission_lock_key(&mission.coord, &mission.repo).unwrap(),
        COORD_NAME
    );
}

#[test]
fn a_linked_worktree_cwd_and_a_primary_cwd_converge_on_the_coordination_key() {
    let mission = coord_mission();
    git(&mission.repo, &["add", "-A"]);
    git(&mission.repo, &["commit", "-q", "-m", "seed"]);
    let worktree = mission.repo.join(".worktrees").join("lane-a");
    git(
        &mission.repo,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "-b",
            "lane-a",
        ],
    );
    let worktree_feature = worktree.join("kitty-specs").join(SLUG);
    assert!(worktree_feature.is_dir());

    // A stale lane copy must never decide the key (the canonical primary meta
    // does), even when the lane is the caller's CWD-shaped input.
    fs::write(
        worktree_feature.join("meta.json"),
        serde_json::json!({"mission_slug": SLUG}).to_string(),
    )
    .unwrap();

    let from_main = mission_lock_path(&mission.primary, Some(&mission.repo)).unwrap();
    let from_linked_cwd = mission_lock_path(&worktree_feature, None).unwrap();
    let from_coord = mission_lock_path(&mission.coord, Some(&mission.repo)).unwrap();
    assert_eq!(from_main, from_linked_cwd);
    assert_eq!(from_main, from_coord);
    assert_eq!(
        mission_lock_key_with(&worktree_feature, None, false).unwrap(),
        COORD_NAME
    );
}

#[test]
fn nested_use_through_the_coordination_directory_reuses_the_primary_alias() {
    let mission = coord_mission();
    let coord_lock =
        acquire_mission_lock(&mission.coord, Some(&mission.repo), LockWait::Bounded(WAIT))
            .expect("coordination lock");
    assert_eq!(coord_lock.key(), COORD_NAME);
    let lock_file = coord_lock.path().to_path_buf();

    // Move the recorded identity inside the hold: the alias registration is the
    // only thing that still ties the primary directory name to the held key.
    fs::write(
        mission.primary.join("meta.json"),
        serde_json::json!({"mission_slug": SLUG}).to_string(),
    )
    .unwrap();
    assert_eq!(
        mission_lock_key(&mission.primary, &mission.repo).unwrap(),
        COORD_NAME,
        "a nested entry through the primary directory must reuse the held key"
    );
    let nested = acquire_mission_lock(
        &mission.primary,
        Some(&mission.repo),
        LockWait::Bounded(WAIT),
    )
    .expect("nested lock");
    assert_eq!(nested.path(), lock_file);
    assert_eq!(nested.key(), COORD_NAME);

    // The holder sidecar belongs to the outermost acquire and survives the
    // nested drop.
    let sidecar = lock_file.with_file_name(format!(
        "{}.holder",
        lock_file.file_name().unwrap().to_string_lossy()
    ));
    assert!(sidecar.is_file());
    drop(nested);
    assert!(sidecar.is_file());
    drop(coord_lock);
    assert!(!sidecar.exists());
    // Outside the hold the key follows the flattened meta again.
    assert_eq!(
        mission_lock_key(&mission.primary, &mission.repo).unwrap(),
        SLUG
    );
}

#[test]
fn a_nested_entry_reuses_the_held_key_after_meta_changes_inside_the_hold() {
    let mission = coord_mission();
    let outer = acquire_mission_lock(
        &mission.primary,
        Some(&mission.repo),
        LockWait::Bounded(WAIT),
    )
    .expect("outer");
    fs::write(
        mission.primary.join("meta.json"),
        serde_json::json!({"mission_slug": SLUG}).to_string(),
    )
    .unwrap();
    assert_eq!(
        mission_lock_key(&mission.primary, &mission.repo).unwrap(),
        COORD_NAME
    );
    assert_eq!(
        mission_lock_key_with(&mission.primary, Some(&mission.repo), true).unwrap(),
        SLUG
    );
    let inner = acquire_mission_lock(
        &mission.primary,
        Some(&mission.repo),
        LockWait::Bounded(WAIT),
    )
    .expect("inner");
    assert_eq!(outer.path(), inner.path());
    drop(inner);
    drop(outer);
    assert_eq!(
        mission_lock_key(&mission.primary, &mission.repo).unwrap(),
        SLUG
    );
}

#[test]
fn a_writer_through_the_coordination_directory_is_excluded_by_a_primary_hold() {
    let mission = coord_mission();
    let feature = mission.primary.clone();
    let repo = mission.repo.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _lock = acquire_mission_lock(&feature, Some(&repo), LockWait::Bounded(WAIT)).unwrap();
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    ready_rx.recv_timeout(WAIT).unwrap();

    let error = acquire_mission_lock(
        &mission.coord,
        Some(&mission.repo),
        LockWait::Bounded(Duration::from_millis(100)),
    )
    .expect_err("a coordination-dir writer must not enter a primary-dir hold");
    match error {
        MutationError::Kernel(KernelError::StatusLockTimeout { holder, .. }) => {
            assert!(holder.contains("held by pid"));
        }
        other => panic!("unexpected error: {other}"),
    }
    release_tx.send(()).unwrap();
    holder.join().unwrap();
}

#[test]
fn an_unresolved_modern_coordination_key_is_refused_unless_fallback_is_requested() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().to_path_buf();
    init_repo(&repo);
    let modern = write_meta(
        &repo.join("kitty-specs").join("no-number-mission"),
        &serde_json::json!({
            "mission_slug": "no-number-mission",
            "coordination_branch": "kitty/mission-x",
        }),
    );
    let error = acquire_mission_lock(&modern, Some(&repo), LockWait::Bounded(WAIT))
        .expect_err("a lost identity must fail closed");
    assert!(matches!(error, MutationError::LockKeyUnresolved { .. }));

    let fallback =
        acquire_mission_lock_with(&modern, Some(&repo), LockWait::Bounded(WAIT), true, false)
            .expect("fallback");
    assert_eq!(fallback.key(), "no-number-mission");
}

#[test]
fn a_birth_lock_keys_on_the_directory_name_without_reading_meta() {
    let mission = coord_mission();
    let post_birth = mission_lock_path(&mission.primary, Some(&mission.repo)).unwrap();
    let birth = acquire_mission_birth_lock(
        &mission.primary,
        Some(&mission.repo),
        LockWait::Bounded(WAIT),
    )
    .expect("birth lock");
    assert_eq!(birth.key(), SLUG);
    assert!(birth.path().ends_with(format!("{SLUG}.status.lock")));
    assert_ne!(
        birth.path(),
        post_birth,
        "the post-birth key is the coordination name; the birth key serializes pre-meta writers"
    );
    // Inside the birth hold the registered hold answers with the birth key,
    // exactly as the baseline's held-key rule does.
    assert_eq!(
        mission_lock_key(&mission.primary, &mission.repo).unwrap(),
        SLUG
    );
}

#[test]
fn a_legacy_numbered_mission_with_no_mid8_keys_on_the_bare_directory_name() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().to_path_buf();
    init_repo(&repo);
    let legacy = write_meta(
        &repo.join("kitty-specs").join("042-legacy"),
        &serde_json::json!({
            "mission_slug": "042-legacy",
            "coordination_branch": "kitty/mission-042-legacy",
        }),
    );
    assert_eq!(mission_lock_key(&legacy, &repo).unwrap(), "042-legacy");
    let lock = acquire_mission_lock(&legacy, Some(&repo), LockWait::Bounded(WAIT)).unwrap();
    assert_eq!(lock.key(), "042-legacy");
}

#[test]
fn untrusted_unicode_mission_identity_is_sliced_by_characters_not_bytes() {
    // Nine multi-byte characters: a byte slice at offset 8 would split the
    // fifth character's encoding. The baseline takes eight characters.
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().to_path_buf();
    init_repo(&repo);
    let mission_id = "ééééééééé";
    let mission = write_meta(
        &repo.join("kitty-specs").join("unicode-demo"),
        &serde_json::json!({
            "mission_slug": "unicode-demo",
            "mission_id": mission_id,
            "coordination_branch": "kitty/mission-unicode-demo",
        }),
    );
    let key = mission_lock_key(&mission, &repo).expect("no panic and a key");
    let expected_mid8: String = mission_id.chars().take(8).collect();
    assert_eq!(key, format!("unicode-demo-{expected_mid8}"));
}

#[test]
fn the_rekey_extension_holds_the_key_a_meta_write_moved() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().to_path_buf();
    init_repo(&repo);
    let legacy_slug = "042-legacy";
    let moved_mid = "01MOVED0XXXXXXXXXXXXXXXXXX";
    let moved_key = format!("{legacy_slug}-{}", &moved_mid[..8]);
    let mission = write_meta(
        &repo.join("kitty-specs").join(legacy_slug),
        &serde_json::json!({
            "mission_slug": legacy_slug,
            "coordination_branch": format!("kitty/mission-{legacy_slug}"),
        }),
    );

    let _hold = acquire_mission_lock(&mission, Some(&repo), LockWait::Bounded(WAIT)).unwrap();
    assert_eq!(mission_lock_key(&mission, &repo).unwrap(), legacy_slug);
    fs::write(
        mission.join("meta.json"),
        serde_json::json!({
            "mission_slug": legacy_slug,
            "mission_id": moved_mid,
            "coordination_branch": format!("kitty/mission-{legacy_slug}"),
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        mission_lock_key_with(&mission, Some(&repo), true).unwrap(),
        moved_key
    );
    assert_eq!(mission_lock_key(&mission, &repo).unwrap(), legacy_slug);

    {
        let extension =
            extend_mission_lock_to_moved_key(&mission, Some(&repo), LockWait::Bounded(WAIT))
                .expect("extension");
        assert!(extension.moved());
        assert!(
            extension
                .path()
                .is_some_and(|path| path.ends_with(format!("{moved_key}.status.lock")))
        );
        assert!(!lock_is_free_in_another_thread(&repo, &moved_key));
    }
    assert!(
        lock_is_free_in_another_thread(&repo, &moved_key),
        "dropping the extension must release the moved key"
    );
    assert!(!lock_is_free_in_another_thread(&repo, legacy_slug));
}

#[test]
fn the_rekey_extension_reports_no_move_and_requires_an_enclosing_hold() {
    let mission = coord_mission();
    let error = extend_mission_lock_to_moved_key(
        &mission.primary,
        Some(&mission.repo),
        LockWait::Bounded(WAIT),
    )
    .expect_err("no enclosing hold");
    assert!(matches!(error, MutationError::ExtensionRequiresHold { .. }));

    let _hold = acquire_mission_lock(
        &mission.primary,
        Some(&mission.repo),
        LockWait::Bounded(WAIT),
    )
    .unwrap();
    let extension = extend_mission_lock_to_moved_key(
        &mission.primary,
        Some(&mission.repo),
        LockWait::Bounded(WAIT),
    )
    .expect("extension");
    assert!(!extension.moved());
    assert!(extension.path().is_none());
}

#[test]
fn the_rekey_extension_times_out_without_releasing_the_enclosing_key() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().to_path_buf();
    init_repo(&repo);
    let legacy_slug = "042-legacy";
    let moved_mid = "01MOVED0XXXXXXXXXXXXXXXXXX";
    let moved_key = format!("{legacy_slug}-{}", &moved_mid[..8]);
    let mission = write_meta(
        &repo.join("kitty-specs").join(legacy_slug),
        &serde_json::json!({
            "mission_slug": legacy_slug,
            "coordination_branch": format!("kitty/mission-{legacy_slug}"),
        }),
    );

    let _hold = acquire_mission_lock(&mission, Some(&repo), LockWait::Bounded(WAIT)).unwrap();
    fs::write(
        mission.join("meta.json"),
        serde_json::json!({
            "mission_slug": legacy_slug,
            "mission_id": moved_mid,
            "coordination_branch": format!("kitty/mission-{legacy_slug}"),
        })
        .to_string(),
    )
    .unwrap();

    // Another writer already holds the moved key (it read the same meta).
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let moved_repo = repo.clone();
    let moved_for_thread = moved_key.clone();
    let holder = std::thread::spawn(move || {
        let moved_dir = moved_repo.join("kitty-specs").join(&moved_for_thread);
        fs::create_dir_all(&moved_dir).unwrap();
        let _new =
            acquire_mission_lock(&moved_dir, Some(&moved_repo), LockWait::Bounded(WAIT)).unwrap();
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    ready_rx.recv_timeout(WAIT).unwrap();

    let error = extend_mission_lock_to_moved_key(
        &mission,
        Some(&repo),
        LockWait::Bounded(Duration::from_millis(100)),
    )
    .expect_err("the moved key is held elsewhere");
    assert!(matches!(
        error,
        MutationError::Kernel(KernelError::StatusLockTimeout { .. })
    ));
    assert!(!lock_is_free_in_another_thread(&repo, legacy_slug));
    release_tx.send(()).unwrap();
    holder.join().unwrap();
}

fn lock_is_free_in_another_thread(repo: &Path, key: &str) -> bool {
    let repo = repo.to_path_buf();
    let key = key.to_owned();
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let common = sk_kernel::lock_families::status_lock_common_dir(&repo).unwrap();
        let outcome = sk_kernel::lock_families::acquire_feature_status_lock(
            &common,
            &key,
            LockWait::Bounded(Duration::from_millis(100)),
        )
        .is_ok();
        tx.send(outcome).unwrap();
    });
    thread.join().unwrap();
    rx.recv().unwrap()
}

// ---------------------------------------------------------------------------
// Cross-language contention with the pinned Python oracle
// ---------------------------------------------------------------------------

const ORACLE_SCRIPT: &str = r#"
import sys
from pathlib import Path

from specify_cli.status.locking import FeatureStatusLockTimeoutError
from specify_cli.status.mission_write import mission_write_lock

mode, mission, repo = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
if mode == "hold":
    with mission_write_lock(mission, repo_root=repo):
        print("READY", flush=True)
        sys.stdin.readline()
    print("RELEASED", flush=True)
elif mode == "try":
    try:
        with mission_write_lock(mission, repo_root=repo, timeout=0.2):
            print("ACQUIRED", flush=True)
    except FeatureStatusLockTimeoutError:
        print("TIMEOUT", flush=True)
        sys.exit(3)
else:
    raise SystemExit(2)
"#;

fn oracle_repo_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|candidate| candidate.join("src/specify_cli").is_dir())?
        .to_path_buf();
    let python = root.join(".venv-rust-oracle/bin/python");
    python.is_file().then_some(root)
}

struct OracleProcess {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
}

impl OracleProcess {
    fn spawn(root: &Path, script: &Path, mode: &str, mission: &Path, repo: &Path) -> Self {
        // Keep the oracle from writing bytecode or per-user state into the
        // checkout/home: the fixture repository's own temporary parent is the
        // only writable location it is given.
        let temp_home = repo.parent().unwrap_or(repo);
        let mut child = Command::new(root.join(".venv-rust-oracle/bin/python"))
            .arg(script)
            .arg(mode)
            .arg(mission)
            .arg(repo)
            .current_dir(root)
            .env("PYTHONPATH", root.join("src"))
            .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("HOME", temp_home)
            .env("XDG_CACHE_HOME", temp_home.join(".cache"))
            .env("XDG_CONFIG_HOME", temp_home.join(".config"))
            .env("XDG_DATA_HOME", temp_home.join(".local/share"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn oracle");
        let stdout = BufReader::new(child.stdout.take().expect("oracle stdout"));
        Self { child, stdout }
    }

    fn read_line(&mut self) -> String {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read oracle line");
        line.trim().to_owned()
    }

    fn release(&mut self) {
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"\n");
            let _ = stdin.flush();
        }
    }

    fn finish(mut self) -> std::process::ExitStatus {
        self.release();
        self.child.wait().expect("wait for oracle")
    }
}

fn oracle_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let mission = repo.join("kitty-specs").join("cross-lang-01ABCDEF");
    fs::create_dir_all(&mission).unwrap();
    fs::write(
        mission.join("meta.json"),
        serde_json::json!({
            "mission_slug": "cross-lang",
            "mission_id": "01ABCDEFXXXXXXXXXXXXXXXXXX",
        })
        .to_string(),
    )
    .unwrap();
    let script = temp.path().join("oracle_lock.py");
    fs::write(&script, ORACLE_SCRIPT).unwrap();
    (temp, repo, mission, script)
}

#[test]
fn the_python_oracle_cannot_enter_a_held_native_mission_lock() {
    let Some(root) = oracle_repo_root() else {
        eprintln!("skipping: pinned Rust oracle is not provisioned");
        return;
    };
    let (_temp, repo, mission, script) = oracle_fixture();
    let _lock = acquire_mission_lock(&mission, Some(&repo), LockWait::Bounded(WAIT)).unwrap();

    let mut contender = OracleProcess::spawn(&root, &script, "try", &mission, &repo);
    assert_eq!(contender.read_line(), "TIMEOUT");
    assert_eq!(contender.finish().code(), Some(3));
}

#[test]
fn a_native_writer_cannot_enter_a_held_python_oracle_lock() {
    let Some(root) = oracle_repo_root() else {
        eprintln!("skipping: pinned Rust oracle is not provisioned");
        return;
    };
    let (_temp, repo, mission, script) = oracle_fixture();
    let mut holder = OracleProcess::spawn(&root, &script, "hold", &mission, &repo);
    assert_eq!(holder.read_line(), "READY");

    let error = acquire_mission_lock(
        &mission,
        Some(&repo),
        LockWait::Bounded(Duration::from_millis(200)),
    )
    .expect_err("a native writer must not enter the oracle's hold");
    match error {
        MutationError::Kernel(KernelError::StatusLockTimeout { holder, .. }) => {
            assert!(holder.contains("held by pid"));
        }
        other => panic!("unexpected error: {other}"),
    }

    holder.release();
    assert_eq!(holder.read_line(), "RELEASED");
    assert!(holder.finish().success());
    // After release the same native door takes the lock.
    let lock = acquire_mission_lock(&mission, Some(&repo), LockWait::Bounded(WAIT)).unwrap();
    drop(lock);
}
