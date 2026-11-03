//! Contracts for source-backed Op write admission and best-effort commits.
//!
//! Real temporary repositories and the live pinned source method surface
//! (`ProfileInvocationExecutor.op_write_refusal` / `_commit_op_record`, with
//! the real writer, Git and protection guard) establish the source-observable
//! answers. No guard or commit is monkeypatched. Child-process tests supply a
//! captured `GIT_INDEX_FILE` or scoped fixed commit metadata/source-valid
//! nesting; they never mutate the parent's process environment.
//!
//! These tests are not a full executor/CLI workflow: they call the reference's
//! method surface directly.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;
use sk_git::GitPath;
use sk_invocation::{
    ADMISSION_PROBE_RECORD, ClosedBy, InvocationId, InvocationWriter, OP_CLOSURES_FILENAME,
    OP_GIT_PROBE_TIMEOUT, OpClosedBy, OpCommitEvidence, OpCommitService, OpCompletedEvent,
    OpCompletedFields, OpOutcome, OpWriteAction, OpWriteRefusalCode, append_op_closure,
    op_git_options,
};

const SEED: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const RECORD: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const ARCHIVE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";

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

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
}

impl Fixture {
    fn new(branch: &str) -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        let tmp = temp.path().join("tmp");
        fs::create_dir_all(&root).expect("create repository directory");
        fs::create_dir_all(&home).expect("create scratch home");
        fs::create_dir_all(&tmp).expect("create scratch tmp");
        let fixture = Self {
            root,
            home,
            tmp,
            temp,
        };
        fixture.git_ok(&["init", "-q", "-b", branch]);
        fixture.git_ok(&["config", "user.name", "Fixture"]);
        fixture.git_ok(&["config", "user.email", "fixture@example.invalid"]);
        fixture.git_ok(&["config", "commit.gpgsign", "false"]);
        fs::write(fixture.root.join("README.md"), b"# fixture\n").expect("write README");
        fixture.git_ok(&["add", "README.md"]);
        fixture.git_ok(&["commit", "-q", "-m", "initial"]);
        fixture
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn scratch(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("XDG_DATA_HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"))
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Committer")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
            .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000");
        for variable in GIT_SELECTORS {
            command.env_remove(variable);
        }
        command.env_remove("GIT_INDEX_FILE");
        command
    }

    fn git_at(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd).args(args).output().expect("run git")
    }

    fn git_at_ok(&self, cwd: &Path, args: &[&str]) -> Vec<u8> {
        let output = self.git_at(cwd, args);
        assert!(
            output.status.success(),
            "git {args:?} failed in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn git_ok(&self, args: &[&str]) -> Vec<u8> {
        self.git_at_ok(&self.root, args)
    }

    fn write(&self, rel: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture directory");
        }
        fs::write(&path, content).expect("write fixture file");
        path
    }

    fn commit_file(&self, rel: &str, content: &str) -> String {
        self.write(rel, content);
        self.git_ok(&["add", "--force", rel]);
        self.git_ok(&["commit", "-q", "-m", &format!("add {rel}")]);
        self.head()
    }

    fn checkout_new(&self, branch: &str) {
        self.git_ok(&["checkout", "-q", "-b", branch]);
    }

    fn checkout(&self, branch: &str) {
        self.git_ok(&["checkout", "-q", branch]);
    }

    fn head(&self) -> String {
        String::from_utf8(self.git_ok(&["rev-parse", "HEAD"]))
            .expect("rev-parse HEAD is UTF-8")
            .trim()
            .to_owned()
    }

    fn subject(&self) -> String {
        String::from_utf8(self.git_ok(&["log", "-1", "--format=%s"]))
            .expect("log subject is UTF-8")
            .trim()
            .to_owned()
    }

    fn commit_paths(&self) -> Vec<String> {
        let raw = self.git_ok(&["show", "--name-only", "--format=", "--no-renames", "HEAD"]);
        let mut paths: Vec<String> = String::from_utf8(raw)
            .expect("name-only is UTF-8")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect();
        paths.sort();
        paths
    }

    fn status(&self) -> String {
        String::from_utf8(self.git_ok(&["status", "--porcelain=v1", "--untracked-files=all"]))
            .expect("status is UTF-8")
    }

    fn stage(&self) -> Vec<u8> {
        self.git_ok(&["ls-files", "-s", "-z"])
    }

    fn refs(&self) -> String {
        String::from_utf8(self.git_ok(&["for-each-ref", "--format=%(refname) %(objectname)"]))
            .expect("refs are UTF-8")
    }

    fn index_at(&self, index: &Path, args: &[&str]) -> Vec<u8> {
        let mut command = self.command(&self.root);
        command.env("GIT_INDEX_FILE", index);
        let output = command.args(args).output().expect("run indexed git");
        assert!(
            output.status.success(),
            "indexed git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// Every `kitty-ops/` file (name and bytes), sorted, for no-write controls.
    fn kitty_ops(&self) -> BTreeMap<String, Vec<u8>> {
        let directory = self.root.join("kitty-ops");
        let mut files = BTreeMap::new();
        let Ok(entries) = fs::read_dir(&directory) else {
            return files;
        };
        for entry in entries {
            let entry = entry.expect("read kitty-ops entry");
            if entry.file_type().expect("entry type").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                files.insert(name, fs::read(entry.path()).expect("read record"));
            }
        }
        files
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            head: self.git_ok(&["rev-parse", "--verify", "HEAD"]),
            status: self.status(),
            stage: self.stage(),
            refs: self.refs(),
            records: self.kitty_ops(),
        }
    }

    /// A repository whose primary `main` tracks one Op record; leaves the
    /// checkout on `main` unless the caller switches.
    fn with_positive_primary() -> Self {
        let fixture = Self::new("main");
        fixture.commit_file(
            &format!("kitty-ops/{SEED}.jsonl"),
            "{\"event\": \"started\"}\n",
        );
        fixture
    }

    fn service_at(&self, project: &Path, record: &Path) -> OpCommitService {
        OpCommitService::with_git_options(project, InvocationWriter::new(record), &op_git_options())
            .with_hatch(None)
    }

    fn service(&self) -> OpCommitService {
        self.service_at(&self.root, &self.root)
    }

    fn write_started(&self, id: &str, object: serde_json::Value) {
        self.write(
            &format!("kitty-ops/{id}.jsonl"),
            format!("{}\n", serde_json::to_string(&object).unwrap()),
        );
    }

    fn write_record_line(&self, id: &str, line: &str) {
        self.write(&format!("kitty-ops/{id}.jsonl"), line.as_bytes());
    }

    /// A close-shaped trail: the started row plus the completed row the caller
    /// durably wrote before asking for the best-effort commit.
    fn write_started_and_completed(&self, id: &str, profile: &str, action: &str) {
        let started = json!({
            "event": "started",
            "invocation_id": id,
            "profile_id": profile,
            "action": action,
        });
        let completed = json!({
            "event": "completed",
            "invocation_id": id,
            "completed_at": "2026-11-02T00:00:00+00:00",
            "outcome": "done",
            "closed_by": "agent",
        });
        self.write_record_line(
            id,
            &format!(
                "{}\n{}\n",
                serde_json::to_string(&started).unwrap(),
                serde_json::to_string(&completed).unwrap()
            ),
        );
    }

    fn record_bytes(&self, id: &str) -> Vec<u8> {
        fs::read(self.root.join("kitty-ops").join(format!("{id}.jsonl")))
            .expect("read record bytes")
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    head: Vec<u8>,
    status: String,
    stage: Vec<u8>,
    refs: String,
    records: BTreeMap<String, Vec<u8>>,
}

fn id(raw: &str) -> InvocationId {
    InvocationId::parse(raw).expect("valid fixture invocation id")
}

fn protect(root: &Path, branches: &str) {
    fs::create_dir_all(root.join(".kittify")).expect("create .kittify");
    fs::write(
        root.join(".kittify/config.yaml"),
        format!("protection:\n  protected_branches: {branches}\n"),
    )
    .expect("write protection config");
}

fn oracle_request(project: &Path, record: &Path, hatch: Option<&str>) -> serde_json::Value {
    json!({
        "project": project,
        "record": record,
        "hatch": hatch,
    })
}

fn oracle_admission(project: &Path, record: &Path, hatch: Option<&str>) -> serde_json::Value {
    let script = r#"
import json, os, sys
from pathlib import Path
from specify_cli.invocation.executor import ProfileInvocationExecutor
request = json.load(sys.stdin)
hatch = request.get("hatch")
if hatch is None:
    os.environ.pop("SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS", None)
else:
    os.environ["SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS"] = hatch
executor = ProfileInvocationExecutor(Path(request["project"]), record_root=Path(request["record"]))
refusal = executor.op_write_refusal()
print(json.dumps(None if refusal is None else {"code": refusal.code, "reason": refusal.reason}))
"#;
    let output = support::run_python(
        script,
        Some(&oracle_request(project, record, hatch).to_string()),
    );
    serde_json::from_str(&output).expect("admission oracle JSON")
}

fn oracle_commit(
    project: &Path,
    record: &Path,
    invocation_id: &str,
    closed_by: &str,
    hatch: Option<&str>,
) -> serde_json::Value {
    let script = r#"
import json, os, sys
from pathlib import Path
from specify_cli.invocation.executor import ProfileInvocationExecutor
request = json.load(sys.stdin)
hatch = request.get("hatch")
if hatch is None:
    os.environ.pop("SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS", None)
else:
    os.environ["SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS"] = hatch
executor = ProfileInvocationExecutor(Path(request["project"]), record_root=Path(request["record"]))
outcome = executor._commit_op_record(request["id"], closed_by=request["closed_by"])
print(json.dumps({"committed": outcome.committed, "refusal": outcome.refusal, "skipped": outcome.skipped}))
"#;
    let request = json!({
        "project": project,
        "record": record,
        "id": invocation_id,
        "closed_by": closed_by,
        "hatch": hatch,
    });
    let output = support::run_python(script, Some(&request.to_string()));
    serde_json::from_str(&output).expect("commit oracle JSON")
}

#[test]
fn oracle_method_surface_is_real_and_cwd_isolated() {
    let fixture = Fixture::with_positive_primary();
    let output = support::run_python(
        "import json,os,kernel.resolution\nfrom pathlib import Path\nfrom specify_cli.invocation.executor import ProfileInvocationExecutor\nprint(json.dumps([os.getcwd(), kernel.resolution.__file__]))",
        None,
    );
    let paths: Vec<String> = serde_json::from_str(&output).unwrap();
    assert!(
        Path::new(&paths[0]).starts_with(std::env::temp_dir()),
        "oracle runs in an isolated temporary CWD: {}",
        paths[0]
    );
    assert_eq!(
        Path::new(&paths[1]),
        support::repository_root().join("src/kernel/resolution.py")
    );
    let refusal = oracle_admission(&fixture.root, &fixture.root, None);
    assert_eq!(
        refusal["code"], "OP_WRITE_PROTECTED_BRANCH",
        "live reference method must actually judge the protected primary"
    );
}

#[test]
fn protected_positive_primary_tracking_refuses_with_zero_side_effects() {
    let fixture = Fixture::with_positive_primary();
    let before = fixture.snapshot();

    let mut service = fixture.service();
    let refusal = service
        .op_write_refusal()
        .expect("admission answers")
        .expect("protected primary refuses");
    assert_eq!(refusal.code, OpWriteRefusalCode::ProtectedBranch);
    assert_eq!(refusal.reason, "protected branch 'main'");
    assert!(
        !refusal
            .reason
            .contains("SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS"),
        "the refusal must not name the operator override"
    );
    assert_eq!(fixture.snapshot(), before, "admission wrote nothing");
    assert!(!fixture.root.join(ADMISSION_PROBE_RECORD).exists());

    let oracle = oracle_admission(&fixture.root, &fixture.root, None);
    assert_eq!(oracle["code"], "OP_WRITE_PROTECTED_BRANCH");
    assert_eq!(oracle["reason"], refusal.reason);
    assert_eq!(fixture.snapshot(), before, "the oracle probe wrote nothing");

    let error = service
        .require_op_write_admitted(OpWriteAction::Open)
        .expect_err("the same write is refused");
    let message = error.to_string();
    assert!(message.starts_with("Refusing to open the Op:"));
    assert!(message.contains("Nothing was written."));
    assert!(!message.contains("SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS"));
}

#[test]
fn ignored_admission_sentinel_admits_but_the_real_record_is_still_judged() {
    let fixture = Fixture::with_positive_primary();
    fixture.write(".gitignore", format!("{ADMISSION_PROBE_RECORD}\n"));
    fixture.git_ok(&["add", ".gitignore"]);
    fixture.git_ok(&["commit", "-q", "-m", "ignore the probe stand-in"]);

    let mut service = fixture.service();
    assert_eq!(
        service.op_write_refusal().expect("admission answers"),
        None,
        "the ignored sentinel is the source's admitted local-write case"
    );
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, None),
        serde_json::Value::Null
    );

    // Admission is not stale authority: the real record id is not ignored, so
    // the close attempts the commit and the protected branch refuses it.
    fixture.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "architect", "action": "review"}),
    );
    let before = fixture.record_bytes(RECORD);
    let head_before = fixture.head();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(!outcome.committed());
    assert_eq!(outcome.refusal(), Some("protected branch 'main'"));
    assert_eq!(fixture.record_bytes(RECORD), before);
    assert_eq!(fixture.head(), head_before);
    assert_eq!(
        outcome.skipped(),
        None,
        "a real record is not skipped by the sentinel's ignore rule"
    );
    // The native refusal wrote nothing, so the live source can judge the same
    // real record in place.
    let oracle = oracle_commit(&fixture.root, &fixture.root, RECORD, "agent", None);
    assert_eq!(oracle["refusal"], outcome.refusal().unwrap());
}

#[test]
fn primary_without_op_records_admits_the_local_write_and_close_skips() {
    let fixture = Fixture::new("main");
    let before = fixture.snapshot();
    let mut service = fixture.service();
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, None),
        serde_json::Value::Null
    );

    fixture.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "architect", "action": "review"}),
    );
    let head_before = fixture.head();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(!outcome.committed());
    assert_eq!(
        outcome.skipped(),
        Some("the primary branch (main) tracks no Op records under kitty-ops/")
    );
    assert_eq!(
        fixture.head(),
        head_before,
        "a skipped close creates no commit"
    );
    assert_eq!(
        oracle_commit(&fixture.root, &fixture.root, RECORD, "agent", None)["skipped"],
        outcome.skipped().unwrap()
    );
}

#[test]
fn unreadable_protection_refuses_only_when_the_commit_would_be_judged() {
    let fixture = Fixture::with_positive_primary();
    protect(&fixture.root, "not-a-list");
    let before = fixture.snapshot();

    let mut service = fixture.service();
    let refusal = service
        .op_write_refusal()
        .expect("admission answers")
        .expect("a malformed protection block refuses");
    assert_eq!(refusal.code, OpWriteRefusalCode::ProtectionConfigInvalid);
    assert_eq!(fixture.snapshot(), before);
    let oracle = oracle_admission(&fixture.root, &fixture.root, None);
    assert_eq!(oracle["code"], "OP_WRITE_PROTECTION_CONFIG_INVALID");
    assert_eq!(
        oracle["reason"], refusal.reason,
        "the Op refusal uses canonical policy detail, not WorkspaceError's extra path wrapper"
    );

    // Skip-before-protection: once the probe path is ignored, the same broken
    // configuration is never read and the local write is admitted.
    fixture.write(".gitignore", format!("{ADMISSION_PROBE_RECORD}\n"));
    fixture.git_ok(&["add", ".gitignore"]);
    fixture.git_ok(&["commit", "-q", "-m", "ignore the probe stand-in"]);
    let mut service = fixture.service();
    assert_eq!(
        service.op_write_refusal().expect("admission answers"),
        None,
        "a skipped commit never reads protection"
    );
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, None),
        serde_json::Value::Null
    );
}

#[test]
fn invalid_protection_is_judged_after_the_source_path_checks() {
    let fixture = Fixture::with_positive_primary();
    protect(&fixture.root, "not-a-list");
    // The admission stand-in is a directory, so the source preflight refuses at
    // step 6b (a directory argument) before it ever reads the protection block.
    fs::create_dir_all(fixture.root.join(ADMISSION_PROBE_RECORD)).unwrap();
    let mut service = fixture.service();
    assert_eq!(
        service.op_write_refusal().expect("admission answers"),
        None,
        "the earlier source path failure admits, so the malformed config is not read"
    );
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, None),
        serde_json::Value::Null
    );
}

#[test]
fn project_and_record_protection_policies_are_unioned() {
    let fixture = Fixture::with_positive_primary();
    fixture.git_ok(&["branch", "trunk"]);
    let worktree = fixture.scratch("linked-worktree");
    fixture.git_ok(&["worktree", "add", "-q", worktree.to_str().unwrap(), "trunk"]);

    // Project protects trunk; the record checkout explicitly protects nothing.
    protect(&fixture.root, "[trunk]");
    protect(&worktree, "[]");
    let mut service = fixture.service_at(&fixture.root, &worktree);
    let refusal = service
        .op_write_refusal()
        .expect("admission answers")
        .expect("the project policy still protects trunk");
    assert_eq!(refusal.code, OpWriteRefusalCode::ProtectedBranch);
    assert_eq!(refusal.reason, "protected branch 'trunk'");
    assert_eq!(
        oracle_admission(&fixture.root, &worktree, None)["reason"],
        refusal.reason
    );

    // Reverse: the project explicitly protects nothing, the record checkout
    // protects trunk. The union still refuses.
    protect(&fixture.root, "[]");
    protect(&worktree, "[trunk]");
    let mut service = fixture.service_at(&fixture.root, &worktree);
    let refusal = service
        .op_write_refusal()
        .expect("admission answers")
        .expect("the record policy still protects trunk");
    assert_eq!(refusal.reason, "protected branch 'trunk'");
    assert_eq!(
        oracle_admission(&fixture.root, &worktree, None)["reason"],
        refusal.reason
    );

    // An explicit empty list is authoritative on its own root: with both roots
    // empty, the same trunk checkout is admitted.
    protect(&fixture.root, "[]");
    protect(&worktree, "[]");
    let mut service = fixture.service_at(&fixture.root, &worktree);
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    assert_eq!(
        oracle_admission(&fixture.root, &worktree, None),
        serde_json::Value::Null
    );

    // A real close lands in the linked checkout's own root (shared common dir,
    // separate worktree/index), leaving the project-root checkout untouched.
    fs::create_dir_all(worktree.join("kitty-ops")).unwrap();
    fs::write(
        worktree.join(format!("kitty-ops/{RECORD}.jsonl")),
        format!(
            "{}\n",
            json!({"event": "started", "invocation_id": RECORD, "profile_id": "linked", "action": "review"})
        ),
    )
    .unwrap();
    let main_head = fixture.head();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(outcome.committed(), "outcome: {outcome:?}");
    assert_eq!(
        fixture.head(),
        main_head,
        "the project checkout did not move"
    );
    let subject =
        String::from_utf8(fixture.git_at_ok(&worktree, &["log", "-1", "--format=%s"])).unwrap();
    assert_eq!(subject.trim(), "op(linked): review [01ARZ3ND]");
}

#[test]
fn irrelevant_protected_name_syntax_is_not_a_config_refusal() {
    let fixture = Fixture::with_positive_primary();
    // A protected name Git would reject is irrelevant while the current branch
    // is explicitly protected: the source never parses the other name.
    protect(&fixture.root, "['bad..name', main]");
    let mut service = fixture.service();
    let refusal = service
        .op_write_refusal()
        .expect("admission answers")
        .expect("main is protected");
    assert_eq!(refusal.reason, "protected branch 'main'");
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, None)["reason"],
        refusal.reason
    );

    // With only the irrelevant name configured, main is not protected and the
    // write is admitted; an invalid unrelated name never becomes a new refusal.
    protect(&fixture.root, "['bad..name']");
    let mut service = fixture.service();
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, None),
        serde_json::Value::Null
    );
}

#[test]
fn operator_hatch_matches_the_source_and_stays_an_explicit_seam() {
    let fixture = Fixture::with_positive_primary();
    fixture.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "p", "action": "a"}),
    );

    let mut guarded = fixture.service();
    assert!(guarded.op_write_refusal().unwrap().is_some());

    let mut hatched = fixture.service().with_hatch(Some("1"));
    assert_eq!(
        hatched.op_write_refusal().expect("admission answers"),
        None,
        "the source operator hatch un-protects every branch"
    );
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, Some("1")),
        serde_json::Value::Null
    );
    let head_before = fixture.head();
    let outcome = hatched.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(outcome.committed(), "the hatch admits the real commit");

    let mut disabled = fixture.service().with_hatch(Some("0"));
    assert!(disabled.op_write_refusal().unwrap().is_some());
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, Some("0"))["code"],
        "OP_WRITE_PROTECTED_BRANCH"
    );
    assert_eq!(
        fixture.head(),
        outcome_commit(&outcome),
        "one commit landed"
    );
    assert_ne!(fixture.head(), head_before);
}

fn outcome_commit(outcome: &sk_invocation::OpCommitOutcome) -> String {
    match outcome.evidence().expect("committed evidence") {
        OpCommitEvidence::Committed { commit, .. } => commit.to_string(),
        other => panic!("expected committed evidence, got {other:?}"),
    }
}

#[test]
fn detached_non_git_bare_and_unborn_roots_report_source_outcomes() {
    // Detached: current branch absent admits, the close skips before refusing.
    let detached = Fixture::with_positive_primary();
    detached.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "p", "action": "a"}),
    );
    detached.git_ok(&["checkout", "-q", "--detach"]);
    let mut service = detached.service();
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    assert_eq!(
        oracle_admission(&detached.root, &detached.root, None),
        serde_json::Value::Null
    );
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert_eq!(outcome.refusal(), Some("detached HEAD"));
    assert_eq!(
        oracle_commit(&detached.root, &detached.root, RECORD, "agent", None)["refusal"],
        "detached HEAD"
    );

    // A plain non-git directory: no current branch; the commit reports the
    // source's literal non-worktree outcome and never touches the row.
    let plain = Fixture::new("main");
    let outside = plain.scratch("plain");
    fs::create_dir_all(&outside).expect("create plain directory");
    let record_path = outside.join("kitty-ops").join(format!("{RECORD}.jsonl"));
    fs::create_dir_all(record_path.parent().unwrap()).expect("create record directory");
    fs::write(
        &record_path,
        format!("{}\n", json!({"event": "started", "invocation_id": RECORD})),
    )
    .expect("write plain record");
    let before = fs::read(&record_path).unwrap();
    let mut service = plain.service_at(&outside, &outside);
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    assert_eq!(
        oracle_admission(&outside, &outside, None),
        serde_json::Value::Null
    );
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert_eq!(outcome.refusal(), Some("not a git worktree"));
    assert_eq!(
        oracle_commit(&outside, &outside, RECORD, "agent", None)["refusal"],
        "not a git worktree"
    );
    assert_eq!(fs::read(&record_path).unwrap(), before);

    // Bare repository: same source outcome.
    let bare = plain.scratch("bare.git");
    let output = plain
        .command(plain.path())
        .args(["init", "-q", "--bare", bare.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut service = plain.service_at(&bare, &bare);
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);

    // Unborn symbolic branch with no main/master/develop ref: the unresolved
    // primary skips the local commit.
    let unborn = Fixture::new("trunk");
    unborn.git_ok(&["checkout", "-q", "--orphan", "fresh"]);
    unborn.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "p", "action": "a"}),
    );
    let mut service = unborn.service();
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    assert_eq!(
        oracle_admission(&unborn.root, &unborn.root, None),
        serde_json::Value::Null
    );
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert_eq!(
        outcome.skipped(),
        Some("primary branch unresolved, so it cannot be shown to track Op records")
    );
    assert_eq!(
        oracle_commit(&unborn.root, &unborn.root, RECORD, "agent", None)["skipped"],
        outcome.skipped().unwrap()
    );
}

#[test]
fn close_rejudges_branch_and_protection_after_the_open() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    fixture.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "architect", "action": "review"}),
    );

    let mut service = fixture.service();
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    fixture.checkout("main");
    let before = fixture.record_bytes(RECORD);
    let head_before = fixture.head();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert_eq!(outcome.refusal(), Some("protected branch 'main'"));
    assert_eq!(fixture.record_bytes(RECORD), before);
    assert_eq!(fixture.head(), head_before);

    // A configuration change is re-read too, not frozen at admission.
    fixture.checkout("topic");
    protect(&fixture.root, "[]");
    let mut service = fixture.service();
    assert_eq!(service.op_write_refusal().expect("admission answers"), None);
    protect(&fixture.root, "[topic]");
    let head_before = fixture.head();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert_eq!(outcome.refusal(), Some("protected branch 'topic'"));
    assert_eq!(fixture.head(), head_before);
    assert_eq!(fixture.record_bytes(RECORD), before);
}

#[test]
fn agent_close_commits_only_its_record_with_the_exact_message() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    fixture.write_started_and_completed(RECORD, "architect", "review");
    fixture.write("unrelated.txt", "unstaged\n");

    let head_before = fixture.head();
    let record_before = fixture.record_bytes(RECORD);
    assert_eq!(
        String::from_utf8_lossy(&record_before).lines().count(),
        2,
        "the audit close is durable before the commit"
    );
    let mut service = fixture.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(outcome.committed(), "outcome: {outcome:?}");
    assert_eq!(outcome.refusal(), None);
    assert_eq!(outcome.skipped(), None);
    let commit = outcome_commit(&outcome);
    assert_eq!(service.last_op_commit(), Some(&outcome));
    assert_eq!(fixture.head(), commit);
    assert_ne!(fixture.head(), head_before);
    assert_eq!(fixture.subject(), "op(architect): review [01ARZ3ND]");
    assert_eq!(
        fixture.commit_paths(),
        vec![format!("kitty-ops/{RECORD}.jsonl")]
    );
    assert_eq!(fixture.record_bytes(RECORD), record_before);
    assert!(
        !fixture
            .root
            .join("kitty-ops")
            .join(OP_CLOSURES_FILENAME)
            .exists(),
        "an agent close does not create the spine"
    );

    // The same close repeated is a genuine no-op, never a fabricated commit.
    let head_after = fixture.head();
    let repeated = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(!repeated.committed());
    assert!(repeated.refusal().is_some(), "outcome: {repeated:?}");
    assert_eq!(fixture.head(), head_after, "no second commit is invented");
}

#[test]
fn doctor_sweep_commits_only_the_closure_spine_and_preserves_the_archive() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    // A pre-existing archive record, already committed and byte-frozen.
    fixture.write_started(
        ARCHIVE,
        json!({"event": "started", "invocation_id": ARCHIVE, "profile_id": "legacy-prof", "action": "plan"}),
    );
    fixture.git_ok(&["add", "--force", &format!("kitty-ops/{ARCHIVE}.jsonl")]);
    fixture.git_ok(&["commit", "-q", "-m", "archive record"]);
    let archive_before = fixture.record_bytes(ARCHIVE);

    let completed = OpCompletedEvent::new(OpCompletedFields {
        invocation_id: id(ARCHIVE),
        completed_at: "2026-11-02T00:00:00+00:00".to_owned(),
        outcome: OpOutcome::Done,
        closed_by: ClosedBy::DoctorSweep,
        evidence_ref: None,
    })
    .expect("valid closure");
    append_op_closure(&fixture.root, &completed).expect("append closure spine");

    let spine_before = fs::read(fixture.root.join("kitty-ops").join(OP_CLOSURES_FILENAME)).unwrap();
    let mut service = fixture.service();
    let outcome = service.commit_op_record(&id(ARCHIVE), OpClosedBy::DoctorSweep);
    assert!(outcome.committed(), "outcome: {outcome:?}");
    assert_eq!(
        fixture.subject(),
        "op(legacy-prof): plan [01ARZ3ND] (doctor sweep)"
    );
    assert_eq!(fixture.commit_paths(), vec!["kitty-ops/op-closures.jsonl"]);
    assert_eq!(
        fixture.record_bytes(ARCHIVE),
        archive_before,
        "the historical archive row was not rewritten"
    );
    let spine_after = fs::read(fixture.root.join("kitty-ops").join(OP_CLOSURES_FILENAME)).unwrap();
    assert_eq!(
        spine_after, spine_before,
        "the commit did not rewrite the spine"
    );
}

#[test]
fn started_record_reader_keeps_legacy_and_default_field_behavior() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let mut service = fixture.service();

    // Raw/legacy object: no profile_id/action at all.
    let legacy = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
    fixture.write_record_line(
        legacy,
        &format!("{{\"event\": \"started\", \"invocation_id\": \"{legacy}\"}}\n"),
    );
    let outcome = service.commit_op_record(&id(legacy), OpClosedBy::Agent);
    assert!(outcome.committed(), "outcome: {outcome:?}");
    assert_eq!(fixture.subject(), "op(unknown): unknown [01ARZ3ND]");

    // Falsy values follow `str(value or "unknown")`; non-strings use Python str.
    let falsy = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
    fixture.write_record_line(
        falsy,
        &format!(
            "{{\"event\": \"started\", \"invocation_id\": \"{falsy}\", \"profile_id\": \"\", \"action\": null}}\n"
        ),
    );
    let outcome = service.commit_op_record(&id(falsy), OpClosedBy::Agent);
    assert!(outcome.committed());
    assert_eq!(fixture.subject(), "op(unknown): unknown [01ARZ3ND]");

    let numeric = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
    fixture.write_record_line(
        numeric,
        &format!(
            "{{\"event\": \"started\", \"invocation_id\": \"{numeric}\", \"profile_id\": 123, \"action\": true}}\n"
        ),
    );
    let outcome = service.commit_op_record(&id(numeric), OpClosedBy::Agent);
    assert!(outcome.committed());
    assert_eq!(fixture.subject(), "op(123): True [01ARZ3ND]");

    // A non-object/empty/bad-JSON first row is the source's invalid-record
    // failure; nothing is committed and the bad bytes stay untouched.
    let invalid = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
    for bytes in [
        b"[]\n".as_slice(),
        b"".as_slice(),
        b"{not json\n".as_slice(),
    ] {
        fixture.write_record_line(invalid, &String::from_utf8_lossy(bytes));
        let head_before = fixture.head();
        let outcome = service.commit_op_record(&id(invalid), OpClosedBy::Agent);
        let expected = format!("Invalid invocation record: {invalid}");
        assert_eq!(outcome.refusal(), Some(expected.as_str()));
        assert_eq!(fixture.head(), head_before);
        assert_eq!(fixture.record_bytes(invalid), bytes);
    }

    let missing = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
    let head_before = fixture.head();
    let outcome = service.commit_op_record(&id(missing), OpClosedBy::Agent);
    let expected = format!("Invalid invocation record: {missing}");
    assert_eq!(outcome.refusal(), Some(expected.as_str()));
    assert_eq!(fixture.head(), head_before);

    // The oracle agrees on the raw-legacy message composition.
    let oracle_fixture = Fixture::with_positive_primary();
    oracle_fixture.checkout_new("topic");
    oracle_fixture.write_record_line(
        legacy,
        &format!("{{\"event\": \"started\", \"invocation_id\": \"{legacy}\"}}\n"),
    );
    let oracle = oracle_commit(
        &oracle_fixture.root,
        &oracle_fixture.root,
        legacy,
        "agent",
        None,
    );
    assert_eq!(oracle["committed"], true);
    assert_eq!(oracle_fixture.subject(), "op(unknown): unknown [01ARZ3ND]");
}

#[test]
fn dirty_staging_and_partial_staging_survive_the_record_commit() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    fixture.write("unrelated.txt", "unrelated\n");
    fixture.write("partial.txt", "one\ntwo\n");
    fixture.git_ok(&["add", "unrelated.txt", "partial.txt"]);
    fixture.write("partial.txt", "one\ntwo\nthree\n");

    let staged_before = fixture.stage();
    fixture.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "p", "action": "a"}),
    );

    let mut service = fixture.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(outcome.committed(), "outcome: {outcome:?}");
    assert_eq!(
        fixture.commit_paths(),
        vec![format!("kitty-ops/{RECORD}.jsonl")],
        "only the record path is committed"
    );
    let staged_after = fixture.stage();
    assert!(
        staged_after
            .windows(b"unrelated.txt".len())
            .any(|w| w == b"unrelated.txt"),
        "the operator's unrelated staged entry stays staged"
    );
    assert!(
        staged_after
            .windows(b"partial.txt".len())
            .any(|w| w == b"partial.txt"),
        "the partially staged entry stays staged"
    );
    assert_ne!(
        staged_before, staged_after,
        "only the record entry is reconciled"
    );
    assert_eq!(
        fixture
            .status()
            .lines()
            .filter(|line| line.contains("partial.txt"))
            .count(),
        1,
        "partial.txt keeps its staged and unstaged split"
    );
}

#[cfg(unix)]
#[test]
fn failing_hooks_keep_the_audit_row_and_landed_hook_extras_keep_typed_evidence() {
    use std::os::unix::fs::PermissionsExt;

    let hook_fixture = Fixture::with_positive_primary();
    hook_fixture.checkout_new("topic");
    let hooks = hook_fixture.root.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    hook_fixture.write_started_and_completed(RECORD, "p", "a");
    let record_before = hook_fixture.record_bytes(RECORD);
    let head_before = hook_fixture.head();
    let mut service = hook_fixture.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(!outcome.committed());
    assert!(outcome.refusal().is_some(), "outcome: {outcome:?}");
    assert_eq!(hook_fixture.head(), head_before);
    assert_eq!(hook_fixture.record_bytes(RECORD), record_before);

    // A hook that stages an extra path makes the commit land with an
    // unrequested path: the native backend reports SafeCommitLanded, and the
    // record row plus the real commit are both retained.
    let landed = Fixture::with_positive_primary();
    landed.checkout_new("topic");
    let hooks = landed.root.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    fs::write(&hook, "#!/bin/sh\ngit add extra.txt\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    landed.write("extra.txt", "extra\n");
    landed.write_started_and_completed(RECORD, "p", "a");
    let record_before = landed.record_bytes(RECORD);
    assert_eq!(String::from_utf8_lossy(&record_before).lines().count(), 2);
    let head_before = landed.head();
    let mut service = landed.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(
        !outcome.committed(),
        "landed-with-extras is not a clean success"
    );
    assert!(outcome.refusal().is_some());
    let evidence = outcome
        .evidence()
        .expect("landed commit evidence is retained");
    let landed_commit = match evidence {
        OpCommitEvidence::SafeCommitLanded {
            commit,
            affected_paths,
        } => {
            assert!(
                affected_paths
                    .iter()
                    .any(|path| path.display_lossy() == "extra.txt")
            );
            commit.clone()
        }
        other => panic!("expected SafeCommitLanded evidence, got {other:?}"),
    };
    assert_eq!(landed.head(), landed_commit.to_string());
    assert_ne!(landed.head(), head_before);
    assert_eq!(landed.record_bytes(RECORD), record_before);
    assert!(
        landed.commit_paths().contains(&"extra.txt".to_owned()),
        "the landed commit is not rewound or hidden"
    );

    // A failing post-commit hook is a warning on a confirmed commit.
    let warned = Fixture::with_positive_primary();
    warned.checkout_new("topic");
    let hooks = warned.root.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-commit");
    fs::write(&hook, "#!/bin/sh\nexit 7\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    warned.write_started_and_completed(RECORD, "p", "a");
    let mut service = warned.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(outcome.committed(), "post-commit failures are warnings");
    match outcome.evidence().unwrap() {
        OpCommitEvidence::Committed { warnings, .. } => {
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.contains("post-commit")),
                "warnings: {warnings:?}"
            );
        }
        other => panic!("expected committed evidence, got {other:?}"),
    }
}

#[test]
fn current_branch_stdout_stripping_precedes_protection() {
    let fixture = Fixture::with_positive_primary();
    let branch = "topic\u{a0}";
    fixture.checkout_new(branch);
    protect(&fixture.root, &serde_json::to_string(&[branch]).unwrap());
    let before = fixture.snapshot();
    assert_eq!(
        oracle_admission(&fixture.root, &fixture.root, None),
        json!(null)
    );
    let mut service = fixture.service();
    assert_eq!(
        service.op_write_refusal().unwrap(),
        None,
        "source strips symbolic stdout before selecting its preflight destination"
    );
    assert_eq!(fixture.snapshot(), before);
    fixture.write_started_and_completed(RECORD, "p", "a");
    let bytes = fixture.record_bytes(RECORD);
    let head = fixture.head();
    let source = oracle_commit(&fixture.root, &fixture.root, RECORD, "agent", None);
    assert_eq!(source["committed"], false);
    assert!(source["refusal"].is_string());
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(
        !outcome.committed(),
        "source refuses the stripped destination: {outcome:?}"
    );
    assert!(outcome.refusal().is_some());
    assert_eq!(fixture.head(), head);
    assert_eq!(fixture.record_bytes(RECORD), bytes);
}

#[cfg(unix)]
#[test]
fn nested_hook_retains_unconfirmed_advance_evidence() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    fixture.write_started_and_completed(RECORD, "p", "a");
    let before = fixture.record_bytes(RECORD);
    let hook = fixture.root.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        "#!/bin/sh\ngit commit --no-verify --allow-empty -qm 'nested from hook'\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let mut service = fixture.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(
        !outcome.committed(),
        "uncertainty is not a confirmed success"
    );
    assert!(
        outcome.refusal().unwrap().contains("unconfirmed advance"),
        "{outcome:?}"
    );
    match outcome.evidence().expect("typed uncertainty survives") {
        OpCommitEvidence::AdvanceUnconfirmed {
            candidate,
            destination,
            observed_tip,
            detail,
        } => {
            assert_eq!(destination, "topic");
            assert_eq!(observed_tip.as_ref().unwrap().as_str(), fixture.head());
            assert_ne!(Some(candidate), observed_tip.as_ref());
            assert_eq!(
                fixture.git_ok(&["cat-file", "-t", candidate.as_str()]),
                b"commit\n"
            );
            assert!(detail.contains("do not assume nothing happened"));
        }
        other => panic!("expected unconfirmed evidence, got {other:?}"),
    }
    assert_eq!(fixture.subject(), "nested from hook");
    assert_eq!(fixture.record_bytes(RECORD), before);
    assert_eq!(service.last_op_commit(), Some(&outcome));
}

#[test]
fn source_and_native_ordinary_routes_force_unsigned_commits() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    fixture.git_ok(&["config", "commit.gpgsign", "true"]);
    fixture.git_ok(&["config", "gpg.program", "/nonexistent/gpg"]);
    fixture.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "p", "action": "a"}),
    );
    let mut service = fixture.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(outcome.committed(), "outcome: {outcome:?}");
    let commit = fixture.head();
    let body = String::from_utf8(fixture.git_ok(&["cat-file", "-p", &commit])).unwrap();
    assert!(
        !body.lines().any(|line| line.starts_with("gpgsig ")),
        "ordinary commits explicitly disable signing"
    );
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    source.git_ok(&["config", "commit.gpgsign", "true"]);
    source.git_ok(&["config", "gpg.program", "/nonexistent/gpg"]);
    source.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "p", "action": "a"}),
    );
    let outcome = oracle_commit(&source.root, &source.root, RECORD, "agent", None);
    assert_eq!(
        outcome["committed"], true,
        "source also forces unsigned: {outcome}"
    );
    assert_eq!(source.subject(), fixture.subject());
    let commit = source.head();
    let body = String::from_utf8(source.git_ok(&["cat-file", "-p", &commit])).unwrap();
    assert!(!body.lines().any(|line| line.starts_with("gpgsig ")));
}

#[test]
fn git_options_default_to_the_source_probe_budget() {
    assert_eq!(OP_GIT_PROBE_TIMEOUT, std::time::Duration::from_secs(30));
    assert_eq!(op_git_options().timeout, OP_GIT_PROBE_TIMEOUT);
}

#[test]
fn admission_probe_path_is_the_ulid_shaped_sentinel() {
    assert_eq!(
        sk_invocation::admission_probe_path(),
        GitPath::from_git_bytes(ADMISSION_PROBE_RECORD.as_bytes()).unwrap()
    );
}

// ---------------------------------------------------------------------------
// Captured alternate index: a child test process supplies GIT_INDEX_FILE.
// ---------------------------------------------------------------------------

#[test]
fn captured_alternate_index_is_reconciled_by_the_real_commit() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    fixture.write_started(
        RECORD,
        json!({"event": "started", "invocation_id": RECORD, "profile_id": "p", "action": "a"}),
    );
    let custom = fixture.scratch("custom-index");
    fs::write(&custom, b"").unwrap();
    // Seed the custom index from HEAD, then stage an unrelated entry there.
    fixture.index_at(&custom, &["read-tree", "HEAD"]);
    fs::write(
        fixture.root.join("unrelated.txt"),
        "staged-only-in-custom\n",
    )
    .unwrap();
    fixture.index_at(&custom, &["add", "unrelated.txt"]);

    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "captured_index_child",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(&fixture.root)
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.home)
        .env("TMPDIR", &fixture.tmp)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_INDEX_FILE", &custom)
        .env("SK_OP_CAPTURE_ROOT", &fixture.root)
        .env("SK_OP_CAPTURE_ID", RECORD)
        .env_remove("SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS")
        .output()
        .expect("run captured-index child");
    assert!(
        output.status.success(),
        "captured-index child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let commit = stdout
        .lines()
        .find_map(|line| line.strip_prefix("commit="))
        .unwrap_or_else(|| panic!("child did not print commit=:\n{stdout}"))
        .trim()
        .to_owned();
    assert_eq!(
        fixture.head(),
        commit,
        "the child's commit is the branch tip"
    );
    assert_eq!(
        fixture.commit_paths(),
        vec![format!("kitty-ops/{RECORD}.jsonl")]
    );

    let custom_entries = fixture.index_at(&custom, &["ls-files", "-s", "-z"]);
    let custom_text = String::from_utf8_lossy(&custom_entries);
    assert!(
        custom_text.contains(&format!("kitty-ops/{RECORD}.jsonl")),
        "the captured index is reconciled with the committed record"
    );
    assert!(
        custom_text.contains("unrelated.txt"),
        "unrelated staged content stays in the captured index"
    );
    let default_entries = fixture.git_ok(&["ls-files", "-s", "-z"]);
    assert!(
        !String::from_utf8_lossy(&default_entries).contains(&format!("kitty-ops/{RECORD}.jsonl")),
        "the default index was not silently retargeted"
    );
}

/// Child half of [`captured_alternate_index_is_reconciled_by_the_real_commit`].
///
/// Ignored in ordinary runs; the parent supplies a real captured
/// `GIT_INDEX_FILE` and the same fixture root. It uses the production service,
/// not a monkeypatched guard.
#[test]
#[ignore]
fn captured_index_child() {
    let Some(root) = std::env::var_os("SK_OP_CAPTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let invocation_id = std::env::var("SK_OP_CAPTURE_ID").expect("child invocation id");
    let mut service = OpCommitService::new(&root, InvocationWriter::new(&root)).with_hatch(None);
    let outcome = service.commit_op_record(&id(&invocation_id), OpClosedBy::Agent);
    match outcome.evidence() {
        Some(OpCommitEvidence::Committed { commit, .. }) => {
            println!("commit={commit}");
        }
        other => panic!("child commit did not land cleanly: {outcome:?} / {other:?}"),
    }
}

/// Write one raw legacy row, run the real native close, and prove the outcome
/// is a genuine commit of that one record with the expected subject.
///
/// The returned OID must be the actual post-commit HEAD object; the audit bytes
/// are compared before/after so a commit never rewrites durable evidence.
fn assert_legacy_commit(
    fixture: &Fixture,
    invocation_id: &str,
    line: &str,
    expected_subject: &str,
) {
    fixture.write_record_line(invocation_id, line);
    let record_before = fixture.record_bytes(invocation_id);
    let head_before = fixture.head();
    let mut service = fixture.service();
    let outcome = service.commit_op_record(&id(invocation_id), OpClosedBy::Agent);
    assert!(outcome.committed(), "{invocation_id}: {outcome:?}");
    assert_eq!(outcome.refusal(), None);
    assert_eq!(outcome.skipped(), None);
    let commit = outcome_commit(&outcome);
    assert_eq!(commit, fixture.head(), "the returned OID is the real HEAD");
    assert_ne!(fixture.head(), head_before, "HEAD advanced");
    assert_eq!(
        String::from_utf8_lossy(&fixture.git_ok(&["cat-file", "-t", &commit])).trim(),
        "commit",
        "the returned OID names a real commit object"
    );
    assert_eq!(fixture.subject(), expected_subject);
    assert_eq!(
        fixture.commit_paths(),
        vec![format!("kitty-ops/{invocation_id}.jsonl")],
        "only the closed record is committed"
    );
    assert_eq!(fixture.record_bytes(invocation_id), record_before);
}

/// Run the pinned source `_commit_op_record` on the same raw row.
fn oracle_legacy_commit(fixture: &Fixture, invocation_id: &str, line: &str) -> String {
    fixture.write_record_line(invocation_id, line);
    let record_before = fixture.record_bytes(invocation_id);
    let head_before = fixture.head();
    let outcome = oracle_commit(&fixture.root, &fixture.root, invocation_id, "agent", None);
    assert_eq!(
        outcome["committed"], true,
        "{invocation_id} source outcome: {outcome:?}"
    );
    assert_ne!(fixture.head(), head_before, "the source committed for real");
    assert_eq!(fixture.record_bytes(invocation_id), record_before);
    assert_eq!(
        fixture.commit_paths(),
        vec![format!("kitty-ops/{invocation_id}.jsonl")]
    );
    fixture.subject()
}

fn started_row(invocation_id: &str, profile: &str, action: &str) -> String {
    format!(
        "{{\"event\": \"started\", \"invocation_id\": \"{invocation_id}\", \"profile_id\": {profile}, \"action\": {action}}}\n"
    )
}

/// F1: `json.loads` accepts the bare non-finite constants. The source commits
/// those rows; native must make the same decision with the same subject.
#[test]
fn legacy_bare_nonfinite_tokens_commit_like_the_pinned_source() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let oracle_fixture = Fixture::with_positive_primary();
    oracle_fixture.checkout_new("topic");

    let cases = [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB3",
            "NaN",
            "\"a\"",
            "op(nan): a [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB4",
            "Infinity",
            "\"a\"",
            "op(inf): a [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB5",
            "-Infinity",
            "\"a\"",
            "op(-inf): a [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB6",
            "\"p\"",
            "NaN",
            "op(p): nan [01ARZ3ND]",
        ),
    ];
    for (invocation_id, profile, action, expected) in cases {
        let line = started_row(invocation_id, profile, action);
        assert_legacy_commit(&fixture, invocation_id, &line, expected);
        assert_eq!(
            oracle_legacy_commit(&oracle_fixture, invocation_id, &line),
            expected,
            "the pinned source subject"
        );
        assert_eq!(fixture.subject(), expected);
    }
}

/// F2: Python dicts keep insertion order and a duplicate key keeps its first
/// position with the last value. `serde_json`'s sorted map cannot carry that,
/// so the committed subject (and therefore the commit object) diverged.
#[test]
fn legacy_ordered_and_duplicate_nested_maps_match_the_pinned_source() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let oracle_fixture = Fixture::with_positive_primary();
    oracle_fixture.checkout_new("topic");

    let cases = [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBA",
            "{\"b\": 1, \"a\": 2}",
            "{\"z\": 3, \"y\": 4}",
            "op({'b': 1, 'a': 2}): {'z': 3, 'y': 4} [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBB",
            "{\"a\": 1, \"b\": 2, \"a\": 3}",
            "\"dup\"",
            "op({'a': 3, 'b': 2}): dup [01ARZ3ND]",
        ),
    ];
    for (invocation_id, profile, action, expected) in cases {
        let line = started_row(invocation_id, profile, action);
        assert_legacy_commit(&fixture, invocation_id, &line, expected);
        assert_eq!(
            oracle_legacy_commit(&oracle_fixture, invocation_id, &line),
            expected,
            "the pinned source subject"
        );
        assert_eq!(fixture.subject(), expected);
    }
}

/// F2: float-token overflow becomes ±inf and underflow becomes signed zero,
/// exactly like CPython's `float`; `1e400` was previously rendered as its
/// source text. Exact integers and finite floats keep the prior formatter.
#[test]
fn legacy_numeric_overflow_underflow_and_exact_integers_match_the_pinned_source() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let oracle_fixture = Fixture::with_positive_primary();
    oracle_fixture.checkout_new("topic");

    let cases = [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB7",
            "1e400",
            "-1e400",
            "op(inf): -inf [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB8",
            "1e-400",
            "-1e-400",
            "op(unknown): unknown [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB9",
            "123456789012345678901234567890",
            "1e300",
            "op(123456789012345678901234567890): 1e+300 [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBG",
            "-0",
            "-0.0",
            "op(unknown): unknown [01ARZ3ND]",
        ),
    ];
    for (invocation_id, profile, action, expected) in cases {
        let line = started_row(invocation_id, profile, action);
        assert_legacy_commit(&fixture, invocation_id, &line, expected);
        assert_eq!(
            oracle_legacy_commit(&oracle_fixture, invocation_id, &line),
            expected,
            "the pinned source subject"
        );
        assert_eq!(fixture.subject(), expected);
    }
}

/// F1/F2 together: a truthy container keeps insertion order and Python `repr`
/// of nested arrays, maps, booleans, null and non-finite floats.
#[test]
fn legacy_nested_collections_and_nonfinite_values_render_like_python() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let oracle_fixture = Fixture::with_positive_primary();
    oracle_fixture.checkout_new("topic");

    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FBC";
    let line = started_row(
        invocation_id,
        "[1, NaN, {\"z\": 1, \"y\": 2}, true, null, [], {}]",
        "\"mixed\"",
    );
    let expected = "op([1, nan, {'z': 1, 'y': 2}, True, None, [], {}]): mixed [01ARZ3ND]";
    assert_legacy_commit(&fixture, invocation_id, &line, expected);
    assert_eq!(
        oracle_legacy_commit(&oracle_fixture, invocation_id, &line),
        expected
    );
    assert_eq!(fixture.subject(), expected);
}

/// Authored strings that merely spell a non-finite token stay verbatim, at the
/// top level and nested; only genuine JSON constants become floats.
#[test]
fn legacy_strings_spelling_nonfinite_tokens_are_not_rewritten() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let oracle_fixture = Fixture::with_positive_primary();
    oracle_fixture.checkout_new("topic");

    let cases = [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBD",
            "\"NaN\"",
            "\"Infinity\"",
            "op(NaN): Infinity [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBE",
            "{\"k\": \"NaN\"}",
            "\"-Infinity\"",
            "op({'k': 'NaN'}): -Infinity [01ARZ3ND]",
        ),
    ];
    for (invocation_id, profile, action, expected) in cases {
        let line = started_row(invocation_id, profile, action);
        assert_legacy_commit(&fixture, invocation_id, &line, expected);
        assert_eq!(
            oracle_legacy_commit(&oracle_fixture, invocation_id, &line),
            expected
        );
        assert_eq!(fixture.subject(), expected);
    }
}

/// Decoded duplicate keys, non-ASCII chunks and escaped controls share the
/// same first-object reader; unused valid/invalid fields are not skipped.
#[test]
fn legacy_decoded_duplicates_escapes_and_unused_fields_match_source() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = concat!(
        r#"{"profile_id":"replaced","profile_\u0069d":{"\u0062":1,"a":2,"b":3},"action":["é","\ud83d\ude00","a\nb"],"unused":NaN}"#,
        "\n"
    );
    let expected = "op({'b': 3, 'a': 2}): ['é', '😀', 'a\\nb'] [01ARZ3ND]";
    assert_eq!(oracle_legacy_commit(&source, RECORD, line), expected);
    assert_legacy_commit(&fixture, RECORD, line, expected);

    let bad = "{\"profile_id\":\"p\",\"action\":\"a\",\"unused\":[1,]}\n";
    fixture.write_record_line(RECORD, bad);
    source.write_record_line(RECORD, bad);
    let head = fixture.head();
    let source_head = source.head();
    let reference = oracle_commit(&source.root, &source.root, RECORD, "agent", None);
    assert_eq!(reference["committed"], false);
    let mut service = fixture.service();
    let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
    assert!(!outcome.committed());
    assert!(
        outcome
            .refusal()
            .unwrap()
            .contains("Invalid invocation record")
    );
    assert_eq!(fixture.head(), head);
    assert_eq!(source.head(), source_head);
    assert_eq!(fixture.record_bytes(RECORD), bad.as_bytes());
    assert_eq!(source.record_bytes(RECORD), bad.as_bytes());
}

/// Exercise source-valid nesting in a subprocess so a reader stack failure
/// becomes an observable test failure, rather than losing the parent suite.
#[test]
fn legacy_source_valid_nesting_commits_without_losing_the_process() {
    const CHILD: &str = "SK_OP_LEGACY_NESTING_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let fixture = Fixture::with_positive_primary();
        fixture.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let depth = 800;
        let profile = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        let line = started_row(RECORD, &profile, "\"nested\"");
        let expected = format!("op({profile}): nested [01ARZ3ND]");
        // Establish source acceptance before attempting the native path.
        assert_eq!(oracle_legacy_commit(&source, RECORD, &line), expected);
        assert_legacy_commit(&fixture, RECORD, &line, &expected);
        assert_eq!(
            fixture.head(),
            source.head(),
            "fixed metadata binds the complete commit object"
        );
        for (profile, action) in [
            ("NaN", "\"constant\""),
            ("{\"b\":1,\"a\":2}", "{\"z\":3,\"y\":4}"),
            ("1e400", "-1e400"),
        ] {
            let line = started_row(RECORD, profile, action);
            let expected = oracle_legacy_commit(&source, RECORD, &line);
            assert_legacy_commit(&fixture, RECORD, &line, &expected);
            assert_eq!(
                fixture.head(),
                source.head(),
                "source/native commit OIDs: {profile}"
            );
            assert_eq!(
                fixture.git_ok(&["cat-file", "-p", "HEAD"]),
                source.git_ok(&["cat-file", "-p", "HEAD"]),
                "complete commit bytes: {profile}"
            );
        }
        return;
    }
    let scratch = tempfile::tempdir().expect("isolated child CWD");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "legacy_source_valid_nesting_commits_without_losing_the_process",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
        .current_dir(scratch.path())
        .output()
        .expect("run nested-row child");
    assert!(
        output.status.success(),
        "nested-row child failed: {:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// These are interpreter defaults, not a claim that every Python call stack
/// accepts a fixed JSON nesting depth.
#[test]
fn legacy_reference_default_resource_budgets_are_bound() {
    let facts = support::run_python(
        "import json,sys; print(json.dumps([sys.getrecursionlimit(),sys.get_int_max_str_digits()]))",
        None,
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&facts).unwrap(),
        json!([1000, 4300])
    );
}

#[test]
fn legacy_default_integer_digit_ceiling_matches_source_without_limiting_floats() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let digits = "7".repeat(4301);
    for line in [
        started_row(RECORD, &digits, "\"a\""),
        format!("{{\"profile_id\":\"p\",\"action\":\"a\",\"unused\":{digits}}}\n"),
    ] {
        fixture.write_record_line(RECORD, &line);
        source.write_record_line(RECORD, &line);
        let head = fixture.head();
        let source_head = source.head();
        let expected = oracle_commit(&source.root, &source.root, RECORD, "agent", None);
        assert_eq!(
            expected["committed"], false,
            "source rejects the oversized integer"
        );
        let mut service = fixture.service();
        let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
        assert!(
            !outcome.committed(),
            "native must not commit a source-refused integer: {outcome:?}"
        );
        assert!(outcome.refusal().is_some());
        assert_eq!(fixture.head(), head);
        assert_eq!(source.head(), source_head);
        assert_eq!(fixture.record_bytes(RECORD), line.as_bytes());
        assert_eq!(source.record_bytes(RECORD), line.as_bytes());
    }
    for number in [
        "7".repeat(4300),
        format!("-{}", "7".repeat(4300)),
        format!("{}.0", "7".repeat(4301)),
    ] {
        let line = started_row(RECORD, &number, "\"boundary\"");
        let expected = oracle_legacy_commit(&source, RECORD, &line);
        assert_legacy_commit(&fixture, RECORD, &line, &expected);
    }
}

/// Existing CPython startup configuration must not become a new native-only
/// refusal. All environment settings are scoped to a child process.
#[test]
fn legacy_integer_digit_environment_matches_source() {
    const CHILD: &str = "SK_OP_LEGACY_INTEGER_ENV_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let limit: usize = std::env::var("PYTHONINTMAXSTRDIGITS")
            .unwrap()
            .parse()
            .unwrap();
        let fixture = Fixture::with_positive_primary();
        fixture.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let digits = if limit == 0 { 4301 } else { limit };
        let line = started_row(RECORD, &"7".repeat(digits), "\"configured\"");
        let expected = oracle_legacy_commit(&source, RECORD, &line);
        assert_legacy_commit(&fixture, RECORD, &line, &expected);
        if limit != 0 {
            let line = started_row(RECORD, &"7".repeat(limit + 1), "\"too-long\"");
            fixture.write_record_line(RECORD, &line);
            source.write_record_line(RECORD, &line);
            let before = fixture.snapshot();
            assert_eq!(
                oracle_commit(&source.root, &source.root, RECORD, "agent", None)["committed"],
                false
            );
            let mut service = fixture.service();
            assert!(
                !service
                    .commit_op_record(&id(RECORD), OpClosedBy::Agent)
                    .committed()
            );
            assert_eq!(fixture.snapshot(), before);
        }
        return;
    }
    let cwd = tempfile::tempdir().unwrap();
    for value in ["0", "640"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "legacy_integer_digit_environment_matches_source",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("PYTHONINTMAXSTRDIGITS", value)
            .current_dir(cwd.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "configured integer child {value}: {:?}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The new reader must return an outcome, not lose the process on a row the
/// source rejects. The parent verifies HEAD/index/record bytes after the child.
#[test]
fn legacy_source_refused_deep_row_returns_without_aborting() {
    const CHILD_ROOT: &str = "SK_OP_LEGACY_REFUSAL_CHILD_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let mut service =
            OpCommitService::new(&root, InvocationWriter::new(&root)).with_hatch(None);
        let outcome = service.commit_op_record(&id(RECORD), OpClosedBy::Agent);
        assert!(
            !outcome.committed(),
            "source-refused deep row must not commit"
        );
        assert!(outcome.refusal().is_some());
        return;
    }
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let nested = format!("{}0{}", "[".repeat(10_000), "]".repeat(10_000));
    let line = format!("{{\"profile_id\":\"p\",\"action\":\"a\",\"unused\":{nested}}}\n");
    fixture.write_record_line(RECORD, &line);
    source.write_record_line(RECORD, &line);
    let before = fixture.snapshot();
    let source_head = source.head();
    let expected = oracle_commit(&source.root, &source.root, RECORD, "agent", None);
    assert_eq!(
        expected["committed"], false,
        "source must establish its refusal first"
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "legacy_source_refused_deep_row_returns_without_aborting",
            "--nocapture",
        ])
        .env(CHILD_ROOT, &fixture.root)
        .current_dir(fixture.temp.path())
        .output()
        .expect("run deep-row child");
    if !output.status.success() {
        eprintln!(
            "retained failing native fixture: {}",
            fixture.temp.path().display()
        );
        eprintln!("retained source fixture: {}", source.temp.path().display());
        let _native = fixture.temp.keep();
        let _source = source.temp.keep();
        panic!(
            "deep-row child must refuse cleanly: {:?}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(source.head(), source_head);
    assert_eq!(source.record_bytes(RECORD), line.as_bytes());
}

/// The looser legacy reader still validates the whole first JSON object:
/// trailing data, a top-level non-object, bad literals and unterminated
/// structures stay `Invalid invocation record: <id>` with no commit and no
/// byte change.
#[test]
fn legacy_invalid_rows_still_refuse_without_writing() {
    let fixture = Fixture::with_positive_primary();
    fixture.checkout_new("topic");
    let mut service = fixture.service();

    let cases = [
        ("01ARZ3NDEKTSV4RRFFQ69G5FBJ", "{\"profile_id\": NaN} x\n"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FBK", "{\"profile_id\": [1,]}\n"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FBM", "[NaN]\n"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FBN", "NaN\n"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FBP", "+Infinity\n"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FBR", "nan\n"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FBS", "{\"profile_id\": NaN\n"),
    ];
    for (invocation_id, line) in cases {
        fixture.write_record_line(invocation_id, line);
        let bytes = fixture.record_bytes(invocation_id);
        let head_before = fixture.head();
        let outcome = service.commit_op_record(&id(invocation_id), OpClosedBy::Agent);
        let expected = format!("Invalid invocation record: {invocation_id}");
        assert_eq!(
            outcome.refusal(),
            Some(expected.as_str()),
            "{invocation_id}"
        );
        assert!(!outcome.committed());
        assert_eq!(fixture.head(), head_before);
        assert_eq!(fixture.record_bytes(invocation_id), bytes);
    }

    // The pinned source also refuses these rows (only its `repr()` wording
    // differs, the already-disclosed F5 boundary).
    let oracle_fixture = Fixture::with_positive_primary();
    oracle_fixture.checkout_new("topic");
    for (invocation_id, line) in &cases {
        oracle_fixture.write_record_line(invocation_id, line);
        let head_before = oracle_fixture.head();
        let outcome = oracle_commit(
            &oracle_fixture.root,
            &oracle_fixture.root,
            invocation_id,
            "agent",
            None,
        );
        assert_eq!(
            outcome["committed"], false,
            "{invocation_id} source outcome: {outcome:?}"
        );
        assert_eq!(oracle_fixture.head(), head_before);
        assert_eq!(oracle_fixture.record_bytes(invocation_id), line.as_bytes());
    }
}
