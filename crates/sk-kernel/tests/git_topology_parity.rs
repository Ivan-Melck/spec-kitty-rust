//! Differential Git-topology parity: Rust discovery vs Git vs pinned Python.
//!
//! Every fixture is a real on-disk layout (a real `git init` / `git worktree`
//! / separate-git-dir / bare repo / absorbed submodule gitdir / invalid
//! marker). The test compares three independent observers:
//!
//! 1. the native Rust resolver,
//! 2. `git rev-parse` itself (resolved and classified exactly as upstream
//!    `kernel.git_topology` does),
//! 3. the pinned Python `kernel.git_topology` probe (when installed).
//!
//! Environment-sensitive cases run in child processes because edition 2024
//! makes `std::env::set_var` unsafe, and this workspace forbids `unsafe`.
//!
//! The test is development evidence, not a runtime dependency.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PINNED_PYTHON: &str = "/home/exedev/.local/share/uv/tools/spec-kitty-cli/bin/python";
const CHILD_OP: &str = "SK_KT_OP";
const CHILD_PATH: &str = "SK_KT_PATH";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Observed {
    Common(PathBuf),
    Toplevel(PathBuf),
    NotAGit,
    Unavailable,
}

impl Observed {
    fn encoded(&self) -> String {
        match self {
            Self::Common(path) => format!("common:{}", path.display()),
            Self::Toplevel(path) => format!("top:{}", path.display()),
            Self::NotAGit => "not-a-git".to_owned(),
            Self::Unavailable => "unavailable".to_owned(),
        }
    }

    fn decode(line: &str) -> Self {
        if let Some(path) = line.strip_prefix("common:") {
            Self::Common(PathBuf::from(path))
        } else if let Some(path) = line.strip_prefix("top:") {
            Self::Toplevel(PathBuf::from(path))
        } else if line == "not-a-git" {
            Self::NotAGit
        } else if line == "unavailable" {
            Self::Unavailable
        } else {
            panic!("unrecognised observation {line:?}");
        }
    }
}

fn python() -> Option<PathBuf> {
    let path = std::env::var_os("SK_KERNEL_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(PINNED_PYTHON));
    path.is_file().then_some(path)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn run_git(cwd: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM")
        .output()
        .expect("run git")
}

fn init_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    let output = run_git(path, &["init", "-q", "-b", "main"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run_git(
        path,
        &[
            "-c",
            "user.email=a@b.c",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

type Case = (String, PathBuf, Vec<(&'static str, PathBuf)>);

fn fixture(tmp: &Path) -> Vec<Case> {
    let repo = tmp.join("repo");
    init_repo(&repo);
    let sub = repo.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let worktree = tmp.join("wt");
    let output = run_git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "-b",
            "lane",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let separate = tmp.join("separate");
    fs::create_dir_all(&separate).unwrap();
    let output = run_git(
        &separate,
        &[
            "init",
            "-q",
            "-b",
            "main",
            "--separate-git-dir",
            tmp.join("separate.git").to_str().unwrap(),
            ".",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bare = tmp.join("bare.git");
    let output = run_git(tmp, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bare_sub = bare.join("objects");
    fs::create_dir_all(&bare_sub).unwrap();

    // Absorbed submodule-style gitdir with core.worktree, matching the real
    // layout Git records under `<super>/.git/modules/<name>`.
    let super_repo = tmp.join("super");
    init_repo(&super_repo);
    let absorbed = super_repo.join(".git/modules/child");
    fs::create_dir_all(absorbed.join("objects")).unwrap();
    fs::create_dir_all(absorbed.join("refs")).unwrap();
    fs::write(absorbed.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    fs::write(
        absorbed.join("config"),
        b"[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tworktree = ../../../child\n",
    )
    .unwrap();
    let child_worktree = super_repo.join("child");
    fs::create_dir_all(&child_worktree).unwrap();
    fs::write(
        child_worktree.join(".git"),
        b"gitdir: ../.git/modules/child\n",
    )
    .unwrap();

    // A `.git` directory that claims HEAD but lacks objects/refs: Git ignores
    // it and keeps walking; the strict lock resolver refuses it.
    let partial = repo.join("partial");
    fs::create_dir_all(partial.join(".git")).unwrap();
    fs::write(partial.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();

    let malformed = tmp.join("malformed");
    fs::create_dir_all(&malformed).unwrap();
    fs::write(malformed.join(".git"), b"not a gitdir pointer\n").unwrap();

    let broken = tmp.join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(".git"), b"gitdir: ../missing.git\n").unwrap();

    let plain = tmp.join("plain");
    fs::create_dir_all(&plain).unwrap();

    let file_anchor = repo.join("tracked.txt");
    fs::write(&file_anchor, b"x").unwrap();

    let mut cases: Vec<Case> = vec![
        ("repo".into(), repo.clone(), vec![]),
        ("subdir".into(), sub, vec![]),
        ("worktree".into(), worktree.clone(), vec![]),
        ("separate".into(), separate, vec![]),
        ("bare-root".into(), bare.clone(), vec![]),
        ("bare-subdir".into(), bare_sub, vec![]),
        ("absorbed-submodule".into(), child_worktree, vec![]),
        (
            "absorbed-submodule-gitdir".into(),
            super_repo.join(".git/modules/child"),
            vec![],
        ),
        (
            "worktree-gitdir".into(),
            repo.join(".git/worktrees/wt"),
            vec![],
        ),
        (
            "git-work-tree-from-git-interior".into(),
            repo.join(".git"),
            vec![("GIT_WORK_TREE", worktree.clone())],
        ),
        ("partial-marker".into(), partial, vec![]),
        ("malformed-pointer".into(), malformed, vec![]),
        ("broken-pointer".into(), broken, vec![]),
        ("plain-dir".into(), plain, vec![]),
        ("git-interior".into(), repo.join(".git"), vec![]),
        ("file-input".into(), file_anchor, vec![]),
        (
            "git-dir-absolute".into(),
            tmp.to_path_buf(),
            vec![("GIT_DIR", repo.join(".git"))],
        ),
        (
            "git-dir-relative".into(),
            tmp.to_path_buf(),
            vec![("GIT_DIR", PathBuf::from("repo/.git"))],
        ),
        (
            "git-common-dir".into(),
            repo.clone(),
            vec![("GIT_COMMON_DIR", PathBuf::from("../bare.git"))],
        ),
        (
            "git-object-directory".into(),
            repo.clone(),
            vec![("GIT_OBJECT_DIRECTORY", repo.join(".git/objects"))],
        ),
        (
            "ceiling-stops".into(),
            repo.join("sub"),
            vec![("GIT_CEILING_DIRECTORIES", repo.clone())],
        ),
        (
            "ceiling-examined".into(),
            repo.join("sub"),
            vec![("GIT_CEILING_DIRECTORIES", repo.join("sub"))],
        ),
        (
            "git-work-tree".into(),
            tmp.to_path_buf(),
            vec![
                ("GIT_DIR", bare.clone()),
                ("GIT_WORK_TREE", worktree.clone()),
            ],
        ),
        (
            "missing-git-dir".into(),
            repo.clone(),
            vec![("GIT_DIR", tmp.join("missing.git"))],
        ),
        (
            "missing-common-dir".into(),
            repo.clone(),
            vec![("GIT_COMMON_DIR", tmp.join("missing.git"))],
        ),
        (
            "empty-common-dir".into(),
            repo.clone(),
            vec![("GIT_COMMON_DIR", {
                let empty = tmp.join("empty-common.git");
                fs::create_dir_all(&empty).unwrap();
                empty
            })],
        ),
        (
            "invalid-object-directory".into(),
            repo.clone(),
            vec![("GIT_OBJECT_DIRECTORY", tmp.join("missing-objects"))],
        ),
    ];
    cases.sort_by(|left, right| left.0.cmp(&right.0));
    cases
}

fn apply_clean_env(command: &mut Command) {
    command.env_remove("GIT_DIR");
    command.env_remove("GIT_COMMON_DIR");
    command.env_remove("GIT_OBJECT_DIRECTORY");
    command.env_remove("GIT_WORK_TREE");
    command.env_remove("GIT_CEILING_DIRECTORIES");
    command.env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM");
}

fn rust_observe(op: &str, path: &Path, env: &[(&'static str, PathBuf)]) -> Observed {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "topology_child", "--nocapture"])
        .env(CHILD_OP, op)
        .env(CHILD_PATH, path);
    apply_clean_env(&mut command);
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("run Rust topology child");
    assert!(
        output.status.success(),
        "Rust child failed for {op} {path:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let observed = stdout
        .lines()
        .map(str::trim)
        .find(|line| {
            line.starts_with("common:")
                || line.starts_with("top:")
                || *line == "not-a-git"
                || *line == "unavailable"
        })
        .unwrap_or_else(|| panic!("Rust child printed no observation:\n{stdout}"));
    Observed::decode(observed)
}

#[test]
fn topology_child() {
    let Some(op) = std::env::var_os(CHILD_OP) else {
        return;
    };
    let path = PathBuf::from(std::env::var_os(CHILD_PATH).expect("child path"));
    let observed = match op.to_string_lossy().as_ref() {
        "common" => match sk_kernel::git_topology::git_common_dir(&path) {
            Ok(path) => Observed::Common(path),
            Err(sk_kernel::KernelError::NotAGitRepository { .. }) => Observed::NotAGit,
            Err(_) => Observed::Unavailable,
        },
        "toplevel" => match sk_kernel::git_topology::git_toplevel(&path) {
            Ok(path) => Observed::Toplevel(path),
            Err(sk_kernel::KernelError::NotAGitRepository { .. }) => Observed::NotAGit,
            Err(_) => Observed::Unavailable,
        },
        other => panic!("unknown operation {other}"),
    };
    println!("{}", observed.encoded());
}

fn git_observe(op: &str, path: &Path, env: &[(&'static str, PathBuf)]) -> Observed {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let probe_dir = if resolved.is_file() {
        resolved.parent().unwrap_or(&resolved).to_path_buf()
    } else {
        resolved.clone()
    };
    let flag = if op == "common" {
        "--git-common-dir"
    } else {
        "--show-toplevel"
    };
    let mut command = Command::new("git");
    command.args(["rev-parse", flag]).current_dir(&probe_dir);
    apply_clean_env(&mut command);
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("run git probe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    if !output.status.success() || stdout.trim().is_empty() {
        if stderr.contains("not a git repository") {
            return Observed::NotAGit;
        }
        return Observed::Unavailable;
    }
    let raw = PathBuf::from(stdout.trim());
    let candidate = if raw.is_absolute() {
        raw
    } else {
        probe_dir.join(raw)
    };
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    if op == "toplevel" {
        return Observed::Toplevel(candidate);
    }
    if resolved.starts_with(&candidate) {
        return Observed::NotAGit;
    }
    Observed::Common(candidate)
}

fn python_observe(op: &str, path: &Path, env: &[(&'static str, PathBuf)]) -> Option<Observed> {
    let python = python()?;
    let function = if op == "common" {
        "git_common_dir"
    } else {
        "git_toplevel"
    };
    let script = format!(
        r#"
import sys
from pathlib import Path
from kernel.git_topology import {function} as probe, NotAGitRepositoryError
try:
    print("ok:" + str(probe(Path(sys.argv[1]))))
except NotAGitRepositoryError:
    print("not-a-git")
except Exception:
    print("unavailable")
"#
    );
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(script)
        .arg(path)
        .env("PYTHONPATH", repo_root().join("src"))
        .current_dir(repo_root());
    apply_clean_env(&mut command);
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("run Python topology probe");
    assert!(
        output.status.success(),
        "python probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Some(if let Some(value) = line.strip_prefix("ok:") {
        if op == "common" {
            Observed::Common(PathBuf::from(value))
        } else {
            Observed::Toplevel(PathBuf::from(value))
        }
    } else if line == "not-a-git" {
        Observed::NotAGit
    } else {
        Observed::Unavailable
    })
}

#[test]
fn native_git_topology_matches_git_and_python_across_layouts_and_environment() {
    let temp = tempfile::tempdir().unwrap();
    let cases = fixture(temp.path());
    let have_python = python().is_some();
    for (name, path, env) in cases {
        for op in ["common", "toplevel"] {
            let rust = rust_observe(op, &path, &env);
            let git = git_observe(op, &path, &env);
            assert_eq!(
                rust,
                git,
                "Rust disagreed with git for {name} {op} path={} env={env:?}",
                path.display()
            );
            if have_python {
                let python = python_observe(op, &path, &env).expect("python");
                assert_eq!(
                    rust,
                    python,
                    "Rust disagreed with Python for {name} {op} path={} env={env:?}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn configured_worktree_matches_python_for_an_absorbed_submodule() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let _cases = fixture(temp.path());
    let common = temp.path().join("super/.git/modules/child");
    let absorbed = temp.path().join("super");
    let rust = sk_kernel::git_topology::git_configured_worktree(&common)
        .unwrap()
        .expect("configured worktree");
    let python = Command::new(python().unwrap())
        .arg("-c")
        .arg(
            "import sys; from pathlib import Path; from kernel.git_topology import git_configured_worktree as f; print(f(Path(sys.argv[1])))",
        )
        .arg(&common)
        .env("PYTHONPATH", repo_root().join("src"))
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert!(python.status.success());
    let value = String::from_utf8_lossy(&python.stdout).trim().to_owned();
    assert_eq!(rust, PathBuf::from(value));
    assert_eq!(rust, absorbed.join("child").canonicalize().unwrap());
}

#[test]
fn strict_lock_discovery_refuses_a_partial_marker_while_parity_probes_ignore_it() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let partial = repo.join("partial");
    fs::create_dir_all(partial.join(".git")).unwrap();
    fs::write(partial.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();

    assert_eq!(
        sk_kernel::git_topology::git_common_dir(&partial).unwrap(),
        repo.join(".git").canonicalize().unwrap()
    );
    assert!(matches!(
        sk_kernel::event_log_append::event_log_lock_path(&partial.join("events.jsonl")),
        Err(sk_kernel::KernelError::LockUnavailable { .. })
    ));
}

#[test]
fn the_fixture_matrix_is_non_vacuous() {
    // A silently empty fixture set would make the parity comparison above
    // pass without observing anything.
    let temp = tempfile::tempdir().unwrap();
    let cases = fixture(temp.path());
    assert!(cases.len() >= 20);
    assert!(cases.iter().any(|(name, _, _)| name == "worktree"));
    assert!(cases.iter().any(|(_, _, env)| !env.is_empty()));
}
