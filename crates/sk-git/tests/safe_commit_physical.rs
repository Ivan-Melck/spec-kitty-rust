//! Source-paired controls for the exact-ref / unborn publication backend.
//!
//! Every case builds **one** fixture, copies it, and then runs the live pinned
//! Python `safe_commit` on one copy and the native
//! [`GitRepository::safe_physical_commit`] on the other. The two copies are
//! snapshotted with the same code (refs, full commit object, parents, tree,
//! every raw reflog byte, worktree files, index, status), so an equal snapshot
//! means equal published state — not equal prose.
//!
//! Identity and dates are fixed through the process environment of the children
//! (`GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE`), never by mutating this process, so
//! object ids are comparable and the runs are hermetic.
//!
//! The live oracle is required: a missing/drifted interpreter or requirement set
//! fails the test. There is no skip path.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{
    BranchName, CommitGuardCapability, CommitMessage, GitError, GitPath, GitRepository, Oid,
    ProtectedBranches, RefName, SafeCommitPhysicalRequest, SafeCommitPlanFailure,
    SafeCommitPreflightStage,
};
use sk_kernel::content_digest::sha256_hex;
use tempfile::TempDir;

const MESSAGE: &str = "physical probe";
const REQUESTED: &str = "kitty-ops/rec.jsonl";
const NBSP: &str = "topic\u{a0}";

// ---------------------------------------------------------------------------
// Isolation
// ---------------------------------------------------------------------------

/// The isolated roots one side of a comparison runs in.
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
        let io = Io {
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
            fs::create_dir_all(directory).expect("create isolation directory");
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
    let mut command = Command::new("git");
    command.current_dir(cwd).args(args).env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    io.apply(&mut command);
    let output = command.output().expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn git_text(io: &Io, cwd: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git(io, cwd, args))
        .trim()
        .to_owned()
}

/// Git stdout with only the line terminator removed.
///
/// This is deliberately **not** `str::trim`: an exact ref may end in NBSP, and a
/// Unicode trim would silently drop it (which is exactly the source-side
/// normalization this backend must not perform).
fn git_line(io: &Io, cwd: &Path, args: &[&str]) -> String {
    let raw = git(io, cwd, args);
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    text
}

/// A read whose non-zero exit is a legitimate answer (an unborn `HEAD`).
fn git_opt(io: &Io, cwd: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.current_dir(cwd).args(args).env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    io.apply(&mut command);
    let output = command.output().expect("run git");
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// Build the shared fixture for one scenario.
///
/// Every branch/ref shape here is a shape the reference's own `safe_commit`
/// reaches: `work` (ordinary), the trailing-NBSP branch with its `topic` shadow,
/// the real `.git/x` + `refs/heads/refs/x` shadow that makes `--short` print
/// `refs/x`, a branch literally named `HEAD`, a branch named `-name`, and the
/// one unborn fixture the source refuses (no shadow at all).
fn build_fixture(io: &Io, scratch_root: &Path, kind: &str) -> PathBuf {
    let repo = scratch_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(io, &repo, &["init", "-q", "-b", "work"]);
    git(io, &repo, &["config", "user.name", "Test Author"]);
    git(io, &repo, &["config", "user.email", "test@example.invalid"]);
    git(io, &repo, &["config", "commit.gpgsign", "false"]);
    fs::create_dir_all(repo.join("kitty-ops")).unwrap();
    fs::write(repo.join("a.txt"), b"one\n").unwrap();
    fs::write(repo.join(REQUESTED), b"{\"k\":1}\n").unwrap();
    git(io, &repo, &["add", "-A"]);
    git(io, &repo, &["commit", "-q", "-m", "seed"]);
    // Now make the requested path differ from the seed, so the commit carries it.
    fs::write(repo.join("a.txt"), b"two\n").unwrap();
    fs::write(repo.join(REQUESTED), b"{\"k\":2}\n").unwrap();
    let seed = git_text(io, &repo, &["rev-parse", "HEAD"]);

    match kind {
        "born_work" => {}
        "born_nbsp" => {
            git(
                io,
                &repo,
                &["update-ref", &format!("refs/heads/{NBSP}"), &seed],
            );
            git(io, &repo, &["update-ref", "refs/heads/topic", &seed]);
            git(
                io,
                &repo,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{NBSP}")],
            );
        }
        "unborn_nbsp" | "unborn_nbsp_unsigned" => {
            git(io, &repo, &["update-ref", "refs/heads/topic", &seed]);
            git(
                io,
                &repo,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{NBSP}")],
            );
        }
        "born_refsx" | "unborn_refsx" | "unborn_refsx_always" => {
            // The real root-level `.git/x` ref is the disambiguator: with it
            // `symbolic-ref --short HEAD` prints `refs/x` instead of `x`.
            fs::write(repo.join(".git/x"), format!("{seed}\n")).unwrap();
            git(io, &repo, &["update-ref", "refs/heads/refs/x", &seed]);
            if kind == "born_refsx" {
                git(io, &repo, &["update-ref", "refs/x", &seed]);
            }
            git(io, &repo, &["symbolic-ref", "HEAD", "refs/x"]);
        }
        "born_head" => {
            git(io, &repo, &["update-ref", "refs/heads/HEAD", &seed]);
            git(io, &repo, &["symbolic-ref", "HEAD", "refs/heads/HEAD"]);
        }
        "born_dash" => {
            git(io, &repo, &["update-ref", "refs/heads/-name", &seed]);
            git(io, &repo, &["symbolic-ref", "HEAD", "refs/heads/-name"]);
        }
        "unborn_nbsp_custom_index" => {
            git(io, &repo, &["update-ref", "refs/heads/topic", &seed]);
            git(
                io,
                &repo,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{NBSP}")],
            );
            // An unrelated staged entry in a *captured* operator index: the
            // commit must never read it, and the reconciliation must touch only
            // the requested path.
            fs::create_dir_all(scratch_root).unwrap();
            let custom = scratch_root.join("custom-index");
            let mut command = Command::new("git");
            command
                .current_dir(&repo)
                .args(["add", "--force", "--", "a.txt"])
                .env_clear();
            command.env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            );
            io.apply(&mut command);
            command.env("GIT_INDEX_FILE", &custom);
            assert!(command.status().unwrap().success(), "seed the custom index");
        }
        "unborn_plain" => {
            // No shadow: the source's step-5 existence check refuses this shape.
            git(io, &repo, &["symbolic-ref", "HEAD", "refs/heads/vacant"]);
        }
        other => panic!("unknown scenario {other}"),
    }
    if kind == "unborn_refsx_always" {
        git(io, &repo, &["config", "core.logAllRefUpdates", "always"]);
    }
    if kind == "unborn_nbsp_unsigned" {
        git(io, &repo, &["config", "commit.gpgsign", "true"]);
        git(
            io,
            &repo,
            &[
                "config",
                "gpg.program",
                "/nonexistent/spec-kitty-test-signer",
            ],
        );
        git(
            io,
            &repo,
            &["config", "user.signingkey", "unavailable-test-key"],
        );
    }
    repo
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let source = entry.path();
        let target = to.join(entry.file_name());
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            copy_tree(&source, &target);
        } else if file_type.is_symlink() {
            let link = fs::read_link(&source).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, &target).unwrap();
            #[cfg(not(unix))]
            fs::copy(&source, &target).unwrap();
        } else {
            fs::copy(&source, &target).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot (one implementation for both sides)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    refs: Vec<String>,
    head: String,
    parents: Vec<String>,
    tree: String,
    commit: String,
    reflogs: BTreeMap<String, String>,
    status: Vec<String>,
    index: Vec<String>,
    files: BTreeMap<String, String>,
}

fn snapshot(io: &Io, repo: &Path) -> Snapshot {
    snapshot_with_index(io, repo, None)
}

/// Snapshot with an explicit operator index (the captured `GIT_INDEX_FILE`).
fn snapshot_with_index(io: &Io, repo: &Path, custom_index: Option<&Path>) -> Snapshot {
    let head = git_opt(io, repo, &["rev-parse", "--verify", "--quiet", "HEAD"]).unwrap_or_default();
    let (parents, tree, commit) = if head.is_empty() {
        (Vec::new(), String::new(), String::new())
    } else {
        let parents = git_text(io, repo, &["rev-list", "--parents", "-n", "1", &head])
            .split_whitespace()
            .skip(1)
            .map(str::to_owned)
            .collect();
        let tree = git_text(io, repo, &["rev-parse", &format!("{head}^{{tree}}")]);
        let commit =
            String::from_utf8_lossy(&git(io, repo, &["cat-file", "commit", &head])).into_owned();
        (parents, tree, commit)
    };
    let mut reflogs = BTreeMap::new();
    let logs = repo.join(".git").join("logs");
    if logs.is_dir() {
        collect_reflogs(&logs, &logs, &mut reflogs);
    }
    let mut files = BTreeMap::new();
    collect_files(repo, repo, &mut files);
    Snapshot {
        refs: git_text(
            io,
            repo,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        )
        .lines()
        .map(str::to_owned)
        .collect(),
        head,
        parents,
        tree,
        commit,
        reflogs,
        status: git_lines_index(io, repo, &["status", "--porcelain=v1"], custom_index),
        index: git_lines_index(io, repo, &["ls-files", "-s", "--stage"], custom_index),
        files,
    }
}

fn git_lines_index(
    io: &Io,
    repo: &Path,
    args: &[&str],
    custom_index: Option<&Path>,
) -> Vec<String> {
    let mut command = Command::new("git");
    command.current_dir(repo).args(args).env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    io.apply(&mut command);
    if let Some(index) = custom_index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output().expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn collect_reflogs(logs: &Path, directory: &Path, out: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_reflogs(logs, &path, out);
        } else {
            // Hex keeps the exact bytes (NBSP refs, NUL-separated fields).
            let bytes = fs::read(&path).unwrap();
            out.insert(
                path.strip_prefix(logs).unwrap().display().to_string(),
                bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
            );
        }
    }
}

fn collect_files(repo: &Path, directory: &Path, out: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let relative = path.strip_prefix(repo).unwrap();
        if relative
            .components()
            .next()
            .is_some_and(|first| first.as_os_str() == ".git")
        {
            continue;
        }
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            collect_files(repo, &path, out);
        } else if file_type.is_file() {
            out.insert(
                relative.display().to_string(),
                sha256_hex(&fs::read(&path).unwrap()),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Live source side
// ---------------------------------------------------------------------------

const SOURCE_DRIVER: &str = r#"
import os
import sys
from pathlib import Path
from specify_cli.git.commit_helpers import safe_commit

repo, dest, message, requested = sys.argv[1:5]
assert Path.cwd() == Path(os.environ["HOME"])
for name in ("TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[name]).parent == Path(os.environ["HOME"]).parent
try:
    safe_commit(
        repo_root=Path(repo),
        worktree_root=Path(repo),
        destination_ref=dest,
        message=message,
        paths=(Path(requested),),
    )
except Exception as exc:  # the reference's typed refusals
    print("result=error")
    print(f"error={type(exc).__name__}")
else:
    print("result=ok")
"#;

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("project root")
}

/// Verify the pinned interpreter through the canonical verifier, then run the
/// reference `safe_commit` and return `(result, error-class)`.
fn run_source(io: &Io, repo: &Path, dest: &str, custom_index: Option<&Path>) -> (String, String) {
    let project = project_root();
    let python = project.join(".venv-rust-oracle").join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    assert!(
        python.is_file(),
        "the pinned oracle interpreter is required"
    );

    let mut verify = Command::new(&python);
    verify.current_dir(&io.home).env_clear();
    verify.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    io.apply(&mut verify);
    verify
        .arg(project.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&project);
    let out = verify.output().expect("run the oracle verifier");
    assert!(
        out.status.success(),
        "exact53 oracle verification failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let driver = repo
        .parent()
        .expect("scratch root")
        .join("source-driver.py");
    fs::write(&driver, SOURCE_DRIVER).unwrap();
    let mut command = Command::new(&python);
    command.env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    io.apply(&mut command);
    command
        .current_dir(&io.home)
        .env("PYTHONPATH", project.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", project.join("packs"))
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&driver)
        .arg(repo)
        .arg(dest)
        .arg(MESSAGE)
        .arg(REQUESTED);
    if let Some(index) = custom_index {
        command.env("GIT_INDEX_FILE", index);
    }
    let out = command.output().expect("run the pinned reference");
    assert!(
        out.status.success(),
        "the source driver failed: {} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut result = String::new();
    let mut error = String::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(value) = line.strip_prefix("result=") {
            result = value.to_owned();
        } else if let Some(value) = line.strip_prefix("error=") {
            error = value.to_owned();
        }
    }
    assert!(!result.is_empty(), "the source driver printed no result");
    (result, error)
}

// ---------------------------------------------------------------------------
// Native side
// ---------------------------------------------------------------------------

/// The native side runs in its own process with a cleared environment, so the
/// fixture is isolated exactly like the source side and object ids are fixed.
#[test]
#[ignore = "subprocess helper for the source-paired matrix"]
fn physical_native_helper() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("isolated HOME"));
    assert_eq!(
        std::env::current_dir().unwrap(),
        home,
        "native product probe needs an actual temporary CWD, not just env_clear"
    );
    for name in [
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ] {
        assert!(
            PathBuf::from(std::env::var_os(name).expect(name)).starts_with(home.parent().unwrap())
        );
    }
    let args: Vec<String> = std::env::args().collect();
    let rest: Vec<&String> = args
        .iter()
        .skip_while(|arg| arg.as_str() != "--")
        .skip(1)
        .collect();
    let (repo, reference, out) = (rest[0], rest[1], rest[2]);
    let destination = RefName::parse(reference).expect("validated exact ref");
    let repository = GitRepository::open(repo).expect("open fixture repository");
    let request = SafeCommitPhysicalRequest::new(
        destination,
        [GitPath::from_git_bytes(REQUESTED.as_bytes()).expect("path")],
    );
    let report = match repository.safe_physical_commit(&request, CommitMessage::from_text(MESSAGE))
    {
        Ok(outcome) => format!(
            "result=ok\ncommit={}\nwas_unborn={}\n",
            outcome.commit(),
            outcome.was_unborn()
        ),
        Err(error) => format!("result=error\nerror={}\n", native_class(&error)),
    };
    fs::write(out, report).unwrap();
}

fn native_class(error: &GitError) -> String {
    let debug = format!("{error:?}");
    debug
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("GitError")
        .to_owned()
}

fn run_native(
    io: &Io,
    repo: &Path,
    reference: &str,
    custom_index: Option<&Path>,
) -> BTreeMap<String, String> {
    let out = repo.parent().unwrap().join("native-result.txt");
    let exe = std::env::current_exe().expect("current test executable");
    let mut command = Command::new(exe);
    command.current_dir(&io.home).env_clear();
    command.env(
        "PATH",
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
    );
    io.apply(&mut command);
    if let Some(index) = custom_index {
        command.env("GIT_INDEX_FILE", index);
    }
    command.args([
        "--ignored",
        "--exact",
        "physical_native_helper",
        "--",
        repo.to_str().expect("UTF-8 path"),
        reference,
        out.to_str().expect("UTF-8 path"),
    ]);
    let output = command.output().expect("spawn the native helper");
    assert!(
        output.status.success(),
        "the native helper failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut fields = BTreeMap::new();
    for line in fs::read_to_string(&out).unwrap().lines() {
        if let Some((key, value)) = line.split_once('=') {
            fields.insert(key.to_owned(), value.to_owned());
        }
    }
    fields
}

// ---------------------------------------------------------------------------
// The paired matrix
// ---------------------------------------------------------------------------

/// One scenario: the fixture kind, the destination the **source** accepts (its
/// own short/judged spelling), and the **exact** ref the native backend targets.
struct Case {
    kind: &'static str,
    source_destination: &'static str,
    native_ref: &'static str,
    source_ok: bool,
    expect_unborn: bool,
    custom_index: bool,
}

const CASES: &[Case] = &[
    Case {
        kind: "unborn_refsx_always",
        source_destination: "refs/x",
        native_ref: "refs/x",
        source_ok: true,
        expect_unborn: true,
        custom_index: false,
    },
    Case {
        kind: "unborn_nbsp_unsigned",
        source_destination: "topic",
        native_ref: "refs/heads/topic\u{a0}",
        source_ok: true,
        expect_unborn: true,
        custom_index: false,
    },
    Case {
        kind: "born_work",
        source_destination: "work",
        native_ref: "refs/heads/work",
        source_ok: true,
        expect_unborn: false,
        custom_index: false,
    },
    Case {
        kind: "born_nbsp",
        source_destination: "topic",
        native_ref: "refs/heads/topic\u{a0}",
        source_ok: true,
        expect_unborn: false,
        custom_index: false,
    },
    Case {
        kind: "unborn_nbsp",
        source_destination: "topic",
        native_ref: "refs/heads/topic\u{a0}",
        source_ok: true,
        expect_unborn: true,
        custom_index: false,
    },
    Case {
        kind: "born_refsx",
        source_destination: "refs/x",
        native_ref: "refs/x",
        source_ok: true,
        expect_unborn: false,
        custom_index: false,
    },
    Case {
        kind: "unborn_refsx",
        source_destination: "refs/x",
        native_ref: "refs/x",
        source_ok: true,
        expect_unborn: true,
        custom_index: false,
    },
    Case {
        kind: "unborn_nbsp_custom_index",
        source_destination: "topic",
        native_ref: "refs/heads/topic\u{a0}",
        source_ok: true,
        expect_unborn: true,
        custom_index: true,
    },
    Case {
        kind: "born_head",
        source_destination: "HEAD",
        native_ref: "refs/heads/HEAD",
        source_ok: true,
        expect_unborn: false,
        custom_index: false,
    },
    Case {
        kind: "born_dash",
        source_destination: "-name",
        native_ref: "refs/heads/-name",
        source_ok: true,
        expect_unborn: false,
        custom_index: false,
    },
];

#[test]
fn the_exact_backend_publishes_the_same_physical_state_as_the_pinned_source() {
    let temp = TempDir::new().unwrap();
    for case in CASES {
        let root = temp.path().join(case.kind);
        let fixture = root.join("fixture");
        let source_root = root.join("source");
        let native_root = root.join("native");
        let fixture_io = Io::create(&root.join("fixture-env"));
        let fixture_repo = build_fixture(&fixture_io, &fixture, case.kind);
        copy_tree(&fixture_repo, &source_root.join("repo"));
        copy_tree(&fixture_repo, &native_root.join("repo"));
        let source_index = case.custom_index.then(|| source_root.join("custom-index"));
        let native_index = case.custom_index.then(|| native_root.join("custom-index"));
        if case.custom_index {
            fs::copy(fixture.join("custom-index"), source_index.as_ref().unwrap()).unwrap();
            fs::copy(fixture.join("custom-index"), native_index.as_ref().unwrap()).unwrap();
        }
        let source_index = source_index.as_deref();
        let native_index = native_index.as_deref();
        let source_io = Io::create(&source_root);
        let native_io = Io::create(&native_root);

        let before_source =
            snapshot_with_index(&source_io, &source_root.join("repo"), source_index);
        let before_native =
            snapshot_with_index(&native_io, &native_root.join("repo"), native_index);
        assert_eq!(
            before_source, before_native,
            "{}: the two fixture copies are not identical before the commit",
            case.kind
        );

        // With a captured operator index, the default `<git-dir>/index` must
        // stay byte-identical: every staging and reconciliation went to the
        // captured authority.
        let default_index_bytes_before = case.custom_index.then(|| {
            (
                fs::read(source_root.join("repo/.git/index")).unwrap(),
                fs::read(native_root.join("repo/.git/index")).unwrap(),
            )
        });
        let default_index_before = case.custom_index.then(|| {
            (
                git_lines_index(
                    &source_io,
                    &source_root.join("repo"),
                    &["ls-files", "-s", "--stage"],
                    None,
                ),
                git_lines_index(
                    &native_io,
                    &native_root.join("repo"),
                    &["ls-files", "-s", "--stage"],
                    None,
                ),
            )
        });

        let (result, error) = run_source(
            &source_io,
            &source_root.join("repo"),
            case.source_destination,
            source_index,
        );
        assert_eq!(
            result == "ok",
            case.source_ok,
            "{}: unexpected source result {result} ({error})",
            case.kind
        );

        let native = run_native(
            &native_io,
            &native_root.join("repo"),
            case.native_ref,
            native_index,
        );
        assert_eq!(
            native.get("result").map(String::as_str),
            Some(if case.source_ok { "ok" } else { "error" }),
            "{}: native result {native:?}",
            case.kind
        );
        assert_eq!(
            native.get("was_unborn").map(String::as_str),
            Some(if case.expect_unborn { "true" } else { "false" }),
            "{}: the plan's real destination state",
            case.kind
        );

        if let Some((source_before, native_before)) = default_index_before {
            assert_eq!(
                git_lines_index(
                    &source_io,
                    &source_root.join("repo"),
                    &["ls-files", "-s", "--stage"],
                    None
                ),
                source_before,
                "{}: the source must not write the default index",
                case.kind
            );
            assert_eq!(
                git_lines_index(
                    &native_io,
                    &native_root.join("repo"),
                    &["ls-files", "-s", "--stage"],
                    None
                ),
                native_before,
                "{}: the native backend must reconcile the captured index, not the default one",
                case.kind
            );
        }
        if let Some((source_bytes, native_bytes)) = default_index_bytes_before {
            assert_eq!(
                fs::read(source_root.join("repo/.git/index")).unwrap(),
                source_bytes,
                "source default index bytes unchanged under captured index"
            );
            assert_eq!(
                fs::read(native_root.join("repo/.git/index")).unwrap(),
                native_bytes,
                "native default index bytes unchanged under captured index"
            );
        }
        let source_after = snapshot_with_index(&source_io, &source_root.join("repo"), source_index);
        let native_after = snapshot_with_index(&native_io, &native_root.join("repo"), native_index);
        assert_eq!(
            source_after, native_after,
            "{}: the published physical state differs from the pinned source",
            case.kind
        );
        if case.source_ok {
            assert!(
                source_after
                    .refs
                    .iter()
                    .any(|line| *line == format!("{} {}", case.native_ref, source_after.head)),
                "{}: the exact destination ref must carry the new commit ({:?})",
                case.kind,
                source_after.refs
            );
            assert_eq!(
                native.get("commit"),
                Some(&source_after.head),
                "{}: the native commit must be the object the source created",
                case.kind
            );
            assert_eq!(
                source_after.parents.len(),
                if case.expect_unborn { 0 } else { 1 },
                "{}: parent count",
                case.kind
            );
            if case.expect_unborn {
                assert!(
                    !source_after.commit.contains("\nparent "),
                    "{}: a root commit object carries no parent header",
                    case.kind
                );
            }
        }
    }
}

/// The source's own step 5 refuses an unborn destination with **no shadow**.
/// The backend still publishes: source adjudication (J) is the Op's job, and the
/// library must not copy a source quirk into a refusal. This is a capability
/// statement, not an Op/workflow parity claim.
#[test]
fn the_exact_backend_is_a_capability_not_a_copy_of_the_source_shadow_rule() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("unborn_plain");
    let fixture = root.join("fixture");
    let source_root = root.join("source");
    let native_root = root.join("native");
    let fixture_repo = build_fixture(
        &Io::create(&root.join("fixture-env")),
        &fixture,
        "unborn_plain",
    );
    copy_tree(&fixture_repo, &source_root.join("repo"));
    copy_tree(&fixture_repo, &native_root.join("repo"));
    let source_io = Io::create(&source_root);
    let native_io = Io::create(&native_root);

    let (result, error) = run_source(&source_io, &source_root.join("repo"), "vacant", None);
    assert_eq!(result, "error");
    assert_eq!(error, "SafeCommitDestinationNotFound");

    let native = run_native(
        &native_io,
        &native_root.join("repo"),
        "refs/heads/vacant",
        None,
    );
    assert_eq!(native.get("result").map(String::as_str), Some("ok"));
    let after = snapshot(&native_io, &native_root.join("repo"));
    assert_eq!(
        after.parents.len(),
        0,
        "an absent destination is a root commit"
    );
    assert!(
        after
            .refs
            .iter()
            .any(|line| line == &format!("refs/heads/vacant {}", after.head))
    );
}

// ---------------------------------------------------------------------------
// Plan shape, guard, races (read-only planning is in-process)
// ---------------------------------------------------------------------------

struct Fixture {
    _temp: TempDir,
    repo: PathBuf,
    io: Io,
}

impl Fixture {
    fn new(kind: &str) -> Self {
        let temp = TempDir::new().unwrap();
        let io = Io::create(temp.path());
        let repo = build_fixture(&io, temp.path(), kind);
        Self {
            _temp: temp,
            repo,
            io,
        }
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.repo).unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        git_text(&self.io, &self.repo, args)
    }

    fn git_line(&self, args: &[&str]) -> String {
        git_line(&self.io, &self.repo, args)
    }

    fn request(&self, reference: &str) -> SafeCommitPhysicalRequest {
        SafeCommitPhysicalRequest::new(
            RefName::parse(reference).unwrap(),
            [GitPath::from_git_bytes(REQUESTED.as_bytes()).unwrap()],
        )
    }

    fn path(&self, bytes: &[u8]) -> GitPath {
        GitPath::from_git_bytes(bytes).unwrap()
    }
}

#[test]
fn a_plan_records_the_real_destination_state_and_the_exact_ref() {
    let born = Fixture::new("born_head");
    let repository = born.open();
    let before = snapshot(&born.io, &born.repo);
    let plan = repository
        .plan_safe_physical_commit(&born.request("refs/heads/HEAD"))
        .expect("an exact born ref plans");
    assert_eq!(plan.destination_ref().as_str(), "refs/heads/HEAD");
    assert!(!plan.is_unborn());
    assert_eq!(
        plan.destination_tip().map(Oid::to_string),
        Some(born.git(&["rev-parse", "refs/heads/HEAD"]))
    );
    assert_eq!(
        plan.head().map(Oid::to_string),
        plan.destination_tip().map(Oid::to_string)
    );
    assert_eq!(plan.route(), sk_git::SafeCommitRoute::Worktree);
    assert_eq!(plan.paths(), &[born.path(REQUESTED.as_bytes())]);
    assert!(plan.guard_verdict().allowed());
    assert_eq!(
        plan.guard_verdict().destination().as_str(),
        "refs/heads/HEAD"
    );
    assert!(!plan.index_authority().is_captured());
    assert_eq!(
        snapshot(&born.io, &born.repo),
        before,
        "planning is read-only: no ref, object, reflog, index or worktree byte moves"
    );

    // `symbolic-ref --short` for this branch is the disambiguated `heads/HEAD`,
    // so a plan that used the short label could not have resolved at all.
    assert_eq!(
        born.git_line(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
        "heads/HEAD"
    );

    let unborn = Fixture::new("unborn_nbsp");
    let repository = unborn.open();
    let before = snapshot(&unborn.io, &unborn.repo);
    let plan = repository
        .plan_safe_physical_commit(&unborn.request("refs/heads/topic\u{a0}"))
        .expect("a genuinely absent exact ref plans");
    assert!(plan.is_unborn());
    assert_eq!(plan.head(), None);
    assert_eq!(plan.destination_tip(), None);
    assert_eq!(
        unborn.git_line(&["symbolic-ref", "HEAD"]),
        "refs/heads/topic\u{a0}"
    );
    // HEAD is genuinely unborn and the judged shadow exists untouched.
    assert_eq!(
        git_opt(
            &unborn.io,
            &unborn.repo,
            &["rev-parse", "--verify", "--quiet", "HEAD"]
        ),
        None,
        "an absent exact destination must also have no HEAD commit"
    );
    let shadow = unborn.git(&["rev-parse", "refs/heads/topic"]);
    assert!(!shadow.is_empty(), "the judged shadow exists");
    assert_eq!(plan.destination_ref().as_str(), "refs/heads/topic\u{a0}");
    assert_ne!(plan.destination_ref().as_str(), "refs/heads/topic");
    assert_eq!(
        unborn.git(&["rev-parse", "refs/heads/topic"]),
        shadow,
        "planning never touches the shadow"
    );
    assert_eq!(
        snapshot(&unborn.io, &unborn.repo),
        before,
        "planning an absent destination is read-only"
    );
}

#[test]
fn an_unborn_destination_that_appeared_is_a_stale_plan_not_an_overwrite() {
    let fixture = Fixture::new("unborn_nbsp");
    let repository = fixture.open();
    let plan = repository
        .plan_safe_physical_commit(&fixture.request("refs/heads/topic\u{a0}"))
        .unwrap();
    let message = CommitMessage::from_text(MESSAGE);
    let prepared = plan.prepare(message).unwrap();

    // Another writer publishes the ref first.
    let foreign = fixture.git(&["rev-parse", "refs/heads/topic"]);
    fixture.git(&["update-ref", "refs/heads/topic\u{a0}", &foreign]);
    let error = repository
        .apply_safe_physical_commit(&prepared)
        .unwrap_err();
    assert!(
        matches!(error, GitError::PlanStale { .. }),
        "an absent-ref create must not overwrite a ref that appeared: {error:?}"
    );
    assert_eq!(
        fixture.git(&["rev-parse", "refs/heads/topic\u{a0}"]),
        foreign,
        "the other writer's tip is intact"
    );
}

#[test]
fn a_born_destination_that_moved_is_a_stale_plan() {
    let fixture = Fixture::new("born_work");
    let repository = fixture.open();
    let plan = repository
        .plan_safe_physical_commit(&fixture.request("refs/heads/work"))
        .unwrap();
    let prepared = plan.prepare(CommitMessage::from_text(MESSAGE)).unwrap();
    let seed = fixture.git(&["rev-parse", "refs/heads/work"]);
    // A real move: commit the worktree changes on the branch behind our back.
    fixture.git(&["add", "-A"]);
    fixture.git(&["commit", "-q", "-m", "competitor"]);
    let moved = fixture.git(&["rev-parse", "refs/heads/work"]);
    assert_ne!(moved, seed);

    let error = repository
        .apply_safe_physical_commit(&prepared)
        .unwrap_err();
    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
    assert_eq!(fixture.git(&["rev-parse", "refs/heads/work"]), moved);
}

#[test]
fn legacy_stale_plan_keeps_its_fully_qualified_error_identity() {
    let fixture = Fixture::new("born_work");
    let repository = fixture.open();
    let request = sk_git::SafeCommitRequest::new(
        BranchName::parse("work").unwrap(),
        MESSAGE,
        [fixture.path(REQUESTED.as_bytes())],
    );
    let plan = repository.plan_safe_commit(&request).unwrap();
    fixture.git(&["add", "-A"]);
    fixture.git(&["commit", "-q", "-m", "competitor"]);
    let error = repository.apply_safe_commit(&plan).unwrap_err();
    match error {
        GitError::PlanStale { what, .. } => assert_eq!(
            what, "refs/heads/work",
            "legacy public error value must not become the short label"
        ),
        other => panic!("expected stale plan, got {other:?}"),
    }
}

#[test]
fn a_corrupt_present_ref_is_not_a_genuinely_absent_destination() {
    let fixture = Fixture::new("unborn_nbsp");
    let reference_file = fixture.repo.join(".git/refs/heads/topic\u{a0}");
    fs::write(&reference_file, b"not-an-object-id\n").unwrap();
    let repository = fixture.open();
    assert!(
        repository
            .plan_safe_physical_commit(&fixture.request("refs/heads/topic\u{a0}"))
            .is_err(),
        "a corrupt present ref is not an unborn plan"
    );
    assert_eq!(fs::read(reference_file).unwrap(), b"not-an-object-id\n");
}

#[test]
fn switching_head_after_planning_is_refused() {
    let fixture = Fixture::new("born_nbsp");
    let repository = fixture.open();
    let plan = repository
        .plan_safe_physical_commit(&fixture.request("refs/heads/topic\u{a0}"))
        .unwrap();
    let prepared = plan.prepare(CommitMessage::from_text(MESSAGE)).unwrap();
    fixture.git(&["symbolic-ref", "HEAD", "refs/heads/topic"]);
    let error = repository
        .apply_safe_physical_commit(&prepared)
        .unwrap_err();
    assert!(
        matches!(error, GitError::SafeCommitHeadMismatch { .. }),
        "the exact destination must still be what HEAD names: {error:?}"
    );
    assert_eq!(
        fixture.git(&["rev-parse", "refs/heads/topic\u{a0}"]),
        fixture.git(&["rev-parse", "refs/heads/topic"])
    );
}

#[test]
fn the_exact_guard_is_the_one_decision_core() {
    let fixture = Fixture::new("born_nbsp");
    let repository = fixture.open();
    let protected = ProtectedBranches::new([BranchName::parse("topic\u{a0}").unwrap()]);
    let refused = repository
        .plan_safe_physical_commit(
            &fixture
                .request("refs/heads/topic\u{a0}")
                .with_protection(protected.clone(), CommitGuardCapability::Standard),
        )
        .unwrap_err();
    assert_eq!(refused.stage(), SafeCommitPreflightStage::Guard);
    assert!(matches!(
        refused.error(),
        GitError::SafeCommitProtectedBranch { destination, .. } if destination == "refs/heads/topic\u{a0}"
    ));

    let allowed = repository
        .plan_safe_physical_commit(
            &fixture
                .request("refs/heads/topic\u{a0}")
                .with_protection(protected, CommitGuardCapability::ReleaseFlow),
        )
        .expect("a booked flow is authorized by the same decision core");
    assert!(allowed.guard_verdict().allowed());
    assert!(allowed.guard_verdict().reason().contains("release_flow"));

    // A destination no `BranchName` can express is never a member of the set;
    // the verdict says so instead of silently widening the set's element type.
    let refsx = Fixture::new("born_refsx");
    let repository = refsx.open();
    let plan = repository
        .plan_safe_physical_commit(&refsx.request("refs/x").with_protection(
            ProtectedBranches::new([BranchName::parse("x").unwrap()]),
            CommitGuardCapability::Standard,
        ))
        .expect("`refs/x` is not expressible in a `BranchName` set");
    assert!(plan.guard_verdict().allowed());
    assert!(
        plan.guard_verdict()
            .reason()
            .contains("not in the resolved protected set")
    );
}

#[test]
fn an_empty_exact_request_is_refused_before_any_destination_probe() {
    let fixture = Fixture::new("born_work");
    let repository = fixture.open();
    let request =
        SafeCommitPhysicalRequest::new(RefName::parse("refs/heads/work").unwrap(), Vec::new());
    let failure: SafeCommitPlanFailure = repository
        .plan_safe_physical_commit(&request)
        .expect_err("an empty path set is refused");
    assert_eq!(failure.stage(), SafeCommitPreflightStage::RequestShape);
    assert!(matches!(
        failure.error(),
        GitError::SafeCommitEmptyChangeset
    ));

    // The prepare-time message stage is shared with the legacy entry points.
    let plan = repository
        .plan_safe_physical_commit(&fixture.request("refs/heads/work"))
        .unwrap();
    let failure = plan
        .prepare(CommitMessage::from_text(""))
        .expect_err("an empty message is refused");
    assert_eq!(failure.stage(), SafeCommitPreflightStage::Message);
}

#[test]
fn a_requested_path_outside_the_policy_is_refused_by_the_shared_path_rules() {
    let fixture = Fixture::new("born_work");
    let repository = fixture.open();
    let request = SafeCommitPhysicalRequest::new(
        RefName::parse("refs/heads/work").unwrap(),
        [fixture.path(b".worktrees/lane/a.txt")],
    );
    let failure = repository
        .plan_safe_physical_commit(&request)
        .expect_err("a `.worktrees/` path is refused");
    assert_eq!(failure.stage(), SafeCommitPreflightStage::PathPolicy);
}

#[test]
fn a_root_candidate_cas_race_retains_real_unconfirmed_evidence() {
    let fixture = Fixture::new("unborn_nbsp");
    let competitor = fixture.git(&["rev-parse", "refs/heads/topic"]);
    let hook = fixture.repo.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        format!("#!/bin/sh\ngit update-ref 'refs/heads/{NBSP}' '{competitor}'\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let repository = fixture.open();
    let error = repository
        .safe_physical_commit(
            &fixture.request(&format!("refs/heads/{NBSP}")),
            CommitMessage::from_text(MESSAGE),
        )
        .unwrap_err();
    let candidate = match error {
        GitError::SafeCommitAdvanceUnconfirmed {
            destination,
            candidate,
            observed_tip,
            ..
        } => {
            assert_eq!(destination, format!("refs/heads/{NBSP}"));
            assert_eq!(observed_tip.unwrap().as_str(), competitor);
            candidate
        }
        other => panic!("candidate exists but root CAS lost; must retain evidence: {other:?}"),
    };
    let object = fixture.git(&["cat-file", "-p", candidate.as_str()]);
    assert!(!object.lines().any(|line| line.starts_with("parent ")));
    assert!(object.ends_with(MESSAGE));
    assert_eq!(
        fixture.git(&["rev-parse", &format!("refs/heads/{NBSP}")]),
        competitor
    );
    assert_eq!(fixture.git(&["rev-parse", "refs/heads/topic"]), competitor);
    assert_eq!(
        fs::read(fixture.repo.join(REQUESTED)).unwrap(),
        b"{\"k\":2}\n"
    );
}

/// A rejecting `pre-commit` hook refuses the root commit and leaves the absent
/// destination exactly as it was: no object, no ref, no seed parent.
#[test]
fn a_rejecting_hook_leaves_an_unborn_destination_absent() {
    let fixture = Fixture::new("unborn_nbsp");
    let hooks = fixture.repo.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    fs::write(&hook, "#!/bin/sh\nprintf 'root boom\\n' >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let repository = fixture.open();
    let plan = repository
        .plan_safe_physical_commit(&fixture.request("refs/heads/topic\u{a0}"))
        .expect("an absent exact ref plans");
    let prepared = plan.prepare(CommitMessage::from_text(MESSAGE)).unwrap();
    let error = repository
        .apply_safe_physical_commit(&prepared)
        .unwrap_err();
    assert!(
        matches!(error, GitError::SafeCommitHookRejected { ref hook, .. } if hook == "pre-commit"),
        "{error:?}"
    );
    assert_eq!(
        git_opt(
            &fixture.io,
            &fixture.repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/topic\u{a0}"]
        ),
        None,
        "a refused root commit must not create the destination ref"
    );
    assert_eq!(
        git_opt(
            &fixture.io,
            &fixture.repo,
            &["rev-parse", "--verify", "--quiet", "HEAD"]
        ),
        None,
        "and HEAD must still be unborn"
    );
}
