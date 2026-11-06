//! Real-repository tests for bounded typed tree/blob reads and merge bases.
//!
//! Every test uses a throw-away repository. Nothing reads the workspace
//! checkout, a network remote, or the operator's index.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{BlobLookup, GitError, GitPath, GitRepository, Oid, StartPoint, TreeEntryKind};

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        Self { _temp: temp, root }
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_in(&self.root, args)
    }

    fn head(&self) -> String {
        String::from_utf8_lossy(&self.git(&["rev-parse", "HEAD"]))
            .trim()
            .to_string()
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).unwrap()
    }
}

fn git_in(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("TMPDIR", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn init(root: &Path) {
    for directory in ["", "home", "config", "tmp"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    git_in(root, &["init", "-q", "-b", "main"]);
}

fn path(bytes: &[u8]) -> GitPath {
    GitPath::from_git_bytes(bytes).unwrap()
}

fn populated() -> Repo {
    let repo = Repo::new();
    init(&repo.root);
    fs::write(repo.root.join("a.txt"), b"alpha\n").unwrap();
    fs::create_dir_all(repo.root.join("dir")).unwrap();
    fs::create_dir_all(repo.root.join("dir2")).unwrap();
    fs::write(repo.root.join("dir/one.txt"), b"one\n").unwrap();
    fs::write(repo.root.join("dir/two.txt"), b"two\n").unwrap();
    fs::write(repo.root.join("dir2/three.txt"), b"three\n").unwrap();
    fs::write(repo.root.join("gone.txt"), b"gone\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("a.txt", repo.root.join("link")).unwrap();
    }
    fs::write(repo.root.join("big.bin"), vec![b'x'; 100]).unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "first"]);
    fs::remove_file(repo.root.join("gone.txt")).unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "second"]);
    repo
}

#[test]
fn reads_an_exact_blob_and_tree_entry_at_head() {
    let repo = populated();
    let repository = repo.open();
    let entry = repository
        .tree_entry_at(&StartPoint::Head, &path(b"a.txt"))
        .unwrap()
        .expect("a.txt entry");
    assert_eq!(entry.kind(), TreeEntryKind::Blob);
    assert_eq!(entry.mode(), "100644");
    let expected = String::from_utf8_lossy(&repo.git(&["rev-parse", "HEAD:a.txt"]))
        .trim()
        .to_string();
    assert_eq!(entry.oid().as_str(), expected);
    assert_eq!(
        repository
            .read_blob_at(&StartPoint::Head, &path(b"a.txt"))
            .unwrap(),
        Some(b"alpha\n".to_vec())
    );
}

#[test]
fn a_prefix_sharing_path_never_answers_for_the_requested_one() {
    let repo = populated();
    let repository = repo.open();
    let tree = repository
        .tree_entry_at(&StartPoint::Head, &path(b"dir"))
        .unwrap()
        .expect("the directory itself is a tree entry");
    assert_eq!(tree.kind(), TreeEntryKind::Tree);
    assert_eq!(
        repository
            .read_blob_at(&StartPoint::Head, &path(b"dir"))
            .unwrap(),
        None,
        "a tree has no blob body"
    );
    let entry = repository
        .tree_entry_at(&StartPoint::Head, &path(b"dir/one.txt"))
        .unwrap()
        .expect("exact nested path");
    assert_eq!(entry.path().to_path_buf(), PathBuf::from("dir/one.txt"));
    assert_eq!(
        repository
            .read_blob_at(&StartPoint::Head, &path(b"dir2/three.txt"))
            .unwrap(),
        Some(b"three\n".to_vec()),
        "the sibling directory answers for its own path"
    );
}

#[test]
fn tree_paths_are_literal_prefix_scoped() {
    let repo = populated();
    let repository = repo.open();
    let dir = repository
        .tree_paths_at(&StartPoint::Head, Some(&path(b"dir")))
        .unwrap();
    let names: Vec<String> = dir.iter().map(GitPath::display_lossy).collect();
    assert_eq!(names, vec!["dir/one.txt", "dir/two.txt"]);
    let all = repository.tree_paths_at(&StartPoint::Head, None).unwrap();
    assert!(
        all.iter()
            .any(|entry| entry.as_os_str() == "dir2/three.txt")
    );
    assert!(all.iter().any(|entry| entry.as_os_str() == "big.bin"));
}

#[test]
fn a_deleted_path_is_absent_and_a_path_with_a_newline_stays_exact() {
    let repo = populated();
    fs::write(repo.root.join("line\nbreak.txt"), b"nl\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "newline"]);
    let repository = repo.open();
    assert!(
        repository
            .tree_entry_at(&StartPoint::Head, &path(b"gone.txt"))
            .unwrap()
            .is_none(),
        "a path deleted in HEAD must be absent"
    );
    assert_eq!(
        repository
            .read_blob_at(&StartPoint::Head, &path(b"line\nbreak.txt"))
            .unwrap(),
        Some(b"nl\n".to_vec())
    );
    let all = repository.tree_paths_at(&StartPoint::Head, None).unwrap();
    assert!(
        all.iter()
            .any(|entry| entry.as_os_str() == "line\nbreak.txt")
    );
}

#[test]
fn a_symlink_is_read_as_its_blob_body() {
    let repo = populated();
    let repository = repo.open();
    let entry = repository
        .tree_entry_at(&StartPoint::Head, &path(b"link"))
        .unwrap()
        .expect("link entry");
    assert_eq!(entry.kind(), TreeEntryKind::Symlink);
    assert_eq!(entry.mode(), "120000");
    assert_eq!(
        repository
            .read_blob_at(&StartPoint::Head, &path(b"link"))
            .unwrap(),
        Some(b"a.txt".to_vec())
    );
}

#[test]
fn a_blob_over_the_bound_refuses_without_reading_it() {
    let repo = populated();
    let repository = repo.open();
    let error = repository
        .read_blob_at_bounded(&StartPoint::Head, &path(b"big.bin"), 10)
        .unwrap_err();
    match error {
        GitError::BlobTooLarge { path, limit } => {
            assert_eq!(path, "big.bin");
            assert_eq!(limit, 10);
        }
        other => panic!("expected BlobTooLarge, got {other:?}"),
    }
    assert!(
        matches!(
            repository.read_blob_at_bounded(&StartPoint::Head, &path(b"a.txt"), 1024 * 1024 * 1024),
            Err(GitError::InvalidPlan { .. })
        ),
        "a bound above the seam cap is refused, not silently clamped"
    );
}

#[test]
fn head_lookup_distinguishes_present_absent_and_unborn() {
    let repo = populated();
    let repository = repo.open();
    assert_eq!(
        repository.read_blob_at_head(&path(b"a.txt")).unwrap(),
        BlobLookup::Present(b"alpha\n".to_vec())
    );
    assert_eq!(
        repository.read_blob_at_head(&path(b"nope.txt")).unwrap(),
        BlobLookup::Absent
    );
    assert_eq!(
        repository.read_blob_at_head(&path(b"dir")).unwrap(),
        BlobLookup::Absent,
        "a directory is not a blob body"
    );

    let unborn = Repo::new();
    init(&unborn.root);
    fs::write(unborn.root.join("a.txt"), b"alpha\n").unwrap();
    let unborn_repository = unborn.open();
    assert_eq!(
        unborn_repository
            .read_blob_at_head(&path(b"a.txt"))
            .unwrap(),
        BlobLookup::NoRevision
    );
    assert_eq!(
        unborn_repository.head_commit().unwrap(),
        None,
        "the underlying probe reports an unborn HEAD as absent"
    );
}

#[test]
fn an_unreadable_head_is_an_error_not_an_absence() {
    let repo = populated();
    fs::write(repo.root.join(".git/HEAD"), b"ref: refs/heads/\xff\n").unwrap();
    let repository = repo.open();
    let error = repository.read_blob_at_head(&path(b"a.txt")).unwrap_err();
    assert!(
        matches!(error, GitError::UnreadableHead { .. }),
        "an unreadable HEAD must surface as a distinguished error: {error:?}"
    );
}

#[test]
fn merge_base_answers_a_real_common_ancestor_and_unrelated_histories() {
    let repo = populated();
    let base = repo.head();
    repo.git(&["checkout", "-q", "-b", "side"]);
    fs::write(repo.root.join("side.txt"), b"side\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "side"]);
    let side = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    fs::write(repo.root.join("main.txt"), b"main\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "main"]);
    let main = repo.head();

    let repository = repo.open();
    assert_eq!(
        repository
            .merge_base(&Oid::parse(&main).unwrap(), &Oid::parse(&side).unwrap())
            .unwrap()
            .as_ref()
            .map(Oid::to_string),
        Some(base),
        "the fork point is the merge base"
    );

    repo.git(&["checkout", "-q", "--orphan", "orphan"]);
    repo.git(&["rm", "-rq", "--cached", "."]);
    fs::write(repo.root.join("orphan.txt"), b"orphan\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "orphan"]);
    let orphan = repo.head();
    assert_eq!(
        repository
            .merge_base(&Oid::parse(&main).unwrap(), &Oid::parse(&orphan).unwrap())
            .unwrap(),
        None,
        "unrelated histories have no merge base, which is not an error"
    );
}

/// A 64 MiB canonical status log is the documented replay bound
/// (`sk-status::MAX_EVENT_LOG_BYTES`), so the explicit bounded read must accept
/// it — with the capture limit actually plumbed through the process seam, not
/// just a raised constant.
#[test]
fn an_explicit_bound_beyond_the_old_seam_cap_reads_a_large_status_log() {
    let repo = populated();
    let payload = vec![b'x'; 17 * 1024 * 1024];
    fs::write(repo.root.join("status.events.jsonl"), &payload).unwrap();
    repo.git(&["add", "status.events.jsonl"]);
    repo.git(&["commit", "-qm", "large status log"]);

    let repository = repo.open();
    let bytes = repository
        .read_blob_at_bounded(
            &StartPoint::Head,
            &path(b"status.events.jsonl"),
            sk_git::MAX_BLOB_BYTES,
        )
        .unwrap()
        .expect("the committed log is a blob at HEAD");
    assert_eq!(bytes.len(), payload.len(), "the log must not be truncated");
    assert_eq!(bytes, payload);
    assert!(
        matches!(
            repository.read_blob_at_head_bounded(
                &path(b"status.events.jsonl"),
                sk_git::DEFAULT_MAX_BLOB_BYTES
            ),
            Err(GitError::BlobTooLarge { .. })
        ),
        "the small default still refuses the oversized log"
    );
}

#[test]
fn a_full_sixty_four_mib_blob_reads_exactly_and_one_byte_over_refuses() {
    let repo = populated();
    let payload = vec![b'y'; 64 * 1024 * 1024];
    fs::write(repo.root.join("status.events.jsonl"), &payload).unwrap();
    repo.git(&["add", "status.events.jsonl"]);
    repo.git(&["commit", "-qm", "sixty-four mebibyte log"]);

    let repository = repo.open();
    let blob = repository
        .read_blob_at_head_bounded(&path(b"status.events.jsonl"), sk_git::MAX_BLOB_BYTES)
        .unwrap();
    match blob {
        BlobLookup::Present(bytes) => {
            assert_eq!(
                bytes.len(),
                payload.len(),
                "the full bound is not truncated"
            );
            assert_eq!(&bytes[..4], b"yyyy");
            assert_eq!(&bytes[bytes.len() - 4..], b"yyyy");
        }
        other => panic!("expected Present, got {}", other.as_str()),
    }
    assert!(
        matches!(
            repository.read_blob_at_head_bounded(
                &path(b"status.events.jsonl"),
                sk_git::MAX_BLOB_BYTES - 1
            ),
            Err(GitError::BlobTooLarge { .. })
        ),
        "one byte over the bound refuses"
    );
}

#[test]
fn a_corrupt_head_is_unreadable_not_a_missing_repository() {
    for content in [b"garbage\n".as_slice(), b"".as_slice(), b"ref: garbage\n"] {
        let repo = populated();
        fs::write(repo.root.join(".git/HEAD"), content).unwrap();
        let error = GitRepository::open(&repo.root)
            .err()
            .unwrap_or_else(|| panic!("a corrupt HEAD must not open as a repository"));
        assert!(
            matches!(error, GitError::UnreadableHead { .. }),
            "HEAD {:?} must be named unreadable, not {:?}",
            String::from_utf8_lossy(content),
            error
        );
    }
}

#[test]
fn an_empty_symbolic_head_target_is_unreadable_not_a_command_failure() {
    let repo = populated();
    fs::write(repo.root.join(".git/HEAD"), b"ref: refs/heads/\n").unwrap();
    let repository = repo.open();
    let error = repository
        .read_blob_at_head(&path(b"a.txt"))
        .expect_err("an empty symbolic target cannot name a commit");
    assert!(
        matches!(error, GitError::UnreadableHead { .. }),
        "expected UnreadableHead, got {error:?}"
    );
}
