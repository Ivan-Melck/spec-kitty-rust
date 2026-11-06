//! CF41: canonical commit-path history against real Git repositories.
//!
//! Every fixture is a throw-away `tempfile` repository driven by the real `git`
//! binary. Nothing here reads the workspace checkout, the operator's index, a
//! remote, or the user's global configuration; no test writes into the fork.
//!
//! The oracle for each assertion is Git's own output for the equivalent
//! query (`rev-list`, `show --name-only`, `diff --name-only`), so a passing test
//! means "the native read agrees with git", not "the native read agrees with a
//! value this test invented".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{GitError, GitPath, GitRepository, Oid, commit_history};

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("create repo dir");
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "CF41 Test"]);
        git(&root, &["config", "user.email", "cf41@example.invalid"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        Self { _temp: temp, root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).expect("open repository")
    }

    /// Write `contents` to `relative`, creating parents.
    fn write(&self, relative: &str, contents: &str) {
        write_bytes(&self.root, relative, contents.as_bytes());
    }

    /// `git add -A && git commit -m <message>`; returns the new commit id.
    fn commit(&self, message: &str) -> Oid {
        git(&self.root, &["add", "-A"]);
        git(&self.root, &["commit", "-q", "-m", message]);
        self.head()
    }

    /// `git commit --allow-empty -m <message>`; returns the new commit id.
    fn empty_commit(&self, message: &str) -> Oid {
        git(
            &self.root,
            &["commit", "-q", "--allow-empty", "-m", message],
        );
        self.head()
    }

    fn head(&self) -> Oid {
        let text = String::from_utf8(git(&self.root, &["rev-parse", "HEAD"])).expect("utf8 head");
        Oid::parse(text.trim()).expect("parse head")
    }

    fn resolve(&self, rev: &str) -> Oid {
        let text = String::from_utf8(git(&self.root, &["rev-parse", rev])).expect("utf8 rev");
        Oid::parse(text.trim()).expect("parse rev")
    }

    /// Git's own `rev-list <range>` answer, as the oracle for range reads.
    fn oracle_rev_list(&self, range: &str) -> Vec<Oid> {
        let text = String::from_utf8(git(&self.root, &["rev-list", range])).expect("utf8 rev-list");
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| Oid::parse(line).expect("parse oid"))
            .collect()
    }

    /// Git's own first-parent diff answer, as the oracle for merge diffs.
    ///
    /// `git diff <rev>^1 <rev>` is the diff against the FIRST parent. The
    /// obvious-looking `git diff-tree -m --first-parent` is NOT that oracle: on
    /// git 2.43 it prints the union of both parent diffs (`--first-parent` only
    /// constrains traversal there), which is precisely the behaviour
    /// `git show --first-parent -m` avoids and the native read composes.
    fn oracle_first_parent_paths(&self, rev: &str) -> Vec<String> {
        let first_parent_diff = format!("{rev}^1");
        let text = String::from_utf8(git(
            &self.root,
            &[
                "diff",
                "--name-only",
                "--no-renames",
                first_parent_diff.as_str(),
                rev,
            ],
        ))
        .expect("utf8 diff");
        text.lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

fn git(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "CF41 Test")
        .env("GIT_AUTHOR_EMAIL", "cf41@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF41 Test")
        .env("GIT_COMMITTER_EMAIL", "cf41@example.invalid")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} failed to spawn: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}:\nstdout: {}\nstderr: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn write_bytes(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(&path, contents).expect("write file");
}

fn display(paths: &[GitPath]) -> Vec<String> {
    paths.iter().map(GitPath::display_lossy).collect()
}

#[test]
fn commit_paths_lists_every_path_a_root_commit_added() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("nested/b.txt", "b\n");
    let root_commit = repo.commit("root");

    let paths = commit_history::commit_paths(&repo.open(), &root_commit).expect("paths");
    let mut listed = display(&paths);
    listed.sort();

    assert_eq!(
        listed,
        vec!["a.txt".to_string(), "nested/b.txt".to_string()]
    );
    assert_eq!(paths.len(), 2);
}

#[test]
fn commit_paths_reports_a_genuinely_empty_commit_as_empty() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit("root");
    let empty = repo.empty_commit("nothing at all");
    assert_eq!(
        repo.oracle_first_parent_paths(empty.as_str()),
        Vec::<String>::new()
    );

    let paths = commit_history::commit_paths(&repo.open(), &empty).expect("paths");
    assert!(paths.is_empty(), "empty commit listed {paths:?}");
}

#[test]
fn commit_paths_shows_what_a_merge_brought_in_relative_to_its_first_parent() {
    let repo = Repo::new();
    repo.write("shared.txt", "base\n");
    repo.commit("base");

    // The side branch adds one file and edits the shared one.
    git(repo.root(), &["checkout", "-q", "-b", "side"]);
    repo.write("side.txt", "side\n");
    repo.write("shared.txt", "side version\n");
    repo.commit("side work");

    // The first parent keeps `shared.txt` untouched apart from the merge result.
    git(repo.root(), &["checkout", "-q", "main"]);
    repo.write("main.txt", "main\n");
    repo.commit("main work");

    git(
        repo.root(),
        &["merge", "-q", "--no-ff", "--no-edit", "side"],
    );
    let merge = repo.head();

    // The merge is real, not silently empty: git itself lists the two paths the
    // first-parent diff carries, and the native read must list them too.
    let oracle = repo.oracle_first_parent_paths(merge.as_str());
    assert_eq!(
        oracle,
        vec!["shared.txt".to_string(), "side.txt".to_string()],
        "fixture precondition: the merge must change real paths"
    );

    let paths = commit_history::commit_paths(&repo.open(), &merge).expect("paths");
    assert_eq!(display(&paths), oracle);
}

#[test]
fn commit_paths_reports_an_empty_merge_as_empty_rather_than_skipping_it() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");

    // The side branch changes a file and then reverts it, so its tree equals
    // `main`'s: merging it produces a commit whose first-parent diff is empty.
    git(repo.root(), &["checkout", "-q", "-b", "identical"]);
    repo.write("a.txt", "changed\n");
    repo.commit("change");
    repo.write("a.txt", "a\n");
    repo.commit("revert");
    git(repo.root(), &["checkout", "-q", "main"]);

    git(
        repo.root(),
        &["merge", "-q", "--no-ff", "--no-edit", "identical"],
    );
    let merge = repo.head();
    assert_eq!(
        String::from_utf8(git(
            repo.root(),
            &["rev-list", "--parents", "-n", "1", merge.as_str()]
        ))
        .expect("utf8")
        .split_whitespace()
        .count(),
        3,
        "fixture precondition: this is a real two-parent merge commit"
    );
    assert_eq!(
        repo.oracle_first_parent_paths(merge.as_str()),
        Vec::<String>::new(),
        "fixture precondition: the merge is genuinely empty on its first parent"
    );

    let paths = commit_history::commit_paths(&repo.open(), &merge).expect("paths");
    assert!(paths.is_empty(), "empty merge listed {paths:?}");
}

#[test]
fn commit_paths_splits_a_rename_into_delete_and_add() {
    let repo = Repo::new();
    repo.write("old-name.txt", "content\n");
    repo.commit("base");
    git(repo.root(), &["mv", "old-name.txt", "new-name.txt"]);
    let rename = repo.commit("rename");

    let paths = commit_history::commit_paths(&repo.open(), &rename).expect("paths");
    let mut listed = display(&paths);
    listed.sort();
    assert_eq!(
        listed,
        vec!["new-name.txt".to_string(), "old-name.txt".to_string()]
    );
}

#[test]
fn commit_paths_preserves_exact_path_codepoints_and_bytes() {
    let repo = Repo::new();
    // Private-use, a Unicode noncharacter, and an embedded tab and space: the
    // exact code points git prints must come back unchanged.
    let pua = "paths/trace-\u{E000}\u{FFFF}.md";
    let spaced = "paths/a name\twith tab.md";
    repo.write(pua, "pua\n");
    repo.write(spaced, "spaced\n");
    let commit = repo.commit("odd names");

    let paths = commit_history::commit_paths(&repo.open(), &commit).expect("paths");
    let mut listed = display(&paths);
    listed.sort();
    assert_eq!(listed, vec![spaced.to_string(), pua.to_string()]);
}

#[cfg(unix)]
#[test]
fn commit_paths_preserves_a_non_utf8_path_as_raw_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let repo = Repo::new();
    repo.write("anchor.txt", "anchor\n");
    repo.commit("base");

    let mut raw = b"paths/raw-".to_vec();
    raw.push(0xFF);
    raw.extend_from_slice(b".md");
    let path = repo
        .root()
        .join(PathBuf::from(OsString::from_vec(raw.clone())));
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, b"raw\n").expect("write raw path");
    git(repo.root(), &["add", "-A"]);
    git(repo.root(), &["commit", "-q", "-m", "raw name"]);
    let commit = repo.head();

    let paths = commit_history::commit_paths(&repo.open(), &commit).expect("paths");
    assert_eq!(paths.len(), 1);
    // The exact byte survives: no replacement character, no lossy round trip.
    assert_eq!(paths[0].to_os_string().into_vec(), raw);
}

#[test]
fn commits_in_range_matches_git_for_a_simple_fork() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let base_commit = repo.commit("base");

    git(repo.root(), &["checkout", "-q", "-b", "side"]);
    repo.write("side.txt", "side\n");
    repo.commit("side one");
    repo.write("side.txt", "side two\n");
    repo.commit("side two");

    git(repo.root(), &["checkout", "-q", "main"]);
    repo.write("main.txt", "main\n");
    let main_tip = repo.commit("main one");

    let repository = repo.open();
    let side_tip = repo.resolve("side");
    let range = format!("{base_commit}..{side_tip}");
    assert_eq!(
        commit_history::commits_in_range(&repository, &base_commit, &side_tip).expect("range"),
        repo.oracle_rev_list(&range)
    );
    assert_eq!(
        commit_history::commits_in_range(&repository, &base_commit, &main_tip).expect("range"),
        repo.oracle_rev_list(&format!("{base_commit}..{main_tip}"))
    );
    assert_eq!(
        commit_history::commits_in_range(&repository, &main_tip, &side_tip).expect("range"),
        repo.oracle_rev_list(&format!("{main_tip}..{side_tip}"))
    );
}

#[test]
fn commits_in_range_is_empty_for_identical_tips_and_reversed_ancestry() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let first = repo.commit("first");
    repo.write("a.txt", "a2\n");
    let second = repo.commit("second");

    let repository = repo.open();
    assert!(
        commit_history::commits_in_range(&repository, &second, &second)
            .expect("identical tips")
            .is_empty()
    );
    // `tip..base` where base is an ancestor of tip is the genuinely empty range.
    assert!(
        commit_history::commits_in_range(&repository, &second, &first)
            .expect("ancestor range")
            .is_empty()
    );
    assert_eq!(
        commit_history::commits_in_range(&repository, &first, &second)
            .expect("forward range")
            .len(),
        1
    );
}

#[test]
fn commits_in_range_refuses_an_object_that_does_not_exist() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let tip = repo.commit("only");

    // A well-formed id that names no object in this repository. The read must
    // be an error, never an empty commit list that would read as "nothing there".
    let missing = Oid::parse("0123456789abcdef0123456789abcdef01234567").expect("valid id");
    let error = commit_history::commits_in_range(&repo.open(), &missing, &tip)
        .expect_err("a missing object must be a typed error");
    assert!(
        matches!(error, GitError::CommandFailed { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn commit_paths_refuses_an_object_that_does_not_exist() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.commit("only");
    let missing = Oid::parse("0123456789abcdef0123456789abcdef01234567").expect("valid id");
    let error = commit_history::commit_paths(&repo.open(), &missing)
        .expect_err("a missing object must be a typed error");
    assert!(
        matches!(error, GitError::CommandFailed { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn commit_paths_reads_root_relative_paths_from_a_subdirectory_checkout() {
    // `-c diff.relative=false` is what makes this true: with a subdirectory
    // cwd, git would otherwise print paths relative to the subdirectory.
    let repo = Repo::new();
    repo.write("top.txt", "top\n");
    repo.write("sub/below.txt", "below\n");
    let commit = repo.commit("root");

    let subdir = repo.root().join("sub");
    let repository = GitRepository::open(&subdir).expect("open from subdirectory");
    let mut listed = display(&commit_history::commit_paths(&repository, &commit).expect("paths"));
    listed.sort();
    assert_eq!(
        listed,
        vec!["sub/below.txt".to_string(), "top.txt".to_string()]
    );
}
