//! Contracts for the generic, read-only `symbolic-ref --short` read.
//!
//! Real temporary repositories establish the Git-observable answers: a normal
//! `origin/HEAD` target, a target that names a branch outside
//! `refs/remotes/`, a dangling target, a plain ref and a missing ref. A fake
//! `git` at the configured executable boundary establishes only what a real
//! repository cannot produce deterministically: exact argv, an arbitrary
//! non-1 exit, a signalled child, capture overflow, a missing executable and a
//! hung child. Those seam cases are Unix-only for the same reason as the
//! existing ignore-probe tests.
//!
//! The generic read is deliberately stricter than the Op commit policy: only
//! exit 1 means "no symbolic target". Any other completed status, an
//! unusable target, and execution/capture failures are typed errors. The Op
//! policy is responsible for the source's tolerance of other completed
//! non-zero answers.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_git::{GitError, GitOptions, GitRepository, RefName};

const GIT_SELECTORS: &[&str] = &[
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

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("create repository directory");
        git_ok(&root, &["init", "-q", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Fixture"]);
        git_ok(&root, &["config", "user.email", "fixture@example.invalid"]);
        git_ok(&root, &["config", "commit.gpgsign", "false"]);
        fs::write(root.join("README.md"), b"# fixture\n").expect("write README");
        git_ok(&root, &["add", "README.md"]);
        git_ok(&root, &["commit", "-q", "-m", "fixture"]);
        Self { _temp: temp, root }
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).expect("open fixture repository")
    }

    fn head(&self) -> String {
        String::from_utf8(git_ok(&self.root, &["rev-parse", "HEAD"]))
            .expect("rev-parse HEAD is UTF-8")
            .trim()
            .to_string()
    }
}

fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    for variable in GIT_SELECTORS {
        command.env_remove(variable);
    }
    command.env_remove("GIT_INDEX_FILE");
    command
}

fn git_raw(cwd: &Path, args: &[&str]) -> Output {
    git_command(cwd).args(args).output().expect("run git")
}

fn git_ok(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = git_raw(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn refname(value: &str) -> RefName {
    RefName::parse(value).expect("valid fixture ref name")
}

#[test]
fn real_origin_head_reports_the_short_symbolic_target() {
    let fixture = Fixture::new();
    let head = fixture.head();
    git_ok(
        &fixture.root,
        &["update-ref", "refs/remotes/origin/main", &head],
    );
    git_ok(
        &fixture.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );

    let repository = fixture.open();
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD"))
            .expect("read origin/HEAD"),
        Some("origin/main".to_string())
    );
}

#[test]
fn a_target_outside_the_remote_namespace_is_shortened_by_git() {
    let fixture = Fixture::new();
    git_ok(
        &fixture.root,
        &["symbolic-ref", "refs/remotes/origin/X", "refs/heads/main"],
    );
    git_ok(
        &fixture.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/Y",
            "refs/remotes/origin/release/1.2",
        ],
    );

    let repository = fixture.open();
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/X"))
            .expect("read refs/remotes/origin/X"),
        Some("main".to_string())
    );
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/Y"))
            .expect("read refs/remotes/origin/Y"),
        Some("origin/release/1.2".to_string())
    );
}

#[test]
fn a_dangling_symbolic_target_is_reported_not_hidden() {
    let fixture = Fixture::new();
    git_ok(
        &fixture.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/gone",
        ],
    );

    let repository = fixture.open();
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD"))
            .expect("read dangling origin/HEAD"),
        Some("origin/gone".to_string())
    );
}

#[test]
fn a_plain_or_missing_ref_has_no_symbolic_target() {
    let fixture = Fixture::new();
    let repository = fixture.open();

    // `refs/heads/main` exists but is a direct ref: git answers exit 1.
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/heads/main"))
            .expect("read plain branch"),
        None
    );
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD"))
            .expect("read absent origin/HEAD"),
        None
    );
}

fn snapshot_worktree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(base: &Path, directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        let entries = fs::read_dir(directory).expect("read directory");
        for entry in entries {
            let entry = entry.expect("read directory entry");
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            let metadata = entry.metadata().expect("stat directory entry");
            if metadata.is_dir() {
                walk(base, &path, files);
            } else if metadata.is_file() {
                files.push((
                    path.strip_prefix(base)
                        .expect("path is under root")
                        .to_owned(),
                    fs::read(&path).expect("read worktree file"),
                ));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
}

#[test]
fn the_read_does_not_change_head_index_staging_or_worktree() {
    let fixture = Fixture::new();
    git_ok(
        &fixture.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/heads/main",
        ],
    );
    let head_before = git_ok(&fixture.root, &["rev-parse", "HEAD"]);
    let index_before = fs::read(fixture.root.join(".git/index")).expect("read index");
    let staged_before = git_ok(&fixture.root, &["ls-files", "-s", "-z"]);
    let worktree_before = snapshot_worktree(&fixture.root);

    let repository = fixture.open();
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD"))
            .expect("read origin/HEAD"),
        Some("main".to_string())
    );

    assert_eq!(
        git_ok(&fixture.root, &["rev-parse", "HEAD"]),
        head_before,
        "HEAD must not change"
    );
    assert_eq!(
        fs::read(fixture.root.join(".git/index")).expect("read index"),
        index_before,
        "the index must not change"
    );
    assert_eq!(
        git_ok(&fixture.root, &["ls-files", "-s", "-z"]),
        staged_before,
        "staging must not change"
    );
    assert_eq!(
        snapshot_worktree(&fixture.root),
        worktree_before,
        "no worktree byte may change"
    );
}

#[cfg(unix)]
fn write_fake_git(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("fake-git");
    fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write fake git");
    let mut permissions = fs::metadata(&path).expect("stat fake git").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("make fake git executable");
    path
}

#[cfg(unix)]
const FAKE_DISCOVERY: &str = r#"
case "$1" in
  rev-parse)
    case "$2" in
      --is-inside-work-tree) printf 'true\n'; exit 0 ;;
      --show-toplevel) pwd; exit 0 ;;
      --is-bare-repository) printf 'false\n'; exit 0 ;;
    esac
    ;;
esac
"#;

#[cfg(unix)]
fn fake_repository(
    temp: &tempfile::TempDir,
    body: &str,
    timeout: std::time::Duration,
) -> (PathBuf, GitRepository) {
    let program = write_fake_git(temp.path(), &format!("{FAKE_DISCOVERY}\n{body}"));
    let options = GitOptions {
        program: program.clone(),
        timeout,
    };
    let repository =
        GitRepository::open_with_options(temp.path(), &options).expect("open with fake git");
    (program, repository)
}

#[cfg(unix)]
#[test]
fn exact_argv_and_exit_codes_are_mapped() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let args_file = temp.path().join("args");
    let body = format!(
        "if [ \"$1\" = symbolic-ref ]; then printf '%s\\n' \"$@\" > '{}'; printf 'origin/main\\n'; exit 0; fi\nexit 0\n",
        args_file.display()
    );
    let (_program, repository) = fake_repository(&temp, &body, std::time::Duration::from_secs(2));

    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD"))
            .expect("read"),
        Some("origin/main".to_string())
    );
    let recorded: Vec<String> = fs::read_to_string(&args_file)
        .expect("read recorded argv")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        recorded,
        vec![
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD"
        ]
    );

    let temp_one = tempfile::tempdir().expect("create tempdir");
    let body = "if [ \"$1\" = symbolic-ref ]; then exit 1; fi\nexit 0\n";
    let (_program, repository) =
        fake_repository(&temp_one, body, std::time::Duration::from_secs(2));
    assert_eq!(
        repository
            .symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD"))
            .expect("read"),
        None
    );
}

#[cfg(unix)]
#[test]
fn another_exit_status_is_a_typed_failure_not_absence() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let body =
        "if [ \"$1\" = symbolic-ref ]; then printf 'probe exploded\\n' >&2; exit 3; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, std::time::Duration::from_secs(2));

    match repository.symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD")) {
        Err(GitError::CommandFailed {
            args,
            status,
            stderr,
            ..
        }) => {
            assert_eq!(
                args,
                vec![
                    "symbolic-ref",
                    "--quiet",
                    "--short",
                    "refs/remotes/origin/HEAD"
                ]
            );
            assert_eq!(status, Some(3));
            assert!(stderr.contains("probe exploded"), "stderr was {stderr:?}");
        }
        Ok(value) => panic!("exit 3 must not become Ok({value:?})"),
        Err(other) => panic!("expected CommandFailed, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_signalled_child_is_a_typed_failure_not_absence() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let body = "if [ \"$1\" = symbolic-ref ]; then kill -TERM $$; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, std::time::Duration::from_secs(2));
    assert!(matches!(
        repository.symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD")),
        Err(GitError::CommandFailed { status: None, .. })
    ));
}

#[cfg(unix)]
#[test]
fn capture_overflow_is_an_error_even_when_exit_one_is_allowed() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let body =
        "if [ \"$1\" = symbolic-ref ]; then head -c 16777217 /dev/zero; exit 1; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, std::time::Duration::from_secs(10));
    match repository.symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD")) {
        Err(GitError::OutputTooLarge {
            stream: "stdout",
            limit,
        }) => assert_eq!(limit, 16 * 1024 * 1024),
        other => panic!("allowed exit 1 must not hide capture failure: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn unavailable_git_is_a_typed_failure_not_absence() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let (program, repository) = fake_repository(
        &temp,
        "if [ \"$1\" = symbolic-ref ]; then exit 0; fi\nexit 0\n",
        std::time::Duration::from_secs(2),
    );
    fs::remove_file(&program).expect("remove fake git");

    assert!(matches!(
        repository.symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD")),
        Err(GitError::GitUnavailable { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_hung_child_times_out_and_is_not_absence() {
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().expect("create tempdir");
    let body = "if [ \"$1\" = symbolic-ref ]; then exec sleep 30; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, Duration::from_millis(120));

    let started = Instant::now();
    match repository.symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD")) {
        Err(GitError::Timeout { args, timeout, .. }) => {
            assert_eq!(args.first().map(String::as_str), Some("symbolic-ref"));
            assert_eq!(timeout, Duration::from_millis(120));
        }
        other => panic!("a timed-out probe must not become absent: {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the probe was not bounded: {:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[test]
fn an_empty_successful_target_is_invalid_output() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let body = "if [ \"$1\" = symbolic-ref ]; then exit 0; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, std::time::Duration::from_secs(2));

    assert!(matches!(
        repository.symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD")),
        Err(GitError::InvalidOutput { detail }) if detail.contains("empty symbolic target")
    ));
}

#[cfg(unix)]
#[test]
fn a_non_utf8_target_is_refused() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let body = "if [ \"$1\" = symbolic-ref ]; then printf '\\377\\n'; exit 0; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, std::time::Duration::from_secs(2));

    assert!(matches!(
        repository.symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD")),
        Err(GitError::NonUtf8 {
            field: "symbolic ref target"
        })
    ));
}
