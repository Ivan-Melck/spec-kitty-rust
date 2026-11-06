//! Captured operator-index authority: a real `GIT_INDEX_FILE` is honoured.
//!
//! These tests exercise the actual selector through a child process, because the
//! authority is captured from the process environment at repository open and the
//! process environment must never be mutated by the adapter. Each child prints
//! the authority it captured, then performs one real safe commit. The parent
//! inspects the repository and both indexes afterwards:
//!
//! * the caller's index receives the commit's reconciliation;
//! * `<git-dir>/index` is byte-identical;
//! * a relative selector resolves against the work-tree root (git's own
//!   semantics), not against the subdirectory the adapter was opened from;
//! * a symlinked index path keeps its spelling and is not retargeted by the
//!   adapter (git's own follow-through behaviour applies);
//! * the process environment still holds the caller's value after a commit.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SELECTORS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_NAMESPACE",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
];

fn git_base(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("TMPDIR", root);
    for variable in SELECTORS {
        command.env_remove(variable);
    }
    command.env_remove("GIT_INDEX_FILE");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    git_with_index(root, args, None)
}

fn git_indexed(root: &Path, args: &[&str], index: &Path) -> Vec<u8> {
    git_with_index(root, args, Some(index))
}

fn git_with_index(root: &Path, args: &[&str], index: Option<&Path>) -> Vec<u8> {
    let mut command = git_base(root);
    command.args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        for directory in [
            "", "home", "config", "data", "state", "cache", "tmp", "sub/deep",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        git(&root, &["init", "-q", "-b", "work"]);
        git(&root, &["config", "user.name", "Fixture"]);
        git(&root, &["config", "user.email", "fixture@example.invalid"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        fs::write(root.join("seed.txt"), b"seed\n").unwrap();
        fs::write(root.join("a.txt"), b"one\n").unwrap();
        fs::write(root.join("other.txt"), b"base\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "initial"]);
        fs::write(root.join("a.txt"), b"two\n").unwrap();
        Self { _temp: temp, root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn head(&self) -> String {
        String::from_utf8_lossy(&git(&self.root, &["rev-parse", "HEAD"]))
            .trim()
            .to_string()
    }

    fn default_index(&self) -> Vec<u8> {
        fs::read(self.root.join(".git/index")).unwrap()
    }

    fn index_entries(&self, index: &Path) -> Vec<String> {
        let raw = git_indexed(&self.root, &["ls-files", "-s", "-z"], index);
        raw.split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(|record| String::from_utf8_lossy(record).into_owned())
            .collect()
    }

    /// Create an index that is `HEAD` plus one unrelated staged file.
    fn make_custom_index(&self, index: &Path) {
        git_indexed(&self.root, &["read-tree", "HEAD"], index);
        fs::write(self.root.join("unrelated.txt"), b"u\n").unwrap();
        git_indexed(&self.root, &["add", "unrelated.txt"], index);
    }
}

struct ChildRun {
    authority: PathBuf,
    source: String,
    environment: String,
    // Successful-outcome observation only. `<none>` on an error does not
    // establish no land: tests also inspect HEAD and the typed error text.
    commit: String,
    lock: PathBuf,
    stdout: String,
}

impl ChildRun {
    /// One `name=value` line printed by the child, if it printed it.
    fn field(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}=");
        self.stdout
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
    }
}

fn run_child(
    root: &Path,
    open_path: &Path,
    cwd: &Path,
    index: Option<&Path>,
    extra: &[(&str, &str)],
) -> ChildRun {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "captured_index_child",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(cwd)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("TMPDIR", root.join("tmp"))
        .env("SK_INDEX_ROOT", open_path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000");
    for (key, value) in extra {
        command.env(key, value);
    }
    for variable in SELECTORS {
        command.env_remove(variable);
    }
    match index {
        Some(index) => command.env("GIT_INDEX_FILE", index),
        None => command.env_remove("GIT_INDEX_FILE"),
    };
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "index-authority child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let field = |name: &str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("child did not print {name}:\n{stdout}"))
            .to_string()
    };
    ChildRun {
        authority: PathBuf::from(field("authority")),
        source: field("source"),
        environment: field("env"),
        commit: field("commit"),
        lock: PathBuf::from(field("lock")),
        stdout,
    }
}

/// The script text a fixture installs as `.git/hooks/pre-commit`.
///
/// It attempts a Git write against the *original* captured index spelling
/// (passed explicitly, because git replaces `GIT_INDEX_FILE` for hooks with the
/// index spelling it is itself using), records that attempt's own exit status
/// without refusing the outer commit, and records whether each candidate lock
/// file exists while the outer commit is running. The source `safe_commit` was
/// measured first against the same shape (see
/// `crates/sk-git/INDEX-LOCK-ENDPOINT-REPORT.md`): its staging add is refused
/// with exit 128 and the captured index bytes do not change, because git's own
/// commit holds the resolved target's `.lock`.
#[cfg(unix)]
const HOOK_ATTEMPTS_CAPTURED_INDEX_WRITE: &str = r#"#!/bin/sh
log="$SK_HOOK_LOG"
orig="$SK_ORIGINAL_INDEX"
{
  echo "visible_git_index_file=${GIT_INDEX_FILE-<unset>}"
  echo "spelled_lock=$(test -e "$SK_SPELLED_LOCK" && echo yes || echo no)"
  echo "target_lock=$(test -e "$SK_TARGET_LOCK" && echo yes || echo no)"
  printf 'hook-owned\n' > hooked.txt
  echo "before_sha=$(sha256sum "$SK_TARGET_INDEX" | cut -d' ' -f1)"
  err=$(GIT_INDEX_FILE="$orig" git add -- hooked.txt 2>&1)
  echo "add_exit=$?"
  echo "add_stderr=$(printf '%s' "$err" | tr '\n' '|')"
  echo "after_sha=$(sha256sum "$SK_TARGET_INDEX" | cut -d' ' -f1)"
} >> "$log" 2>&1
exit 0
"#;

#[cfg(unix)]
fn install_hook(root: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;

    let hook = root.join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().expect("hooks directory")).unwrap();
    fs::write(&hook, script).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
}

/// The path a lock file is expected at: `<index spelling with .lock appended>`.
fn lock_of(index: &Path) -> PathBuf {
    let mut lock = index.as_os_str().to_os_string();
    lock.push(".lock");
    PathBuf::from(lock)
}

#[test]
fn an_absolute_custom_index_is_honoured_and_the_default_index_is_untouched() {
    let fixture = Fixture::new();
    let custom = fixture.path().join("custom-index");
    fixture.make_custom_index(&custom);
    let before_default = fixture.default_index();
    let before_custom = fixture.index_entries(&custom);

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&custom),
        &[],
    );

    assert_eq!(run.source, "environment");
    assert_eq!(run.authority, custom);
    assert_eq!(run.environment, custom.display().to_string());
    assert_eq!(fixture.head(), run.commit);
    assert_eq!(
        fixture.default_index(),
        before_default,
        "the default index must not be written when a custom index is captured"
    );
    let after_custom = fixture.index_entries(&custom);
    assert_ne!(
        after_custom, before_custom,
        "the custom index must be reconciled"
    );
    let committed_blob = String::from_utf8_lossy(&git_indexed(
        fixture.path(),
        &["rev-parse", &format!("{}:a.txt", run.commit)],
        &custom,
    ))
    .trim()
    .to_string();
    let staged_blob = |entries: &[String], path: &str| -> Option<String> {
        entries
            .iter()
            .find(|entry| entry.ends_with(&format!("\t{path}")))
            .and_then(|entry| entry.split(' ').nth(1))
            .map(str::to_string)
    };
    assert_eq!(
        staged_blob(&after_custom, "a.txt").as_deref(),
        Some(committed_blob.as_str()),
        "the custom index must hold the committed blob: {after_custom:?}"
    );
    assert_eq!(
        staged_blob(&before_custom, "unrelated.txt"),
        staged_blob(&after_custom, "unrelated.txt"),
        "unrelated staged content in the custom index must survive"
    );
}

#[test]
fn a_relative_selector_resolves_against_the_work_tree_root() {
    let fixture = Fixture::new();
    let custom = fixture.path().join("custom-index");
    fixture.make_custom_index(&custom);
    let before_default = fixture.default_index();
    let sub = fixture.path().join("sub/deep");

    // The child is started in a subdirectory and opens "."; git resolves a
    // relative GIT_INDEX_FILE against the work-tree root it runs in.
    let run = run_child(
        fixture.path(),
        &sub,
        &sub,
        Some(Path::new("custom-index")),
        &[],
    );

    assert_eq!(run.source, "environment");
    assert_eq!(run.authority, custom);
    assert_eq!(run.environment, "custom-index");
    assert_eq!(fixture.head(), run.commit);
    assert!(
        !sub.join("custom-index").exists(),
        "a relative selector must not resolve against the probe subdirectory"
    );
    let committed_blob = String::from_utf8_lossy(&git_indexed(
        fixture.path(),
        &["rev-parse", &format!("{}:a.txt", run.commit)],
        &custom,
    ))
    .trim()
    .to_string();
    let entries = fixture.index_entries(&custom);
    assert!(
        entries
            .iter()
            .any(|entry| { entry.ends_with("\ta.txt") && entry.contains(&committed_blob) }),
        "the root-relative custom index must be the one reconciled: {entries:?}"
    );
    assert_eq!(
        fixture.default_index(),
        before_default,
        "the default index must not be written when a custom index is captured"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_index_keeps_its_spelling_and_gits_own_follow_through() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let real = fixture.path().join("real-index");
    let link = fixture.path().join("link-index");
    fixture.make_custom_index(&real);
    symlink("real-index", &link).unwrap();
    let before_default = fixture.default_index();
    let before_real = fs::read(&real).unwrap();

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&link),
        &[],
    );

    assert_eq!(run.source, "environment");
    assert_eq!(
        run.authority, link,
        "the adapter must preserve the symlink spelling, not resolve it to {real:?}"
    );
    assert!(
        !run.stdout
            .contains(&format!("authority={}", real.display())),
        "the authority must not be the symlink target"
    );
    assert_eq!(fixture.head(), run.commit);
    assert_eq!(fixture.default_index(), before_default);
    assert_ne!(
        fs::read(&real).unwrap(),
        before_real,
        "git follows the symlink and writes the target"
    );
    let metadata = fs::symlink_metadata(&link).unwrap();
    assert!(
        metadata.file_type().is_symlink(),
        "the symlink itself must survive the commit"
    );
}

// ---------------------------------------------------------------------------
// The lock endpoint is git's own: the final path component is chased.
//
// Git's lock setup resolves a symlinked final component and creates
// `<resolved target>.lock`; the measured source behaviour is that its own
// `safe_commit` therefore refuses a hook write to the captured index with
// exit 128 while the captured index bytes stay put. These are the permanent
// consumer regressions for that endpoint.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_hook_writing_the_captured_index_is_refused_at_gits_lock_endpoint() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let real = fixture.path().join("real-index");
    let link = fixture.path().join("link-index");
    let spelled_lock = lock_of(&link);
    let target_lock = lock_of(&real);
    fixture.make_custom_index(&real);
    symlink("real-index", &link).unwrap();
    let hook_log = fixture.path().join("hook.log");
    install_hook(fixture.path(), HOOK_ATTEMPTS_CAPTURED_INDEX_WRITE);
    let before_default = fixture.default_index();
    let before_real = fs::read(&real).unwrap();

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&link),
        &[
            ("SK_HOOK_LOG", hook_log.to_str().unwrap()),
            ("SK_ORIGINAL_INDEX", "link-index"),
            ("SK_TARGET_INDEX", real.to_str().unwrap()),
            ("SK_SPELLED_LOCK", spelled_lock.to_str().unwrap()),
            ("SK_TARGET_LOCK", target_lock.to_str().unwrap()),
        ],
    );

    assert_eq!(run.source, "environment");
    let log = fs::read_to_string(&hook_log).expect("the pre-commit hook must run");
    let log_field = |name: &str| -> String {
        let prefix = format!("{name}=");
        log.lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("the hook did not record {name}:\n{log}"))
            .to_string()
    };
    assert_eq!(
        log_field("target_lock"),
        "yes",
        "the resolved target's lock is held while the hook runs:\n{log}"
    );
    assert_eq!(
        log_field("spelled_lock"),
        "no",
        "the spelled candidate is not git's lock name:\n{log}"
    );
    assert_eq!(
        log_field("add_exit"),
        "128",
        "a hook write to the captured index is refused exactly as git refuses it:\n{log}"
    );
    assert_eq!(
        log_field("before_sha"),
        log_field("after_sha"),
        "the refused hook write must not change the captured index:\n{log}"
    );

    assert_eq!(fixture.head(), run.commit, "the outer commit still lands");
    assert_eq!(
        run.field("object_type"),
        Some("commit"),
        "the landed object is a real commit: {}",
        run.stdout
    );
    assert!(
        run.field("object_size")
            .and_then(|size| size.parse::<u64>().ok())
            .is_some_and(|size| size > 0),
        "the landed commit has a bounded body: {}",
        run.stdout
    );
    assert_eq!(
        run.field("object_parents"),
        Some("1"),
        "the ordinary route creates one parent, never a fabricated one: {}",
        run.stdout
    );
    assert_eq!(fixture.default_index(), before_default);
    let entries = fixture.index_entries(&real);
    assert!(
        entries.iter().any(|entry| entry.ends_with("\ta.txt")),
        "the requested path is reconciled into the captured index: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.ends_with("\tunrelated.txt")),
        "unrelated staged content survives: {entries:?}"
    );
    assert!(
        !entries.iter().any(|entry| entry.ends_with("\thooked.txt")),
        "the hook's refused write must not be staged: {entries:?}"
    );
    assert_ne!(
        fs::read(&real).unwrap(),
        before_real,
        "the commit still reconciles the requested path into the captured index"
    );
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the captured spelling stays a symlink"
    );
    assert!(!spelled_lock.exists(), "no lock is left at the candidate");
    assert!(!target_lock.exists(), "the owned lock is released");
    assert_eq!(
        run.lock,
        root.join("real-index.lock"),
        "the commit must hold the lock git itself uses for the captured index: {}",
        run.stdout
    );
}

#[cfg(unix)]
#[test]
fn a_foreign_lock_at_the_resolved_endpoint_blocks_apply_and_is_preserved() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let real = fixture.path().join("real-index");
    let link = fixture.path().join("link-index");
    let target_lock = lock_of(&real);
    fixture.make_custom_index(&real);
    symlink("real-index", &link).unwrap();
    let before_default = fixture.default_index();
    let before_real = fs::read(&real).unwrap();
    let before_head = fixture.head();

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&link),
        &[
            ("SK_INDEX_MODE", "plan-foreign"),
            ("SK_TARGET_LOCK", target_lock.to_str().unwrap()),
        ],
    );

    let error = run.field("apply_error").unwrap_or("<missing>").to_string();
    assert!(
        error.starts_with("OperationInProgress:index-lock")
            || error.starts_with("SafeCommitIndexLocked"),
        "a foreign lock at git's endpoint must refuse the apply, got {error}:\n{}",
        run.stdout
    );
    assert_eq!(
        run.field("commit"),
        Some("<none>"),
        "no commit may land under a foreign lock"
    );
    assert_eq!(
        fs::read(&target_lock).unwrap(),
        b"foreign lock\n",
        "the foreign lock is preserved byte-for-byte"
    );
    assert_eq!(fs::read(&real).unwrap(), before_real);
    assert_eq!(fixture.default_index(), before_default);
    assert_eq!(fixture.head(), before_head);
    assert!(
        !lock_of(&link).exists(),
        "the spelled candidate is never locked"
    );
    assert_eq!(
        run.lock,
        root.join("real-index.lock"),
        "the checked lock is git's endpoint: {}",
        run.stdout
    );

    // The same foreign lock present *before* the read-only plan is refused
    // there, without any mutation.
    let fixture = Fixture::new();
    let real = fixture.path().join("real-index");
    let link = fixture.path().join("link-index");
    let target_lock = lock_of(&real);
    fixture.make_custom_index(&real);
    symlink("real-index", &link).unwrap();
    let before_real = fs::read(&real).unwrap();
    let before_head = fixture.head();
    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&link),
        &[
            ("SK_INDEX_MODE", "plan-foreign"),
            ("SK_TARGET_LOCK", target_lock.to_str().unwrap()),
            ("SK_FOREIGN_BEFORE_PLAN", "1"),
        ],
    );
    assert_eq!(
        run.field("plan_error"),
        Some("OperationInProgress:index-lock"),
        "a foreign lock at git's endpoint refuses the read-only plan: {}",
        run.stdout
    );
    assert_eq!(fs::read(&target_lock).unwrap(), b"foreign lock\n");
    assert_eq!(fs::read(&real).unwrap(), before_real);
    assert_eq!(fixture.head(), before_head);
    assert!(!lock_of(&link).exists());
}

#[cfg(unix)]
#[test]
fn a_link_retargeted_after_the_plan_moves_the_lock_to_the_new_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let real = fixture.path().join("real-index");
    let second = fixture.path().join("second-index");
    let link = fixture.path().join("link-index");
    fixture.make_custom_index(&real);
    git_indexed(fixture.path(), &["read-tree", "HEAD"], &second);
    let before_second = fs::read(&second).unwrap();
    let before_real = fs::read(&real).unwrap();
    symlink("real-index", &link).unwrap();
    let hook_log = fixture.path().join("hook.log");
    install_hook(fixture.path(), HOOK_ATTEMPTS_CAPTURED_INDEX_WRITE);

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&link),
        &[
            ("SK_INDEX_MODE", "retarget"),
            ("SK_LINK", link.to_str().unwrap()),
            ("SK_SECOND_TARGET", "second-index"),
            ("SK_HOOK_LOG", hook_log.to_str().unwrap()),
            ("SK_ORIGINAL_INDEX", "link-index"),
            ("SK_TARGET_INDEX", second.to_str().unwrap()),
            ("SK_SPELLED_LOCK", lock_of(&link).to_str().unwrap()),
            ("SK_TARGET_LOCK", lock_of(&second).to_str().unwrap()),
        ],
    );

    assert_eq!(run.field("apply_error"), Some("none"));
    assert_eq!(
        fixture.head(),
        run.commit,
        "the retarget is legal, not stale"
    );
    let log = fs::read_to_string(&hook_log).expect("the pre-commit hook must run");
    assert!(
        log.contains("target_lock=yes") && log.contains("spelled_lock=no"),
        "the lock moved with the retarget:\n{log}"
    );
    assert!(
        log.contains("add_exit=128"),
        "the hook write is still refused at the new endpoint:\n{log}"
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        PathBuf::from("second-index"),
        "the retargeted spelling is preserved"
    );
    assert_eq!(
        fs::read(&real).unwrap(),
        before_real,
        "the previous target is never written once the link moved"
    );
    let entries = fixture.index_entries(&second);
    assert!(
        entries.iter().any(|entry| entry.ends_with("\ta.txt")),
        "the commit reconciles the new target: {entries:?}"
    );
    assert_ne!(fs::read(&second).unwrap(), before_second);
    assert_eq!(
        PathBuf::from(run.field("retargeted_lock").expect("retargeted lock")),
        root.join("second-index.lock"),
        "the endpoint is derived when it is used, not frozen when the handle opened: {}",
        run.stdout
    );
    assert_eq!(
        PathBuf::from(run.field("planned_lock").expect("planned lock")),
        root.join("real-index.lock")
    );
    assert_eq!(run.lock, root.join("real-index.lock"));
}

#[cfg(unix)]
#[test]
fn a_link_chain_and_an_absolute_link_resolve_before_locking() {
    use std::os::unix::fs::symlink;

    // A two-link relative chain (git chases both).
    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let real = fixture.path().join("real-index");
    let middle = fixture.path().join("middle-index");
    let chain = fixture.path().join("chain-index");
    fixture.make_custom_index(&real);
    symlink("real-index", &middle).unwrap();
    symlink("middle-index", &chain).unwrap();

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&chain),
        &[],
    );
    assert_eq!(
        run.lock,
        root.join("real-index.lock"),
        "a link chain is resolved to the file git locks"
    );
    assert_eq!(fixture.head(), run.commit);
    assert!(
        fs::symlink_metadata(&chain)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the chain's spelling survives"
    );
    assert!(
        fs::symlink_metadata(&middle)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the intermediate link survives"
    );

    // An absolute target in a fresh fixture.
    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let real = fixture.path().join("real-index");
    let link = fixture.path().join("link-index");
    fixture.make_custom_index(&real);
    let absolute = fs::canonicalize(&real).unwrap();
    symlink(&absolute, &link).unwrap();

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&link),
        &[],
    );
    assert_eq!(run.lock, root.join("real-index.lock"));
    assert_eq!(fixture.head(), run.commit);
    assert_eq!(fs::read_link(&link).unwrap(), absolute);
}

#[cfg(unix)]
#[test]
fn a_dangling_final_link_is_locked_where_git_creates_the_index() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let link = fixture.path().join("link-index");
    symlink("not-yet-index", &link).unwrap();
    let before_default = fixture.default_index();

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&link),
        &[],
    );

    assert_eq!(run.lock, root.join("not-yet-index.lock"));
    let created = fixture.path().join("not-yet-index");
    assert!(
        created.is_file(),
        "git creates the index at a dangling link's target"
    );
    let entries = fixture.index_entries(&created);
    assert!(
        entries.iter().any(|entry| entry.ends_with("\ta.txt")),
        "the created target receives the reconciliation: {entries:?}"
    );
    assert_eq!(fixture.head(), run.commit);
    assert_eq!(
        fs::read_link(&link).unwrap(),
        PathBuf::from("not-yet-index")
    );
    assert_eq!(fixture.default_index(), before_default);
}

#[cfg(unix)]
#[test]
fn a_symlinked_default_index_is_locked_at_its_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.path()).unwrap();
    fs::create_dir_all(fixture.path().join("alt")).unwrap();
    let alt = fixture.path().join("alt/real-index");
    git_indexed(fixture.path(), &["read-tree", "HEAD"], &alt);
    fs::write(fixture.path().join("unrelated.txt"), b"u\n").unwrap();
    git_indexed(fixture.path(), &["add", "unrelated.txt"], &alt);
    fs::remove_file(fixture.path().join(".git/index")).unwrap();
    symlink("../alt/real-index", fixture.path().join(".git/index")).unwrap();
    let before_alt = fs::read(&alt).unwrap();

    let run = run_child(fixture.path(), fixture.path(), fixture.path(), None, &[]);

    assert_eq!(run.source, "default");
    assert_eq!(
        run.lock,
        root.join(".git/../alt/real-index.lock"),
        "the default index can itself be a symlink; its target is what git locks, \
joined without normalisation exactly as git spells it"
    );
    assert_eq!(fixture.head(), run.commit);
    assert!(
        fs::symlink_metadata(fixture.path().join(".git/index"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the `.git/index` symlink survives"
    );
    let entries = fixture.index_entries(&alt);
    assert!(
        entries.iter().any(|entry| entry.ends_with("\ta.txt")),
        "the target is reconciled: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.ends_with("\tunrelated.txt")),
        "unrelated staging survives: {entries:?}"
    );
    assert_ne!(fs::read(&alt).unwrap(), before_alt);
    assert!(
        !fixture.path().join(".git/index.lock").exists(),
        "the spelled `<git-dir>/index.lock` is not git's lock name here"
    );
}

#[test]
fn a_linked_worktree_default_index_lock_stays_in_its_own_git_dir() {
    let fixture = Fixture::new();
    let linked = fixture.path().join("linked");
    let linked_str = linked.to_str().unwrap();
    git(
        fixture.path(),
        &["worktree", "add", "-q", linked_str, "-b", "linked"],
    );
    let main_before = fixture.default_index();
    fs::write(linked.join("a.txt"), b"linked change\n").unwrap();

    let run = run_child(
        fixture.path(),
        &linked,
        &linked,
        None,
        &[("SK_BRANCH", "linked")],
    );

    assert_eq!(run.source, "default");
    assert!(
        run.lock.ends_with("worktrees/linked/index.lock"),
        "a linked checkout locks its own git directory: {}",
        run.lock.display()
    );
    let linked_head = String::from_utf8_lossy(&git(&linked, &["rev-parse", "HEAD"]))
        .trim()
        .to_string();
    assert_eq!(linked_head, run.commit);
    assert_eq!(
        fixture.default_index(),
        main_before,
        "the primary checkout's index is untouched"
    );
}

#[test]
fn without_a_selector_the_default_index_authority_is_used() {
    let fixture = Fixture::new();
    let before_default = fixture.default_index();
    let run = run_child(fixture.path(), fixture.path(), fixture.path(), None, &[]);

    assert_eq!(run.source, "default");
    assert_eq!(run.environment, "<unset>");
    assert_eq!(run.authority, fixture.path().join(".git/index"));
    assert_eq!(fixture.head(), run.commit);
    assert_ne!(
        fixture.default_index(),
        before_default,
        "the default index is the operator index when no selector is captured"
    );
}

#[test]
fn a_commit_under_a_captured_index_does_not_mutate_the_process_environment() {
    let fixture = Fixture::new();
    let custom = fixture.path().join("custom-index");
    fixture.make_custom_index(&custom);
    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&custom),
        &[],
    );
    assert_eq!(
        run.environment,
        custom.display().to_string(),
        "the adapter must not clear or rewrite the caller's GIT_INDEX_FILE"
    );
}

#[test]
fn a_merge_start_and_conclusion_write_only_the_captured_index() {
    let fixture = Fixture::new();
    // A side branch with one clean, non-conflicting commit.
    let base = fixture.head();
    git(&fixture.root, &["checkout", "-q", "-b", "side"]);
    fs::write(fixture.root.join("side.txt"), b"side\n").unwrap();
    git(&fixture.root, &["add", "-A"]);
    git(&fixture.root, &["commit", "-qm", "side"]);
    let theirs = fixture.head();
    git(&fixture.root, &["checkout", "-q", "work"]);
    assert_eq!(fixture.head(), base);

    let custom = fixture.path().join("merge-index");
    // A clean captured index (HEAD, no operator staging): a merge start refuses
    // a dirty checkout, and with GIT_INDEX_FILE set the index *is* the checkout
    // state git reports.
    git_indexed(&fixture.root, &["read-tree", "HEAD"], &custom);
    let before_default = fixture.default_index();

    let run = run_child(
        fixture.path(),
        fixture.path(),
        fixture.path(),
        Some(&custom),
        &[("SK_INDEX_MODE", "merge"), ("SK_MERGE_THEIRS", &theirs)],
    );

    assert_eq!(run.source, "environment");
    assert_eq!(
        fixture.default_index(),
        before_default,
        "the default index is untouched"
    );
    let entries = fixture.index_entries(&custom);
    assert!(
        entries.iter().any(|entry| entry.ends_with("\tside.txt")),
        "the merge wrote the captured index: {entries:?}"
    );
    let parents = String::from_utf8_lossy(&git(
        &fixture.root,
        &["rev-list", "--parents", "-n", "1", &run.commit],
    ))
    .trim()
    .to_string();
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "the conclusion created a two-parent merge commit: {parents}"
    );
    assert_eq!(run.commit, fixture.head());
    assert!(!fixture.root.join(".git/MERGE_HEAD").exists());
}

#[test]
#[ignore = "subprocess helper; the parent supplies the captured environment"]
fn captured_index_child() {
    use sk_git::{BranchName, GitPath, GitRepository, SafeCommitRequest};

    let open_path = PathBuf::from(std::env::var_os("SK_INDEX_ROOT").expect("SK_INDEX_ROOT"));
    assert_isolated_child(&open_path);
    let repository = GitRepository::open(&open_path).expect("open repository");
    let authority = repository
        .operator_index_path()
        .expect("operator index path");
    println!("authority={}", authority.display());
    println!("source={}", repository.operator_index().source().as_str());
    println!(
        "env={}",
        std::env::var_os("GIT_INDEX_FILE")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unset>".to_string())
    );
    println!(
        "lock={}",
        repository
            .operator_index_lock_path()
            .expect("operator index lock path")
            .display()
    );
    assert_eq!(
        repository.has_captured_index(),
        std::env::var_os("GIT_INDEX_FILE").is_some(),
        "capture state must follow the inherited selector"
    );

    if std::env::var("SK_INDEX_MODE").as_deref() == Ok("merge") {
        use sk_git::{MergeStartStatus, Oid, StartPoint};
        let theirs = std::env::var("SK_MERGE_THEIRS").expect("SK_MERGE_THEIRS");
        let plan = repository
            .plan_merge_start(&StartPoint::Oid(Oid::parse(&theirs).expect("theirs oid")))
            .expect("merge start plan");
        let outcome = repository.apply_merge_start(&plan).expect("merge start");
        assert_eq!(outcome.status(), MergeStartStatus::Stopped);
        let conclusion = repository
            .plan_merge_conclusion(plan.head(), Some("merged via captured index"))
            .expect("conclusion plan");
        let landed = repository
            .apply_merge_conclusion(&conclusion)
            .expect("conclusion");
        println!("commit={}", landed.new_head());
        return;
    }

    let branch = std::env::var("SK_BRANCH").unwrap_or_else(|_| "work".to_string());
    let request = SafeCommitRequest::new(
        BranchName::parse(&branch).expect("branch"),
        "env index",
        [GitPath::from_git_bytes(b"a.txt").expect("path")],
    );

    if std::env::var("SK_INDEX_MODE").as_deref() == Ok("plan-foreign") {
        // A foreign lock exists at git's physical endpoint. Before the plan it
        // must refuse the read-only plan; after the plan (and before the apply)
        // it must refuse the apply. Both preserve it.
        let lock = PathBuf::from(std::env::var_os("SK_TARGET_LOCK").expect("SK_TARGET_LOCK"));
        if std::env::var_os("SK_FOREIGN_BEFORE_PLAN").is_some() {
            std::fs::write(&lock, b"foreign lock\n").expect("write the foreign lock");
            match repository.plan_safe_commit(&request) {
                Ok(_) => println!("plan_error=none"),
                Err(error) => println!("plan_error={}", describe_git_error(&error)),
            }
            println!("commit=<none>");
            return;
        }
        let plan = repository
            .plan_safe_commit(&request)
            .expect("safe-commit plan");
        std::fs::write(&lock, b"foreign lock\n").expect("write the foreign lock");
        match repository.apply_safe_commit(&plan) {
            Ok(outcome) => {
                println!("apply_error=none");
                println!("commit={}", outcome.commit());
            }
            Err(error) => {
                println!("apply_error={}", describe_git_error(&error));
                println!("commit=<none>");
            }
        }
        return;
    }

    #[cfg(unix)]
    if std::env::var("SK_INDEX_MODE").as_deref() == Ok("retarget") {
        // The captured spelling is a symlink. Retarget it after the read-only
        // plan and before the apply: a legal retarget must not be refused, and
        // the lock must be derived at apply time from the new target.
        let plan = repository
            .plan_safe_commit(&request)
            .expect("safe-commit plan");
        println!(
            "planned_lock={}",
            repository
                .operator_index_lock_path()
                .expect("planned lock")
                .display()
        );
        let link = PathBuf::from(std::env::var_os("SK_LINK").expect("SK_LINK"));
        let second = std::env::var_os("SK_SECOND_TARGET").expect("SK_SECOND_TARGET");
        std::fs::remove_file(&link).expect("unlink the captured spelling");
        std::os::unix::fs::symlink(&second, &link).expect("retarget the captured spelling");
        println!(
            "retargeted_lock={}",
            repository
                .operator_index_lock_path()
                .expect("retargeted lock")
                .display()
        );
        match repository.apply_safe_commit(&plan) {
            Ok(outcome) => {
                println!("apply_error=none");
                println!("commit={}", outcome.commit());
            }
            Err(error) => {
                println!("apply_error={}", describe_git_error(&error));
                println!("commit=<none>");
            }
        }
        return;
    }

    let outcome = repository.safe_commit(&request).expect("safe commit");
    println!("commit={}", outcome.commit());
    println!("{}", describe_object(&open_path, outcome.commit().as_str()));
}

/// Bounded metadata for the landed commit object: type, size and parent count.
///
/// Successful Git reads bind these facts to an existing object. Parent count
/// alone is not parent identity or full Source/native object equivalence.
fn describe_object(root: &Path, commit: &str) -> String {
    let run = |args: &[&str]| -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "object read {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let kind = run(&["cat-file", "-t", commit]);
    let size = run(&["cat-file", "-s", commit]);
    let parents = run(&["rev-list", "--parents", "-n", "1", commit]);
    let parent_count = parents.split_whitespace().count().saturating_sub(1);
    format!("object_type={kind}\nobject_size={size}\nobject_parents={parent_count}")
}

/// Refuse to act unless this process really owns a fixture-rooted CWD/HOME/XDG/TMP.
///
/// The fixture root is the parent of `HOME`; `SK_INDEX_ROOT` may be a
/// subdirectory of it (the relative-selector case opens `.` from `sub/deep`).
fn assert_isolated_child(_open_path: &Path) {
    let home =
        PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| panic!("HOME must be set")));
    let canonical_root = fs::canonicalize(
        fs::canonicalize(&home)
            .expect("HOME must be a real fixture directory")
            .parent()
            .expect("HOME must live under the fixture root"),
    )
    .expect("canonicalise the fixture root");
    for variable in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "TMPDIR",
    ] {
        let observed = PathBuf::from(
            std::env::var_os(variable).unwrap_or_else(|| panic!("{variable} must be set")),
        );
        let canonical = fs::canonicalize(&observed).unwrap_or_else(|error| {
            panic!(
                "{variable} must be a real fixture directory ({}): {error}",
                observed.display()
            )
        });
        assert_eq!(
            canonical.parent(),
            Some(canonical_root.as_path()),
            "{variable} must be the fixture's own root, not the ambient one: {}",
            observed.display()
        );
    }
    let cwd = std::env::current_dir().expect("current directory");
    assert!(
        cwd.starts_with(&canonical_root),
        "the child must run inside the fixture: {} not under {}",
        cwd.display(),
        canonical_root.display()
    );
}

/// A stable, greppable description of the refusal variants these tests assert.
fn describe_git_error(error: &sk_git::GitError) -> String {
    match error {
        sk_git::GitError::SafeCommitIndexLocked { path, .. } => {
            format!("SafeCommitIndexLocked:{}", path.display())
        }
        sk_git::GitError::SafeCommitIndexLockUnavailable { .. } => {
            "SafeCommitIndexLockUnavailable".to_string()
        }
        sk_git::GitError::OperationInProgress { operation } => {
            format!("OperationInProgress:{operation}")
        }
        other => format!("Other:{other}"),
    }
}
