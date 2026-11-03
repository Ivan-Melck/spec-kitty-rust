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
        for directory in ["", "home", "config", "tmp", "sub/deep"] {
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
    commit: String,
    stdout: String,
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
        stdout,
    }
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

    let request = SafeCommitRequest::new(
        BranchName::parse("work").expect("branch"),
        "env index",
        [GitPath::from_git_bytes(b"a.txt").expect("path")],
    );
    let outcome = repository.safe_commit(&request).expect("safe commit");
    println!("commit={}", outcome.commit());
}
