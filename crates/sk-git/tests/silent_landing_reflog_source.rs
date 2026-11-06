//! Source-first parity for the MESSAGE-LESS landing ref advance.
//!
//! Source advances a branch with `git update-ref <ref> <new> <old>` and NO
//! `-m` (`ref_advance._update_branch_ref_cas(..., message=None)`), so git
//! records an EMPTY reflog subject. `-m ""` is a different request: git refuses
//! it outright ("Refusing to perform update with empty message"), so a Native
//! adapter cannot express Source's write by passing an empty string. H4's
//! `advance_branch_ref` therefore states the absence explicitly
//! (`plan_silent_ref_transaction` / `apply_silent_ref_transaction`), and this
//! test binds the result to Source's own RAW reflog bytes:
//!
//! * both refs/heads/<branch> log files, entry for entry, parsed into
//!   `old new actor <time> <tz>\\t<message>` — every field except the wall-clock
//!   stamp, which is asserted to lie inside THAT side's observed run window;
//! * the landing entry's message is EMPTY on both sides and its raw line has no
//!   tab at all (an empty `-m ""` would have failed, and a custom message would
//!   have content);
//! * `.git/logs/HEAD` carries the same entry, because the branch is checked out;
//! * the strict Native prepare contract is still separate: an empty message is
//!   REFUSED by `plan_ref_transaction`, and a non-empty message still reaches
//!   the reflog through the one executor.
//!
//! Order: the pinned Source runs first on its own copy, its bytes are asserted,
//! then the Native call runs on a byte-identical copy of the same seed and must
//! agree.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_git::{BranchName, GitRepository, RefChange, RefName};

const ORACLE: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.git.ref_advance import advance_branch_ref

repo = Path(sys.argv[1])
old = sys.argv[2]
new = sys.argv[3]
branch = sys.argv[4]
advance_branch_ref(repo, branch, new, expected_old_sha=old)

# Split the actor/identity from the stamp: git writes
# "<old> <new> <name> <email> <time> <tz>\t<message>".
def parse(path):
    if not path.exists():
        return []
    result = []
    for line in path.read_bytes().split(b"\n"):
        if not line:
            continue
        header, sep, message = line.partition(b"\t")
        text = header.decode("utf-8", "surrogateescape")
        old_oid, new_oid, rest = text.split(" ", 2)
        name, rest = rest.split(" <", 1)
        email, rest = rest.split("> ", 1)
        stamp, tz = rest.split(" ")
        result.append({
            "raw": line.decode("utf-8", "surrogateescape"),
            "has_tab": bool(sep),
            "old": old_oid,
            "new": new_oid,
            "name": name,
            "email": email,
            "time": int(stamp),
            "tz": tz,
            "message": message.decode("utf-8", "surrogateescape"),
        })
    return result

print(json.dumps({
    "parsed_branch": parse(repo / ".git" / "logs" / "refs" / "heads" / branch),
    "parsed_head": parse(repo / ".git" / "logs" / "HEAD"),
    "final_tip": __import__("subprocess").run(
        ["git", "rev-parse", f"refs/heads/{branch}"], cwd=repo, capture_output=True, text=True, check=True
    ).stdout.strip(),
}))
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python() -> PathBuf {
    oracle::python()
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

fn git(dir: &Path, args: &[&str]) -> String {
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
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        text(&output.stderr)
    );
    text(&output.stdout).trim().to_owned()
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("copy target");
    for entry in fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path()).expect("metadata");
        if metadata.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// A seed with `main` at the first commit and a second commit reachable from
/// `tip` (the landing target), so the advance is a fast-forward on both sides.
fn build_seed(sandbox: &Sandbox) -> (PathBuf, String, String) {
    let seed = sandbox.path().join("seed");
    fs::create_dir_all(&seed).expect("seed dir");
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.name", "Test"]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "commit.gpgsign", "false"]);
    fs::write(seed.join("file.txt"), "one\n").expect("write");
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-qm", "first"]);
    let first = git(&seed, &["rev-parse", "HEAD"]);
    fs::write(seed.join("file.txt"), "two\n").expect("write");
    git(&seed, &["commit", "-qam", "second"]);
    let second = git(&seed, &["rev-parse", "HEAD"]);
    git(&seed, &["branch", "tip", &second]);
    git(&seed, &["update-ref", "refs/heads/main", &first]);
    git(&seed, &["reset", "--hard", "-q", &first]);
    (seed, first, second)
}

#[derive(Clone, Debug)]
struct Entry {
    raw: String,
    has_tab: bool,
    old: String,
    new: String,
    name: String,
    email: String,
    time: i64,
    tz: String,
    message: String,
}

fn parse_entries(path: &Path) -> Vec<Entry> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8(bytes).expect("reflog is UTF-8 here");
    let mut entries = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let (header, message) = match line.split_once('\t') {
            Some((header, message)) => (header, message.to_owned()),
            None => (line, String::new()),
        };
        let (old, rest) = header.split_once(' ').expect("old oid");
        let (new, rest) = rest.split_once(' ').expect("new oid");
        let (name, rest) = rest.split_once(" <").expect("name");
        let (email, rest) = rest.split_once("> ").expect("email");
        let (stamp, tz) = rest.rsplit_once(' ').expect("stamp");
        entries.push(Entry {
            raw: line.to_owned(),
            has_tab: line.contains('\t'),
            old: old.to_owned(),
            new: new.to_owned(),
            name: name.to_owned(),
            email: email.to_owned(),
            time: stamp.parse().expect("stamp integer"),
            tz: tz.to_owned(),
            message,
        });
    }
    entries
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

#[test]
fn the_landing_ref_advance_records_source_absent_message_raw_reflog() {
    let sandbox = Sandbox::new();
    let (seed, first, second) = build_seed(&sandbox);
    let script = sandbox.path().join("source-advance.py");
    fs::write(&script, ORACLE).expect("write oracle");

    // ---------------- Source FIRST ----------------
    let source_repo = sandbox.path().join("source");
    copy_tree(&seed, &source_repo);
    let started = now_epoch();
    let source = run(
        &python(),
        &[
            &script.display().to_string(),
            &source_repo.display().to_string(),
            &first,
            &second,
            "main",
        ],
        &sandbox.path().join("cwd"),
        &sandbox.pairs(),
    );
    let finished = now_epoch();
    assert!(
        source.status.success(),
        "the pinned Source advance failed (a harness/premise failure, not a business red):\n{}\n{}",
        text(&source.stdout),
        text(&source.stderr)
    );
    let source_value: serde_json::Value =
        serde_json::from_slice(&source.stdout).expect("source json");

    let source_branch = parse_entries(&source_repo.join(".git/logs/refs/heads/main"));
    let source_head = parse_entries(&source_repo.join(".git/logs/HEAD"));
    assert_eq!(
        source_value["final_tip"].as_str(),
        Some(second.as_str()),
        "the Source advance moved the ref to the landing commit"
    );
    let source_landing = source_branch.last().expect("Source wrote a reflog entry");
    assert_eq!(
        (source_landing.old.as_str(), source_landing.new.as_str()),
        (first.as_str(), second.as_str()),
        "the Source landing entry is a compare-and-swap from the observed tip"
    );
    assert_eq!(
        source_landing.message, "",
        "Source's landing reflog subject is EMPTY (no -m)"
    );
    assert!(
        !source_landing.has_tab,
        "Source's raw landing reflog line carries no tab and no message: {:?}",
        source_landing.raw
    );
    assert!(
        (started..=finished).contains(&source_landing.time),
        "the Source landing stamp {} lies in its observed window [{started}, {finished}]",
        source_landing.time
    );
    assert_eq!(
        (source_landing.name.as_str(), source_landing.email.as_str()),
        ("Test", "test@example.com"),
        "the Source reflog actor is the repository identity"
    );
    assert_eq!(
        source_landing.tz, "+0000",
        "the Source reflog timezone is the committer's"
    );
    // Source's advance appends TWO HEAD entries because HEAD is a symref to the
    // advanced branch: the branch update (empty message) and the checkout resync
    // (`reset: moving to main`). Both are bound here, not assumed.
    let seed_head = parse_entries(&seed.join(".git/logs/HEAD"));
    assert_eq!(
        source_head.len(),
        seed_head.len() + 2,
        "Source's advance appends the branch update and the resync to HEAD's log"
    );
    let source_branch_update = &source_head[seed_head.len()];
    assert_eq!(
        (
            source_branch_update.old.as_str(),
            source_branch_update.new.as_str(),
            source_branch_update.message.as_str()
        ),
        (first.as_str(), second.as_str(), ""),
        "HEAD's branch-update entry carries the empty Source message"
    );
    assert!(!source_branch_update.has_tab);
    let source_resync = &source_head[seed_head.len() + 1];
    assert_eq!(
        (
            source_resync.old.as_str(),
            source_resync.new.as_str(),
            source_resync.message.as_str()
        ),
        (second.as_str(), second.as_str(), "reset: moving to main"),
        "Source resyncs the checkout with `reset --hard <branch>`"
    );

    // ---------------- Native SECOND, byte-identical seed ----------------
    let native_repo = sandbox.path().join("native");
    copy_tree(&seed, &native_repo);
    let repository = GitRepository::open(&native_repo).expect("open native repository");
    let branch = BranchName::parse("main").expect("branch");
    let old = sk_git::Oid::parse(&first).expect("old oid");
    let new = sk_git::Oid::parse(&second).expect("new oid");
    let native_started = now_epoch();
    let resynced = repository
        .advance_branch_ref(&branch, &old, &new)
        .expect("the Native advance succeeds");
    let native_finished = now_epoch();
    assert_eq!(
        resynced.len(),
        1,
        "the PRIMARY checkout holds the branch, so Source (and this adapter) resync it: {resynced:?}"
    );
    let native_branch = parse_entries(&native_repo.join(".git/logs/refs/heads/main"));
    let native_head = parse_entries(&native_repo.join(".git/logs/HEAD"));

    assert_eq!(
        native_branch.len(),
        source_branch.len(),
        "the same number of reflog entries is written"
    );
    for (index, (native_entry, source_entry)) in
        native_branch.iter().zip(source_branch.iter()).enumerate()
    {
        let is_landing = index + 1 == native_branch.len();
        assert_eq!(
            (native_entry.old.as_str(), native_entry.new.as_str()),
            (source_entry.old.as_str(), source_entry.new.as_str()),
            "entry {index}: the compare-and-swap values match Source"
        );
        assert_eq!(
            (native_entry.name.as_str(), native_entry.email.as_str()),
            (source_entry.name.as_str(), source_entry.email.as_str()),
            "entry {index}: the reflog actor matches Source"
        );
        assert_eq!(
            native_entry.tz, source_entry.tz,
            "entry {index}: the reflog timezone matches Source"
        );
        assert_eq!(
            native_entry.message, source_entry.message,
            "entry {index}: the reflog MESSAGE matches Source byte for byte"
        );
        assert_eq!(
            native_entry.has_tab, source_entry.has_tab,
            "entry {index}: the raw line shape matches Source ({:?} vs {:?})",
            native_entry.raw, source_entry.raw
        );
        if is_landing {
            assert_eq!(
                native_entry.message, "",
                "the Native landing reflog subject is EMPTY, exactly as Source's"
            );
            assert!(
                (native_started..=native_finished).contains(&native_entry.time),
                "the Native landing stamp {} lies in its observed window [{native_started}, {native_finished}]",
                native_entry.time
            );
        } else {
            assert_eq!(
                native_entry.raw, source_entry.raw,
                "entry {index} was written before either run and is byte-identical"
            );
        }
    }
    assert_eq!(
        native_head.len(),
        source_head.len(),
        "HEAD's reflog has the same shape on both sides"
    );
    assert_eq!(
        native_head.len(),
        source_head.len(),
        "HEAD's log has the same number of entries on both sides"
    );
    for (index, (native_entry, source_entry)) in
        native_head.iter().zip(source_head.iter()).enumerate()
    {
        assert_eq!(
            (native_entry.old.as_str(), native_entry.new.as_str()),
            (source_entry.old.as_str(), source_entry.new.as_str()),
            "HEAD entry {index}: the old/new object ids match Source"
        );
        assert_eq!(
            (native_entry.name.as_str(), native_entry.email.as_str()),
            (source_entry.name.as_str(), source_entry.email.as_str()),
            "HEAD entry {index}: the actor matches Source"
        );
        assert_eq!(
            (native_entry.message.as_str(), native_entry.has_tab),
            (source_entry.message.as_str(), source_entry.has_tab),
            "HEAD entry {index}: the MESSAGE and raw shape match Source ({:?} vs {:?})",
            native_entry.raw,
            source_entry.raw
        );
        if index + 2 >= native_head.len() {
            assert!(
                (native_started..=native_finished).contains(&native_entry.time),
                "HEAD entry {index}: the stamp {} lies in the Native run's observed window",
                native_entry.time
            );
        } else {
            assert_eq!(
                native_entry.raw, source_entry.raw,
                "HEAD entry {index} predates either run and is byte-identical"
            );
        }
    }

    // ---------------- the strict contract is still separate ----------------
    let empty = repository.plan_ref_transaction(
        "",
        vec![RefChange::set(
            RefName::parse("refs/heads/strict").expect("ref"),
            new.clone(),
            None,
        )],
    );
    assert!(
        empty.is_err(),
        "the strict prepare contract still REFUSES an empty reflog message"
    );
    let strict_ref = RefName::parse("refs/heads/strict").expect("ref");
    let plan = repository
        .plan_ref_transaction(
            "strict: a caller that records a message still can",
            vec![RefChange::set(strict_ref.clone(), new.clone(), None)],
        )
        .expect("strict plan");
    let outcome = repository
        .apply_ref_transaction(&plan)
        .expect("strict apply");
    assert_eq!(
        outcome.message(),
        "strict: a caller that records a message still can"
    );
    let strict_entries = parse_entries(&native_repo.join(".git/logs/refs/heads/strict"));
    assert_eq!(
        strict_entries
            .last()
            .expect("strict entry")
            .message
            .as_str(),
        "strict: a caller that records a message still can",
        "the strict form still writes its message through the one executor"
    );
}
