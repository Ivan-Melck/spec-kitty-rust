//! Source-compatible path proof controls for the Op target projection.
//!
//! These are real-Git controls for the read-only facts the pinned reference
//! asks at preflight steps 6a/6b: relative paths stay lexical, a missing entry,
//! a symlink, an unstattable entry, and a loop all pass the directory rule, a
//! real directory (not a gitlink) refuses, `.worktrees/` refuses, an absolute
//! path resolves inside the worktree or passes through when it is outside, and a
//! failed gitlink probe is a directory refusal — not an invented second policy.
//!
//! The native planner keeps its own stricter typed errors; these tests do not
//! claim native path equivalence, only the source projection's facts.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use sk_git::{GitPath, GitRepository, SourcePathRefusal, source_preflight_path};

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("create repo");
        let fixture = Self { temp, root };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "Fixture"]);
        fixture.git(&["config", "user.email", "fixture@example.invalid"]);
        fs::write(fixture.root.join("README.md"), b"# fixture\n").expect("write readme");
        fixture.git(&["add", "README.md"]);
        fixture.git(&["commit", "-q", "-m", "initial"]);
        fixture
    }

    fn git(&self, args: &[&str]) {
        let home = self.temp.path().join("home");
        fs::create_dir_all(&home).expect("create home");
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository(&self) -> GitRepository {
        GitRepository::open(&self.root).expect("open fixture repository")
    }

    fn path(&self, raw: &[u8]) -> GitPath {
        GitPath::from_git_bytes(raw).expect("valid git path")
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, bytes).expect("write fixture file");
    }
}

#[test]
fn missing_file_and_regular_file_pass_the_source_path_rule() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    assert!(source_preflight_path(&repository, &fixture.path(b"kitty-ops/missing.jsonl")).is_ok());
    fixture.write("kitty-ops/present.jsonl", b"{}\n");
    assert!(source_preflight_path(&repository, &fixture.path(b"kitty-ops/present.jsonl")).is_ok());
}

#[test]
fn worktrees_prefix_refuses_at_six_a() {
    let fixture = Fixture::new();
    fixture.write(".worktrees/lane/x.jsonl", b"{}\n");
    let repository = fixture.repository();
    let error = source_preflight_path(&repository, &fixture.path(b".worktrees/lane/x.jsonl"))
        .expect_err("a .worktrees path refuses");
    assert!(
        matches!(error, SourcePathRefusal::Source { .. }),
        "{error:?}"
    );
}

#[test]
fn a_real_directory_refuses_but_lexical_relative_paths_are_kept() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("kitty-ops/record.jsonl")).unwrap();
    let repository = fixture.repository();
    let error = source_preflight_path(&repository, &fixture.path(b"kitty-ops/record.jsonl"))
        .expect_err("a directory argument refuses");
    assert!(
        matches!(error, SourcePathRefusal::Source { .. }),
        "{error:?}"
    );

    // The relative '..' survives Path construction. Once stat finds a real
    // directory, Source GitPath.parse rejects this probe BEFORE the index
    // command catch: it is ValueError, not a SafeCommit directory refusal.
    fs::create_dir_all(fixture.root.join("sub")).unwrap();
    fs::create_dir_all(fixture.root.join("dir")).unwrap();
    let error = source_preflight_path(&repository, &fixture.path(b"sub/../dir"))
        .expect_err("the lexical relative path fails source probe parsing");
    assert!(
        matches!(error, SourcePathRefusal::Unreadable { .. }),
        "{error:?}"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_missing_targets_and_relative_parent_loops_pass() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    symlink("target", fixture.root.join("link")).unwrap();
    symlink("loop", fixture.root.join("loop")).unwrap();
    let repository = fixture.repository();
    // A final symlink is one 120000 entry, never a directory.
    assert!(source_preflight_path(&repository, &fixture.path(b"link")).is_ok());
    // A loop in a relative parent is unstattable: is_symlink/is_dir both say
    // false in the reference, so the path rule passes (staging would fail later).
    assert!(source_preflight_path(&repository, &fixture.path(b"loop/x.jsonl")).is_ok());
}

#[test]
fn failed_gitlink_probe_is_a_directory_refusal() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("gitlink-shaped")).unwrap();
    let repository = fixture.repository();
    // The source folds a failed `index_entries` probe into "not a gitlink", so
    // the directory refusal stands rather than becoming a probe error.
    fs::write(fixture.root.join(".git/index"), b"not an index").unwrap();
    let error = source_preflight_path(&repository, &fixture.path(b"gitlink-shaped"))
        .expect_err("the failed probe still refuses the directory");
    assert!(
        matches!(error, SourcePathRefusal::Source { .. }),
        "{error:?}"
    );
}

#[test]
fn absolute_paths_resolve_inside_and_pass_through_outside() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let inside = fixture.root.join("kitty-ops/inside.jsonl");
    fs::create_dir_all(inside.parent().unwrap()).unwrap();
    fs::write(&inside, b"{}\n").unwrap();
    let inside = inside.as_os_str().as_encoded_bytes().to_vec();
    assert!(source_preflight_path(&repository, &fixture.path(&inside)).is_ok());

    let outside = fixture.temp.path().join("outside.jsonl");
    fs::write(&outside, b"{}\n").unwrap();
    let outside = outside.as_os_str().as_encoded_bytes().to_vec();
    assert!(
        source_preflight_path(&repository, &fixture.path(&outside)).is_ok(),
        "an outside path is passed as-is, not rejected by containment here"
    );
}
