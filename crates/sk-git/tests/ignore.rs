//! Contracts for the generic ignore-rule probe.
//!
//! The rule semantics are exercised against real temporary Git repositories.
//! A fake `git` is used only for seam-level contracts that a real repository
//! cannot produce deterministically (an arbitrary non-1 exit, a missing
//! executable, a hung child, and exact argv capture). The captured
//! operator-index contract runs in a child process for the same reason
//! `tests/index_authority.rs` does: the selector is captured from the inherited
//! environment, and the test process environment must never be mutated.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use sk_git::{GitError, GitOptions, GitPath, GitRepository};

const RULES: &str = "*.log\n!important.log\nsub/*.tmp\n!sub/keep.tmp\nnested/*\n!nested/keep.txt\n";
const DEEP_RULES: &str = "*.cache\n!keep.cache\n";

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

/// One real repository containing ignored, negated, nested, missing, tracked
/// and awkwardly spelled paths.
struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let root = temp.path().join("repo");
        for directory in ["", "sub", "nested", "deep"] {
            fs::create_dir_all(root.join(directory)).expect("create fixture directory");
        }

        git_ok(&root, &["init", "-q", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Fixture"]);
        git_ok(&root, &["config", "user.email", "fixture@example.invalid"]);
        git_ok(&root, &["config", "commit.gpgsign", "false"]);

        fs::write(root.join(".gitignore"), RULES).expect("write root ignore rules");
        fs::write(root.join("deep/.gitignore"), DEEP_RULES).expect("write nested ignore rules");
        fs::write(root.join("a.txt"), b"tracked, unignored\n").expect("write a.txt");
        fs::write(root.join("tracked.log"), b"tracked but ignored\n").expect("write tracked.log");
        fs::write(root.join("keep.log"), b"ignored\n").expect("write keep.log");
        fs::write(root.join("important.log"), b"negated\n").expect("write important.log");
        fs::write(root.join("sub/a.tmp"), b"ignored\n").expect("write sub/a.tmp");
        fs::write(root.join("sub/keep.tmp"), b"negated\n").expect("write sub/keep.tmp");
        fs::write(root.join("nested/n.txt"), b"ignored\n").expect("write nested/n.txt");
        fs::write(root.join("nested/keep.txt"), b"negated\n").expect("write nested/keep.txt");
        fs::write(root.join("deep/a.cache"), b"ignored\n").expect("write deep/a.cache");
        fs::write(root.join("deep/keep.cache"), b"negated\n").expect("write deep/keep.cache");
        for name in [
            "-leading.log",
            "colon:name.log",
            ":literal.log",
            "with space.log",
            "café.log",
            "hash#name.log",
            "quote'name.log",
            "-leading.txt",
            "colon:plain.txt",
        ] {
            fs::write(root.join(name), b"edge\n").expect("write edge path");
        }

        git_ok(&root, &["add", ".gitignore", "deep/.gitignore", "a.txt"]);
        git_ok(&root, &["add", "-f", "tracked.log"]);
        git_ok(&root, &["commit", "-q", "-m", "fixture"]);

        Self { temp, root }
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).expect("open fixture repository")
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

fn git_with_index_raw(cwd: &Path, args: &[&str], index: &Path) -> Output {
    git_command(cwd)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()
        .expect("run git with an explicit index")
}

fn git_with_index_ok(cwd: &Path, args: &[&str], index: &Path) -> Vec<u8> {
    let output = git_with_index_raw(cwd, args, index);
    assert!(
        output.status.success(),
        "git {args:?} with index {} failed in {}: {}",
        index.display(),
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn path(raw: &str) -> GitPath {
    GitPath::from_git_bytes(raw.as_bytes()).expect("git path")
}

fn assert_ignored(repository: &GitRepository, raw: &str) {
    assert!(
        repository.is_path_ignored(&path(raw)).expect("probe"),
        "{raw:?} must match an ignore rule"
    );
}

fn assert_not_ignored(repository: &GitRepository, raw: &str) {
    assert!(
        !repository.is_path_ignored(&path(raw)).expect("probe"),
        "{raw:?} must not match an ignore rule"
    );
}

/// Every regular file below `root`, excluding repository metadata, with its
/// exact bytes.
fn snapshot_worktree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("read worktree directory") {
            let entry = entry.expect("read worktree entry");
            let path = entry.path();
            if path.file_name() == Some(std::ffi::OsStr::new(".git")) {
                continue;
            }
            let file_type = entry.file_type().expect("worktree entry type");
            if file_type.is_dir() {
                walk(base, &path, files);
            } else {
                let relative = path
                    .strip_prefix(base)
                    .expect("path below root")
                    .to_path_buf();
                files.insert(relative, fs::read(&path).expect("read worktree file"));
            }
        }
    }

    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

#[test]
fn real_repository_ignored_and_unignored_paths() {
    let fixture = Fixture::new();
    let repository = fixture.open();

    for ignored in ["keep.log", "sub/a.tmp", "nested/n.txt"] {
        assert_ignored(&repository, ignored);
    }
    for unignored in [
        "important.log",
        "sub/keep.tmp",
        "nested/keep.txt",
        "a.txt",
        "plain.txt",
    ] {
        assert_not_ignored(&repository, unignored);
    }
}

#[test]
fn tracked_but_ignored_is_true_where_ordinary_check_ignore_is_false() {
    let fixture = Fixture::new();
    let repository = fixture.open();

    assert!(
        git_raw(
            &fixture.root,
            &["ls-files", "--error-unmatch", "tracked.log"]
        )
        .status
        .success(),
        "the fixture must track tracked.log in the real index"
    );
    assert_ignored(&repository, "tracked.log");

    let ordinary = git_raw(&fixture.root, &["check-ignore", "-q", "--", "tracked.log"]);
    assert_eq!(
        ordinary.status.code(),
        Some(1),
        "plain check-ignore must answer \"not ignored\" for the tracked path"
    );
    let no_index = git_raw(
        &fixture.root,
        &["check-ignore", "-q", "--no-index", "--", "tracked.log"],
    );
    assert_eq!(
        no_index.status.code(),
        Some(0),
        "the --no-index control must answer \"ignored\""
    );
}

#[test]
fn missing_but_ignored_paths_are_answered_without_touching_the_filesystem() {
    let fixture = Fixture::new();
    let repository = fixture.open();

    for ignored in ["missing.log", "sub/missing.tmp", "deep/missing.cache"] {
        assert!(
            !fixture.root.join(ignored).exists(),
            "the fixture must not create {ignored}"
        );
        assert_ignored(&repository, ignored);
    }
    assert_not_ignored(&repository, "missing/plain.txt");
}

#[test]
fn nested_ignore_files_and_negations_are_honoured() {
    let fixture = Fixture::new();
    let repository = fixture.open();

    // Root directory rule plus a negation that re-includes its child.
    assert_ignored(&repository, "nested/n.txt");
    assert_not_ignored(&repository, "nested/keep.txt");
    // A nested .gitignore applies only below its directory.
    assert_ignored(&repository, "deep/a.cache");
    assert_not_ignored(&repository, "deep/keep.cache");
    assert_not_ignored(&repository, "a.cache");
}

#[test]
fn path_spelling_is_passed_as_literal_data() {
    let fixture = Fixture::new();
    let repository = fixture.open();

    // A leading option-looking name proves the `--` separator is present.
    for ignored in [
        "-leading.log",
        "colon:name.log",
        ":literal.log",
        "with space.log",
        "café.log",
        "hash#name.log",
        "quote'name.log",
    ] {
        assert_ignored(&repository, ignored);
    }
    for unignored in ["-leading.txt", "colon:plain.txt"] {
        assert_not_ignored(&repository, unignored);
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_path_bytes_are_preserved() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let fixture = Fixture::new();
    let repository = fixture.open();
    let ignored = b"caf\xe9-\xff.log";
    let unignored = b"caf\xe9-\xff.txt";
    fs::write(fixture.root.join(OsStr::from_bytes(ignored)), b"ignored\n").expect("write raw path");
    fs::write(fixture.root.join(OsStr::from_bytes(unignored)), b"plain\n").expect("write raw path");

    assert!(
        repository
            .is_path_ignored(&GitPath::from_git_bytes(ignored).expect("raw path"))
            .expect("probe"),
        "a non-UTF-8 basename must still match *.log"
    );
    assert!(
        !repository
            .is_path_ignored(&GitPath::from_git_bytes(unignored).expect("raw path"))
            .expect("probe"),
        "an unignored non-UTF-8 basename must stay false"
    );
}

#[test]
fn probe_leaves_head_index_staging_and_worktree_bytes_unchanged() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let index = fixture.root.join(".git/index");

    let head_before = git_ok(&fixture.root, &["rev-parse", "HEAD"]);
    let branch_before = git_ok(&fixture.root, &["symbolic-ref", "HEAD"]);
    let index_before = fs::read(&index).expect("read default index");
    let staged_before = git_ok(&fixture.root, &["ls-files", "-s", "-z"]);
    let worktree_before = snapshot_worktree(&fixture.root);

    for probed in [
        "keep.log",
        "tracked.log",
        "important.log",
        "a.txt",
        "missing.log",
        "-leading.log",
        "colon:name.log",
        "deep/a.cache",
        "deep/keep.cache",
    ] {
        let _ = repository.is_path_ignored(&path(probed)).expect("probe");
    }

    assert_eq!(
        fs::read(&index).expect("read default index"),
        index_before,
        "the default index must be byte-identical"
    );
    assert_eq!(
        git_ok(&fixture.root, &["rev-parse", "HEAD"]),
        head_before,
        "HEAD must not move"
    );
    assert_eq!(
        git_ok(&fixture.root, &["symbolic-ref", "HEAD"]),
        branch_before,
        "the checked-out branch must not change"
    );
    assert_eq!(
        git_ok(&fixture.root, &["ls-files", "-s", "-z"]),
        staged_before,
        "operator staging must not change"
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
fn probe_invokes_exact_argv_and_maps_exit_codes() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let args_file = temp.path().join("args");
    let body = format!(
        "if [ \"$1\" = check-ignore ]; then printf '%s\\n' \"$@\" > '{}'; exit 0; fi\nexit 0\n",
        args_file.display()
    );
    let (_program, repository) = fake_repository(&temp, &body, std::time::Duration::from_secs(2));

    assert!(
        repository
            .is_path_ignored(&path("-leading.log"))
            .expect("probe")
    );
    let recorded: Vec<String> = fs::read_to_string(&args_file)
        .expect("read recorded argv")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        recorded,
        vec!["check-ignore", "-q", "--no-index", "--", "-leading.log"]
    );

    let temp_one = tempfile::tempdir().expect("create tempdir");
    let body = "if [ \"$1\" = check-ignore ]; then exit 1; fi\nexit 0\n";
    let (_program, repository) =
        fake_repository(&temp_one, body, std::time::Duration::from_secs(2));
    assert!(
        !repository
            .is_path_ignored(&path("plain.log"))
            .expect("probe")
    );
}

#[cfg(unix)]
#[test]
fn non_one_exit_status_is_a_typed_failure_not_false() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let body =
        "if [ \"$1\" = check-ignore ]; then printf 'probe exploded\\n' >&2; exit 3; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, std::time::Duration::from_secs(2));

    match repository.is_path_ignored(&path("broken.log")) {
        Err(GitError::CommandFailed {
            args,
            status,
            stderr,
            ..
        }) => {
            assert_eq!(
                args,
                vec!["check-ignore", "-q", "--no-index", "--", "broken.log"]
            );
            assert_eq!(status, Some(3));
            assert!(stderr.contains("probe exploded"), "stderr was {stderr:?}");
        }
        Ok(value) => panic!("exit 3 must not become Ok({value})"),
        Err(other) => panic!("expected CommandFailed, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn unavailable_git_is_a_typed_failure_not_false() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let (program, repository) = fake_repository(
        &temp,
        "if [ \"$1\" = check-ignore ]; then exit 0; fi\nexit 0\n",
        std::time::Duration::from_secs(2),
    );
    fs::remove_file(&program).expect("remove fake git");

    match repository.is_path_ignored(&path("anything.log")) {
        Err(GitError::GitUnavailable { .. }) => {}
        Ok(value) => panic!("a missing git must not become Ok({value})"),
        Err(other) => panic!("expected GitUnavailable, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_hung_probe_times_out_and_is_not_false() {
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().expect("create tempdir");
    let body = "if [ \"$1\" = check-ignore ]; then exec sleep 30; fi\nexit 0\n";
    let (_program, repository) = fake_repository(&temp, body, Duration::from_millis(120));

    let started = Instant::now();
    match repository.is_path_ignored(&path("slow.log")) {
        Err(GitError::Timeout { args, timeout, .. }) => {
            assert_eq!(args.first().map(String::as_str), Some("check-ignore"));
            assert_eq!(timeout, Duration::from_millis(120));
        }
        Ok(value) => panic!("a timed-out probe must not become Ok({value})"),
        Err(other) => panic!("expected Timeout, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the probe was not bounded: {:?}",
        started.elapsed()
    );
}

fn run_ignore_child(root: &Path, index: &Path) -> String {
    let mut command = Command::new(std::env::current_exe().expect("current test executable"));
    command
        .args([
            "--exact",
            "captured_index_probe_child",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .env("TMPDIR", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("SK_IGNORE_ROOT", root)
        .env("GIT_INDEX_FILE", index);
    for variable in GIT_SELECTORS {
        command.env_remove(variable);
    }
    let output = command.output().expect("run captured-index child");
    assert!(
        output.status.success(),
        "captured-index child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("child output is UTF-8")
}

#[test]
fn captured_operator_index_remains_bound_while_no_index_ignores_tracking() {
    let fixture = Fixture::new();
    let custom = fixture.temp.path().join("custom-index");
    git_with_index_ok(&fixture.root, &["read-tree", "HEAD"], &custom);
    fs::write(fixture.root.join("extra.log"), b"custom index only\n")
        .expect("write custom-index-only path");
    git_with_index_ok(&fixture.root, &["add", "-f", "extra.log"], &custom);

    let head_before = git_ok(&fixture.root, &["rev-parse", "HEAD"]);
    let default_index = fixture.root.join(".git/index");
    let default_before = fs::read(&default_index).expect("read default index");
    let custom_before = fs::read(&custom).expect("read custom index");
    let default_staged_before = git_ok(&fixture.root, &["ls-files", "-s", "-z"]);
    let custom_staged_before = git_with_index_ok(&fixture.root, &["ls-files", "-s", "-z"], &custom);
    let worktree_before = snapshot_worktree(&fixture.root);

    let stdout = run_ignore_child(&fixture.root, &custom);
    let field = |name: &str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("child did not print {name}:\n{stdout}"))
            .to_string()
    };
    assert_eq!(PathBuf::from(field("authority")), custom);
    assert_eq!(field("captured"), "true");
    assert_eq!(field("source"), "environment");
    assert_eq!(PathBuf::from(field("environ")), custom);

    let result = |name: &str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(&format!("result={name}\t")))
            .unwrap_or_else(|| panic!("child did not report {name}:\n{stdout}"))
            .to_string()
    };
    for ignored in ["tracked.log", "extra.log", "keep.log"] {
        assert_eq!(result(ignored), "true", "{ignored} must be ignored");
    }
    for unignored in ["important.log", "a.txt"] {
        assert_eq!(
            result(unignored),
            "false",
            "{unignored} must not be ignored"
        );
    }

    // Discriminating control: the custom operator index tracks extra.log, so a
    // plain check-ignore with that same selector says "not ignored", while the
    // default index does not track it. The probe is true under both because
    // --no-index removes tracked membership from the question.
    assert_eq!(
        git_with_index_raw(
            &fixture.root,
            &["check-ignore", "-q", "--", "extra.log"],
            &custom
        )
        .status
        .code(),
        Some(1),
        "plain check-ignore must see the captured custom index's tracking"
    );
    assert_eq!(
        git_raw(&fixture.root, &["check-ignore", "-q", "--", "extra.log"])
            .status
            .code(),
        Some(0),
        "the default index does not track extra.log"
    );
    for ordinary in [
        git_raw(&fixture.root, &["check-ignore", "-q", "--", "tracked.log"]),
        git_with_index_raw(
            &fixture.root,
            &["check-ignore", "-q", "--", "tracked.log"],
            &custom,
        ),
    ] {
        assert_eq!(
            ordinary.status.code(),
            Some(1),
            "both indexes track tracked.log"
        );
    }
    for no_index in [
        git_raw(
            &fixture.root,
            &["check-ignore", "-q", "--no-index", "--", "extra.log"],
        ),
        git_with_index_raw(
            &fixture.root,
            &["check-ignore", "-q", "--no-index", "--", "extra.log"],
            &custom,
        ),
    ] {
        assert_eq!(no_index.status.code(), Some(0));
    }

    assert_eq!(
        fs::read(&default_index).expect("read default index"),
        default_before
    );
    assert_eq!(fs::read(&custom).expect("read custom index"), custom_before);
    assert_eq!(git_ok(&fixture.root, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        git_ok(&fixture.root, &["ls-files", "-s", "-z"]),
        default_staged_before
    );
    assert_eq!(
        git_with_index_ok(&fixture.root, &["ls-files", "-s", "-z"], &custom),
        custom_staged_before
    );
    assert_eq!(snapshot_worktree(&fixture.root), worktree_before);
}

#[test]
#[ignore = "subprocess helper; the parent supplies the captured environment"]
fn captured_index_probe_child() {
    let root = PathBuf::from(std::env::var_os("SK_IGNORE_ROOT").expect("SK_IGNORE_ROOT"));
    let repository = GitRepository::open(&root).expect("open repository");
    println!(
        "authority={}",
        repository
            .operator_index_path()
            .expect("operator index path")
            .display()
    );
    println!("captured={}", repository.has_captured_index());
    println!("source={}", repository.operator_index().source().as_str());
    println!(
        "environ={}",
        std::env::var_os("GIT_INDEX_FILE")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unset>".to_string())
    );
    for raw in [
        "tracked.log",
        "extra.log",
        "keep.log",
        "important.log",
        "a.txt",
    ] {
        let path = GitPath::from_git_bytes(raw.as_bytes()).expect("path");
        println!(
            "result={raw}\t{}",
            repository.is_path_ignored(&path).expect("probe")
        );
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root resolves")
}

fn oracle_python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = repository_root();
            let executable = if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            };
            let python = std::env::var_os("SK_KERNEL_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".venv-rust-oracle").join(executable));
            assert!(
                python.is_file(),
                "required Python oracle is missing at {}; provision .venv-rust-oracle with \
                 uv sync --frozen --no-dev --python 3.11 \
                 (UV_PROJECT_ENVIRONMENT=.venv-rust-oracle)",
                python.display()
            );
            let result = Command::new(&python)
                .arg(root.join("scripts/rust/verify_oracle.py"))
                .arg("--root")
                .arg(&root)
                .output()
                .expect("execute required Python oracle verifier");
            assert!(
                result.status.success(),
                "Python oracle is not lock-exact: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            python
        })
        .clone()
}

#[test]
fn matches_the_python_source_authority_on_the_same_fixture() {
    // UTF-8 spellings only: the reference passes a POSIX string, so a raw
    // non-UTF-8 name is covered by the native test above and not here.
    let cases = [
        "keep.log",
        "important.log",
        "sub/a.tmp",
        "sub/keep.tmp",
        "nested/n.txt",
        "nested/keep.txt",
        "deep/a.cache",
        "deep/keep.cache",
        "missing.log",
        "missing/plain.txt",
        "tracked.log",
        "a.txt",
        "-leading.log",
        "colon:name.log",
        "with space.log",
        "café.log",
    ];
    let fixture = Fixture::new();
    let repository = fixture.open();

    let script = r#"
import sys
from pathlib import Path
from specify_cli.invocation.op_commit_policy import is_path_ignored

root = Path(sys.argv[1])
for line in sys.stdin:
    rel = line.rstrip("\n")
    print("true" if is_path_ignored(root, rel) else "false")
"#;
    let root = repository_root();
    let mut child = Command::new(oracle_python())
        .arg("-c")
        .arg(script)
        .arg(&fixture.root)
        .current_dir(&root)
        .env("PYTHONPATH", root.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the pinned Python oracle");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("piped stdin");
        for case in cases {
            writeln!(stdin, "{case}").expect("write oracle input");
        }
    }
    let output = child.wait_with_output().expect("wait for the oracle");
    assert!(
        output.status.success(),
        "oracle script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<bool> = String::from_utf8(output.stdout)
        .expect("oracle output is UTF-8")
        .lines()
        .map(|line| match line {
            "true" => true,
            "false" => false,
            other => panic!("unexpected oracle answer {other:?}"),
        })
        .collect();
    assert_eq!(
        expected.len(),
        cases.len(),
        "the oracle must answer every case exactly once"
    );

    for (case, oracle) in cases.iter().zip(expected) {
        let native = repository
            .is_path_ignored(&path(case))
            .expect("native probe");
        assert_eq!(
            native, oracle,
            "{case:?} disagrees with the Python source authority"
        );
    }
}
