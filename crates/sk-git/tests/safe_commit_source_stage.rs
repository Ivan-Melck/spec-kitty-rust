//! Source-first controls for the ordered operator-index stage and faithful
//! requested-patch recovery.
//!
//! Every comparison builds two identical fixtures. The live pinned Python
//! `safe_commit` runs **first** and its result is asserted before the native
//! route runs in its own child process with a temporary CWD and isolated roots.
//! The native side must then leave the same Git-observable state: the same
//! staged entries, the same HEAD/refs and the same record bytes. This file is a
//! live acceptance layer over facts the committed corpus does not bind; the
//! corpus in `fixtures/safe_commit/` is unchanged.
//!
//! The reference order this binds is:
//!
//! 1. capture the requested-only binary patch of what is staged (pre-mutation),
//! 2. unstage only the currently staged requested paths against the CURRENT
//!    head (or `rm --cached` when unborn),
//! 3. `git add --force` each requested path into the **real** index,
//! 4. only then cross the message's argv boundary.
//!
//! A failure after step 3 that did not land restores the requested-only patch
//! from the current index; an argv-boundary failure leaves the stage.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use sk_git::{
    CommitMessage, GitPath, GitRepository, RefName, SafeCommitLateMessage,
    SafeCommitPhysicalRequest,
};
use tempfile::TempDir;

const REQUESTED: &str = "kitty-ops/rec.jsonl";
const SECOND: &str = "kitty-ops/second.jsonl";
const UNRELATED: &str = "u.cfg";
const PARTIAL: &str = "p.cfg";
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

/// The Git-observable state one side leaves behind.
#[derive(Debug, PartialEq, Eq)]
struct State {
    head: String,
    /// Index vs HEAD, by content (`git diff --cached --name-status`).
    cached: String,
    stage: String,
    refs: String,
    /// Every worktree file's exact bytes (`.git` excluded), so the comparison
    /// never depends on index stat data or the racy-clean heuristic.
    files: std::collections::BTreeMap<String, Vec<u8>>,
}

impl State {
    fn read(fixture: &Fixture) -> Self {
        let head = git_raw(
            &fixture.io,
            &fixture.worktree,
            &["rev-parse", "--verify", "HEAD"],
        );
        let cached = git_raw(
            &fixture.io,
            &fixture.worktree,
            &["diff", "--cached", "--name-status", "--no-renames"],
        );
        let mut files = std::collections::BTreeMap::new();
        collect_files(&fixture.worktree, &fixture.worktree, &mut files);
        Self {
            head: String::from_utf8_lossy(&head.stdout).trim().to_owned(),
            cached: String::from_utf8_lossy(&cached.stdout).into_owned(),
            stage: String::from_utf8_lossy(&git(
                &fixture.io,
                &fixture.worktree,
                &["ls-files", "-s", "-z"],
            ))
            .into_owned(),
            refs: String::from_utf8_lossy(&git(
                &fixture.io,
                &fixture.worktree,
                &["for-each-ref", "--format=%(refname) %(objectname)"],
            ))
            .into_owned(),
            files,
        }
    }

    /// The requested path's staged entry, when it has one.
    fn staged_entry(&self, path: &str) -> Option<String> {
        self.stage
            .split('\0')
            .find(|entry| entry.ends_with(&format!("\t{path}")))
            .map(str::to_owned)
    }
}

/// Every regular file under `root` except the git directory, keyed by its
/// slash-separated relative spelling.
fn collect_files(
    root: &Path,
    current: &Path,
    into: &mut std::collections::BTreeMap<String, Vec<u8>>,
) {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            collect_files(root, &path, into);
        } else if metadata.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(&path) {
                let key = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                into.insert(
                    format!("{key}->symlink"),
                    target.to_string_lossy().as_bytes().to_vec(),
                );
            }
        } else if metadata.is_file() {
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if let Ok(bytes) = fs::read(&path) {
                into.insert(key, bytes);
            }
        }
    }
}

struct Fixture {
    _temp: TempDir,
    io: Io,
    repo_root: PathBuf,
    worktree: PathBuf,
    index_link: Option<PathBuf>,
}

impl Fixture {
    fn git(&self, args: &[&str]) -> Vec<u8> {
        git(&self.io, &self.worktree, args)
    }

    fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) {
        let path = self.worktree.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn index_file(&self) -> PathBuf {
        match &self.index_link {
            Some(path) => path.clone(),
            None => self.worktree.join(".git").join("index"),
        }
    }

    fn install_hook(&self, name: &str, body: &str) {
        let directory = PathBuf::from(git_line(
            &self.io,
            &self.worktree,
            &["rev-parse", "--git-path", "hooks"],
        ));
        let directory = if directory.is_absolute() {
            directory
        } else {
            self.worktree.join(directory)
        };
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\nexit 0\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn install_rejecting_hook(&self, name: &str, body: &str) {
        let directory = PathBuf::from(git_line(
            &self.io,
            &self.worktree,
            &["rev-parse", "--git-path", "hooks"],
        ));
        let directory = if directory.is_absolute() {
            directory
        } else {
            self.worktree.join(directory)
        };
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\nexit 1\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

/// One fixture shape shared by every case.
#[derive(Clone, Copy)]
struct Shape {
    /// A linked worktree instead of the primary checkout.
    linked: bool,
    /// Capture the index through a symlinked `GIT_INDEX_FILE`.
    symlinked_index: bool,
    /// A repository with no commit at all (unborn destination).
    unborn: bool,
    /// A second tracked record path.
    second_path: bool,
    /// The requested record starts as a staged rename of a tracked file.
    rename: bool,
    /// The requested record starts as a staged deletion.
    deletion: bool,
    /// The requested record starts as a staged binary change.
    binary: bool,
    /// The requested record starts as a staged mode change.
    mode: bool,
    /// An unrelated tracked file is fully staged.
    unrelated: bool,
    /// A different tracked file is partially staged.
    partial: bool,
}

impl Shape {
    const PLAIN: Self = Self {
        linked: false,
        symlinked_index: false,
        unborn: false,
        second_path: false,
        rename: false,
        deletion: false,
        binary: false,
        mode: false,
        unrelated: false,
        partial: false,
    };

    fn linked() -> Self {
        Self {
            linked: true,
            ..Self::PLAIN
        }
    }

    fn symlinked_index() -> Self {
        Self {
            symlinked_index: true,
            ..Self::PLAIN
        }
    }

    fn unborn() -> Self {
        Self {
            unborn: true,
            ..Self::PLAIN
        }
    }

    fn second() -> Self {
        Self {
            second_path: true,
            ..Self::PLAIN
        }
    }

    fn rename() -> Self {
        Self {
            rename: true,
            ..Self::PLAIN
        }
    }

    fn deletion() -> Self {
        Self {
            deletion: true,
            ..Self::PLAIN
        }
    }

    fn binary() -> Self {
        Self {
            binary: true,
            ..Self::PLAIN
        }
    }

    fn mode() -> Self {
        Self {
            mode: true,
            ..Self::PLAIN
        }
    }

    fn with_unrelated_partial() -> Self {
        Self {
            unrelated: true,
            partial: true,
            ..Self::PLAIN
        }
    }
}

fn build(shape: Shape) -> Fixture {
    let temp = tempfile::tempdir().expect("fixture tempdir");
    let io = Io::create(temp.path());
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    let blank = repo_root.join("cwd");
    fs::create_dir_all(&blank).unwrap();
    git(&io, &repo_root, &["init", "-q", "-b", "work"]);
    git(&io, &repo_root, &["config", "user.name", "Test Author"]);
    git(
        &io,
        &repo_root,
        &["config", "user.email", "test@example.invalid"],
    );
    git(&io, &repo_root, &["config", "commit.gpgsign", "false"]);
    let mut worktree = repo_root.clone();

    fs::create_dir_all(worktree.join("kitty-ops")).unwrap();
    fs::write(worktree.join("README.md"), b"seed\n").unwrap();
    if shape.unborn {
        // A genuinely unborn destination: the requested record is the only
        // tracked-ish path and `HEAD` resolves to no commit.
        fs::write(worktree.join(REQUESTED), b"{\"k\":1}\n").unwrap();
        if shape.second_path {
            fs::write(worktree.join(SECOND), b"{\"k\":1}\n").unwrap();
        }
        let index_link = if shape.symlinked_index {
            let target = temp.path().join("real.idx");
            fs::write(&target, b"").unwrap();
            let link = temp.path().join("index-link");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &link).unwrap();
            #[cfg(not(unix))]
            {
                fs::write(&link, b"").unwrap();
            }
            Some(link)
        } else {
            None
        };
        return Fixture {
            _temp: temp,
            io,
            repo_root,
            worktree,
            index_link,
        };
    }

    if shape.rename {
        fs::write(worktree.join("old.txt"), b"old contents\n").unwrap();
    } else {
        fs::write(worktree.join(REQUESTED), b"{\"k\":1}\n").unwrap();
    }
    if shape.binary {
        fs::write(worktree.join(REQUESTED), [0u8, 1, 2, 3, 0xff]).unwrap();
    }
    if shape.mode {
        fs::write(worktree.join(REQUESTED), b"#!/bin/sh\necho hi\n").unwrap();
    }
    if shape.unrelated {
        fs::write(worktree.join(UNRELATED), b"unrelated v1\n").unwrap();
    }
    if shape.partial {
        fs::write(worktree.join(PARTIAL), b"partial v1\n").unwrap();
    }
    git(&io, &repo_root, &["add", "-A"]);
    git(&io, &repo_root, &["commit", "-q", "-m", "seed"]);

    if shape.linked {
        let lane = temp.path().join("lane");
        git(
            &io,
            &repo_root,
            &[
                "worktree",
                "add",
                "-q",
                lane.to_str().unwrap(),
                "-b",
                "work-lane",
            ],
        );
        worktree = lane;
    }

    if shape.rename {
        git(&io, &worktree, &["mv", "old.txt", REQUESTED]);
    } else if shape.deletion {
        fs::remove_file(worktree.join(REQUESTED)).unwrap();
        git(&io, &worktree, &["add", "--force", "--", REQUESTED]);
    } else {
        fs::write(worktree.join(REQUESTED), b"{\"k\":2}\n").unwrap();
    }
    if shape.second_path {
        fs::write(worktree.join(SECOND), b"{\"k\":2}\n").unwrap();
    }
    if shape.binary {
        fs::write(worktree.join(REQUESTED), [0u8, 9, 8, 7, 0xfe, 0x00, 0x01]).unwrap();
    }
    if shape.mode {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(worktree.join(REQUESTED), fs::Permissions::from_mode(0o755))
                .unwrap();
        }
    }
    // A pre-existing staged state for the requested path: stage v2, then make
    // the working tree v3 so the captured patch and the worktree differ.
    if !shape.deletion && !shape.rename {
        git(&io, &worktree, &["add", "--force", "--", REQUESTED]);
        fs::write(worktree.join(REQUESTED), b"{\"k\":3}\n").unwrap();
    }
    if shape.second_path && !shape.deletion {
        git(&io, &worktree, &["add", "--force", "--", SECOND]);
        fs::write(worktree.join(SECOND), b"{\"k\":3}\n").unwrap();
    }
    if shape.unrelated {
        fs::write(worktree.join(UNRELATED), b"unrelated v2\n").unwrap();
        git(&io, &worktree, &["add", "--force", "--", UNRELATED]);
    }
    if shape.partial {
        fs::write(worktree.join(PARTIAL), b"partial v2\n").unwrap();
        git(&io, &worktree, &["add", "--force", "--", PARTIAL]);
        fs::write(worktree.join(PARTIAL), b"partial v3\n").unwrap();
    }

    let index_link = if shape.symlinked_index {
        let real = temp.path().join("real.idx");
        let current = worktree.join(".git").join("index");
        fs::copy(&current, &real).unwrap();
        let link = temp.path().join("index-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        fs::copy(&real, &link).unwrap();
        // Replace the real index with the link so both the default spelling and
        // the captured selector exercise the same physical endpoint.
        fs::remove_file(&current).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &current).unwrap();
        #[cfg(not(unix))]
        fs::copy(&real, &current).unwrap();
        Some(link)
    } else {
        None
    };

    Fixture {
        _temp: temp,
        io,
        repo_root,
        worktree,
        index_link,
    }
}

/// The two identical fixtures one comparison needs.
fn twins(shape: Shape) -> (Fixture, Fixture) {
    (build(shape), build(shape))
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

/// The live reference call, with the executor's own argument shape.
const SOURCE_DRIVER: &str = r#"
import os, sys
from pathlib import Path
from specify_cli.git.commit_helpers import safe_commit

home = os.environ["HOME"]
assert os.getcwd() == home, os.getcwd()
for name in ("TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[name]).parent == Path(home).parent, (name, os.environ[name])

repo_root, worktree, destination, kind = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
paths = tuple(Path(p) for p in sys.argv[5:])
message = "subject"
if kind == "surrogate":
    message = "subject" + chr(0xD800)
elif kind == "nul":
    message = "subject" + chr(0) + "tail"
elif kind == "equal_index_timestamp":
    # Source accepts os.PathLike arguments at Popen's argv conversion boundary.
    # This is a public helper/supplier probe, not the Op str-message surface.
    class BoundaryMessage:
        def __fspath__(self):
            stamp = (Path(worktree) / paths[0]).stat().st_mtime_ns
            os.utime(Path(worktree) / ".git/index", ns=(stamp, stamp))
            return "subject"
    message = BoundaryMessage()
try:
    safe_commit(
        repo_root=Path(repo_root),
        worktree_root=Path(worktree),
        destination_ref=destination,
        message=message,
        paths=paths,
    )
except Exception as exc:
    print("result=error")
    print(f"error={type(exc).__name__}")
    print(f"detail={exc}")
else:
    print("result=ok")
"#;

/// One source invocation. `message_kind` decides the payload the reference
/// composes, so an unencodable code point or a NUL is a real one.
fn run_source(fixture: &Fixture, message_kind: &str, paths: &[&str], destination: &str) -> String {
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
        .env("PYTHONUTF8", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&driver)
        .arg(&fixture.repo_root)
        .arg(&fixture.worktree)
        .arg(destination)
        .arg(message_kind)
        .args(paths);
    if let Some(index) = &fixture.index_link {
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

fn source_error_class(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("error="))
        .unwrap_or_else(|| panic!("source driver printed no error line: {text}"))
        .to_owned()
}

fn source_ok(text: &str) -> bool {
    text.lines().any(|line| line == "result=ok")
}

/// The native child helper argument order.
fn run_native(fixture: &Fixture, message_kind: &str, paths: &[&str], destination: &str) -> String {
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
    if let Some(index) = &fixture.index_link {
        command.env("GIT_INDEX_FILE", index);
    }
    command
        .args([
            "--ignored",
            "--exact",
            "stage_native_helper",
            "--nocapture",
            "--",
        ])
        .arg(&fixture.worktree)
        .arg(format!("refs/heads/{destination}"))
        .arg(message_kind)
        .arg(paths.len().to_string())
        .args(paths)
        .arg(&out);
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
#[ignore = "subprocess helper for the source-first stage matrix"]
fn stage_native_helper() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("isolated HOME"));
    assert_eq!(
        std::env::current_dir().unwrap(),
        home,
        "native stage probe needs an actual temporary CWD, not just env_clear"
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
    let destination = RefName::parse(rest[1]).expect("full destination ref");
    let message_kind = rest[2];
    let count: usize = rest[3].parse().expect("path count");
    let paths: Vec<GitPath> = rest[4..4 + count]
        .iter()
        .map(|path| GitPath::from_git_bytes(path.as_bytes()).expect("path"))
        .collect();
    let out = PathBuf::from(rest[4 + count]);
    let repository = GitRepository::open(&worktree).expect("open fixture");
    let request = SafeCommitPhysicalRequest::new(destination, paths);
    let plan = match repository.plan_safe_physical_commit(&request) {
        Ok(plan) => plan,
        Err(failure) => {
            fs::write(out, format!("result=plan_error\n{failure:?}\n")).unwrap();
            return;
        }
    };
    let report = match message_kind {
        "ok" | "equal_index_timestamp" => {
            match repository.apply_safe_physical_commit_with_message(&plan, || {
                if message_kind == "equal_index_timestamp" {
                    let stamp = fs::metadata(worktree.join(rest[4]))?.modified()?;
                    let file = fs::OpenOptions::new()
                        .write(true)
                        .open(worktree.join(".git/index"))?;
                    file.set_times(fs::FileTimes::new().set_modified(stamp))?;
                }
                Ok::<CommitMessage, std::io::Error>(CommitMessage::from_text(MESSAGE))
            }) {
                Ok(outcome) => format!("result=ok\ncommit={}\n", outcome.commit()),
                Err(SafeCommitLateMessage::MessageNotProduced(error)) => {
                    format!("result=error\nerror=MessageNotProduced\n{error}\n")
                }
                Err(SafeCommitLateMessage::MessageRejected(error)) => {
                    format!("result=error\nerror=MessageRejected\n{error}\n")
                }
                Err(SafeCommitLateMessage::Commit(error)) => {
                    format!("result=error\nerror=Commit\ndebug={error:?}\n{error}\n")
                }
            }
        }
        "surrogate" => {
            match repository.apply_safe_physical_commit_with_message(&plan, || {
                Err::<CommitMessage, std::io::Error>(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "stub argv encoding refusal",
                ))
            }) {
                Ok(outcome) => format!("result=ok\ncommit={}\n", outcome.commit()),
                Err(SafeCommitLateMessage::MessageNotProduced(error)) => {
                    format!("result=error\nerror=MessageNotProduced\n{error}\n")
                }
                Err(SafeCommitLateMessage::MessageRejected(error)) => {
                    format!("result=error\nerror=MessageRejected\n{error}\n")
                }
                Err(SafeCommitLateMessage::Commit(error)) => {
                    format!("result=error\nerror=Commit\ndebug={error:?}\n{error}\n")
                }
            }
        }
        "nul" => match repository.apply_safe_physical_commit_with_message(&plan, || {
            Ok::<CommitMessage, std::io::Error>(CommitMessage::from_bytes(
                b"subject\0tail".to_vec(),
            ))
        }) {
            Ok(outcome) => format!("result=ok\ncommit={}\n", outcome.commit()),
            Err(SafeCommitLateMessage::MessageNotProduced(error)) => {
                format!("result=error\nerror=MessageNotProduced\n{error}\n")
            }
            Err(SafeCommitLateMessage::MessageRejected(error)) => {
                format!("result=error\nerror=MessageRejected\n{error}\n")
            }
            Err(SafeCommitLateMessage::Commit(error)) => {
                format!("result=error\nerror=Commit\ndebug={error:?}\n{error}\n")
            }
        },
        other => panic!("unknown message kind {other}"),
    };
    fs::write(out, report).unwrap();
}

fn native_class(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("error="))
        .unwrap_or_else(|| panic!("native helper printed no error line: {text}"))
        .to_owned()
}

fn paths_for(shape: Shape) -> Vec<&'static str> {
    if shape.second_path {
        vec![REQUESTED, SECOND]
    } else {
        vec![REQUESTED]
    }
}

/// Run the source first, assert its footprint, then require the native route to
/// leave the same Git state.
fn compare(
    label: &str,
    shape: Shape,
    message_kind: &str,
    expect_source_ok: bool,
) -> (String, String) {
    let (source, native) = twins(shape);
    let paths = paths_for(shape);
    let destination = if shape.linked {
        // Both checkouts publish into the same branch name.
        "work-lane"
    } else {
        "work"
    };
    let source_before = State::read(&source);
    let report = run_source(&source, message_kind, &paths, destination);
    let source_after = State::read(&source);
    let source_class = if expect_source_ok {
        assert!(
            source_ok(&report),
            "{label}: the reference must commit: {report}"
        );
        "ok".to_string()
    } else {
        assert!(
            !source_ok(&report),
            "{label}: the reference must not commit: {report}"
        );
        assert_eq!(
            source_before.head, source_after.head,
            "{label}: the reference never advances HEAD on a refusal"
        );
        source_error_class(&report)
    };

    let native_report = run_native(&native, message_kind, &paths, destination);
    let native_after = State::read(&native);
    if expect_source_ok {
        assert!(
            native_report.starts_with("result=ok"),
            "{label}: native must commit: {native_report}"
        );
        assert_eq!(
            native_after.head, source_after.head,
            "{label}: identical landed commit"
        );
    } else {
        assert!(
            !native_report.starts_with("result=ok"),
            "{label}: native must refuse: {native_report}"
        );
        assert_eq!(
            native_after.head, source_before.head,
            "{label}: native never advances HEAD on a refusal"
        );
        // The reference's error class is asserted first (above); the native
        // class is reported for the matrix, not forced to be the same string.
        assert!(
            !native_class(&native_report).is_empty(),
            "{label}: native names its class: {native_report}"
        );
    }
    assert_eq!(
        native_after, source_after,
        "{label}: the two routes must leave the same Git state\nsource: {source_after:#?}\nnative: {native_after:#?}\nsource report: {report}\nnative report: {native_report}"
    );
    (
        source_class,
        if expect_source_ok {
            "ok".to_string()
        } else {
            native_class(&native_report)
        },
    )
}

#[test]
fn argv_boundary_failure_leaves_the_stage_source_then_native() {
    for kind in ["surrogate", "nul"] {
        let (source_class, _native) = compare(&format!("{kind} plain"), Shape::PLAIN, kind, false);
        assert!(
            source_class == "UnicodeEncodeError" || source_class == "ValueError",
            "{kind}: unexpected reference class {source_class}"
        );
    }
}

#[test]
fn argv_boundary_failure_stages_in_a_linked_worktree_source_then_native() {
    let (source_class, _) = compare("surrogate linked", Shape::linked(), "surrogate", false);
    assert_eq!(source_class, "UnicodeEncodeError");
}

#[test]
fn argv_boundary_failure_stages_through_a_symlinked_index_source_then_native() {
    let (source_class, _) = compare(
        "surrogate symlinked index",
        Shape::symlinked_index(),
        "surrogate",
        false,
    );
    assert_eq!(source_class, "UnicodeEncodeError");
}

#[test]
fn an_unborn_destination_has_no_source_stage_equivalent() {
    // The reference's ordinary route refuses a destination ref that does not
    // exist (`SafeCommitDestinationNotFound`) *before* it stages anything, so
    // there is no Source stage footprint to compare. The native exact-ref
    // route does support an unborn destination; this records that difference
    // instead of pretending it is a Source pair.
    let (source, native) = twins(Shape::unborn());
    let paths = vec![REQUESTED];
    let source_before = State::read(&source);
    let report = run_source(&source, "surrogate", &paths, "work");
    assert!(!source_ok(&report), "the reference must refuse: {report}");
    assert_eq!(
        source_error_class(&report),
        "SafeCommitDestinationNotFound",
        "the reference has no unborn ordinary destination: {report}"
    );
    assert_eq!(
        State::read(&source),
        source_before,
        "the reference stages nothing for an unborn destination"
    );

    let native_report = run_native(&native, "surrogate", &paths, "work");
    assert!(!native_report.starts_with("result=ok"), "{native_report}");
    assert_eq!(native_class(&native_report), "MessageNotProduced");
    let native_after = State::read(&native);
    assert_eq!(
        native_after.head, source_before.head,
        "no root commit is invented on an argv-boundary failure"
    );
    assert!(
        native_after.staged_entry(REQUESTED).is_some(),
        "the native unborn route stages the requested path before the same boundary: {native_after:#?}"
    );
}

#[test]
fn multi_path_second_stage_failure_restores_requested_only_source_then_native() {
    let (source_class, native) = compare("two paths", Shape::second(), "surrogate", false);
    assert_eq!(source_class, "UnicodeEncodeError");
    // The second path exists in the fixture, so this pair proves the successful
    // two-path stage before the same encoding failure.
    assert_eq!(native, "MessageNotProduced");
}

#[test]
fn missing_second_path_restores_before_encoding_source_then_native() {
    // The reference stages the first requested path and then fails on the
    // missing second one; the argv encode class is never reached.
    let (source, native) = twins(Shape::second());
    fs::remove_file(source.worktree.join(SECOND)).unwrap();
    fs::remove_file(native.worktree.join(SECOND)).unwrap();
    let paths = vec![REQUESTED, SECOND];
    let source_before = State::read(&source);
    let report = run_source(&source, "surrogate", &paths, "work");
    assert!(!source_ok(&report), "missing second path must refuse");
    assert!(
        source_error_class(&report) == "RuntimeError",
        "reference staging failure class: {report}"
    );
    assert!(
        report.contains("failed to stage"),
        "reference names the staging failure: {report}"
    );
    assert_eq!(
        source_before.head,
        State::read(&source).head,
        "nothing landed"
    );

    let native_report = run_native(&native, "surrogate", &paths, "work");
    assert!(
        !native_report.starts_with("result=ok"),
        "native must refuse the missing second path"
    );
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "the restored requested-only footprint must match\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn successful_commit_leaves_reference_requested_staging_source_then_native() {
    let (_source, _native) = compare("normal", Shape::PLAIN, "ok", true);
}

#[test]
fn successful_commit_keeps_unrelated_and_partial_staging_source_then_native() {
    let (_source, _native) = compare(
        "unrelated partial",
        Shape::with_unrelated_partial(),
        "ok",
        true,
    );
}

#[test]
fn hook_rejection_restores_requested_staging_source_then_native() {
    let (source, native) = twins(Shape::with_unrelated_partial());
    for fixture in [&source, &native] {
        fixture.install_rejecting_hook("pre-commit", "echo rejected >&2");
    }
    let paths = vec![REQUESTED];
    let source_before = State::read(&source);
    let report = run_source(&source, "ok", &paths, "work");
    assert!(!source_ok(&report), "the reference hook must reject");
    assert_eq!(source_error_class(&report), "RuntimeError");
    assert_eq!(source_before.head, State::read(&source).head);

    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "a rejected commit restores the requested-only patch\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn unchanged_changeset_restores_requested_staging_source_then_native() {
    let (source, native) = twins(Shape::with_unrelated_partial());
    for fixture in [&source, &native] {
        // Restore the requested path to its HEAD content so the changeset is a
        // genuine no-op while the unrelated and partial staging stay put.
        fixture.write(REQUESTED, b"{\"k\":1}\n");
        fixture.git(&["add", "--force", "--", REQUESTED]);
        fixture.git(&["reset", "-q", "--", REQUESTED]);
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(
        !source_ok(&report),
        "the reference refuses a no-op changeset"
    );
    assert_eq!(source_error_class(&report), "SafeCommitStagedTreeUnchanged");
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "a no-op restores the requested-only patch\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn empty_commit_message_restores_requested_staging_source_then_native() {
    let (source, native) = twins(Shape::with_unrelated_partial());
    for fixture in [&source, &native] {
        fixture.install_hook("prepare-commit-msg", ": > \"$1\"");
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(
        !source_ok(&report),
        "the reference refuses an empty message"
    );
    assert_eq!(source_error_class(&report), "RuntimeError");
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "an empty cleaned message restores the requested-only patch\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
#[cfg(unix)]
fn readonly_message_artifact_restores_requested_staging_source_then_native() {
    use std::os::unix::fs::PermissionsExt;

    let (source, native) = twins(Shape::with_unrelated_partial());
    for fixture in [&source, &native] {
        let spelling = git_line(
            &fixture.io,
            &fixture.worktree,
            &["rev-parse", "--git-path", "COMMIT_EDITMSG"],
        );
        let path = PathBuf::from(&spelling);
        let path = if path.is_absolute() {
            path
        } else {
            fixture.worktree.join(path)
        };
        fs::write(&path, b"old artifact\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(
        !source_ok(&report),
        "the reference cannot write the artifact"
    );
    assert_eq!(source_error_class(&report), "RuntimeError");
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "an artifact write failure restores the requested-only patch\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn staged_binary_change_is_captured_and_restored_source_then_native() {
    let (source, native) = twins(Shape::binary());
    for fixture in [&source, &native] {
        fixture.install_rejecting_hook("pre-commit", "echo rejected >&2");
    }
    let paths = vec![REQUESTED];
    let source_before = State::read(&source);
    assert!(
        source_before.staged_entry(REQUESTED).is_some(),
        "the binary change starts staged"
    );
    let report = run_source(&source, "ok", &paths, "work");
    assert!(!source_ok(&report));
    assert_eq!(source_error_class(&report), "RuntimeError");
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "a binary staged patch restores identically\nsource: {report}\nnative: {native_report}"
    );
    assert_eq!(
        State::read(&native).staged_entry(REQUESTED),
        source_before.staged_entry(REQUESTED),
        "the captured binary blob is replayed exactly"
    );
}

#[test]
fn staged_rename_is_captured_and_restored_source_then_native() {
    let (source, native) = twins(Shape::rename());
    for fixture in [&source, &native] {
        fixture.install_rejecting_hook("pre-commit", "echo rejected >&2");
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(!source_ok(&report));
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "a rename pair restores identically\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn staged_deletion_is_captured_and_restored_source_then_native() {
    let (source, native) = twins(Shape::deletion());
    for fixture in [&source, &native] {
        fixture.install_rejecting_hook("pre-commit", "echo rejected >&2");
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(!source_ok(&report));
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "a staged deletion restores identically\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn staged_mode_change_is_captured_and_restored_source_then_native() {
    let (source, native) = twins(Shape::mode());
    for fixture in [&source, &native] {
        fixture.install_rejecting_hook("pre-commit", "echo rejected >&2");
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(!source_ok(&report));
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "a staged mode change restores identically\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn corrupt_present_index_fails_the_capture_gate_source_then_native() {
    let (source, native) = twins(Shape::PLAIN);
    for fixture in [&source, &native] {
        fs::write(fixture.index_file(), b"DIRCnot-an-index").unwrap();
    }
    let paths = vec![REQUESTED];
    let source_before = fs::read(source.index_file()).unwrap();
    let report = run_source(&source, "ok", &paths, "work");
    assert!(
        !source_ok(&report),
        "the reference cannot capture a corrupt index"
    );
    assert_eq!(source_error_class(&report), "SafeCommitRecoveryFailed");
    assert_eq!(
        fs::read(source.index_file()).unwrap(),
        source_before,
        "the reference never mutates a corrupt index"
    );

    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(
        !native_report.starts_with("result=ok"),
        "native must refuse before mutating a corrupt index"
    );
    assert!(
        native_report.contains("SafeCommitPatchCaptureFailed"),
        "the capture gate, not a staging failure: {native_report}"
    );
    assert_eq!(
        fs::read(native.index_file()).unwrap(),
        source_before,
        "the corrupt index is left exactly as found"
    );
}

#[test]
fn a_current_head_change_replays_the_patch_against_the_new_head_source_then_native() {
    let (source, native) = twins(Shape::with_unrelated_partial());
    for fixture in [&source, &native] {
        // A sibling commit that does not touch the requested path, published as
        // a ref update by the rejecting hook (no index lock needed).
        fixture.git(&["branch", "advance"]);
        fixture.write("advance.txt", b"advance\n");
        fixture.git(&["add", "--force", "--", "advance.txt"]);
        fixture.git(&["commit", "-q", "-m", "advance"]);
        let advanced = String::from_utf8_lossy(&fixture.git(&["rev-parse", "HEAD"]))
            .trim()
            .to_owned();
        fixture.git(&["update-ref", "refs/heads/advance", &advanced]);
        fixture.git(&["reset", "-q", "--soft", "HEAD^"]);
        fixture.install_rejecting_hook(
            "pre-commit",
            &format!("git update-ref refs/heads/work {advanced}"),
        );
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(!source_ok(&report));
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "recovery unstage runs against the CURRENT head\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn recovery_rejection_keeps_the_original_failure_source_then_native() {
    let (source, native) = twins(Shape::with_unrelated_partial());
    for fixture in [&source, &native] {
        // A sibling commit that CHANGES the requested path, then a rejecting
        // hook that moves the branch to it: the captured patch can no longer
        // apply, which is the reference's own recovery-failure class.
        fixture.git(&["branch", "advance"]);
        fixture.write(REQUESTED, b"{\"advanced\":true}\n");
        fixture.git(&["add", "--force", "--", REQUESTED]);
        fixture.git(&["commit", "-q", "-m", "advance"]);
        let advanced = String::from_utf8_lossy(&fixture.git(&["rev-parse", "HEAD"]))
            .trim()
            .to_owned();
        fixture.git(&["update-ref", "refs/heads/advance", &advanced]);
        fixture.git(&["reset", "-q", "--soft", "HEAD^"]);
        fixture.install_rejecting_hook(
            "pre-commit",
            &format!("git update-ref refs/heads/work {advanced}"),
        );
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(!source_ok(&report));
    assert_eq!(
        source_error_class(&report),
        "SafeCommitRecoveryFailed",
        "the reference's recovery failure class: {report}"
    );
    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(!native_report.starts_with("result=ok"));
    assert!(
        native_report.contains("SafeCommitRecoveryFailed"),
        "native reports a recovery failure, not success: {native_report}"
    );
    assert!(
        native_report.contains("SafeCommitHookRejected") || native_report.contains("rejected"),
        "the original failure is preserved: {native_report}"
    );
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "a failed restore leaves the same index footprint\nsource: {report}\nnative: {native_report}"
    );
}

#[test]
fn foreign_index_bytes_survive_requested_patch_recovery_source_then_native() {
    let (source, native) = twins(Shape::PLAIN);
    for side in [&source, &native] {
        side.write(REQUESTED, b"pre-existing requested stage\n");
        side.git(&["add", "--force", "--", REQUESTED]);
        side.write(REQUESTED, b"current requested worktree\n");
        side.install_rejecting_hook(
            "pre-commit",
            r#"d=$(git rev-parse --absolute-git-dir) || exit 8
printf 'FOREIGN INDEX BYTES' > "$d/index""#,
        );
    }
    let source_head = source.git(&["rev-parse", "HEAD"]);
    let native_head = native.git(&["rev-parse", "HEAD"]);
    let source_report = run_source(&source, "ok", &[REQUESTED], "work");
    assert_eq!(
        source_error_class(&source_report),
        "SafeCommitRecoveryFailed",
        "{source_report}"
    );
    assert_eq!(
        fs::read(source.index_file()).unwrap(),
        b"FOREIGN INDEX BYTES"
    );
    assert_eq!(source.git(&["rev-parse", "HEAD"]), source_head);
    let native_report = run_native(&native, "ok", &[REQUESTED], "work");
    assert!(
        native_report.contains("SafeCommitRecoveryFailed"),
        "{native_report}"
    );
    assert!(
        native_report.contains("original_failure"),
        "retain the original failure: {native_report}"
    );
    assert_eq!(
        fs::read(native.index_file()).unwrap(),
        b"FOREIGN INDEX BYTES"
    );
    assert_eq!(native.git(&["rev-parse", "HEAD"]), native_head);
    for side in [&source, &native] {
        assert!(!side.worktree.join(".git/index.lock").exists());
        assert_eq!(
            fs::read(side.worktree.join(REQUESTED)).unwrap(),
            b"current requested worktree\n"
        );
    }
}

#[test]
fn a_foreign_index_lock_is_never_taken_or_removed() {
    // Native ownership control: the reference has no equivalent refusal class
    // (its `git add` fails and its own restore also fails), so this asserts the
    // owned-artifact contract instead of claiming a Source comparison.
    let fixture = build(Shape::PLAIN);
    let lock = fixture.index_file().with_extension("lock");
    fs::write(&lock, b"foreign lock body").unwrap();
    let head = State::read(&fixture).head;
    let report = run_native(&fixture, "ok", &[REQUESTED], "work");
    assert!(!report.starts_with("result=ok"), "{report}");
    assert!(
        report.contains("OperationInProgress") || report.contains("SafeCommitIndexLocked"),
        "the existing lock is refused, never taken: {report}"
    );
    assert_eq!(fs::read(&lock).unwrap(), b"foreign lock body");
    assert_eq!(State::read(&fixture).head, head);
}

#[test]
#[cfg(unix)]
fn an_index_retargeted_during_staging_is_not_published_under_the_old_lock() {
    // Native ownership control. A clean filter for the requested path retargets
    // the captured symlinked index between the lock and the next publish; the
    // route must refuse rather than install a body at a target its lock does
    // not cover.
    let fixture = build(Shape::symlinked_index());
    let link = fixture.index_link.clone().expect("symlink fixture");
    let other = fixture.repo_root.parent().unwrap().join("other.idx");
    fs::write(&other, b"").unwrap();
    let hooks = PathBuf::from(git_line(
        &fixture.io,
        &fixture.worktree,
        &["rev-parse", "--git-path", "hooks"],
    ));
    let hooks = if hooks.is_absolute() {
        hooks
    } else {
        fixture.worktree.join(hooks)
    };
    fs::create_dir_all(&hooks).unwrap();
    let script = hooks.parent().unwrap().join("retarget.sh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nrm -f '{}'\nln -s '{}' '{}'\ncat\n",
            link.display(),
            other.display(),
            link.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fixture.git(&["config", "filter.retarget.clean", script.to_str().unwrap()]);
    fixture.git(&["config", "filter.retarget.required", "true"]);
    fs::write(
        fixture.worktree.join(".gitattributes"),
        format!("{REQUESTED} filter=retarget\n"),
    )
    .unwrap();

    let report = run_native(&fixture, "ok", &[REQUESTED], "work");
    assert!(!report.starts_with("result=ok"), "{report}");
    assert!(
        report.contains("SafeCommitIndexPublishFailed"),
        "the retarget is refused, not published under the old lock: {report}"
    );
    assert!(
        report.contains("refusing to publish another target's index"),
        "the refusal names the endpoint change: {report}"
    );
}

/// A clean filter that counts its own invocations and embeds that count in the
/// content it returns (so the staged blob proves how many passes ran).
#[cfg(unix)]
#[test]
fn shared_repository_index_permissions_match_after_encoding_failure() {
    use std::os::unix::fs::PermissionsExt;
    for shared in ["umask", "group", "all", "0660", "0666"] {
        let (source, native) = twins(Shape::PLAIN);
        for side in [&source, &native] {
            side.git(&["config", "core.sharedRepository", shared]);
        }
        let paths = [REQUESTED];
        let source_report = run_source(&source, "surrogate", &paths, "work");
        assert_eq!(
            source_error_class(&source_report),
            "UnicodeEncodeError",
            "{source_report}"
        );
        let expected = fs::metadata(source.index_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let native_report = run_native(&native, "surrogate", &paths, "work");
        assert!(
            native_report.contains("MessageNotProduced"),
            "{native_report}"
        );
        let observed = fs::metadata(native.index_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            observed, expected,
            "core.sharedRepository={shared}: Source staged index mode must be preserved before the same encoding refusal"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn shared_repository_lock_mode_matrix_source_then_native() {
    for shared in ["umask", "group", "all", "0660", "0666", "<bare>"] {
        let (source, native) = twins(Shape::PLAIN);
        for side in [&source, &native] {
            if shared == "<bare>" {
                let config = side.worktree.join(".git/config");
                let mut bytes = fs::read(&config).unwrap();
                bytes.extend_from_slice(b"\n[core]\nsharedRepository\n");
                fs::write(config, bytes).unwrap();
            } else {
                side.git(&["config", "core.sharedRepository", shared]);
            }
            side.install_hook(
                "pre-commit",
                r#"d=$(git rev-parse --absolute-git-dir) || exit 8
stat -c %a "$d/index.lock" > "$d/shared-lock-mode" || exit 9"#,
            );
        }
        let source_report = run_source(&source, "ok", &[REQUESTED], "work");
        assert!(source_ok(&source_report), "{shared}: {source_report}");
        let expected = fs::read(source.worktree.join(".git/shared-lock-mode")).unwrap();
        let native_report = run_native(&native, "ok", &[REQUESTED], "work");
        assert!(
            native_report.starts_with("result=ok"),
            "{shared}: {native_report}"
        );
        assert_eq!(
            fs::read(native.worktree.join(".git/shared-lock-mode")).unwrap(),
            expected,
            "{shared}: hook-visible lock permissions"
        );
        assert_eq!(
            native.git(&["cat-file", "commit", "HEAD"]),
            source.git(&["cat-file", "commit", "HEAD"])
        );
    }
}

#[cfg(unix)]
#[test]
fn authored_shared_repository_values_follow_git_parsing() {
    use std::os::unix::fs::PermissionsExt;
    for shared in [
        "<bare>",
        "",
        "+0660",
        " 0660",
        "-1",
        "8",
        "1k",
        "0777777777777777777777777777777777",
    ] {
        let (source, native) = twins(Shape::PLAIN);
        for side in [&source, &native] {
            if shared == "<bare>" {
                let config = side.worktree.join(".git/config");
                let mut bytes = fs::read(&config).unwrap();
                bytes.extend_from_slice(b"\n[core]\nsharedRepository\n");
                fs::write(config, bytes).unwrap();
            } else {
                side.git(&["config", "core.sharedRepository", shared]);
            }
        }
        let source_report = run_source(&source, "surrogate", &[REQUESTED], "work");
        assert_eq!(
            source_error_class(&source_report),
            "UnicodeEncodeError",
            "{shared}: {source_report}"
        );
        let expected = fs::metadata(source.index_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let native_report = run_native(&native, "surrogate", &[REQUESTED], "work");
        assert!(
            native_report.contains("MessageNotProduced"),
            "{shared}: {native_report}"
        );
        assert_eq!(
            fs::metadata(native.index_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            expected,
            "{shared}: same Source-produced stage mode"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn future_mtime_candidate_keeps_source_hook_observation() {
    let (source, native) = twins(Shape::PLAIN);
    let source_counter = source
        .repo_root
        .parent()
        .unwrap()
        .join("source-filter-count");
    let native_counter = native
        .repo_root
        .parent()
        .unwrap()
        .join("native-filter-count");
    for (side, counter) in [(&source, &source_counter), (&native, &native_counter)] {
        install_counting_filter(side, counter, true);
        side.git(&["add", "--force", "--", REQUESTED]);
        side.write(REQUESTED, b"{\"k\":4}\n");
        set_mtime(side, REQUESTED, 2_051_222_400);
        fs::write(counter, b"0").unwrap();
        side.git(&["config", "core.sharedRepository", "0660"]);
        side.install_hook(
            "pre-commit",
            r#"timestamp=$(stat -c %Y "$GIT_INDEX_FILE") || exit 8
now=$(date +%s) || exit 9
if test "$timestamp" -gt "$((now+10))"; then exit 51; fi
d=$(git rev-parse --absolute-git-dir) || exit 10
GIT_INDEX_FILE="$d/index.lock" git show :kitty-ops/rec.jsonl > "$d/observed-lock-blob" || exit 11
stat -c %a "$d/index.lock" > "$d/observed-lock-mode" || exit 12"#,
        );
    }
    let source_report = run_source(&source, "ok", &[REQUESTED], "work");
    assert!(
        source_ok(&source_report),
        "Source accepts its normal hook-visible index timestamp: {source_report}"
    );
    let native_report = run_native(&native, "ok", &[REQUESTED], "work");
    assert!(native_report.starts_with("result=ok"), "{native_report}");
    assert_eq!(
        fs::read(native.worktree.join(".git/observed-lock-blob")).unwrap(),
        fs::read(source.worktree.join(".git/observed-lock-blob")).unwrap(),
        "the hook sees Source's post-prepare real-index lock body, not the earlier stage body"
    );
    assert_eq!(
        fs::read(native.worktree.join(".git/observed-lock-mode")).unwrap(),
        fs::read(source.worktree.join(".git/observed-lock-mode")).unwrap(),
        "the held lock also follows sharedRepository permissions"
    );
    assert_eq!(
        native.git(&["cat-file", "commit", "HEAD"]),
        source.git(&["cat-file", "commit", "HEAD"])
    );
    assert_eq!(State::read(&native), State::read(&source));
}

fn install_counting_filter(fixture: &Fixture, counter: &Path, stateful: bool) {
    let body = if stateful {
        "printf 'filter=%s\\n' \"$n\""
    } else {
        ":"
    };
    let script = fixture
        .repo_root
        .parent()
        .unwrap()
        .join("counting-filter.sh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nn=0\n[ -f '{counter}' ] && n=$(cat '{counter}')\nn=$((n+1))\nprintf '%s' \"$n\" > '{counter}'\n{{ ps -o args= -p $PPID 2>/dev/null || true; }} >> '{counter}.log'\n{body}\ncat\n",
            body = body,
            counter = counter.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fixture.git(&["config", "filter.counter.clean", script.to_str().unwrap()]);
    fixture.git(&["config", "filter.counter.required", "true"]);
    fixture.write(".gitattributes", format!("{REQUESTED} filter=counter\n"));
}

#[test]
fn equal_index_and_entry_timestamps_preserve_source_raciness() {
    let (source, native) = twins(Shape::PLAIN);
    let source_counter = source
        .repo_root
        .parent()
        .unwrap()
        .join("source-filter-count");
    let native_counter = native
        .repo_root
        .parent()
        .unwrap()
        .join("native-filter-count");
    for (side, counter) in [(&source, &source_counter), (&native, &native_counter)] {
        install_counting_filter(side, counter, true);
        side.git(&["add", "--force", "--", REQUESTED]);
        side.write(REQUESTED, b"{\"k\":4}\n");
        set_mtime(side, REQUESTED, 978_307_200);
        fs::write(counter, b"0").unwrap();
    }
    let source_report = run_source(&source, "equal_index_timestamp", &[REQUESTED], "work");
    assert!(
        source_ok(&source_report),
        "actual Source argv-boundary control: {source_report}"
    );
    let expected = source.git(&["cat-file", "commit", "HEAD"]);
    let source_count = counter_value(&source_counter);
    let native_report = run_native(&native, "equal_index_timestamp", &[REQUESTED], "work");
    assert!(native_report.starts_with("result=ok"), "{native_report}");
    assert_eq!(
        native.git(&["cat-file", "commit", "HEAD"]),
        expected,
        "the copied index must retain the source index timestamp at the equal-second/equal-nanosecond racy boundary; source passes={source_count}, native passes={}",
        counter_value(&native_counter)
    );
    assert_eq!(State::read(&native), State::read(&source));
}

fn counter_value(path: &Path) -> u32 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// How many of a filter log's invocations came from an `add --force` (our own
/// staging pass), as opposed to git's internal re-hash/refresh passes.
fn stage_add_passes(log: &str) -> usize {
    log.lines()
        .filter(|line| line.contains("add --force"))
        .count()
}

fn filter_log(counter: &Path) -> String {
    fs::read_to_string(format!("{}.log", counter.display())).unwrap_or_default()
}

/// Force a deterministic worktree mtime for the racy-clean branches of git's
/// index refresh (see builtin/commit.c prepare_index). 978307200 is 2001-01-01
/// (strictly older than any index written by this test: non-racy); 2051222400 is
/// 2035-01-01 (strictly newer than the index: racy).
fn set_mtime(fixture: &Fixture, relative: &str, epoch_seconds: u64) {
    let path = fixture.worktree.join(relative);
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch_seconds);
    file.set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

#[test]
fn an_idempotent_clean_filter_leaves_the_same_state_as_the_reference() {
    // Both sides run the same counting clean filter on identical fixtures. The
    // filter itself records each invocation, so nothing external perturbs the
    // measurement. The reference's `git commit --only` has three staging sites
    // per path: the call's own `git add --force`, the partial-commit prepare
    // re-add of the real index, and the false-index add (builtin/commit.c
    // prepare_index). The native order runs exactly those three; the filter
    // invocation count itself stays timing dependent (git's racily-clean rule)
    // and is recorded, not invented. The asserted fact is the full post-commit
    // state equality.
    let (source, native) = twins(Shape::PLAIN);
    let source_counter = source
        .repo_root
        .parent()
        .unwrap()
        .join("source-filter-count");
    let native_counter = native
        .repo_root
        .parent()
        .unwrap()
        .join("native-filter-count");
    for (fixture, counter) in [(&source, &source_counter), (&native, &native_counter)] {
        install_counting_filter(fixture, counter, false);
        fixture.git(&["add", "--force", "--", REQUESTED]);
        fs::write(fixture.worktree.join(REQUESTED), b"{\"k\":4}\n").unwrap();
        fs::write(counter, b"0").unwrap();
        let _ = fs::remove_file(format!("{}.log", counter.display()));
    }
    let paths = vec![REQUESTED];
    let report = run_source(&source, "ok", &paths, "work");
    assert!(source_ok(&report), "the reference commits: {report}");
    let source_count = counter_value(&source_counter);
    let source_log = filter_log(&source_counter);

    let native_report = run_native(&native, "ok", &paths, "work");
    assert!(native_report.starts_with("result=ok"), "{native_report}");
    let native_count = counter_value(&native_counter);
    let native_log = filter_log(&native_counter);

    assert!(source_count > 0, "the filter actually ran on the reference");
    let adds = stage_add_passes(&native_log);
    assert!(
        adds >= paths.len() && adds <= 3 * paths.len(),
        "at most one filter invocation per reference stage site (operator stage, partial-commit real-index re-add, false-index add); the exact racily-clean branch is timing dependent (adds {adds})\nnative passes ({native_count}):\n{native_log}"
    );
    assert_eq!(
        State::read(&native),
        State::read(&source),
        "identical post-commit state with an idempotent filter\nsource passes ({source_count}):\n{source_log}\nnative passes ({native_count}):\n{native_log}"
    );
}

#[test]
fn a_stateful_clean_filter_preserves_source_object_and_index() {
    // A clean filter whose output changes on every invocation makes the filter
    // pass number authored commit data. Git 2.43's partial commit re-adds the
    // requested paths first to the real index (`add_to_index` with
    // `CE_MATCH_RACY_IS_DIRTY`) and then to the false index seeded from HEAD;
    // `fill_stat_cache_info` marks each added entry `CE_UPTODATE`, so the
    // refresh between the two adds does not add a pass for it. A stat-clean
    // entry skips the first re-add entirely. Both branches are made
    // deterministic here by forcing the worktree mtime: 2001 is strictly older
    // than the index (non-racy, two invocations), 2035 is strictly newer
    // (racy, three invocations). The real index keeps an earlier pass than the
    // landed object, so an `M` residue on the requested path is reference data,
    // not something to normalize away.
    for (label, mtime) in [
        ("non-racy old mtime", 978_307_200u64),
        ("racy future mtime", 2_051_222_400u64),
    ] {
        let expected_count = if mtime == 978_307_200 { 2 } else { 3 };
        for repeat in 1..=3 {
            let (source, native) = twins(Shape::PLAIN);
            let source_counter = source
                .repo_root
                .parent()
                .unwrap()
                .join("source-filter-count");
            let native_counter = native
                .repo_root
                .parent()
                .unwrap()
                .join("native-filter-count");
            for (fixture, counter) in [(&source, &source_counter), (&native, &native_counter)] {
                install_counting_filter(fixture, counter, true);
                fixture.git(&["add", "--force", "--", REQUESTED]);
                fs::write(fixture.worktree.join(REQUESTED), b"{\"k\":4}\n").unwrap();
                set_mtime(fixture, REQUESTED, mtime);
                fs::write(counter, b"0").unwrap();
                let _ = fs::remove_file(format!("{}.log", counter.display()));
            }
            let paths = vec![REQUESTED];
            let report = run_source(&source, "ok", &paths, "work");
            assert!(
                source_ok(&report),
                "{label} #{repeat}: reference commits: {report}"
            );
            let source_count = counter_value(&source_counter);
            let source_log = filter_log(&source_counter);

            let native_report = run_native(&native, "ok", &paths, "work");
            assert!(
                native_report.starts_with("result=ok"),
                "{label} #{repeat}: {native_report}"
            );
            let native_count = counter_value(&native_counter);
            let native_log = filter_log(&native_counter);
            assert_eq!(
                native_count, source_count,
                "{label} #{repeat}: the same source algorithm and timestamps must drive the same filter passes\nsource log:\n{source_log}\nnative log:\n{native_log}"
            );
            assert_eq!(
                source_count, expected_count,
                "{label} #{repeat}: the forced mtime selects the reference's non-racy/racy branch\nsource log:\n{source_log}"
            );
            assert_eq!(
                source.git(&["cat-file", "commit", "HEAD"]),
                native.git(&["cat-file", "commit", "HEAD"]),
                "{label} #{repeat}: stateful clean-filter output is authored commit data (full commit object, including tree)"
            );
            let source_state = State::read(&source);
            let native_state = State::read(&native);
            assert_eq!(
                source_state.cached,
                format!("M\t{REQUESTED}\n"),
                "{label} #{repeat}: the reference leaves its own requested index one pass behind the landed object\nsource passes ({source_count}):\n{source_log}"
            );
            assert_eq!(
                native_state.cached, source_state.cached,
                "{label} #{repeat}: the native real index carries the same residue\nnative passes ({native_count}):\n{native_log}"
            );
            assert_eq!(
                native_state, source_state,
                "{label} #{repeat}: full post-commit state parity\nsource: {source_state:#?}\nnative: {native_state:#?}"
            );
            assert_ne!(
                source.git(&["rev-parse", &format!("HEAD:{REQUESTED}")]),
                source.git(&["rev-parse", &format!(":{REQUESTED}")]),
                "{label} #{repeat}: the residue is a different filtered blob, not a clean index"
            );
        }
    }
}
