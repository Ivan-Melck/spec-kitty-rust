//! The full symbolic `HEAD` authority and the short `--short` label.
//!
//! [`GitRepository::symbolic_head_ref`] is the one validated full-target read
//! (`refs/...`), promoted in place from the private unborn-HEAD proof. It keeps
//! the same argv and process seam as [`GitRepository::current_branch`], which
//! stays the `git symbolic-ref --short HEAD` **label**: under tag ambiguity Git
//! prints `heads/<name>`, so the short label is never a fully-qualified ref.
//!
//! These tests cover detached, unborn, a non-breaking-space branch name, tag
//! disambiguation, a full non-`refs/heads/` namespace target, corrupt `HEAD`
//! content, and a Git-legal 1099-byte branch name. No invented ref-name length
//! bound is asserted anywhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{GitError, GitRepository, RefName};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new(branch: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", branch]);
        for (key, value) in [
            ("user.name", "Test Author"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            git_ok(&path, &["config", key, value]);
        }
        fs::write(path.join("a.txt"), b"one\n").unwrap();
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "initial"]);
        Self { _temp: temp, path }
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.path).unwrap()
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_ok(&self.path, args)
    }

    fn head_file(&self) -> PathBuf {
        self.path.join(".git/HEAD")
    }
}

fn git_ok(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("HOME", cwd.join("home"))
        .env("XDG_CONFIG_HOME", cwd.join("config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn ref_name(name: &str) -> RefName {
    RefName::parse(name).unwrap()
}

#[test]
fn a_born_checkout_reports_its_full_symbolic_target() {
    let fixture = Fixture::new("main");
    let repository = fixture.open();
    assert_eq!(
        repository.symbolic_head_ref().unwrap(),
        Some(ref_name("refs/heads/main"))
    );
    assert_eq!(
        repository.current_branch().unwrap().as_deref(),
        Some("main")
    );
}

#[test]
fn an_unborn_checkout_reports_the_symbolic_target_and_no_commit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    fs::create_dir_all(&path).unwrap();
    git_ok(&path, &["init", "-q", "-b", "main"]);
    let repository = GitRepository::open(&path).unwrap();
    assert_eq!(
        repository.symbolic_head_ref().unwrap(),
        Some(ref_name("refs/heads/main"))
    );
    assert_eq!(repository.head_commit().unwrap(), None);
}

#[test]
fn a_detached_head_is_none_not_a_failure() {
    let fixture = Fixture::new("main");
    fixture.git(&["checkout", "-q", "--detach"]);
    let repository = fixture.open();
    assert_eq!(repository.symbolic_head_ref().unwrap(), None);
    assert_eq!(repository.current_branch().unwrap(), None);
}

#[test]
fn a_non_breaking_space_branch_name_survives_exactly() {
    let branch = "lane\u{00a0}unicode";
    let fixture = Fixture::new(branch);
    let repository = fixture.open();
    assert_eq!(
        repository.symbolic_head_ref().unwrap(),
        Some(ref_name(&format!("refs/heads/{branch}")))
    );
    assert_eq!(
        repository.current_branch().unwrap().as_deref(),
        Some(branch),
        "the short label keeps the exact code points"
    );
}

#[test]
fn tag_disambiguation_is_visible_only_in_the_short_label() {
    let fixture = Fixture::new("main");
    fixture.git(&["branch", "topic"]);
    fixture.git(&["tag", "topic"]);
    fixture.git(&["checkout", "-q", "topic"]);
    let repository = fixture.open();

    assert_eq!(
        String::from_utf8_lossy(&fixture.git(&["symbolic-ref", "--quiet", "--short", "HEAD"]))
            .trim(),
        "heads/topic",
        "git's documented --short ambiguity: the label is not a fully-qualified ref"
    );
    assert_eq!(
        repository.current_branch().unwrap().as_deref(),
        Some("heads/topic"),
        "current_branch stays the --short label, unchanged"
    );
    assert_eq!(
        repository.symbolic_head_ref().unwrap(),
        Some(ref_name("refs/heads/topic")),
        "the full read names the real branch, never the tag"
    );
}

#[test]
fn a_full_non_head_namespace_target_is_returned_verbatim() {
    let fixture = Fixture::new("main");
    let commit = fixture.git(&["rev-parse", "HEAD"]);
    let commit = std::str::from_utf8(&commit).unwrap().trim();
    fixture.git(&["update-ref", "refs/spec-kitty/lane-tip/x", commit]);
    fixture.git(&["symbolic-ref", "HEAD", "refs/spec-kitty/lane-tip/x"]);
    let repository = fixture.open();
    assert_eq!(
        repository.symbolic_head_ref().unwrap(),
        Some(ref_name("refs/spec-kitty/lane-tip/x")),
        "the full read validates and returns non-branch namespaces unchanged"
    );
    assert_eq!(
        repository.head_commit().unwrap().as_deref(),
        Some(commit),
        "the target still resolves to its commit"
    );
}

#[test]
fn a_git_legal_1099_byte_branch_name_is_accepted() {
    let components: Vec<String> = std::iter::repeat_n("x".repeat(100), 10)
        .chain(["y".repeat(89)])
        .collect();
    let branch = components.join("/");
    assert_eq!(
        branch.len(),
        1099,
        "the fixture is the measured Git-legal name"
    );
    let fixture = Fixture::new("main");
    fixture.git(&["branch", &branch]);
    fixture.git(&["checkout", "-q", &branch]);
    let repository = fixture.open();
    assert_eq!(
        repository.symbolic_head_ref().unwrap(),
        Some(ref_name(&format!("refs/heads/{branch}"))),
        "no invented length bound is applied to a Git-legal ref"
    );
}

#[test]
fn corrupt_head_content_is_unreadable_not_absent() {
    let fixture = Fixture::new("main");
    let repository = fixture.open();

    for content in [
        b"ref: refs/heads/\n".to_vec(),
        b"ref: refs/heads/bad..name\n".to_vec(),
        b"garbage\n".to_vec(),
    ] {
        fs::write(fixture.head_file(), &content).unwrap();
        let error = repository
            .symbolic_head_ref()
            .expect_err("a corrupt HEAD is never a detached answer");
        assert!(
            matches!(error, GitError::UnreadableHead { .. }),
            "{content:?}: {error:?}"
        );
        assert!(
            matches!(
                repository.current_branch(),
                Err(GitError::CommandFailed { .. })
            ),
            "the short label keeps its existing failure for a corrupt HEAD"
        );
    }
}

#[test]
fn completed_nonzero_observation_is_distinct_from_successful_invalid_output() {
    let fixture = Fixture::new("main");
    let repository = fixture.open();
    fs::write(fixture.head_file(), b"ref: refs/heads/\n").unwrap();
    let observed = repository.observe_symbolic_head().unwrap();
    assert_eq!(observed.status_code(), Some(128));
    assert_eq!(observed.target_text(), None);
    assert!(matches!(
        repository.symbolic_head_ref(),
        Err(GitError::UnreadableHead { .. })
    ));
    fs::write(fixture.head_file(), b"ref: refs/x\n").unwrap();
    assert_eq!(
        repository.observe_symbolic_head().unwrap().target_text(),
        Some("refs/x"),
        "observation retains a successful target even when the stricter publication type cannot represent it"
    );
    assert!(
        matches!(
            repository.symbolic_head_ref(),
            Err(GitError::UnreadableHead { .. })
        ),
        "strict full-ref API validation is unchanged"
    );
    fs::write(fixture.head_file(), b"ref: refs/heads/raw\xff\n").unwrap();
    assert!(
        matches!(
            repository.observe_symbolic_head(),
            Err(GitError::UnreadableHead { .. })
        ),
        "successful malformed stdout is an error, not a completed-nonzero answer"
    );
}

#[test]
fn exact_ref_existence_does_not_require_a_commit_or_a_born_head() {
    let fixture = Fixture::new("main");
    let repository = fixture.open();
    assert!(repository.ref_exists(&ref_name("refs/heads/main")).unwrap());
    assert!(
        !repository
            .ref_exists(&ref_name("refs/heads/missing"))
            .unwrap()
    );
    let blob = fixture.git(&["rev-parse", "HEAD:a.txt"]);
    let blob = std::str::from_utf8(&blob).unwrap().trim();
    fixture.git(&["update-ref", "refs/spec-kitty/blob", blob]);
    assert!(
        repository
            .ref_exists(&ref_name("refs/spec-kitty/blob"))
            .unwrap(),
        "rev-parse existence is not commit peeling"
    );
}

#[test]
fn a_non_utf8_head_target_is_unreadable() {
    let fixture = Fixture::new("main");
    fs::write(fixture.head_file(), b"ref: refs/heads/garbage\xff\n").unwrap();
    let repository = fixture.open();
    let error = repository.symbolic_head_ref().unwrap_err();
    assert!(
        matches!(
            &error,
            GitError::UnreadableHead { detail }
                if detail == "HEAD names a ref that is not valid UTF-8"
        ),
        "{error:?}"
    );
}
