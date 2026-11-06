//! CF45: Source-compatible Git read capabilities.
//!
//! Each test builds a fresh real Git repository (or repositories) under a
//! temporary directory and drives the public `sk-git` API. Nothing outside the
//! fixture is read or written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{
    GitError, GitRepository, MAX_SPARSE_PATTERN_FILE_BYTES, SourceRevision, StatusOptions,
    UntrackedMode, WorktreePorcelainObservation,
};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("repo dir");
        git_ok(&root, &["init", "-q", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "CF45"]);
        git_ok(&root, &["config", "user.email", "cf45@example.invalid"]);
        git_ok(&root, &["config", "commit.gpgsign", "false"]);
        fs::write(root.join("a.txt"), b"one\n").expect("write a.txt");
        git_ok(&root, &["add", "a.txt"]);
        git_ok(&root, &["commit", "-q", "-m", "initial"]);
        Self { _temp: temp, root }
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_ok(&self.root, args)
    }

    fn repository(&self) -> GitRepository {
        GitRepository::open(&self.root).expect("open repository")
    }

    fn canonical_root(&self) -> PathBuf {
        fs::canonicalize(&self.root).expect("canonicalize repo")
    }
}

fn git_ok(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = git_raw(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}\n{}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    output.stdout
}

fn git_raw(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "CF45")
        .env("GIT_AUTHOR_EMAIL", "cf45@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF45")
        .env("GIT_COMMITTER_EMAIL", "cf45@example.invalid")
        .output()
        .expect("run git")
}

// ---------------------------------------------------------------------------
// 1. Status: untracked mode follows the repository configuration
// ---------------------------------------------------------------------------

#[test]
fn status_config_mode_follows_show_untracked_files_and_explicit_modes_do_not() {
    let repo = Repo::new();
    fs::write(repo.root.join("new.txt"), b"new\n").expect("write untracked");
    let repository = repo.repository();

    // config=no: the Source-shaped read omits untracked entries entirely...
    repo.git(&["config", "status.showUntrackedFiles", "no"]);
    assert!(
        repository
            .status_source_observed()
            .expect("status with config=no")
            .is_empty()
    );
    // ...while the explicit Native modes keep overriding the configuration.
    let explicit_all = repository
        .status_with(&StatusOptions {
            untracked: UntrackedMode::All,
            ignored: false,
        })
        .expect("explicit all");
    assert_eq!(explicit_all.len(), 1);
    assert_eq!(explicit_all[0].path().display_lossy(), "new.txt");
    assert!(explicit_all[0].is_untracked());
    assert_eq!(
        repository.status().expect("default StatusOptions").len(),
        1,
        "StatusOptions::default() is still the explicit Normal mode"
    );

    // config=normal and config=all both report the untracked file.
    for mode in ["normal", "all"] {
        repo.git(&["config", "status.showUntrackedFiles", mode]);
        let entries = repository
            .status_source_observed()
            .expect("status with config mode");
        assert_eq!(entries.len(), 1, "config={mode}");
        assert_eq!(entries[0].path().display_lossy(), "new.txt");
    }

    // An invalid configured mode is a real git failure, never a clean listing.
    repo.git(&["config", "status.showUntrackedFiles", "bogus"]);
    match repository.status_source_observed() {
        Err(GitError::CommandFailed { stderr, .. }) => {
            assert!(
                stderr.contains("Invalid untracked files mode"),
                "unexpected stderr: {stderr}"
            );
        }
        other => panic!("expected a typed status failure, got {other:?}"),
    }
    repo.git(&["config", "--unset", "status.showUntrackedFiles"]);
}

// ---------------------------------------------------------------------------
// 2. Remote-tracking roster: symbolic HEAD, remotes, nested branch names
// ---------------------------------------------------------------------------

#[test]
fn remote_tracking_roster_includes_symbolic_head_and_source_branch_split() {
    let repo = Repo::new();
    let origin = repo.root.parent().unwrap().join("origin.git");
    git_ok(
        repo.root.parent().unwrap(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    repo.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    repo.git(&["push", "-q", "origin", "main"]);
    // A nested coordination-style branch name: the Source split keeps the
    // whole tail after `<remote>/`.
    repo.git(&[
        "push",
        "-q",
        "origin",
        "main:refs/heads/kitty/mission-cf45-lane-a",
    ]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);

    let repository = repo.repository();
    let roster = repository
        .remote_tracking_roster()
        .expect("remote-tracking roster");

    let head = roster
        .entries()
        .iter()
        .find(|entry| entry.reference().as_str() == "refs/remotes/origin/HEAD")
        .expect("symbolic remote HEAD is listed");
    assert_eq!(head.remote(), "origin");
    assert_eq!(head.branch(), "HEAD");
    assert!(head.is_symbolic());
    assert_eq!(
        head.symbolic_target().map(|target| target.as_str()),
        Some("refs/remotes/origin/main")
    );
    assert!(
        head.oid().is_some(),
        "a symbolic ref reports its target's id"
    );

    let main = roster
        .entries()
        .iter()
        .find(|entry| entry.reference().as_str() == "refs/remotes/origin/main")
        .expect("ordinary remote-tracking branch");
    assert_eq!((main.remote(), main.branch()), ("origin", "main"));
    assert!(!main.is_symbolic());

    assert!(
        roster.contains_branch("kitty/mission-cf45-lane-a"),
        "the Source partition keeps the whole branch tail"
    );
    assert!(roster.contains_branch("main"));
    // Faithful to the reference: the symbolic HEAD's branch component is "HEAD".
    assert!(roster.contains_branch("HEAD"));
    assert!(!roster.contains_branch("mission-cf45-lane-a"));

    // The general ref listing is reusable and rejects a non-ref namespace.
    let heads = repository.refs_under("refs/heads/").expect("refs/heads/");
    assert!(
        heads
            .iter()
            .any(|entry| entry.reference().as_str() == "refs/heads/main")
    );
    assert!(matches!(
        repository.refs_under("heads/"),
        Err(GitError::InvalidPlan { .. })
    ));
}

// ---------------------------------------------------------------------------
// 3. Raw worktree porcelain text (Source's literal `worktree {path}` spelling)
// ---------------------------------------------------------------------------

#[test]
fn worktree_porcelain_observation_preserves_the_source_text_spelling() {
    let repo = Repo::new();
    let worktree = repo.root.parent().unwrap().join("wt lane space");
    git_ok(
        &repo.root,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "-b",
            "lane-space",
        ],
    );

    let observation: WorktreePorcelainObservation = repo
        .repository()
        .worktree_porcelain_observation()
        .expect("worktree porcelain");
    assert!(
        observation.succeeded(),
        "status {:?}",
        observation.status_code()
    );
    let text = observation.stdout_text().expect("UTF-8 porcelain text");
    let expected = format!("worktree {}", worktree.display());
    assert!(
        text.contains(&expected),
        "the literal Source substring {expected:?} is missing from {text:?}"
    );
    assert!(text.contains("branch refs/heads/lane-space"));
    assert!(
        !text.contains('\0'),
        "the Source observation is the newline-delimited porcelain text"
    );
}

#[test]
fn worktree_porcelain_observation_exposes_a_failed_command_instead_of_an_empty_list() {
    let repo = Repo::new();
    let repository = repo.repository();
    // Make the read-only handle's repository metadata disappear underneath it.
    fs::rename(repo.root.join(".git"), repo.root.join(".git-gone")).expect("hide .git");

    let observation = repository
        .worktree_porcelain_observation()
        .expect("a completed non-zero exit is an observation, not an error");
    assert!(!observation.succeeded());
    assert_eq!(observation.status_code(), Some(128));
    assert!(observation.stdout_bytes().is_empty());
    assert!(
        observation.stderr_lossy().contains("not a git repository"),
        "stderr: {:?}",
        observation.stderr_lossy()
    );

    // The typed roster keeps its own failure contract (an error, not absence).
    assert!(matches!(
        repository.worktrees(),
        Err(GitError::CommandFailed { .. })
    ));
}

// ---------------------------------------------------------------------------
// 4. Source sparse pattern-file read: newlines, whitespace, symlink, no bound
// ---------------------------------------------------------------------------

#[test]
fn sparse_pattern_source_read_applies_python_text_semantics() {
    let repo = Repo::new();
    fs::create_dir_all(repo.root.join(".git/info")).expect("info dir");
    fs::write(
        repo.root.join(".git/info/sparse-checkout"),
        b"  /*  \r\n# comment\n\n  !x/y  \r\n\x0b  z  ",
    )
    .expect("write pattern file");

    let repository = repo.repository();
    assert_eq!(
        repository
            .sparse_pattern_file_text_source()
            .expect("Source text read")
            .as_deref(),
        Some("  /*  \n# comment\n\n  !x/y  \n\x0b  z  ")
    );
    assert_eq!(
        repository
            .sparse_pattern_file_patterns_source()
            .expect("Source pattern read"),
        Some(vec![
            "/*".to_string(),
            "# comment".to_string(),
            "!x/y".to_string(),
            "z".to_string(),
        ])
    );

    // A final symlink is followed by the ordinary Source read; the bounded,
    // no-follow Native reader deliberately refuses it. The two capabilities
    // stay distinct.
    let target = repo.root.join("patterns.real");
    fs::write(&target, b"/*\n").expect("write symlink target");
    let link = repo.root.join(".git/info/sparse-checkout");
    fs::remove_file(&link).expect("remove regular pattern file");
    symlink(&target, &link);
    assert_eq!(
        repository
            .sparse_pattern_file_text_source()
            .expect("Source follows the symlink")
            .as_deref(),
        Some("/*\n")
    );
    assert!(repository.sparse_pattern_file_text().is_err());

    // Restore a regular pattern file for the two bound checks below.
    fs::remove_file(&link).expect("remove symlink");

    // Exactly one byte over the Native bound: the existing bounded reader is the
    // authority and refuses with its documented `SparsePatternFileTooLarge`.
    let exact = vec![b'x'; (MAX_SPARSE_PATTERN_FILE_BYTES + 1) as usize];
    fs::write(&link, &exact).expect("write boundary pattern file");
    assert!(matches!(
        repository.sparse_pattern_file_text(),
        Err(GitError::SparsePatternFileTooLarge { .. })
    ));
    assert_eq!(
        repository
            .sparse_pattern_file_text_source()
            .expect("Source reads past the Native bound")
            .map(|text| text.len()),
        Some(exact.len())
    );

    // Well beyond the bound: Source still reads the whole file; the bounded
    // reader still refuses (its kernel no-follow bounded read errors before it
    // can return bytes).
    let payload = "# pad\n".repeat((MAX_SPARSE_PATTERN_FILE_BYTES as usize / 6) + 8);
    assert!(payload.len() as u64 > MAX_SPARSE_PATTERN_FILE_BYTES + 1);
    fs::write(&link, payload.as_bytes()).expect("write large pattern file");
    let text = repository
        .sparse_pattern_file_text_source()
        .expect("unbounded Source read")
        .expect("present");
    assert_eq!(text.len(), payload.len());
    let bounded = repository.sparse_pattern_file_text();
    assert!(
        matches!(
            bounded,
            Err(GitError::Io { .. }) | Err(GitError::SparsePatternFileTooLarge { .. })
        ),
        "the bounded reader must refuse a >1 MiB pattern file, got {bounded:?}"
    );
    assert_eq!(
        repository
            .sparse_pattern_file_patterns_source()
            .expect("patterns"),
        Some(vec!["# pad".to_string()])
    );

    // Absent file: both Source reads answer None, not an error.
    fs::remove_file(&link).expect("remove large pattern file");
    assert_eq!(
        repository
            .sparse_pattern_file_text_source()
            .expect("absent"),
        None
    );
    assert_eq!(
        repository
            .sparse_pattern_file_patterns_source()
            .expect("absent"),
        None
    );
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create symlink");
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) {
    fs::copy(target, link).expect("copy (no symlink support)");
}

// ---------------------------------------------------------------------------
// 5. Source revision seam: short branch, tag, remote name, failures
// ---------------------------------------------------------------------------

#[test]
fn source_revision_reads_branch_tag_and_remote_tracking_names() {
    let repo = Repo::new();
    repo.git(&["tag", "v1"]);
    repo.git(&["checkout", "-q", "-b", "lane-a"]);
    fs::write(repo.root.join("a.txt"), b"two\n").expect("write second revision");
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "-q", "-m", "second"]);
    let second_oid = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).expect("oid text");
    let second_oid = second_oid.trim().to_string();
    repo.git(&["checkout", "-q", "main"]);

    let origin = repo.root.parent().unwrap().join("origin.git");
    git_ok(
        repo.root.parent().unwrap(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    repo.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    repo.git(&["push", "-q", "origin", "main", "lane-a"]);
    repo.git(&["fetch", "-q", "origin"]);

    let repository = repo.repository();
    let path = sk_git::GitPath::from_git_bytes(b"a.txt").expect("path");

    for (label, revision, expected) in [
        ("short branch", "lane-a", b"two\n".as_slice()),
        ("annotated/lightweight tag", "v1", b"one\n".as_slice()),
        (
            "remote-tracking spelling",
            "origin/lane-a",
            b"two\n".as_slice(),
        ),
        ("HEAD", "HEAD", b"one\n".as_slice()),
        ("full object id", second_oid.as_str(), b"two\n".as_slice()),
    ] {
        let parsed = SourceRevision::parse(revision).expect(label);
        assert_eq!(
            repository
                .read_blob_at_revision(&parsed, &path)
                .unwrap_or_else(|error| panic!("{label}: {error}"))
                .as_deref(),
            Some(expected),
            "{label}"
        );
    }

    // The same seam reads the tree listing at a short branch name.
    let lane = SourceRevision::parse("lane-a").expect("lane-a");
    assert_eq!(
        repository
            .tree_paths_at_revision(&lane, Some(&path))
            .expect("tree paths"),
        vec![path.clone()]
    );
    assert!(
        repository
            .tree_entry_at_revision(&lane, &path)
            .expect("tree entry")
            .is_some()
    );

    // Absent path at a valid revision is None; an unresolvable revision is the
    // ordinary typed command failure.
    let absent = sk_git::GitPath::from_git_bytes(b"missing.txt").expect("path");
    assert_eq!(
        repository
            .read_blob_at_revision(&lane, &absent)
            .expect("absent path"),
        None
    );
    let missing = SourceRevision::parse("no-such-revision").expect("valid operand");
    assert!(matches!(
        repository.read_blob_at_revision(&missing, &path),
        Err(GitError::CommandFailed { .. })
    ));

    // Argv safety: an option-shaped operand is refused before Git runs.
    for invalid in ["", "-x", "--output=/tmp/x"] {
        assert!(
            matches!(
                SourceRevision::parse(invalid),
                Err(GitError::InvalidPlan { .. })
            ),
            "{invalid:?} must be refused"
        );
    }
    assert!(matches!(
        SourceRevision::parse("a\0b"),
        Err(GitError::InvalidPlan { .. })
    ));
}

// ---------------------------------------------------------------------------
// Fixture sanity: the repository handle points at the canonical root
// ---------------------------------------------------------------------------

#[test]
fn fixture_root_is_canonical() {
    let repo = Repo::new();
    assert_eq!(repo.repository().root(), repo.canonical_root().as_path());
}
