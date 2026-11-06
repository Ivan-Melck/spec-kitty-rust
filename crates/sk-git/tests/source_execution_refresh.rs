//! Source-first parity for the SOURCE-EXECUTION freshness path (the one that
//! FETCHES).
//!
//! `origin_freshness._check_remote` does not merely look: it runs ONE bounded
//! `ls-remote --heads` per remote, then ONE bounded `fetch --no-tags
//! --no-recurse-submodules --no-auto-gc --refmap= <remote>
//! +refs/heads/<b>:refs/remotes/<remote>/<b> …` for the branches whose tracking
//! ref is not already at the listed tip, and only THEN compares each local
//! branch with that tracking ref. H4 adds the same execution path as
//! `GitRepository::refresh_branches`; the read-only `check_branch_freshness`
//! query stays exactly as it was, and is not a waiver for this one.
//!
//! This test prepares TWO real clones of one real bare origin, runs the pinned
//! Source FIRST on the first clone and asserts its facts (verdicts, tracking
//! refs, `FETCH_HEAD`, untouched local branches), then runs the Native path on
//! the second clone and compares:
//!
//! * every verdict field (`state`, `remote`, `behind`, `ahead`, detail, remote
//!   tip, timeout bound);
//! * which branches the refresh actually transferred (`fetched`) and the
//!   resulting `refs/remotes/<remote>/*` values;
//! * `refs/heads/*` is UNCHANGED — the fetch writes tracking refs only;
//! * `FETCH_HEAD` byte for byte after binding each side's own origin URL;
//! * the `no_remote` note (a clone whose remote was removed but whose tracking
//!   refs remain) versus a clone that never had a remote (silent).

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_git::{BranchName, GitRepository};

const ORACLE: &str = r#"
import json, subprocess, sys
from pathlib import Path
from specify_cli.git.origin_freshness import check_branches

repo = Path(sys.argv[1])
branches = sys.argv[2:]
verdicts = check_branches(repo, branches)

def git(*args):
    return subprocess.run(["git", *args], cwd=repo, capture_output=True, text=True)

def ref(name):
    out = git("rev-parse", "--verify", "--quiet", name)
    return out.stdout.strip() if out.returncode == 0 else None

fetch_head = repo / ".git" / "FETCH_HEAD"
rows = []
for verdict in verdicts:
    rows.append({
        "branch": verdict.branch,
        "remote": verdict.remote,
        "state": verdict.state.value,
        "behind": verdict.behind,
        "ahead": verdict.ahead,
        "detail": verdict.detail,
        "remote_tip": verdict.remote_sha,
        "timeout_seconds": verdict.timeout_seconds,
    })
tracking = {}
for verdict in verdicts:
    if verdict.remote is None:
        continue
    tracking[verdict.branch] = ref(f"refs/remotes/{verdict.remote}/{verdict.branch}")
print(json.dumps({
    "verdicts": rows,
    "tracking": tracking,
    "branches": {b: ref(f"refs/heads/{b}") for b in branches},
    "fetch_head": fetch_head.read_text(encoding="utf-8", errors="surrogateescape") if fetch_head.exists() else None,
    "origin_url": git("remote", "get-url", "origin").stdout.strip() or None,
}))
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in [
            "home", "cache", "config", "data", "state", "runtime", "tmp", "cwd",
        ] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox root");
        }
        Self { root: temp.keep() }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn pairs(&self) -> Vec<(String, String)> {
        let root = repo_root();
        vec![
            (
                "HOME".into(),
                self.path().join("home").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".into(),
                self.path().join("cache").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".into(),
                self.path().join("config").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".into(),
                self.path().join("data").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".into(),
                self.path().join("state").display().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR".into(),
                self.path().join("runtime").display().to_string(),
            ),
            (
                "TMPDIR".into(),
                self.path().join("tmp").display().to_string(),
            ),
            ("TMP".into(), self.path().join("tmp").display().to_string()),
            ("TEMP".into(), self.path().join("tmp").display().to_string()),
            (
                "PYTHONPATH".into(),
                format!("{}:{}", root.display(), root.join("src").display()),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".into(),
                root.join("packs").display().to_string(),
            ),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ("PYTHONUTF8".into(), "1".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
    }
}

fn run(program: &Path, args: &[&str], cwd: &Path, env: &[(String, String)]) -> Output {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).env_clear();
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run process")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn git_status(dir: &Path, args: &[&str], must_succeed: bool) -> (bool, String) {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2001-02-03T04:05:06+0000")
        .env("GIT_COMMITTER_DATE", "2001-02-03T04:05:06+0000")
        .output()
        .expect("run git");
    if must_succeed {
        assert!(
            output.status.success(),
            "git {args:?} in {} failed: {}",
            dir.display(),
            text(&output.stderr)
        );
    }
    (
        output.status.success(),
        text(&output.stdout).trim().to_owned(),
    )
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_status(dir, args, true).1
}

fn read_ref(dir: &Path, reference: &str) -> Option<String> {
    let (ok, value) = git_status(dir, &["rev-parse", "--verify", "--quiet", reference], false);
    ok.then_some(value)
}

fn build_origin(sandbox: &Sandbox) -> (PathBuf, BTreeMap<String, String>) {
    let origin = sandbox.path().join("origin.git");
    let work = sandbox.path().join("origin-work");
    fs::create_dir_all(&work).expect("work dir");
    git(
        sandbox.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().expect("origin path"),
        ],
    );
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["config", "user.name", "Test"]);
    git(&work, &["config", "user.email", "test@example.com"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    fs::write(work.join("file.txt"), "one\n").expect("write");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-qm", "first"]);
    let first = git(&work, &["rev-parse", "HEAD"]);
    fs::write(work.join("file.txt"), "two\n").expect("write");
    git(&work, &["commit", "-qam", "second"]);
    let second = git(&work, &["rev-parse", "HEAD"]);
    for branch in ["lane-uptodate", "remote-only", "lane-behind"] {
        git(&work, &["branch", branch, &second]);
    }
    git(
        &work,
        &[
            "remote",
            "add",
            "origin",
            origin.to_str().expect("origin path"),
        ],
    );
    git(
        &work,
        &[
            "push",
            "-q",
            "origin",
            "main",
            "lane-uptodate",
            "remote-only",
            "lane-behind",
        ],
    );
    let mut tips = BTreeMap::new();
    tips.insert("first".to_owned(), first);
    tips.insert("second".to_owned(), second.clone());
    tips.insert("main".to_owned(), second.clone());
    tips.insert("lane-uptodate".to_owned(), second.clone());
    tips.insert("remote-only".to_owned(), second.clone());
    tips.insert("lane-behind".to_owned(), second);
    (origin, tips)
}

/// Make one clone exercise every verdict class the refresh can reach:
///
/// * `main`, `lane-behind` — local commits below origin's tip AND their tracking
///   refs REMOVED (so they are fetched);
/// * `lane-uptodate` — tracking ref already at the tip (NOT fetched);
/// * `remote-only` — on the origin, no local branch;
/// * `local-only` — local branch the origin never lists;
/// * `ghost` — names a remote that is not configured at all;
/// * `broken-lane` — names a configured remote whose URL does not resolve.
fn prepare_clone(clone: &Path, tips: &BTreeMap<String, String>) {
    let first = tips["first"].as_str();
    let second = tips["second"].as_str();
    // `main` is checked out: move the ref and reset, never `git branch`.
    git(clone, &["update-ref", "refs/heads/main", first]);
    git(clone, &["reset", "--hard", "-q", first]);
    for branch in ["lane-behind", "local-only", "ghost", "broken-lane"] {
        git(clone, &["branch", branch, first]);
    }
    git(clone, &["branch", "lane-uptodate", second]);
    for branch in ["main", "lane-behind", "remote-only"] {
        git_status(
            clone,
            &["update-ref", "-d", &format!("refs/remotes/origin/{branch}")],
            true,
        );
    }
    git(clone, &["config", "branch.ghost.remote", "ghost"]);
    git(
        clone,
        &[
            "remote",
            "add",
            "broken",
            "/nonexistent/spec-kitty-h4-origin.git",
        ],
    );
    git(clone, &["config", "branch.broken-lane.remote", "broken"]);
}

fn make_clone(
    sandbox: &Sandbox,
    name: &str,
    origin: &Path,
    tips: &BTreeMap<String, String>,
) -> PathBuf {
    let clone = sandbox.path().join(name);
    git(
        sandbox.path(),
        &[
            "clone",
            "-q",
            "--no-hardlinks",
            origin.to_str().expect("origin path"),
            clone.to_str().expect("clone path"),
        ],
    );
    prepare_clone(&clone, tips);
    clone
}

fn snapshot(dir: &Path, branches: &[&str]) -> (serde_json::Value, serde_json::Value) {
    let mut tracking = serde_json::Map::new();
    let mut local = serde_json::Map::new();
    for branch in branches {
        tracking.insert(
            (*branch).to_owned(),
            serde_json::Value::from(read_ref(dir, &format!("refs/remotes/origin/{branch}"))),
        );
        local.insert(
            (*branch).to_owned(),
            serde_json::Value::from(read_ref(dir, &format!("refs/heads/{branch}"))),
        );
    }
    (
        serde_json::Value::Object(tracking),
        serde_json::Value::Object(local),
    )
}

fn native_verdicts(repository: &GitRepository, branches: &[&str]) -> Vec<serde_json::Value> {
    let parsed: Vec<BranchName> = branches
        .iter()
        .map(|branch| BranchName::parse(branch).expect("branch"))
        .collect();
    let refreshed = repository
        .refresh_branches(&parsed)
        .expect("the Native refresh completes");
    refreshed
        .iter()
        .map(|branch| {
            let verdict = branch.verdict();
            serde_json::json!({
                "branch": verdict.branch().as_str(),
                "remote": verdict.remote().map(|remote| remote.as_str()),
                "state": verdict.state().as_str(),
                "behind": verdict.behind(),
                "ahead": verdict.ahead(),
                "detail": verdict.detail(),
                "remote_tip": verdict.remote_tip().map(|oid| oid.as_str()),
                "timeout_seconds": verdict.timeout_seconds(),
                "fetched": branch.fetched(),
                "tracking_tip": branch.tracking_tip().map(|oid| oid.as_str()),
            })
        })
        .collect()
}

/// The branch names FETCH_HEAD lists, in file order.
fn fetch_head_branches(value: &str) -> Vec<String> {
    value
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let quoted = line
                .split_once('\'')
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(reference, _)| reference.to_owned())
                .unwrap_or_else(|| panic!("FETCH_HEAD line names no ref: {line:?}"));
            quoted
                .strip_prefix("refs/heads/")
                .unwrap_or(&quoted)
                .to_owned()
        })
        .collect()
}

#[test]
fn the_source_execution_refresh_ls_remote_fetches_and_compares_like_source() {
    let sandbox = Sandbox::new();
    let (origin, tips) = build_origin(&sandbox);
    let script = sandbox.path().join("source-refresh.py");
    fs::write(&script, ORACLE).expect("write oracle");
    let branches = [
        "main",
        "lane-behind",
        "lane-uptodate",
        "local-only",
        "remote-only",
        "ghost",
        "broken-lane",
    ];

    // TWO clones of the same origin, prepared identically BEFORE either runs.
    let source_repo = make_clone(&sandbox, "source", &origin, &tips);
    let native_repo = make_clone(&sandbox, "native", &origin, &tips);
    let (_, source_before) = snapshot(&source_repo, &branches);
    let (_, native_before) = snapshot(&native_repo, &branches);
    assert_eq!(
        source_before, native_before,
        "both clones start from the same branch state"
    );

    // ---------------- Source FIRST ----------------
    let script_arg = script.display().to_string();
    let source_arg = source_repo.display().to_string();
    let mut source_args: Vec<&str> = vec![&script_arg, &source_arg];
    source_args.extend(branches.iter().copied());
    let source = run(
        &oracle::python(),
        &source_args,
        &sandbox.path().join("cwd"),
        &sandbox.pairs(),
    );
    assert!(
        source.status.success(),
        "the pinned Source refresh failed (a harness/premise failure, not a business red):\n{}\n{}",
        text(&source.stdout),
        text(&source.stderr)
    );
    let source_value: serde_json::Value =
        serde_json::from_slice(&source.stdout).expect("source json");
    let source_rows = source_value["verdicts"]
        .as_array()
        .expect("verdicts")
        .clone();
    assert_eq!(source_rows.len(), branches.len(), "one verdict per branch");
    let lookup = |rows: &[serde_json::Value], branch: &str| -> serde_json::Value {
        rows.iter()
            .find(|row| row["branch"] == branch)
            .cloned()
            .unwrap_or_else(|| panic!("no verdict for {branch}"))
    };

    // The Source facts this test exists to bind.
    let source_main = lookup(&source_rows, "main");
    assert_eq!(
        source_main["state"], "behind",
        "Source FETCHED main's tip and compared against the tracking ref: {source_main}"
    );
    assert_eq!(source_main["behind"], 1, "exactly the missing commit");
    assert_eq!(
        source_main["remote_tip"],
        tips["second"].as_str(),
        "the verdict carries the LISTED remote tip"
    );
    assert_eq!(
        source_value["tracking"]["main"],
        tips["second"].as_str(),
        "Source's refresh left refs/remotes/origin/main at the remote tip"
    );
    assert_eq!(
        lookup(&source_rows, "lane-behind")["state"],
        "behind",
        "a second fetched branch compares behind too"
    );
    assert_eq!(
        lookup(&source_rows, "lane-uptodate")["state"],
        "up_to_date",
        "an already-tracked tip still compares up to date"
    );
    assert_eq!(
        lookup(&source_rows, "local-only")["state"],
        "remote_missing",
        "a branch the remote does not list is remote_missing"
    );
    assert_eq!(
        lookup(&source_rows, "remote-only")["state"],
        "local_missing",
        "a fetched branch with no local ref is local_missing"
    );
    assert_eq!(
        lookup(&source_rows, "ghost")["state"],
        "unreachable",
        "a branch pointed at an unconfigured remote is unreachable, never absent"
    );
    assert_eq!(
        lookup(&source_rows, "broken-lane")["state"],
        "unreachable",
        "a configured remote whose URL does not resolve is unreachable"
    );
    let (source_tracking, source_branches) = snapshot(&source_repo, &branches);
    assert_eq!(
        source_before, source_branches,
        "the refresh NEVER writes refs/heads"
    );
    let source_fetch_head = source_value["fetch_head"]
        .as_str()
        .expect("Source's fetch wrote FETCH_HEAD")
        .to_owned();
    let fetched_branches = fetch_head_branches(&source_fetch_head);
    assert_eq!(
        fetched_branches,
        vec!["main", "lane-behind", "remote-only"],
        "Source fetches only the branches whose tracking ref is missing/moved, in input order"
    );

    // ---------------- Native SECOND, identical clone ----------------
    let repository = GitRepository::open(&native_repo).expect("open native repository");
    let native_rows = native_verdicts(&repository, &branches);
    for (index, branch) in branches.iter().enumerate() {
        let source_row = &source_rows[index];
        let native_row = &native_rows[index];
        assert_eq!(
            native_row["branch"], source_row["branch"],
            "verdict order is the input order"
        );
        for field in [
            "state",
            "remote",
            "behind",
            "ahead",
            "remote_tip",
            "timeout_seconds",
        ] {
            assert_eq!(
                native_row[field], source_row[field],
                "{branch}: the {field} field matches Source"
            );
        }
        match source_row["state"].as_str() {
            Some("compare_failed" | "unreachable") => assert!(
                native_row["detail"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "{branch}: a failed contact carries the git diagnostic"
            ),
            _ => assert_eq!(
                native_row["detail"], source_row["detail"],
                "{branch}: the detail matches Source"
            ),
        }
        assert_eq!(
            native_row["tracking_tip"], source_row["remote_tip"],
            "{branch}: the tracking ref the refresh observed is the listed tip"
        );
        let expected_fetch = fetched_branches.iter().any(|candidate| candidate == branch);
        assert_eq!(
            native_row["fetched"].as_bool(),
            Some(expected_fetch),
            "{branch}: the Native refresh transferred exactly the branches Source did"
        );
    }
    let (native_tracking, native_branches) = snapshot(&native_repo, &branches);
    assert_eq!(
        native_before, native_branches,
        "the Native refresh NEVER writes refs/heads either"
    );
    assert_eq!(
        native_tracking, source_tracking,
        "every refs/remotes/origin/* value is identical to the Source side's"
    );
    let native_fetch_head =
        fs::read_to_string(native_repo.join(".git/FETCH_HEAD")).expect("native FETCH_HEAD");
    assert_eq!(
        fetch_head_branches(&native_fetch_head),
        fetched_branches,
        "FETCH_HEAD lists the same branches in the same order"
    );
    // Bind each side's own origin URL, then the file must be byte-identical:
    // the same shas, the same not-for-merge markers, the same quoting.
    let source_url = source_value["origin_url"]
        .as_str()
        .expect("Source clone has an origin URL")
        .to_owned();
    let native_url = git(&native_repo, &["remote", "get-url", "origin"]);
    assert_eq!(
        source_url, native_url,
        "both clones name the same bare origin, so the URL is the same fact"
    );
    // git prints the transport's own name for the remote in FETCH_HEAD, which
    // is the configured URL without a trailing `.git`.
    let display_url = source_url
        .strip_suffix(".git")
        .unwrap_or(&source_url)
        .to_owned();
    assert!(
        source_fetch_head.contains(&display_url),
        "the Source FETCH_HEAD names the origin it fetched from ({display_url:?}): {source_fetch_head:?}"
    );
    assert_eq!(
        native_fetch_head, source_fetch_head,
        "FETCH_HEAD is byte-identical: the same shas, not-for-merge markers, quoting and order"
    );
}

#[test]
fn a_repository_with_no_remote_reports_the_removed_remote_note_like_source() {
    let sandbox = Sandbox::new();
    let script = sandbox.path().join("source-refresh.py");
    fs::write(&script, ORACLE).expect("write oracle");

    for (label, stale_tracking) in [("never-had", false), ("removed", true)] {
        let repo = sandbox.path().join(label);
        fs::create_dir_all(&repo).expect("repo dir");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        fs::write(repo.join("file.txt"), "one\n").expect("write");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);
        if stale_tracking {
            let tip = git(&repo, &["rev-parse", "HEAD"]);
            git(&repo, &["update-ref", "refs/remotes/origin/main", &tip]);
        }

        let source = run(
            &oracle::python(),
            &[
                &script.display().to_string(),
                &repo.display().to_string(),
                "main",
            ],
            &sandbox.path().join("cwd"),
            &sandbox.pairs(),
        );
        assert!(
            source.status.success(),
            "{label}: the pinned Source refresh failed (a premise failure):\n{}\n{}",
            text(&source.stdout),
            text(&source.stderr)
        );
        let source_value: serde_json::Value =
            serde_json::from_slice(&source.stdout).expect("source json");
        let source_row = &source_value["verdicts"][0];
        assert_eq!(source_row["state"], "no_remote", "{label}: Source verdict");

        let repository = GitRepository::open(&repo).expect("open repository");
        let native = native_verdicts(&repository, &["main"]);
        let native_row = &native[0];
        assert_eq!(
            native_row["state"], source_row["state"],
            "{label}: the Native no_remote verdict matches Source"
        );
        assert_eq!(
            native_row["detail"], source_row["detail"],
            "{label}: the removed-remote note matches Source"
        );
        if stale_tracking {
            assert!(
                source_row["detail"]
                    .as_str()
                    .is_some_and(|detail| !detail.is_empty()),
                "{label}: a clone whose remote was removed carries the note"
            );
        } else {
            assert!(
                source_row["detail"].is_null(),
                "{label}: a clone that never had a remote stays silent"
            );
        }
    }
}
