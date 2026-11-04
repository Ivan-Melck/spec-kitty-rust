//! Source-first controls for the ordinary-worktree `COMMIT_EDITMSG` artifact.
//!
//! Source-comparison cases build two identical fixtures: live pinned Python
//! `safe_commit` runs first, then native `GitRepository::safe_commit` runs in
//! its own child with a temporary CWD and isolated roots. The source side is
//! asserted first, so native refusal cannot mask an unobserved source result.
//! Separate native ownership-contract controls are positive fault injections,
//! not Source comparisons or invented Source business reds.
//!
//! The corpus in `fixtures/safe_commit/` is unchanged: this file is a live
//! acceptance layer for facts (argv spelling, artifact timing, object
//! readback) that the earlier corpus did not bind.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use sk_git::{BranchName, GitPath, GitRepository, SafeCommitRequest};
use tempfile::TempDir;

const REQUESTED: &str = "kitty-ops/rec.jsonl";
const MESSAGE: &str = "subject";

struct Io {
    home: PathBuf,
    tmp: PathBuf,
    config: PathBuf,
    cache: PathBuf,
    data: PathBuf,
    state: PathBuf,
}

impl Io {
    fn create(root: &Path) -> Self {
        let io = Self {
            home: root.join("home"),
            tmp: root.join("tmp"),
            config: root.join("config"),
            cache: root.join("cache"),
            data: root.join("data"),
            state: root.join("state"),
        };
        for directory in [
            &io.home,
            &io.tmp,
            &io.config,
            &io.cache,
            &io.data,
            &io.state,
            &root.join("cwd"),
        ] {
            fs::create_dir_all(directory).expect("create isolation root");
        }
        io
    }

    fn apply(&self, command: &mut Command) {
        command
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("XDG_DATA_HOME", &self.data)
            .env("XDG_STATE_HOME", &self.state)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
            .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
            .env("TZ", "UTC")
            .env("LC_ALL", "C")
            .env("LANG", "C");
    }
}

fn git(io: &Io, cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = git_raw(io, cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn git_raw(io: &Io, cwd: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    command.current_dir(cwd).args(args).env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    io.apply(&mut command);
    command.output().expect("run git")
}

fn git_line(io: &Io, cwd: &Path, args: &[&str]) -> String {
    let raw = git(io, cwd, args);
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    text
}

struct Fixture {
    _temp: TempDir,
    io: Io,
    repo_root: PathBuf,
    worktree: PathBuf,
}

fn fixture(io: &Io, linked: bool) -> Fixture {
    let temp = tempfile::tempdir().expect("fixture tempdir");
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    git(io, &repo_root, &["init", "-q", "-b", "work"]);
    git(io, &repo_root, &["config", "user.name", "Test Author"]);
    git(
        io,
        &repo_root,
        &["config", "user.email", "test@example.invalid"],
    );
    git(io, &repo_root, &["config", "commit.gpgsign", "false"]);
    fs::create_dir_all(repo_root.join("kitty-ops")).unwrap();
    fs::write(repo_root.join("README.md"), b"seed\n").unwrap();
    fs::write(repo_root.join(REQUESTED), b"{\"k\":1}\n").unwrap();
    git(io, &repo_root, &["add", "-A"]);
    git(io, &repo_root, &["commit", "-q", "-m", "seed"]);

    let worktree = if linked {
        let lane = temp.path().join("lane");
        git(
            io,
            &repo_root,
            &[
                "worktree",
                "add",
                "-q",
                lane.to_str().unwrap(),
                "-b",
                "lane",
            ],
        );
        lane
    } else {
        repo_root.clone()
    };
    fs::write(worktree.join(REQUESTED), b"{\"k\":2}\n").unwrap();
    let io = Io::create(temp.path());
    Fixture {
        _temp: temp,
        io,
        repo_root,
        worktree,
    }
}

fn hooks_dir(fixture: &Fixture) -> PathBuf {
    git_path(fixture, "hooks")
}

fn install(fixture: &Fixture, name: &str, body: &str) {
    let directory = hooks_dir(fixture);
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\nexit 0\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn artifact_spelling(fixture: &Fixture) -> String {
    git_line(
        &fixture.io,
        &fixture.worktree,
        &["rev-parse", "--git-path", "COMMIT_EDITMSG"],
    )
}

fn artifact_absolute(fixture: &Fixture) -> PathBuf {
    let path = PathBuf::from(artifact_spelling(fixture));
    if path.is_absolute() {
        path
    } else {
        fixture.worktree.join(path)
    }
}

fn git_dir(fixture: &Fixture) -> PathBuf {
    PathBuf::from(git_line(
        &fixture.io,
        &fixture.worktree,
        &["rev-parse", "--absolute-git-dir"],
    ))
}

fn read_git_file(fixture: &Fixture, name: &str) -> Vec<u8> {
    fs::read(git_dir(fixture).join(name))
        .unwrap_or_else(|error| panic!("read {name} in {}: {error}", git_dir(fixture).display()))
}

fn git_dir_text(fixture: &Fixture, name: &str) -> String {
    String::from_utf8_lossy(&read_git_file(fixture, name)).into_owned()
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("project root")
}

fn python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = project_root();
            let python = root.join(".venv-rust-oracle").join(if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            });
            assert!(python.is_file(), "required pinned oracle interpreter");
            let scratch = tempfile::tempdir().unwrap();
            let output = Command::new(&python)
                .current_dir(scratch.path())
                .env("HOME", scratch.path())
                .env("TMPDIR", scratch.path())
                .env("XDG_CONFIG_HOME", scratch.path())
                .env("XDG_CACHE_HOME", scratch.path())
                .env("XDG_DATA_HOME", scratch.path())
                .env("XDG_STATE_HOME", scratch.path())
                .arg(root.join("scripts/rust/verify_oracle.py"))
                .arg("--root")
                .arg(&root)
                .output()
                .expect("verify pinned oracle");
            assert!(
                output.status.success(),
                "exact53 oracle verification failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            python
        })
        .clone()
}

const SOURCE_DRIVER: &str = r#"
import os
import sys
from pathlib import Path
from specify_cli.git.commit_helpers import safe_commit

home = os.environ["HOME"]
assert os.getcwd() == home, os.getcwd()
for name in ("TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[name]).parent == Path(home).parent, (name, os.environ[name])

repo_root = Path(sys.argv[1])
worktree = Path(sys.argv[2])
destination = sys.argv[3]
message = sys.argv[4]
requested = Path(sys.argv[5])
try:
    safe_commit(
        repo_root=repo_root,
        worktree_root=worktree,
        destination_ref=destination,
        message=message,
        paths=(requested,),
    )
except Exception as exc:
    print(f"result=error")
    print(f"error={type(exc).__name__}")
    print(f"detail={exc}")
else:
    print("result=ok")
"#;

fn run_source(fixture: &Fixture, destination: &str, message: &str) -> String {
    run_source_with_index(fixture, destination, message, None)
}

fn run_source_with_index(
    fixture: &Fixture,
    destination: &str,
    message: &str,
    custom_index: Option<&Path>,
) -> String {
    let project = project_root();
    let driver = fixture.repo_root.parent().unwrap().join("source-driver.py");
    fs::write(&driver, SOURCE_DRIVER).unwrap();
    let mut command = Command::new(python());
    command.env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    fixture.io.apply(&mut command);
    command
        .current_dir(&fixture.io.home)
        .env("PYTHONPATH", project.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", project.join("packs"))
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&driver)
        .arg(&fixture.repo_root)
        .arg(&fixture.worktree)
        .arg(destination)
        .arg(message)
        .arg(REQUESTED);
    if let Some(index) = custom_index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output().expect("run pinned source");
    assert!(
        output.status.success(),
        "source driver failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("result="), "source driver printed no result");
    text
}

fn source_ok(text: &str) -> bool {
    text.lines().any(|line| line == "result=ok")
}

fn run_native(fixture: &Fixture, destination: &str, message: &str) -> String {
    run_native_with_index(fixture, destination, message, None)
}

fn run_native_with_index(
    fixture: &Fixture,
    destination: &str,
    message: &str,
    custom_index: Option<&Path>,
) -> String {
    run_native_using(fixture, destination, message, custom_index, None)
}

fn run_native_using(
    fixture: &Fixture,
    destination: &str,
    message: &str,
    custom_index: Option<&Path>,
    program: Option<&Path>,
) -> String {
    let out = fixture
        .repo_root
        .parent()
        .unwrap()
        .join("native-result.txt");
    let exe = std::env::current_exe().expect("current test executable");
    let mut command = Command::new(exe);
    command.current_dir(&fixture.io.home).env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    fixture.io.apply(&mut command);
    if let Some(index) = custom_index {
        command.env("GIT_INDEX_FILE", index);
    }
    command
        .args([
            "--ignored",
            "--exact",
            "artifact_native_helper",
            "--nocapture",
            "--",
        ])
        .arg(&fixture.worktree)
        .arg(destination)
        .arg(message)
        .arg(REQUESTED)
        .arg(&out);
    if let Some(program) = program {
        command.arg(program);
    }
    let output = command.output().expect("run native helper");
    assert!(
        output.status.success(),
        "native helper failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(out).expect("native helper result file")
}

#[test]
#[ignore = "subprocess helper for the source-first artifact matrix"]
fn artifact_native_helper() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("isolated HOME"));
    assert_eq!(
        std::env::current_dir().unwrap(),
        home,
        "native artifact probe needs an actual temporary CWD, not just env_clear"
    );
    for name in [
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ] {
        let value = PathBuf::from(std::env::var_os(name).expect(name));
        assert_eq!(
            value.parent(),
            home.parent(),
            "{name} must be isolated beside HOME"
        );
    }
    let args: Vec<String> = std::env::args().collect();
    let rest: Vec<&str> = args
        .iter()
        .skip_while(|arg| arg.as_str() != "--")
        .skip(1)
        .map(String::as_str)
        .collect();
    let worktree = PathBuf::from(rest[0]);
    let destination = BranchName::parse(rest[1]).expect("branch");
    let message = rest[2];
    let requested = GitPath::from_git_bytes(rest[3].as_bytes()).expect("path");
    let out = PathBuf::from(rest[4]);
    let options = sk_git::GitOptions {
        program: rest
            .get(5)
            .map(PathBuf::from)
            .unwrap_or_else(|| "git".into()),
        ..Default::default()
    };
    let repository = GitRepository::open_with_options(&worktree, &options).expect("open fixture");
    let request = SafeCommitRequest::new(destination, message, [requested]);
    let report = match repository.safe_commit(&request) {
        Ok(outcome) => format!("result=ok\ncommit={}\n", outcome.commit()),
        Err(error) => {
            let extra = match &error {
                sk_git::GitError::SafeCommitAdvanceUnconfirmed {
                    candidate,
                    observed_tip,
                    ..
                } => format!(
                    "candidate={}\nobserved_tip={}\n",
                    candidate,
                    observed_tip
                        .as_ref()
                        .map(|tip| tip.as_str())
                        .unwrap_or("<unreadable>")
                ),
                _ => String::new(),
            };
            format!("result=error\nerror={error:?}\n{extra}")
        }
    };
    fs::write(out, report).unwrap();
}

fn object_message(fixture: &Fixture) -> Vec<u8> {
    let raw = git(
        &fixture.io,
        &fixture.worktree,
        &["cat-file", "commit", "HEAD"],
    );
    let split = raw
        .windows(2)
        .position(|window| window == b"\n\n")
        .expect("commit object body");
    raw[split + 2..].to_vec()
}

/// Install a hook set that records the exact argv spelling, the pre-commit
/// artifact state, and the pre-commit `index.lock` body.
fn record_hooks(fixture: &Fixture, prepare_edit: &str) {
    install(
        fixture,
        "pre-commit",
        r#"d=$(git rev-parse --absolute-git-dir) || exit 1
p=$(git rev-parse --git-path COMMIT_EDITMSG) || exit 1
printf '%s' "$p" > "$d/precommit-spelling"
if [ -f "$p" ]; then cat "$p" > "$d/precommit-content"; else printf 'ABSENT' > "$d/precommit-content"; fi
l=$(git rev-parse --git-path index.lock)
if [ -f "$l" ]; then cp "$l" "$d/precommit-lock"; else printf 'ABSENT' > "$d/precommit-lock"; fi"#,
    );
    install(
        fixture,
        "prepare-commit-msg",
        &format!(
            r#"d=$(git rev-parse --absolute-git-dir) || exit 1
printf '%s' "$1" > "$d/prepare-spelling"
cat "$1" > "$d/prepare-content"
{prepare_edit}"#
        ),
    );
    install(
        fixture,
        "commit-msg",
        r#"d=$(git rev-parse --absolute-git-dir) || exit 1
printf '%s' "$1" > "$d/commit-msg-spelling"
cat "$1" > "$d/commit-msg-content""#,
    );
}

fn git_path(fixture: &Fixture, name: &str) -> PathBuf {
    let spelling = PathBuf::from(git_line(
        &fixture.io,
        &fixture.worktree,
        &["rev-parse", "--git-path", name],
    ));
    if spelling.is_absolute() {
        spelling
    } else {
        fixture.worktree.join(spelling)
    }
}

fn assert_artifact_facts(fixture: &Fixture, stale: &[u8], expected_message: &[u8]) {
    let spelling = artifact_spelling(fixture);
    assert_eq!(git_dir_text(fixture, "precommit-spelling"), spelling);
    assert_eq!(git_dir_text(fixture, "prepare-spelling"), spelling);
    assert_eq!(git_dir_text(fixture, "commit-msg-spelling"), spelling);
    assert_eq!(read_git_file(fixture, "precommit-content"), stale);
    assert_eq!(read_git_file(fixture, "prepare-content"), expected_message);
    assert_eq!(
        read_git_file(fixture, "commit-msg-content"),
        expected_message
    );
    assert_eq!(
        fs::read(artifact_absolute(fixture)).unwrap(),
        expected_message
    );
    assert_eq!(object_message(fixture), expected_message);
    assert!(
        !git_path(fixture, "index.lock").exists(),
        "the owned lock is released; foreign locks are never deleted"
    );
}

fn run_pair(linked: bool, seed: Option<&[u8]>, prepare_edit: &str) -> (Fixture, Fixture) {
    let ios = tempfile::tempdir().unwrap();
    let source_io = Io::create(&ios.path().join("source"));
    let native_io = Io::create(&ios.path().join("native"));
    let source = fixture(&source_io, linked);
    let native = fixture(&native_io, linked);
    record_hooks(&source, prepare_edit);
    record_hooks(&native, prepare_edit);
    for fixture in [&source, &native] {
        match seed {
            Some(bytes) => fs::write(artifact_absolute(fixture), bytes).unwrap(),
            None => {
                let _ = fs::remove_file(artifact_absolute(fixture));
            }
        }
    }
    // Keep the isolation tempdir alive through the assertions.
    std::mem::forget(ios);
    (source, native)
}

#[test]
fn primary_old_artifact_prepare_reads_source_then_native() {
    let (source, native) = run_pair(false, Some(b"OLD-SENTINEL\n"), "");
    let source_result = run_source(&source, "work", MESSAGE);
    assert!(source_ok(&source_result), "source control: {source_result}");
    assert_artifact_facts(&source, b"OLD-SENTINEL\n", b"subject\n");

    let native_result = run_native(&native, "work", MESSAGE);
    assert!(
        native_result.lines().any(|line| line == "result=ok"),
        "native must commit the source-valid row: {native_result}"
    );
    assert_artifact_facts(&native, b"OLD-SENTINEL\n", b"subject\n");
}

#[test]
fn linked_absent_artifact_uses_absolute_gitpath_source_then_native() {
    let (source, native) = run_pair(true, None, "");
    let source_result = run_source(&source, "lane", MESSAGE);
    assert!(source_ok(&source_result), "source control: {source_result}");
    assert!(
        PathBuf::from(artifact_spelling(&source)).is_absolute(),
        "a linked worktree artifact is absolute"
    );
    assert_artifact_facts(&source, b"ABSENT", b"subject\n");

    let native_result = run_native(&native, "lane", MESSAGE);
    assert!(
        native_result.lines().any(|line| line == "result=ok"),
        "native must commit the source-valid row: {native_result}"
    );
    assert_artifact_facts(&native, b"ABSENT", b"subject\n");
}

fn result_ok(text: &str) -> bool {
    text.lines().any(|line| line == "result=ok")
}

fn assert_both_reject(source: &str, native: &str) {
    assert!(!result_ok(source), "source must refuse this case: {source}");
    assert!(!result_ok(native), "native must refuse this case: {native}");
}

#[test]
fn hook_edited_artifact_is_not_written_back_into_object_source_then_native() {
    let edit = r#"printf '   \n\nappended junk\n' >> "$1""#;
    let (source, native) = run_pair(false, Some(b"OLD-SENTINEL\n"), edit);
    let source_result = run_source(&source, "work", MESSAGE);
    assert!(source_ok(&source_result), "source control: {source_result}");
    assert_eq!(
        fs::read(artifact_absolute(&source)).unwrap(),
        b"subject\n   \n\nappended junk\n"
    );
    assert_eq!(object_message(&source), b"subject\n\nappended junk\n");

    let native_result = run_native(&native, "work", MESSAGE);
    assert!(result_ok(&native_result), "native: {native_result}");
    assert_eq!(
        fs::read(artifact_absolute(&native)).unwrap(),
        b"subject\n   \n\nappended junk\n"
    );
    assert_eq!(object_message(&native), b"subject\n\nappended junk\n");
    assert_eq!(
        git(
            &source.io,
            &source.worktree,
            &["cat-file", "commit", "HEAD"]
        ),
        git(
            &native.io,
            &native.worktree,
            &["cat-file", "commit", "HEAD"]
        ),
        "the full commit object is the source's"
    );
}

#[test]
fn pre_commit_rejection_leaves_the_old_artifact_source_then_native() {
    let (source, native) = run_pair(false, Some(b"OLD-SENTINEL\n"), "");
    for fixture in [&source, &native] {
        install(fixture, "pre-commit", "exit 1");
    }
    let source_result = run_source(&source, "work", MESSAGE);
    let native_result = run_native(&native, "work", MESSAGE);
    assert_both_reject(&source_result, &native_result);
    assert_eq!(
        fs::read(artifact_absolute(&source)).unwrap(),
        b"OLD-SENTINEL\n"
    );
    assert_eq!(
        fs::read(artifact_absolute(&native)).unwrap(),
        b"OLD-SENTINEL\n"
    );
    assert!(!git_path(&source, "index.lock").exists());
    assert!(!git_path(&native, "index.lock").exists());
}

#[test]
fn unchanged_changeset_updates_the_artifact_before_refusing_source_then_native() {
    let (source, native) = run_pair(false, Some(b"OLD-SENTINEL\n"), "");
    for fixture in [&source, &native] {
        fs::write(fixture.worktree.join(REQUESTED), b"{\"k\":1}\n").unwrap();
    }
    let source_result = run_source(&source, "work", "  spaced subject  ");
    let native_result = run_native(&native, "work", "  spaced subject  ");
    assert_both_reject(&source_result, &native_result);
    assert_eq!(
        fs::read(artifact_absolute(&source)).unwrap(),
        b"  spaced subject\n"
    );
    assert_eq!(
        fs::read(artifact_absolute(&native)).unwrap(),
        b"  spaced subject\n"
    );
}

#[test]
fn authored_symlink_is_followed_and_preserved_source_then_native() {
    let (source, native) = run_pair(false, None, "");
    for fixture in [&source, &native] {
        let artifact = artifact_absolute(fixture);
        let target = fixture.worktree.join("authored-message.txt");
        fs::write(&target, b"OLD-SENTINEL\n").unwrap();
        let _ = fs::remove_file(&artifact);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &artifact).unwrap();
    }
    let source_result = run_source(&source, "work", MESSAGE);
    assert!(source_ok(&source_result), "source control: {source_result}");
    let native_result = run_native(&native, "work", MESSAGE);
    assert!(result_ok(&native_result), "native: {native_result}");
    for fixture in [&source, &native] {
        let artifact = artifact_absolute(fixture);
        assert!(
            fs::symlink_metadata(&artifact)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the authored link is preserved"
        );
        assert_eq!(
            fs::read(fixture.worktree.join("authored-message.txt")).unwrap(),
            b"subject\n"
        );
        assert_eq!(object_message(fixture), b"subject\n");
    }
}

#[test]
fn dangling_symlink_is_created_through_source_then_native() {
    let (source, native) = run_pair(false, None, "");
    for fixture in [&source, &native] {
        let artifact = artifact_absolute(fixture);
        let target = fixture.worktree.join("dangling-target.txt");
        let _ = fs::remove_file(&target);
        let _ = fs::remove_file(&artifact);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &artifact).unwrap();
    }
    let source_result = run_source(&source, "work", MESSAGE);
    assert!(source_ok(&source_result), "source control: {source_result}");
    let native_result = run_native(&native, "work", MESSAGE);
    assert!(result_ok(&native_result), "native: {native_result}");
    for fixture in [&source, &native] {
        assert!(
            fs::symlink_metadata(artifact_absolute(fixture))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(fixture.worktree.join("dangling-target.txt")).unwrap(),
            b"subject\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn read_only_artifact_refuses_and_keeps_authored_bytes_source_then_native() {
    use std::os::unix::fs::PermissionsExt;
    let (source, native) = run_pair(false, Some(b"READ-ONLY\n"), "");
    for fixture in [&source, &native] {
        let artifact = artifact_absolute(fixture);
        fs::set_permissions(&artifact, fs::Permissions::from_mode(0o444)).unwrap();
    }
    let source_result = run_source(&source, "work", MESSAGE);
    let native_result = run_native(&native, "work", MESSAGE);
    assert_both_reject(&source_result, &native_result);
    for fixture in [&source, &native] {
        assert_eq!(
            fs::read(artifact_absolute(fixture)).unwrap(),
            b"READ-ONLY\n"
        );
    }
}

#[test]
fn pre_commit_hook_sees_a_prospective_dirc_lock_with_unrelated_staging() {
    let (source, native) = run_pair(false, Some(b"OLD-SENTINEL\n"), "");
    let body = r#"d=$(git rev-parse --absolute-git-dir) || exit 1
l=$(git rev-parse --git-path index.lock)
if [ -f "$l" ]; then cp "$l" "$d/lock-body"; GIT_INDEX_FILE="$l" git ls-files -s > "$d/lock-entries"; else printf 'ABSENT' > "$d/lock-body"; fi"#;
    for fixture in [&source, &native] {
        install(fixture, "pre-commit", body);
        fs::write(
            fixture.worktree.join("unrelated.txt"),
            b"unrelated staged\n",
        )
        .unwrap();
        git(&fixture.io, &fixture.worktree, &["add", "unrelated.txt"]);
    }
    let source_result = run_source(&source, "work", MESSAGE);
    assert!(source_ok(&source_result), "source control: {source_result}");
    let native_result = run_native(&native, "work", MESSAGE);
    assert!(result_ok(&native_result), "native: {native_result}");
    for fixture in [&source, &native] {
        let lock = read_git_file(fixture, "lock-body");
        assert!(
            lock.starts_with(b"DIRC"),
            "the hook-visible lock is a real index body, not an empty semaphore"
        );
        let entries = String::from_utf8_lossy(&read_git_file(fixture, "lock-entries")).into_owned();
        assert!(
            entries.contains(REQUESTED),
            "requested blob in lock: {entries}"
        );
        assert!(
            entries.contains("unrelated.txt"),
            "unrelated staged entry preserved in lock: {entries}"
        );
        assert!(
            !git_path(fixture, "index.lock").exists(),
            "the owned lock is released after the CAS"
        );
    }
}

#[test]
fn captured_custom_index_keeps_default_index_bytes_source_then_native() {
    let (source, native) = run_pair(false, Some(b"OLD-SENTINEL\n"), "");
    let source_custom = source
        .repo_root
        .parent()
        .unwrap()
        .join("source-custom-index");
    let native_custom = native
        .repo_root
        .parent()
        .unwrap()
        .join("native-custom-index");
    for (fixture, custom) in [(&source, &source_custom), (&native, &native_custom)] {
        let mut command = Command::new("git");
        command.current_dir(&fixture.worktree).env_clear();
        command.env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        );
        fixture.io.apply(&mut command);
        command
            .env("GIT_INDEX_FILE", custom)
            .args(["read-tree", "HEAD"]);
        let output = command.output().unwrap();
        assert!(output.status.success(), "seed custom index");
    }
    let source_default = fs::read(source.worktree.join(".git/index")).unwrap();
    let native_default = fs::read(native.worktree.join(".git/index")).unwrap();

    let source_result = run_source_with_index(&source, "work", MESSAGE, Some(&source_custom));
    assert!(
        source_ok(&source_result),
        "source custom index: {source_result}"
    );
    let native_result = run_native_with_index(&native, "work", MESSAGE, Some(&native_custom));
    assert!(
        result_ok(&native_result),
        "native custom index: {native_result}"
    );

    assert_eq!(fs::read(artifact_absolute(&source)).unwrap(), b"subject\n");
    assert_eq!(fs::read(artifact_absolute(&native)).unwrap(), b"subject\n");
    assert_eq!(object_message(&source), b"subject\n");
    assert_eq!(object_message(&native), b"subject\n");
    assert_eq!(
        fs::read(source.worktree.join(".git/index")).unwrap(),
        source_default,
        "the default operator index is untouched when a custom index is captured"
    );
    assert_eq!(
        fs::read(native.worktree.join(".git/index")).unwrap(),
        native_default,
        "the default operator index is untouched when a custom index is captured"
    );
}

#[test]
fn losing_the_owned_index_lock_never_publishes_under_a_foreign_replacement() {
    for phase in ["pre-commit", "prepare-commit-msg", "commit-msg"] {
        let sandbox = tempfile::tempdir().unwrap();
        let native = fixture(&Io::create(sandbox.path()), false);
        let before = git_line(&native.io, &native.worktree, &["rev-parse", "HEAD"]);
        fs::write(artifact_absolute(&native), b"OLD-OWNED-MESSAGE\n").unwrap();
        install(
            &native,
            phase,
            "d=$(git rev-parse --absolute-git-dir) || exit 7\nrm \"$d/index.lock\" || exit 8\nprintf 'foreign replacement\\n' > \"$d/index.lock\"",
        );
        let result = run_native(&native, "work", MESSAGE);
        assert!(
            !source_ok(&result),
            "ownership loss must be a refusal, not a success"
        );
        assert_eq!(
            git_line(&native.io, &native.worktree, &["rev-parse", "HEAD"]),
            before,
            "{phase}: no candidate may publish after its artifact lock was replaced: {result}"
        );
        assert_eq!(
            fs::read(git_dir(&native).join("index.lock")).unwrap(),
            b"foreign replacement\n"
        );
        if phase == "pre-commit" {
            assert_eq!(
                fs::read(artifact_absolute(&native)).unwrap(),
                b"OLD-OWNED-MESSAGE\n",
                "must not write the shared message after ownership is lost"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn post_object_lock_loss_retains_the_real_candidate_without_publishing() {
    let sandbox = tempfile::tempdir().unwrap();
    let native = fixture(&Io::create(sandbox.path()), false);
    let before = git_line(&native.io, &native.worktree, &["rev-parse", "HEAD"]);
    let wrapper = native.worktree.join("git-post-object-wrapper.sh");
    fs::write(&wrapper, "#!/bin/sh\nreal=$(command -v git) || exit 8\nfor a in \"$@\"; do\n if [ \"$a\" = commit-tree ]; then\n  \"$real\" \"$@\"; result=$?\n  if [ \"$result\" = 0 ]; then\n   d=$(\"$real\" rev-parse --absolute-git-dir) || exit 9\n   mv \"$d/index.lock\" \"$d/attempt-owned-lock\" || exit 10\n   printf 'post-object foreign\\n' > \"$d/index.lock\"\n  fi\n  exit \"$result\"\n fi\ndone\nexec \"$real\" \"$@\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let result = run_native_using(&native, "work", MESSAGE, None, Some(&wrapper));
    assert!(result.contains("SafeCommitAdvanceUnconfirmed"), "{result}");
    assert!(result.contains("do not assume nothing happened"));
    let candidate = result
        .lines()
        .find_map(|line| line.strip_prefix("candidate="))
        .expect("actual candidate from typed backend error");
    assert_eq!(
        git(&native.io, &native.worktree, &["cat-file", "-t", candidate]),
        b"commit\n"
    );
    assert_eq!(
        git_line(&native.io, &native.worktree, &["rev-parse", "HEAD"]),
        before
    );
    assert!(
        result
            .lines()
            .any(|line| line == format!("observed_tip={before}"))
    );
    assert_eq!(
        fs::read(git_dir(&native).join("index.lock")).unwrap(),
        b"post-object foreign\n"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_replacement_is_not_owned_or_removed() {
    let sandbox = tempfile::tempdir().unwrap();
    let native = fixture(&Io::create(sandbox.path()), false);
    let before = git_line(&native.io, &native.worktree, &["rev-parse", "HEAD"]);
    let foreign = git_dir(&native).join("foreign-lock-target");
    fs::write(&foreign, b"preserve foreign target\n").unwrap();
    // A hook replaces the name, not the held file. Neither link nor target
    // becomes ours, regardless of the target's contents or mode.
    install(
        &native,
        "prepare-commit-msg",
        "d=$(git rev-parse --absolute-git-dir) || exit 7\nmv \"$d/index.lock\" \"$d/attempt-owned-lock\" || exit 8\nln -s foreign-lock-target \"$d/index.lock\" || exit 9",
    );
    let result = run_native(&native, "work", MESSAGE);
    assert!(!result_ok(&result), "{result}");
    assert_eq!(
        git_line(&native.io, &native.worktree, &["rev-parse", "HEAD"]),
        before
    );
    assert_eq!(
        fs::read_link(git_dir(&native).join("index.lock")).unwrap(),
        PathBuf::from("foreign-lock-target")
    );
    assert_eq!(fs::read(foreign).unwrap(), b"preserve foreign target\n");
    // Keep the platform-specific symlink exercise explicit; no Windows proof.
    assert!(
        std::fs::symlink_metadata(git_dir(&native).join("index.lock"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn directory_artifact_refuses_like_git_source_then_native() {
    let (source, native) = run_pair(false, None, "");
    for fixture in [&source, &native] {
        let artifact = artifact_absolute(fixture);
        let _ = fs::remove_file(&artifact);
        fs::create_dir_all(&artifact).unwrap();
    }
    let source_result = run_source(&source, "work", MESSAGE);
    let native_result = run_native(&native, "work", MESSAGE);
    assert_both_reject(&source_result, &native_result);
    for fixture in [&source, &native] {
        assert!(artifact_absolute(fixture).is_dir());
        assert!(!git_path(fixture, "index.lock").exists());
    }
}
