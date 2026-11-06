//! Source-first proof for the `mission_number` BAKE.
//!
//! Source (`consolidation/mission_number/bake.py::_bake_mission_number_into_mission_branch`)
//! decides the number from the TARGET branch's view and writes it as its OWN
//! commit on the MISSION branch — a detached checkout of the mission branch, the
//! single `meta.json` field, `git commit --only`, then a fast-forward of the
//! branch ref — BEFORE the mission→target landing. The number is what the
//! post-landing projection later records on the target; the bake itself is NOT
//! part of the `record done transitions` commit.
//!
//! This file binds that behaviour to the pinned Source, on real repositories:
//!
//! * the decided number (`_compute_next_mission_number_or_none` vs
//!   `bake_mission_number_into_branch`);
//! * the bake COMMIT object — parents, author/committer identity, message, and
//!   that its diff is the ONE `meta.json` path with every pre-existing field
//!   preserved and only `mission_number` added;
//! * the MISSION branch's RAW reflog: one new entry, empty subject, no tab
//!   (Source fast-forwards with a bare `update-ref`, exactly like the landing);
//! * the TARGET branch is untouched by the bake;
//! * RETRY idempotency: a second bake at the same number writes NOTHING on either
//!   side (no second commit, no second reflog entry);
//! * the no-op paths: the target already carries the number (both sides return
//!   no bake) and a missing mission branch is skipped on both sides.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_git::{BranchName, GitRepository};
use sk_workspace::consolidation::bake_mission_number_into_branch;

const ORACLE: &str = r#"
import json, subprocess, sys
from pathlib import Path
from specify_cli.consolidation.mission_number.bake import (
    _compute_next_mission_number_or_none,
    _write_mission_number_to_branch,
)

repo = Path(sys.argv[1])
slug = sys.argv[2]
mission_branch = sys.argv[3]
target_branch = sys.argv[4]
mode = sys.argv[5]

if mode == "next":
    number = _compute_next_mission_number_or_none(repo, slug, target_branch)
    print(json.dumps({"next": number}))
elif mode == "write":
    wrote = _write_mission_number_to_branch(
        repo, mission_branch, slug, int(sys.argv[6]), target_branch=target_branch
    )
    print(json.dumps({"wrote": bool(wrote)}))
else:  # pragma: no cover
    raise SystemExit(f"unknown mode {mode}")
"#;

const SLUG: &str = "fx-01M6NUMBAKE";
const SIBLING: &str = "other-01M6NUMSIBL";
const MISSION_BRANCH: &str = "kitty/mission-fx-01M6NUMBAKE";
const TARGET_BRANCH: &str = "main";

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
        "git {args:?} in {} failed: {}",
        dir.display(),
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

/// A real repository: `main` holds two missions (this one unbaked, a sibling at
/// `mission_number: 3`), and `kitty/mission-…` is the mission branch at the same
/// commit, exactly the shape a lane-based mission has before consolidation.
fn build_fixture(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.path().join("seed");
    fs::create_dir_all(repo.join("kitty-specs").join(SLUG)).expect("mission dir");
    fs::create_dir_all(repo.join("kitty-specs").join(SIBLING)).expect("sibling dir");
    git(&repo, &["init", "-q", "-b", TARGET_BRANCH]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(
        repo.join("kitty-specs").join(SLUG).join("meta.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "mission_slug": SLUG,
            "mission_id": "01M6NUMBAKE0000000000000000",
            "mission_number": null,
            "mission_type": "software-dev",
            "status_phase": "1",
            "target_branch": TARGET_BRANCH,
            "topology": "lanes",
            "operator_extension": {"note": "must survive the bake"},
        }))
        .expect("json")
            + "\n",
    )
    .expect("write mission meta");
    fs::write(
        repo.join("kitty-specs").join(SIBLING).join("meta.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "mission_slug": SIBLING,
            "mission_number": 3,
        }))
        .expect("json")
            + "\n",
    )
    .expect("write sibling meta");
    fs::write(repo.join("src.txt"), "code\n").expect("write code");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "planning"]);
    git(&repo, &["branch", MISSION_BRANCH]);
    repo
}

#[derive(Clone, Debug)]
struct RawEntry {
    old: String,
    new: String,
    name: String,
    email: String,
    tz: String,
    message: String,
    has_tab: bool,
}

fn raw_reflog(repo: &Path, reference: &str) -> Vec<RawEntry> {
    let path = repo.join(".git").join("logs").join(reference);
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    String::from_utf8(bytes)
        .expect("utf-8 reflog")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (header, message) = match line.split_once('\t') {
                Some((header, message)) => (header, message.to_owned()),
                None => (line, String::new()),
            };
            let (old, rest) = header.split_once(' ').expect("old");
            let (new, rest) = rest.split_once(' ').expect("new");
            let (name, rest) = rest.split_once(" <").expect("name");
            let (email, rest) = rest.split_once("> ").expect("email");
            let (_stamp, tz) = rest.rsplit_once(' ').expect("stamp");
            RawEntry {
                old: old.to_owned(),
                new: new.to_owned(),
                name: name.to_owned(),
                email: email.to_owned(),
                tz: tz.to_owned(),
                message,
                has_tab: line.contains('\t'),
            }
        })
        .collect()
}

fn commit_message(repo: &Path, oid: &str) -> String {
    git(repo, &["log", "-1", "--format=%B", oid])
}

fn commit_parents(repo: &Path, oid: &str) -> Vec<String> {
    git(repo, &["log", "-1", "--format=%P", oid])
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn changed_paths(repo: &Path, oid: &str) -> Vec<String> {
    git(
        repo,
        &["show", "--name-only", "--format=", "--no-renames", oid],
    )
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(str::to_owned)
    .collect()
}

fn source_call(
    sandbox: &Sandbox,
    script: &Path,
    repo: &Path,
    mode: &str,
    number: Option<i64>,
) -> serde_json::Value {
    let number_text = number.map(|value| value.to_string()).unwrap_or_default();
    let mut args = vec![
        script.display().to_string(),
        repo.display().to_string(),
        SLUG.to_owned(),
        MISSION_BRANCH.to_owned(),
        TARGET_BRANCH.to_owned(),
        mode.to_owned(),
    ];
    if number.is_some() {
        args.push(number_text);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run(
        &oracle::python(),
        &args,
        &sandbox.path().join("cwd"),
        &sandbox.pairs(),
    );
    assert!(
        output.status.success(),
        "the pinned Source bake call {mode} failed (a harness/premise failure, not a business red):\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("source json")
}

#[test]
fn the_mission_number_bake_is_its_own_mission_branch_commit_like_source() {
    let sandbox = Sandbox::new();
    let script = sandbox.path().join("bake-oracle.py");
    fs::write(&script, ORACLE).expect("write oracle");
    let seed = build_fixture(&sandbox);
    let source_repo = sandbox.path().join("source");
    let native_repo = sandbox.path().join("native");
    copy_tree(&seed, &source_repo);
    copy_tree(&seed, &native_repo);

    let target_before = git(&source_repo, &["rev-parse", TARGET_BRANCH]);
    let mission_before = git(&source_repo, &["rev-parse", MISSION_BRANCH]);
    let source_reflog_before = raw_reflog(&source_repo, &format!("refs/heads/{MISSION_BRANCH}"));

    // ---------------- Source FIRST ----------------
    let next = source_call(&sandbox, &script, &source_repo, "next", None);
    let number = next["next"].as_i64().expect("Source decided a number");
    assert_eq!(
        number, 4,
        "the target view's maximum (sibling mission_number=3) plus one"
    );
    let wrote = source_call(&sandbox, &script, &source_repo, "write", Some(number));
    assert_eq!(
        wrote["wrote"].as_bool(),
        Some(true),
        "Source wrote the bake"
    );
    let source_commit = git(&source_repo, &["rev-parse", MISSION_BRANCH]);
    assert_ne!(
        source_commit, mission_before,
        "the bake advances the MISSION branch"
    );
    assert_eq!(
        git(&source_repo, &["rev-parse", TARGET_BRANCH]),
        target_before,
        "the bake NEVER moves the target"
    );
    assert_eq!(
        commit_parents(&source_repo, &source_commit),
        vec![mission_before.clone()],
        "the bake commit is a single-parent commit on the mission branch"
    );
    assert_eq!(
        commit_message(&source_repo, &source_commit),
        format!("chore({SLUG}): assign mission_number={number}"),
        "Source's bake message"
    );
    assert_eq!(
        changed_paths(&source_repo, &source_commit),
        vec![format!("kitty-specs/{SLUG}/meta.json")],
        "the bake commit carries ONLY meta.json"
    );
    let source_meta = git(
        &source_repo,
        &[
            "show",
            &format!("{source_commit}:kitty-specs/{SLUG}/meta.json"),
        ],
    );
    let source_doc: serde_json::Value = serde_json::from_str(&source_meta).expect("meta json");
    assert_eq!(source_doc["mission_number"], serde_json::json!(number));
    assert_eq!(
        source_doc["operator_extension"]["note"],
        serde_json::json!("must survive the bake"),
        "every pre-existing field survives the bake"
    );
    let source_reflog_after = raw_reflog(&source_repo, &format!("refs/heads/{MISSION_BRANCH}"));
    assert_eq!(
        source_reflog_after.len(),
        source_reflog_before.len() + 1,
        "the bake appends exactly ONE reflog entry"
    );
    let source_entry = source_reflog_after.last().expect("bake entry");
    assert_eq!(
        (source_entry.old.as_str(), source_entry.new.as_str()),
        (mission_before.as_str(), source_commit.as_str()),
        "the reflog entry is the branch fast-forward"
    );
    assert_eq!(
        source_entry.message, "",
        "Source's branch fast-forward carries NO reflog message"
    );
    assert!(
        !source_entry.has_tab,
        "the raw reflog line has no tab and no message: {:?}",
        source_entry
    );

    // ---- Source RETRY: the same number again writes nothing ----
    let retry = source_call(&sandbox, &script, &source_repo, "write", Some(number));
    assert_eq!(
        retry["wrote"].as_bool(),
        Some(false),
        "Source's bake is idempotent at the same number"
    );
    assert_eq!(
        git(&source_repo, &["rev-parse", MISSION_BRANCH]),
        source_commit,
        "a retried bake creates no second commit"
    );
    assert_eq!(
        raw_reflog(&source_repo, &format!("refs/heads/{MISSION_BRANCH}")).len(),
        source_reflog_after.len(),
        "a retried bake appends no reflog entry"
    );

    // ---------------- Native SECOND, byte-identical seed ----------------
    let repository = GitRepository::open(&native_repo).expect("open repository");
    let mission_branch = BranchName::parse(MISSION_BRANCH).expect("mission branch");
    let target_branch = BranchName::parse(TARGET_BRANCH).expect("target branch");
    let native_reflog_before = raw_reflog(&native_repo, &format!("refs/heads/{MISSION_BRANCH}"));
    let assignment =
        bake_mission_number_into_branch(&repository, SLUG, &mission_branch, &target_branch)
            .expect("the Native bake completes");
    assert_eq!(
        (assignment.number, assignment.baked),
        (number, true),
        "the Native bake decides the same number and reports it baked: {assignment:?}"
    );
    assert_eq!(
        assignment.note, None,
        "a successful bake surfaces no note: {assignment:?}"
    );
    let native_commit = git(&native_repo, &["rev-parse", MISSION_BRANCH]);
    assert_eq!(
        assignment
            .branch_tip
            .as_ref()
            .map(|oid| oid.as_str().to_owned()),
        Some(native_commit.clone()),
        "the assignment reports the branch tip the landing must use"
    );
    assert_eq!(
        git(&native_repo, &["rev-parse", TARGET_BRANCH]),
        target_before,
        "the Native bake never moves the target either"
    );
    assert_eq!(
        commit_parents(&native_repo, &native_commit),
        vec![mission_before.clone()],
        "the Native bake commit has the same single parent"
    );
    assert_eq!(
        commit_message(&native_repo, &native_commit),
        commit_message(&source_repo, &source_commit),
        "the bake commit MESSAGE matches Source"
    );
    assert_eq!(
        changed_paths(&native_repo, &native_commit),
        changed_paths(&source_repo, &source_commit),
        "the bake commit touches the same ONE path"
    );
    let native_meta = git(
        &native_repo,
        &[
            "show",
            &format!("{native_commit}:kitty-specs/{SLUG}/meta.json"),
        ],
    );
    assert_eq!(
        native_meta, source_meta,
        "the baked meta.json BYTES match Source (fields preserved, only mission_number added)"
    );
    let native_reflog_after = raw_reflog(&native_repo, &format!("refs/heads/{MISSION_BRANCH}"));
    assert_eq!(
        native_reflog_after.len(),
        native_reflog_before.len() + 1,
        "the Native bake appends exactly ONE raw reflog entry"
    );
    let native_entry = native_reflog_after.last().expect("bake entry");
    assert_eq!(
        (
            native_entry.old.as_str(),
            native_entry.message.as_str(),
            native_entry.has_tab,
            native_entry.name.as_str(),
            native_entry.email.as_str(),
            native_entry.tz.as_str()
        ),
        (
            source_entry.old.as_str(),
            source_entry.message.as_str(),
            source_entry.has_tab,
            source_entry.name.as_str(),
            source_entry.email.as_str(),
            source_entry.tz.as_str()
        ),
        "the RAW reflog entry matches Source field for field (the old oid is the shared seed tip; \
         the new oid is each run's own commit, asserted against `branch_tip` above)"
    );
    assert_eq!(
        native_entry.new, native_commit,
        "the Native reflog entry introduces the Native bake commit"
    );
    assert_eq!(
        source_entry.new, source_commit,
        "the Source reflog entry introduces the Source bake commit"
    );

    // ---- Native RETRY: same number again writes nothing ----
    let retry = bake_mission_number_into_branch(&repository, SLUG, &mission_branch, &target_branch)
        .expect("the Native retry completes");
    assert_eq!(
        (retry.number, retry.baked),
        (number, false),
        "the Native bake is idempotent at the same number: {retry:?}"
    );
    assert!(
        retry.note.is_some(),
        "the idempotency hit is surfaced: {retry:?}"
    );
    assert_eq!(
        git(&native_repo, &["rev-parse", MISSION_BRANCH]),
        native_commit,
        "the Native retry creates no second commit"
    );
    assert_eq!(
        raw_reflog(&native_repo, &format!("refs/heads/{MISSION_BRANCH}")).len(),
        native_reflog_after.len(),
        "the Native retry appends no reflog entry"
    );

    // ---- the number is NOT part of a done-transitions commit here ----
    assert!(
        !commit_message(&native_repo, &native_commit).contains("record done transitions"),
        "the bake is its own commit, not folded into the done-transitions commit"
    );
}

#[test]
fn a_target_that_already_carries_the_number_makes_the_bake_a_no_op_on_both_sides() {
    let sandbox = Sandbox::new();
    let script = sandbox.path().join("bake-oracle.py");
    fs::write(&script, ORACLE).expect("write oracle");
    let seed = build_fixture(&sandbox);
    // Rewrite the mission's number on the target branch itself.
    let repo = sandbox.path().join("numbered");
    copy_tree(&seed, &repo);
    let meta_path = repo.join("kitty-specs").join(SLUG).join("meta.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).expect("meta")).expect("json");
    document["mission_number"] = serde_json::json!(9);
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&document).expect("json") + "\n",
    )
    .expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "number on target"]);
    // The mission branch keeps the UNNUMBERED meta.json.
    git(
        &repo,
        &[
            "update-ref",
            &format!("refs/heads/{MISSION_BRANCH}"),
            &git(&repo, &["rev-parse", "HEAD~1"]),
        ],
    );
    let mission_before = git(&repo, &["rev-parse", MISSION_BRANCH]);

    let source_next = source_call(&sandbox, &script, &repo, "next", None);
    assert!(
        source_next["next"].is_null(),
        "Source's step 1 short-circuits when the TARGET already carries the number: {source_next}"
    );

    let repository = GitRepository::open(&repo).expect("open repository");
    let assignment = bake_mission_number_into_branch(
        &repository,
        SLUG,
        &BranchName::parse(MISSION_BRANCH).expect("branch"),
        &BranchName::parse(TARGET_BRANCH).expect("branch"),
    )
    .expect("the Native bake completes");
    assert_eq!(
        (assignment.number, assignment.baked),
        (9, false),
        "the number comes from the target and no bake happens: {assignment:?}"
    );
    assert_eq!(
        git(&repo, &["rev-parse", MISSION_BRANCH]),
        mission_before,
        "the mission branch is untouched"
    );
}

#[test]
fn a_missing_mission_branch_is_skipped_on_both_sides() {
    let sandbox = Sandbox::new();
    let script = sandbox.path().join("bake-oracle.py");
    fs::write(&script, ORACLE).expect("write oracle");
    let seed = build_fixture(&sandbox);
    let repo = sandbox.path().join("no-branch");
    copy_tree(&seed, &repo);
    git(&repo, &["branch", "-D", MISSION_BRANCH]);

    let wrote = source_call(&sandbox, &script, &repo, "write", Some(1));
    assert_eq!(
        wrote["wrote"].as_bool(),
        Some(false),
        "Source skips a missing mission branch"
    );
    let repository = GitRepository::open(&repo).expect("open repository");
    let assignment = bake_mission_number_into_branch(
        &repository,
        SLUG,
        &BranchName::parse(MISSION_BRANCH).expect("branch"),
        &BranchName::parse(TARGET_BRANCH).expect("branch"),
    )
    .expect("the Native bake completes");
    assert!(
        !assignment.baked,
        "the Native bake writes nothing for a missing branch: {assignment:?}"
    );
    assert!(
        assignment
            .note
            .as_deref()
            .is_some_and(|note| note.contains("does not exist")),
        "the skip is surfaced, never silent: {assignment:?}"
    );
}
