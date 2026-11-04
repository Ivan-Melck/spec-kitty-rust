//! Byte-message bridge contracts for the real `OpCommitService`.
//!
//! Each test runs inside an isolated child process with its own temporary
//! CWD, `HOME`, every `XDG_*` root, `TMPDIR`, Git config, and fixed
//! author/committer metadata. The child supplies one explicit startup locale /
//! `PYTHONUTF8` context; the native service captures that context and the live
//! pinned interpreter (`support::run_python`) inherits exactly the same one, so
//! `os.fsencode` and the native encoder are compared under identical inputs.
//!
//! Source ground truth measured before implementation (pinned CPython 3.11.15,
//! Git 2.43): `os.fsencode` maps `\udc80..\udcff` to the single bytes
//! `0x80..0xff` (surrogateescape) in every supported context and refuses every
//! other surrogate; `git commit -m` then converts those raw bytes from the
//! locale charset into the commit object's UTF-8 body while the reflog keeps the
//! pre-conversion bytes. `git commit-tree -F` (the accepted native byte route)
//! applies the same conversion, so the complete commit objects are directly
//! comparable; `i18n.commitEncoding` is Git's own decision on both sides.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;
use sk_invocation::{
    ClosedBy, InvocationId, InvocationWriter, OP_CLOSURES_FILENAME, OpClosedBy, OpCommitService,
    OpCompletedEvent, OpCompletedFields, OpOutcome, append_op_closure, op_git_options,
};

const SEED: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const ARCHIVE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
/// UTF-8 locale with the pinned interpreter's UTF-8 mode explicitly disabled.
const UTF8_MODE_ZERO: &[(&str, &str)] = &[
    ("LC_ALL", "C.UTF-8"),
    ("LC_CTYPE", "C.UTF-8"),
    ("LANG", "C.UTF-8"),
    ("PYTHONUTF8", "0"),
];
/// The C locale with UTF-8 mode disabled and C-locale coercion disabled, so
/// `sys.getfilesystemencoding()` really is `ascii`.
const FORCED_ASCII: &[(&str, &str)] = &[
    ("LC_ALL", "C"),
    ("LC_CTYPE", "C"),
    ("LANG", "C"),
    ("PYTHONUTF8", "0"),
    ("PYTHONCOERCECLOCALE", "0"),
];
/// The C locale through the interpreter's automatic UTF-8 mode.
const UTF8_MODE_ONE: &[(&str, &str)] = &[
    ("LC_ALL", "C"),
    ("LC_CTYPE", "C"),
    ("LANG", "C"),
    ("PYTHONUTF8", "1"),
];

const FIXED_METADATA: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Test Author"),
    ("GIT_AUTHOR_EMAIL", "test@example.com"),
    ("GIT_COMMITTER_NAME", "Test Committer"),
    ("GIT_COMMITTER_EMAIL", "test@example.com"),
    ("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000"),
    ("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000"),
];

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
    "GIT_INDEX_FILE",
];

const ENVIRONMENT_VARIABLES: &[&str] = &[
    "LC_ALL",
    "LC_CTYPE",
    "LANG",
    "LANGUAGE",
    "PYTHONUTF8",
    "PYTHONCOERCECLOCALE",
    "PYTHONIOENCODING",
    "LOCPATH",
    "LOCALE_ALIAS_PATH",
    "SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS",
];

/// Run `test_name` in an isolated child with `locale`, or report that this
/// process *is* that child.
fn child(test_name: &str, guard: &str, locale: &[(&str, &str)]) -> bool {
    child_with_os_env(test_name, guard, locale, &[])
}

fn child_with_os_env(
    test_name: &str,
    guard: &str,
    locale: &[(&str, &str)],
    extra: &[(&str, std::ffi::OsString)],
) -> bool {
    if std::env::var_os(guard).is_some() {
        return true;
    }
    let scratch = tempfile::tempdir().expect("isolated child CWD");
    let root = scratch.path();
    for directory in [
        "home",
        "cache",
        "config",
        "data",
        "state",
        "tmp",
        "git-config",
        "cwd",
    ] {
        fs::create_dir_all(root.join(directory)).expect("create isolated root");
    }
    let mut command = Command::new(std::env::current_exe().expect("current test binary"));
    command
        .args(["--exact", test_name, "--nocapture"])
        .current_dir(root.join("cwd"))
        .env("HOME", root.join("home"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TMPDIR", root.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            root.join("git-config").join("gitconfig"),
        )
        .env(guard, "1");
    for name in ENVIRONMENT_VARIABLES.iter().chain(GIT_SELECTORS) {
        command.env_remove(name);
    }
    for (name, value) in locale {
        command.env(name, value);
    }
    for (name, value) in FIXED_METADATA {
        command.env(name, value);
    }
    for (name, value) in extra {
        command.env(name, value);
    }
    let output = command.output().expect("run the isolated child");
    assert!(
        output.status.success(),
        "{test_name} child failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Surface the child's measured source codec for the evidence log; the
    // assertions already ran inside the child.
    print!("{}", String::from_utf8_lossy(&output.stdout));
    false
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
}

impl Fixture {
    fn new() -> Self {
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
            _temp: temp,
        };
        fixture.git_ok(&["init", "-q", "-b", "main"]);
        fixture.git_ok(&["config", "user.name", "Fixture"]);
        fixture.git_ok(&["config", "user.email", "fixture@example.invalid"]);
        fixture.git_ok(&["config", "commit.gpgsign", "false"]);
        fixture.write("README.md", "# fixture\n");
        fixture.git_ok(&["add", "README.md"]);
        fixture.git_ok(&["commit", "-q", "-m", "initial"]);
        fixture
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
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"));
        for variable in GIT_SELECTORS {
            command.env_remove(variable);
        }
        command
    }

    fn git_at(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd).args(args).output().expect("run git")
    }

    fn git_ok(&self, args: &[&str]) -> Vec<u8> {
        let output = self.git_at(&self.root, args);
        assert!(
            output.status.success(),
            "git {args:?} failed in {}: {}",
            self.root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn write(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&path, content).expect("write fixture file");
        path
    }

    fn write_record_line(&self, invocation_id: &str, line: &str) {
        self.write(&format!("kitty-ops/{invocation_id}.jsonl"), line.as_bytes());
    }

    fn record_bytes(&self, invocation_id: &str) -> Vec<u8> {
        fs::read(
            self.root
                .join("kitty-ops")
                .join(format!("{invocation_id}.jsonl")),
        )
        .expect("read record bytes")
    }

    /// A primary checkout whose `main` tracks one ULID-shaped Op record; the
    /// caller checks out a normal topic branch afterwards.
    fn with_positive_primary() -> Self {
        let fixture = Self::new();
        fixture.write(
            &format!("kitty-ops/{SEED}.jsonl"),
            "{\"event\": \"started\"}\n",
        );
        fixture.git_ok(&["add", "--force", &format!("kitty-ops/{SEED}.jsonl")]);
        fixture.git_ok(&["commit", "-q", "-m", "seed record"]);
        fixture
    }

    fn checkout_new(&self, branch: &str) {
        self.git_ok(&["checkout", "-q", "-b", branch]);
    }

    fn service(&self) -> OpCommitService {
        OpCommitService::with_git_options(
            &self.root,
            InvocationWriter::new(&self.root),
            &op_git_options(),
        )
        .with_hatch(None)
    }

    fn head(&self) -> String {
        String::from_utf8_lossy(&self.git_ok(&["rev-parse", "--verify", "HEAD"]))
            .trim()
            .to_owned()
    }

    fn branch(&self) -> String {
        String::from_utf8_lossy(&self.git_ok(&["symbolic-ref", "--quiet", "HEAD"]))
            .trim()
            .to_owned()
    }

    /// The complete raw commit object (`cat-file commit`), not a re-encoded
    /// `log` view.
    fn commit_object(&self) -> Vec<u8> {
        self.git_ok(&["cat-file", "commit", "HEAD"])
    }

    /// The commit message payload exactly as stored in the object body.
    fn message_bytes(&self) -> Vec<u8> {
        let object = self.commit_object();
        let split = object
            .windows(2)
            .position(|window| window == b"\n\n")
            .expect("a commit object separates headers and body");
        object[split + 2..].to_vec()
    }

    fn commit_paths(&self) -> Vec<String> {
        let raw = self.git_ok(&["show", "--name-only", "--format=", "--no-renames", "HEAD"]);
        String::from_utf8_lossy(&raw)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn status(&self) -> String {
        String::from_utf8_lossy(&self.git_ok(&["status", "--porcelain"])).into_owned()
    }

    fn staged(&self) -> String {
        String::from_utf8_lossy(&self.git_ok(&["diff", "--cached", "--name-only"])).into_owned()
    }

    /// The last reflog entry's message, before any Git message conversion.
    fn reflog_message(&self, branch: &str) -> Vec<u8> {
        let path = self.root.join(".git/logs/refs/heads").join(branch);
        let log = fs::read(path).expect("read the branch reflog");
        let last = log
            .split(|byte| *byte == b'\n')
            .rfind(|line| !line.is_empty())
            .expect("one reflog line");
        let tab = last
            .iter()
            .position(|byte| *byte == b'\t')
            .expect("a reflog line carries a message");
        last[tab + 1..].to_vec()
    }

    fn git_path(&self, name: &str) -> PathBuf {
        let raw = self.git_ok(&["rev-parse", "--git-path", name]);
        let spelling = PathBuf::from(String::from_utf8_lossy(&raw).trim_end_matches('\n'));
        if spelling.is_absolute() {
            spelling
        } else {
            self.root.join(spelling)
        }
    }

    fn artifact_bytes(&self) -> Vec<u8> {
        fs::read(self.git_path("COMMIT_EDITMSG")).expect("read COMMIT_EDITMSG")
    }

    fn hook_log(&self, name: &str) -> Vec<u8> {
        fs::read(self.git_path(name)).unwrap_or_default()
    }

    fn install_hook(&self, name: &str, body: &str) {
        let path = self.git_path("hooks").join(name);
        fs::create_dir_all(path.parent().unwrap()).expect("create hooks directory");
        fs::write(&path, format!("#!/bin/sh\n{body}\nexit 0\n")).expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod hook");
        }
    }
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

/// The live pinned `_commit_op_record`, in the child's own startup context.
fn oracle_commit(project: &Path, invocation_id: &str, closed_by: &str) -> serde_json::Value {
    let script = r#"
import json, sys
from pathlib import Path
from specify_cli.invocation.executor import ProfileInvocationExecutor
request = json.load(sys.stdin)
executor = ProfileInvocationExecutor(Path(request["project"]), record_root=Path(request["project"]))
outcome = executor._commit_op_record(request["id"], closed_by=request["closed_by"])
print(json.dumps({"committed": outcome.committed, "refusal": outcome.refusal, "skipped": outcome.skipped}))
"#;
    let request = json!({
        "project": project,
        "id": invocation_id,
        "closed_by": closed_by,
    });
    let output = support::run_python(script, Some(&request.to_string()));
    serde_json::from_str(&output).expect("commit oracle JSON")
}

fn started_line(invocation_id: &str, profile: &str, action: &str) -> String {
    format!(
        "{{\"event\": \"started\", \"invocation_id\": \"{invocation_id}\", \"profile_id\": {profile}, \"action\": {action}}}\n"
    )
}

/// Write the same durable row on both sides and return it.
fn write_pair(native: &Fixture, source: &Fixture, invocation_id: &str, line: &str) -> Vec<u8> {
    native.write_record_line(invocation_id, line);
    source.write_record_line(invocation_id, line);
    let bytes = native.record_bytes(invocation_id);
    assert_eq!(source.record_bytes(invocation_id), bytes);
    bytes
}

fn native_commit(native: &Fixture, invocation_id: &str, closed_by: OpClosedBy) -> String {
    let mut service = native.service();
    let outcome = service.commit_op_record(&id(invocation_id), closed_by);
    assert!(
        outcome.committed(),
        "{invocation_id}: native must commit through the real service: {outcome:?}"
    );
    assert_eq!(outcome.refusal(), None);
    assert_eq!(outcome.skipped(), None);
    assert_eq!(service.last_op_commit(), Some(&outcome));
    let evidence = outcome.evidence().expect("typed commit evidence");
    match evidence {
        sk_invocation::OpCommitEvidence::Committed { commit, .. } => {
            let landed = commit.to_string();
            assert_eq!(landed, native.head());
            landed
        }
        other => panic!("{invocation_id}: unexpected evidence {other:?}"),
    }
}

fn assert_source_committed(reference: &serde_json::Value, invocation_id: &str) {
    assert_eq!(
        reference["committed"], true,
        "{invocation_id}: the pinned source commits this row: {reference:?}"
    );
    assert_eq!(reference["refusal"], serde_json::Value::Null);
    assert_eq!(reference["skipped"], serde_json::Value::Null);
}

/// The shared controls every committed row must keep.
fn assert_commit_shape(native: &Fixture, source: &Fixture, invocation_id: &str, branch: &str) {
    let record_path = format!("kitty-ops/{invocation_id}.jsonl");
    assert_eq!(native.branch(), "refs/heads/topic");
    assert_eq!(source.branch(), "refs/heads/topic");
    assert_eq!(native.commit_paths(), vec![record_path.clone()]);
    assert_eq!(source.commit_paths(), vec![record_path]);
    assert_eq!(native.status(), "");
    assert_eq!(source.status(), "");
    assert_eq!(native.staged(), "");
    assert_eq!(source.staged(), "");
    assert_eq!(
        native.head(),
        source.head(),
        "{invocation_id}: equal bound metadata and payload bind the commit id"
    );
    assert_eq!(
        native.commit_object(),
        source.commit_object(),
        "{invocation_id}: complete commit object bytes"
    );
    assert_eq!(
        native.reflog_message(branch),
        source.reflog_message(branch),
        "{invocation_id}: the pre-conversion payload bytes are identical"
    );
}

/// The live source's actual `sys.getfilesystemencoding()` in this child.
fn source_filesystem_encoding() -> String {
    support::run_python("import sys; print(sys.getfilesystemencoding())", None)
}

/// Run the pinned source oracle and the real native service on the same row.
///
/// The host may not have a locale installed, in which case the source falls
/// back to its own startup default; only the *pair* is required to agree, and
/// both business calls still run. When the source commits, the full object and
/// reflog equality controls apply; when it refuses, the native side must refuse
/// with a truthful capability/encoding failure and leave the tip untouched.
fn commit_pair(native: &Fixture, source: &Fixture, invocation_id: &str) -> bool {
    let native_head = native.head();
    let source_head = source.head();
    let reference = oracle_commit(&source.root, invocation_id, "agent");
    let source_committed = reference["committed"].as_bool() == Some(true);
    let mut service = native.service();
    let outcome = service.commit_op_record(&id(invocation_id), OpClosedBy::Agent);
    assert_eq!(
        outcome.committed(),
        source_committed,
        "{invocation_id}: source verdict {reference:?} vs native {outcome:?}"
    );
    if source_committed {
        assert_eq!(reference["refusal"], serde_json::Value::Null);
        assert_eq!(reference["skipped"], serde_json::Value::Null);
        assert_eq!(outcome.refusal(), None);
        assert_eq!(outcome.skipped(), None);
        assert_eq!(service.last_op_commit(), Some(&outcome));
        let evidence = outcome.evidence().expect("typed commit evidence");
        match evidence {
            sk_invocation::OpCommitEvidence::Committed { commit, .. } => {
                assert_eq!(commit.to_string(), native.head());
            }
            other => panic!("{invocation_id}: unexpected evidence {other:?}"),
        }
        assert_commit_shape(native, source, invocation_id, "topic");
    } else {
        assert!(
            reference["refusal"].as_str().is_some(),
            "{invocation_id}: the source refusal is recorded: {reference:?}"
        );
        let refusal = outcome
            .refusal()
            .expect("a truthful native refusal when the source refuses");
        assert!(
            !refusal.contains("Invalid invocation record"),
            "{invocation_id}: the row is valid; got {refusal:?}"
        );
        assert_eq!(native.head(), native_head, "{invocation_id}: nothing lands");
        assert_eq!(
            source.head(),
            source_head,
            "{invocation_id}: source lands nothing"
        );
    }
    source_committed
}

#[cfg(unix)]
#[test]
fn nontext_coercion_flag_does_not_block_a_resolved_utf8_locale() {
    use std::os::unix::ffi::OsStringExt;
    const NAME: &str = "nontext_coercion_flag_does_not_block_a_resolved_utf8_locale";
    if !child_with_os_env(
        NAME,
        "SK_OP_BYTE_RAW_COERCION_FLAG_CHILD",
        UTF8_MODE_ZERO,
        &[(
            "PYTHONCOERCECLOCALE",
            std::ffi::OsString::from_vec(vec![0xff]),
        )],
    ) {
        return;
    }
    assert_eq!(
        support::run_python("import sys; print(sys.getfilesystemencoding())", None),
        "utf-8"
    );
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FCV";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\u00e9\udc80""#, r#""a""#);
    write_pair(&native, &source, invocation_id, &line);
    assert!(commit_pair(&native, &source, invocation_id));
}

#[test]
fn unavailable_locale_uses_real_c_fallback_and_auto_utf8() {
    const NAME: &str = "unavailable_locale_uses_real_c_fallback_and_auto_utf8";
    if !child(
        NAME,
        "SK_OP_BYTE_BAD_LOCALE_AUTO_CHILD",
        &[("LC_ALL", "xx_YY.BOGUS-8")],
    ) {
        return;
    }
    // No real installation can carry this name: the failed libc lookup leaves
    // the actual C locale, whose state enables PEP 540 auto UTF-8 regardless
    // of the text after the dot.
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    assert_eq!(codec, "utf-8");
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FCA";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\u00e9""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);
    assert!(
        commit_pair(&native, &source, invocation_id),
        "{invocation_id}: the source commits in the real C fallback"
    );
    assert_eq!(
        native.reflog_message("topic"),
        b"commit: op(\xC3\xA9): a [01ARZ3ND]".to_vec()
    );
    assert_eq!(
        native.message_bytes(),
        b"op(\xC3\xA9): a [01ARZ3ND]\n".to_vec()
    );
    assert_eq!(native.record_bytes(invocation_id), record);
}

#[test]
fn unavailable_utf8_named_locale_with_mode_zero_is_ascii_not_its_suffix() {
    const NAME: &str = "unavailable_utf8_named_locale_with_mode_zero_is_ascii_not_its_suffix";
    if !child(
        NAME,
        "SK_OP_BYTE_BAD_LOCALE_ZERO_CHILD",
        &[
            ("LC_ALL", "xx_YY.UTF-8"),
            ("PYTHONUTF8", "0"),
            ("PYTHONCOERCECLOCALE", "0"),
        ],
    ) {
        return;
    }
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    assert_eq!(
        codec, "ascii",
        "an unavailable UTF-8 name must not become UTF-8"
    );
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FCB";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\u00e9""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);
    let native_head = native.head();
    let reference = oracle_commit(&source.root, invocation_id, "agent");
    assert_eq!(reference["committed"], false, "{reference:?}");
    assert!(
        reference["refusal"]
            .as_str()
            .is_some_and(|reason| reason.contains("UnicodeEncodeError")),
        "{invocation_id}: {reference:?}"
    );
    let outcome = native
        .service()
        .commit_op_record(&id(invocation_id), OpClosedBy::Agent);
    assert!(
        !outcome.committed(),
        "native must not guess UTF-8 from an unavailable locale suffix: {outcome:?}"
    );
    let refusal = outcome.refusal().expect("a truthful native refusal");
    assert!(
        refusal.contains("U+00E9") && refusal.contains("not encodable"),
        "{invocation_id}: the real ASCII codec refuses the scalar: {refusal:?}"
    );
    assert_eq!(native.head(), native_head);
    assert_eq!(native.record_bytes(invocation_id), record);
}

#[test]
fn installed_locale_without_codeset_keeps_ascii_payloads_publishable() {
    const NAME: &str = "installed_locale_without_codeset_keeps_ascii_payloads_publishable";
    if !child(
        NAME,
        "SK_OP_BYTE_LOCALE_NO_CODESET_CHILD",
        &[("LC_ALL", "en_US")],
    ) {
        return;
    }
    // On this host `en_US` is installed with an actual ISO-8859-1 `LC_CTYPE`;
    // an ASCII payload is publishable in that codec. A host without the locale
    // resolves the failed lookup to its own C fallback, and the pair must still
    // agree instead of asserting installed host data universally.
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FCA";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""implementer""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);
    assert!(
        commit_pair(&native, &source, invocation_id),
        "{invocation_id}: an ASCII payload stays publishable in the actual codec ({codec})"
    );
    assert_eq!(
        native.message_bytes(),
        b"op(implementer): a [01ARZ3ND]\n".to_vec()
    );
    assert_eq!(source.message_bytes(), native.message_bytes());
    assert_eq!(native.record_bytes(invocation_id), record);
}

#[test]
fn installed_latin1_locale_matches_real_payload_and_reflog_bytes() {
    const NAME: &str = "installed_latin1_locale_matches_real_payload_and_reflog_bytes";
    if !child(
        NAME,
        "SK_OP_BYTE_LOCALE_LATIN1_CHILD",
        &[("LC_ALL", "en_US.ISO-8859-1")],
    ) {
        return;
    }
    // This host's actual `en_US` metadata is ISO-8859-1, so the requested
    // `en_US.ISO-8859-1` resolves to the base locale's Latin-1 data. If a host
    // has no such locale, both sides take the same C fallback and the full
    // pair still runs; only the byte-level assertions depend on the codec.
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FCA";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\u00e9\udc80""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);
    assert!(
        commit_pair(&native, &source, invocation_id),
        "{invocation_id}: the source commits this payload in the actual codec ({codec})"
    );
    match codec.as_str() {
        "iso8859-1" => {
            assert_eq!(
                native.reflog_message("topic"),
                b"commit: op(\xE9\x80): a [01ARZ3ND]".to_vec(),
                "Latin-1 maps U+00E9 and the escaped byte directly"
            );
            assert_eq!(
                native.message_bytes(),
                b"op(\xC3\xA9\xC2\x80): a [01ARZ3ND]\n".to_vec(),
                "Git converts the Latin-1 payload, not the Op layer"
            );
        }
        "utf-8" => {
            assert_eq!(
                native.reflog_message("topic"),
                b"commit: op(\xC3\xA9\x80): a [01ARZ3ND]".to_vec()
            );
            assert_eq!(
                native.message_bytes(),
                b"op(\xC3\xA9\xC2\x80): a [01ARZ3ND]\n".to_vec()
            );
        }
        _ => {}
    }
    assert_eq!(
        source.reflog_message("topic"),
        native.reflog_message("topic")
    );
    assert_eq!(source.message_bytes(), native.message_bytes());
    assert_eq!(native.record_bytes(invocation_id), record);
}

#[test]
fn utf8_mode_override_wins_before_an_unavailable_locale() {
    const NAME: &str = "utf8_mode_override_wins_before_an_unavailable_locale";
    if !child(
        NAME,
        "SK_OP_BYTE_LOCALE_OVERRIDE_CHILD",
        &[("LC_ALL", "xx_YY.BOGUS-8"), ("PYTHONUTF8", "1")],
    ) {
        return;
    }
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    assert_eq!(codec, "utf-8", "PYTHONUTF8=1 wins before the locale");
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FCV";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\u00e9\udc80""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);
    assert!(commit_pair(&native, &source, invocation_id));
    assert_eq!(
        native.reflog_message("topic"),
        b"commit: op(\xC3\xA9\x80): a [01ARZ3ND]".to_vec()
    );
    assert_eq!(native.record_bytes(invocation_id), record);
}

#[test]
fn unavailable_locale_coercion_matches_the_installed_target() {
    const NAME: &str = "unavailable_locale_coercion_matches_the_installed_target";
    if !child(
        NAME,
        "SK_OP_BYTE_LOCALE_COERCION_CHILD",
        &[("LC_CTYPE", "xx_YY.BOGUS-8"), ("PYTHONUTF8", "0")],
    ) {
        return;
    }
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    assert!(
        matches!(codec.as_str(), "utf-8" | "ascii"),
        "mode 0 either coerces to an installed C.UTF-8 or stays ASCII: {codec}"
    );

    // ASCII and escaped bytes are publishable in both outcomes.
    let escape_id = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(escape_id, r#""p""#, r#""\udc80""#);
    let record = write_pair(&native, &source, escape_id, &line);
    assert!(commit_pair(&native, &source, escape_id));
    assert_eq!(
        native.reflog_message("topic"),
        b"commit: op(p): \x80 [01ARZ3ND]".to_vec()
    );
    assert_eq!(native.record_bytes(escape_id), record);

    // The non-ASCII outcome follows the source's actual coercion result.
    let scalar_id = "01ARZ3NDEKTSV4RRFFQ69G5FD1";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(scalar_id, r#""\u00e9""#, r#""a""#);
    let record = write_pair(&native, &source, scalar_id, &line);
    let committed = commit_pair(&native, &source, scalar_id);
    assert_eq!(
        committed,
        codec == "utf-8",
        "{scalar_id}: the native coercion target agrees with the source ({codec})"
    );
    if committed {
        assert_eq!(
            native.reflog_message("topic"),
            b"commit: op(\xC3\xA9): a [01ARZ3ND]".to_vec()
        );
    }
    assert_eq!(native.record_bytes(scalar_id), record);
}

#[test]
fn ascii_transparent_locale_keeps_ascii_and_escape_payloads() {
    const NAME: &str = "ascii_transparent_locale_keeps_ascii_and_escape_payloads";
    if !child(
        NAME,
        "SK_OP_BYTE_LOCALE_ASCII_TRANSPARENT_CHILD",
        &[("LC_ALL", "ja_JP.EUC-JP")],
    ) {
        return;
    }
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");

    // A legacy charset this adapter does not fully implement must not make an
    // ASCII row or an escaped byte unpublishable when the source publishes it.
    let ascii_id = "01ARZ3NDEKTSV4RRFFQ69G5FCW";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(ascii_id, r#""p""#, r#""a""#);
    let record = write_pair(&native, &source, ascii_id, &line);
    assert!(commit_pair(&native, &source, ascii_id));
    assert_eq!(native.message_bytes(), b"op(p): a [01ARZ3ND]\n".to_vec());
    assert_eq!(native.record_bytes(ascii_id), record);

    let escape_id = "01ARZ3NDEKTSV4RRFFQ69G5FCX";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(escape_id, r#""p""#, r#""\udc80""#);
    let record = write_pair(&native, &source, escape_id, &line);
    assert!(commit_pair(&native, &source, escape_id));
    assert_eq!(
        native.reflog_message("topic"),
        b"commit: op(p): \x80 [01ARZ3ND]".to_vec()
    );
    assert_eq!(
        native.message_bytes(),
        b"op(p): \xC2\x80 [01ARZ3ND]\n".to_vec()
    );
    assert_eq!(native.record_bytes(escape_id), record);

    // A non-ASCII scalar needs the real legacy codec. On this host the source
    // encodes it; the native side is an honest codec gap (not "invalid"). A
    // host without the locale has both sides in the C fallback and must agree.
    let scalar_id = "01ARZ3NDEKTSV4RRFFQ69G5FCY";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(scalar_id, r#""\u65e5""#, r#""a""#);
    let record = write_pair(&native, &source, scalar_id, &line);
    if codec == "euc_jp" {
        let native_head = native.head();
        let reference = oracle_commit(&source.root, scalar_id, "agent");
        assert_eq!(reference["committed"], true, "{reference:?}");
        let outcome = native
            .service()
            .commit_op_record(&id(scalar_id), OpClosedBy::Agent);
        assert!(!outcome.committed(), "{outcome:?}");
        let refusal = outcome.refusal().expect("a truthful native gap");
        assert!(
            refusal.contains("EUC-JP") && refusal.contains("U+65E5"),
            "{scalar_id}: the real resolved charset is named: {refusal:?}"
        );
        assert!(
            !refusal.contains("Invalid invocation record"),
            "{scalar_id}: the row is valid; got {refusal:?}"
        );
        assert_eq!(native.head(), native_head);
    } else {
        let committed = commit_pair(&native, &source, scalar_id);
        if committed {
            assert_eq!(source.message_bytes(), native.message_bytes());
        }
    }
    assert_eq!(native.record_bytes(scalar_id), record);
}

#[test]
fn locpath_fixture_resolves_real_ctype_files_like_the_source() {
    const NAME: &str = "locpath_fixture_resolves_real_ctype_files_like_the_source";
    // Copy this host's actual `C.utf8` `LC_CTYPE` data into a temporary
    // LOCPATH tree under a name the default locale root does not install.
    let fixture = tempfile::tempdir().expect("LOCPATH fixture root");
    let directory = fixture.path().join("zz_ZZ");
    fs::create_dir_all(&directory).expect("create fixture locale directory");
    let host = [
        "/usr/lib/locale/C.utf8/LC_CTYPE",
        "/usr/lib/locale/C.UTF-8/LC_CTYPE",
    ]
    .iter()
    .map(Path::new)
    .find(|path| path.is_file())
    .expect("this glibc host has actual C.utf8 LC_CTYPE data");
    fs::copy(host, directory.join("LC_CTYPE")).expect("copy actual LC_CTYPE data");
    let locpath = fixture.path().to_str().expect("UTF-8 fixture path");
    if !child(
        NAME,
        "SK_OP_BYTE_LOCALE_LOCPATH_CHILD",
        &[
            ("LOCPATH", locpath),
            ("LC_ALL", "zz_ZZ.UTF-8"),
            ("PYTHONUTF8", "0"),
            ("PYTHONCOERCECLOCALE", "0"),
        ],
    ) {
        return;
    }
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    assert_eq!(
        codec, "utf-8",
        "LOCPATH is searched and the normalized UTF-8 suffix matches the actual codeset"
    );
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FD2";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\u00e9\udc80""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);
    assert!(commit_pair(&native, &source, invocation_id));
    assert_eq!(
        native.reflog_message("topic"),
        b"commit: op(\xC3\xA9\x80): a [01ARZ3ND]".to_vec()
    );
    assert_eq!(native.record_bytes(invocation_id), record);
}

#[test]
fn latin15_locale_matches_the_source_for_non_ascii_payloads() {
    const NAME: &str = "latin15_locale_matches_the_source_for_non_ascii_payloads";
    if !child(
        NAME,
        "SK_OP_BYTE_LATIN15_CHILD",
        &[("LC_ALL", "de_DE@euro"), ("PYTHONUTF8", "0")],
    ) {
        return;
    }
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    for (invocation_id, profile) in [
        ("01ARZ3NDEKTSV4RRFFQ69G5FE0", r#""\u00e9""#),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FE1",
            r#""\u20ac\u0160\u0161\u017d\u017e\u0152\u0153\u0178\udca4""#,
        ),
    ] {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let line = started_line(invocation_id, profile, r#""a""#);
        let record = write_pair(&native, &source, invocation_id, &line);
        let committed = commit_pair(&native, &source, invocation_id);
        if codec == "iso8859-15" {
            assert!(
                committed,
                "the source's actual Latin-15 codec publishes these scalars"
            );
            if invocation_id.ends_with("FE1") {
                assert_eq!(
                    native.reflog_message("topic"),
                    b"commit: op(\xa4\xa6\xa8\xb4\xb8\xbc\xbd\xbe\xa4): a [01ARZ3ND]"
                );
            }
        }
        assert_eq!(native.record_bytes(invocation_id), record);
    }
    // The eight displaced Latin-1 scalars are not representable in Latin-15.
    for (index, scalar) in [
        0x00a4, 0x00a6, 0x00a8, 0x00b4, 0x00b8, 0x00bc, 0x00bd, 0x00be,
    ]
    .into_iter()
    .enumerate()
    {
        let invocation_id = format!("01ARZ3NDEKTSV4RRFFQ69G5FE{}", index + 2);
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let profile = format!("\"\\u{scalar:04x}\"");
        write_pair(
            &native,
            &source,
            &invocation_id,
            &started_line(&invocation_id, &profile, r#""a""#),
        );
        let committed = commit_pair(&native, &source, &invocation_id);
        if codec == "iso8859-15" {
            assert!(!committed);
        }
    }
}

#[test]
fn locale_alias_resolves_to_the_installed_codeset() {
    const NAME: &str = "locale_alias_resolves_to_the_installed_codeset";
    if !child(
        NAME,
        "SK_OP_BYTE_LOCALE_ALIAS_CHILD",
        &[("LC_ALL", "german"), ("PYTHONUTF8", "0")],
    ) {
        return;
    }
    // This glibc's `locale.alias` maps `german` to `de_DE.ISO-8859-1`, which
    // resolves to the installed `de_DE` Latin-1 metadata. The locale is pinned
    // by LC_ALL, so a host without the alias stays in the real C/ASCII state;
    // another distro's alias may select a different real codec, so only the
    // pair equality is universal and the measured codec drives the bytes.
    let codec = source_filesystem_encoding();
    println!("{NAME}: source filesystem encoding = {codec}");
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FD3";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\u00e9""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);
    let committed = commit_pair(&native, &source, invocation_id);
    if codec == "iso8859-1" {
        assert!(
            committed,
            "{invocation_id}: the alias-resolved Latin-1 codec commits like the source"
        );
        assert_eq!(
            native.reflog_message("topic"),
            b"commit: op(\xE9): a [01ARZ3ND]".to_vec()
        );
    }
    assert_eq!(native.record_bytes(invocation_id), record);
}

// ---------------------------------------------------------------------------
// UTF-8 startup context: the bridge must commit used low-surrogate payloads.
// ---------------------------------------------------------------------------

/// One committed-row case: `.jsonl` id, profile JSON, action JSON, the commit
/// object body, and the reflog message.
type ByteCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [u8],
    &'static [u8],
);

#[test]
fn utf8_mode_zero_commits_used_low_surrogate_bytes() {
    const GUARD: &str = "SK_OP_BYTE_UTF8_MODE_ZERO_CHILD";
    const NAME: &str = "utf8_mode_zero_commits_used_low_surrogate_bytes";
    if !child(NAME, GUARD, UTF8_MODE_ZERO) {
        return;
    }

    // (row id, profile field, action field, object body, reflog)
    //
    // `git commit`/`git commit-tree` both convert the invalid-UTF-8 payload
    // bytes from the locale charset into the object body (0x80 -> U+0080,
    // 0xff -> U+00FF) while the reflog keeps the pre-conversion bytes. Both
    // sides share that Git behaviour, so the complete object is identical.
    let cases: [ByteCase; 5] = [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB3",
            r#""\udc80""#,
            r#""a""#,
            b"op(\xc2\x80): a [01ARZ3ND]\n",
            b"commit: op(\x80): a [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB4",
            r#""p""#,
            r#""\udcff""#,
            b"op(p): \xc3\xbf [01ARZ3ND]\n",
            b"commit: op(p): \xff [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB5",
            r#""\udc80\udcff""#,
            r#""a""#,
            b"op(\xc2\x80\xc3\xbf): a [01ARZ3ND]\n",
            b"commit: op(\x80\xff): a [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB6",
            r#""\udc80\u00e9\ud83d\ude00""#,
            r#""a""#,
            b"op(\xc2\x80\xc3\xa9\xf0\x9f\x98\x80): a [01ARZ3ND]\n",
            b"commit: op(\x80\xc3\xa9\xf0\x9f\x98\x80): a [01ARZ3ND]",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB7",
            r#""\u00e9\ud83d\ude00""#,
            r#""\udc80""#,
            b"op(\xc3\xa9\xf0\x9f\x98\x80): \xc2\x80 [01ARZ3ND]\n",
            b"commit: op(\xc3\xa9\xf0\x9f\x98\x80): \x80 [01ARZ3ND]",
        ),
    ];
    for (invocation_id, profile, action, body, reflog) in cases {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let line = started_line(invocation_id, profile, action);
        let record = write_pair(&native, &source, invocation_id, &line);

        assert_source_committed(
            &oracle_commit(&source.root, invocation_id, "agent"),
            invocation_id,
        );
        native_commit(&native, invocation_id, OpClosedBy::Agent);

        assert_commit_shape(&native, &source, invocation_id, "topic");
        assert_eq!(
            native.reflog_message("topic"),
            reflog.to_vec(),
            "{invocation_id}: native reflog carries the pre-conversion bytes"
        );
        assert_eq!(
            source.reflog_message("topic"),
            reflog.to_vec(),
            "{invocation_id}: source reflog carries the pre-conversion bytes"
        );
        assert_eq!(
            native.message_bytes(),
            body.to_vec(),
            "{invocation_id}: native object body"
        );
        assert_eq!(
            source.message_bytes(),
            body.to_vec(),
            "{invocation_id}: source object body"
        );
        // The durable audit row is untouched and still the row both sides read.
        assert_eq!(native.record_bytes(invocation_id), record);
        assert_eq!(source.record_bytes(invocation_id), record);
    }
}

#[test]
fn utf8_mode_zero_binds_complete_objects_for_utf8_payloads() {
    const GUARD: &str = "SK_OP_BYTE_UTF8_OBJECT_CHILD";
    const NAME: &str = "utf8_mode_zero_binds_complete_objects_for_utf8_payloads";
    if !child(NAME, GUARD, UTF8_MODE_ZERO) {
        return;
    }
    let cases: [(&str, &str, &str); 3] = [
        ("01ARZ3NDEKTSV4RRFFQ69G5FB8", r#""\u00e9""#, r#""a""#),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FB9",
            r#""\ud83d\ude00""#,
            r#""\u00e9""#,
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBA",
            r#""\u00e9\ud83d\ude00\u00a0""#,
            r#""a""#,
        ),
    ];
    for (invocation_id, profile, action) in cases {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let line = started_line(invocation_id, profile, action);
        let record = write_pair(&native, &source, invocation_id, &line);

        assert_source_committed(
            &oracle_commit(&source.root, invocation_id, "agent"),
            invocation_id,
        );
        native_commit(&native, invocation_id, OpClosedBy::Agent);

        assert_commit_shape(&native, &source, invocation_id, "topic");
        assert_eq!(
            native.head(),
            source.head(),
            "{invocation_id}: equal metadata binds the complete commit object"
        );
        assert_eq!(
            native.commit_object(),
            source.commit_object(),
            "{invocation_id}: complete commit object bytes"
        );
        assert_eq!(native.record_bytes(invocation_id), record);
    }
}

#[test]
fn utf8_mode_one_commits_used_low_surrogate_bytes() {
    const GUARD: &str = "SK_OP_BYTE_UTF8_MODE_ONE_CHILD";
    const NAME: &str = "utf8_mode_one_commits_used_low_surrogate_bytes";
    if !child(NAME, GUARD, UTF8_MODE_ONE) {
        return;
    }
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FBB";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""\udc80""#, r#""\u00e9""#);
    let record = write_pair(&native, &source, invocation_id, &line);

    assert_source_committed(
        &oracle_commit(&source.root, invocation_id, "agent"),
        invocation_id,
    );
    native_commit(&native, invocation_id, OpClosedBy::Agent);

    assert_commit_shape(&native, &source, invocation_id, "topic");
    assert_eq!(
        native.reflog_message("topic"),
        b"commit: op(\x80): \xc3\xa9 [01ARZ3ND]".to_vec()
    );
    assert_eq!(
        native.message_bytes(),
        b"op(\xc2\x80): \xc3\xa9 [01ARZ3ND]\n"
    );
    assert_eq!(source.message_bytes(), native.message_bytes());
    assert_eq!(native.record_bytes(invocation_id), record);
}

// ---------------------------------------------------------------------------
// Forced-ASCII startup context: low escapes stay commit-capable, ordinary
// non-ASCII and astral payloads are truthfully unencodable.
// ---------------------------------------------------------------------------

#[test]
fn forced_ascii_context_matches_source_bytes_and_refusals() {
    const GUARD: &str = "SK_OP_BYTE_ASCII_CHILD";
    const NAME: &str = "forced_ascii_context_matches_source_bytes_and_refusals";
    if !child(NAME, GUARD, FORCED_ASCII) {
        return;
    }

    // Escaped bytes remain encodable (surrogateescape) even in ASCII; Git then
    // converts the raw payload in the object body on both sides.
    for (invocation_id, profile, action, body) in [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBC",
            r#""\udc80""#,
            r#""a""#,
            b"op(\xc2\x80): a [01ARZ3ND]\n".as_slice(),
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBD",
            r#""p""#,
            r#""\udcff""#,
            b"op(p): \xc3\xbf [01ARZ3ND]\n".as_slice(),
        ),
    ] {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let line = started_line(invocation_id, profile, action);
        let record = write_pair(&native, &source, invocation_id, &line);

        assert_source_committed(
            &oracle_commit(&source.root, invocation_id, "agent"),
            invocation_id,
        );
        native_commit(&native, invocation_id, OpClosedBy::Agent);

        assert_commit_shape(&native, &source, invocation_id, "topic");
        assert_eq!(native.message_bytes(), body.to_vec());
        assert_eq!(source.message_bytes(), body.to_vec());
        assert_eq!(native.record_bytes(invocation_id), record);
    }

    // Ordinary non-ASCII and astral scalars are unencodable in ASCII: the
    // source raises at its Popen boundary, the native encoder refuses late.
    for (invocation_id, profile, action, code) in [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBE",
            r#""\u00e9""#,
            r#""a""#,
            "U+00E9",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBF",
            r#""p""#,
            r#""\ud83d\ude00""#,
            "U+1F600",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBG",
            r#""a\u00a0b""#,
            r#""a""#,
            "U+00A0",
        ),
    ] {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let line = started_line(invocation_id, profile, action);
        let record = write_pair(&native, &source, invocation_id, &line);
        let head = native.head();
        let source_head = source.head();

        let reference = oracle_commit(&source.root, invocation_id, "agent");
        assert_eq!(
            reference["committed"], false,
            "{invocation_id}: the source refuses with UnicodeEncodeError: {reference:?}"
        );
        assert!(
            reference["refusal"]
                .as_str()
                .is_some_and(|reason| reason.contains("UnicodeEncodeError")),
            "{invocation_id}: {reference:?}"
        );
        assert_eq!(source.head(), source_head, "the source commits nothing");

        let mut service = native.service();
        let outcome = service.commit_op_record(&id(invocation_id), OpClosedBy::Agent);
        assert!(!outcome.committed(), "{invocation_id}: {outcome:?}");
        let refusal = outcome.refusal().expect("a truthful native refusal");
        assert!(
            refusal.contains(code) && refusal.contains("not encodable"),
            "{invocation_id}: names the unencodable code point, got {refusal:?}"
        );
        assert!(
            !refusal.contains("Invalid invocation record"),
            "{invocation_id}: the row is valid; got {refusal:?}"
        );
        assert_eq!(native.head(), head, "nothing landed");
        assert_eq!(native.record_bytes(invocation_id), record);
        // Measured footprint difference, NOT waived: the source stages the
        // record before its encoding failure; the native plan precedes staging.
        assert_eq!(
            source.staged(),
            format!("kitty-ops/{invocation_id}.jsonl\n"),
            "{invocation_id}: source stage-before-encoding footprint"
        );
        assert_eq!(native.staged(), "", "{invocation_id}: native adds nothing");
    }
}

// ---------------------------------------------------------------------------
// Precedence: protection refuses before message encoding, skip before branch.
// ---------------------------------------------------------------------------

#[test]
fn protection_refuses_before_message_encoding() {
    const GUARD: &str = "SK_OP_BYTE_PROTECTION_CHILD";
    const NAME: &str = "protection_refuses_before_message_encoding";
    if !child(NAME, GUARD, UTF8_MODE_ZERO) {
        return;
    }
    for (invocation_id, profile) in [
        ("01ARZ3NDEKTSV4RRFFQ69G5FBH", r#""\udc80""#),
        ("01ARZ3NDEKTSV4RRFFQ69G5FBJ", r#""\ud800""#),
    ] {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        protect(&native.root, "[topic]");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        protect(&source.root, "[topic]");
        let line = started_line(invocation_id, profile, r#""a""#);
        let record = write_pair(&native, &source, invocation_id, &line);
        let head = native.head();
        let source_head = source.head();
        // The protection configuration file itself is untracked in this fixture;
        // compare the whole read-only state before and after instead of "clean".
        let status = native.status();
        let staged = native.staged();

        let reference = oracle_commit(&source.root, invocation_id, "agent");
        assert_eq!(reference["committed"], false, "{reference:?}");
        assert!(
            reference["refusal"]
                .as_str()
                .is_some_and(|reason| reason.contains("protected branch")),
            "{invocation_id}: the source refuses on protection, not encoding: {reference:?}"
        );

        let mut service = native.service();
        let outcome = service.commit_op_record(&id(invocation_id), OpClosedBy::Agent);
        assert!(!outcome.committed(), "{invocation_id}: {outcome:?}");
        let refusal = outcome.refusal().expect("the guard's refusal");
        assert!(
            refusal.contains("protected branch"),
            "{invocation_id}: the canonical guard precedes encoding: {refusal:?}"
        );
        assert!(
            refusal.contains("topic"),
            "{invocation_id}: the refusal names the destination: {refusal:?}"
        );
        assert_eq!(native.head(), head);
        assert_eq!(source.head(), source_head);
        assert_eq!(native.status(), status);
        assert_eq!(native.staged(), staged);
        assert_eq!(native.record_bytes(invocation_id), record);
    }
}

#[test]
fn unencodable_surrogates_are_truthful_capability_failures() {
    const GUARD: &str = "SK_OP_BYTE_SURROGATE_CHILD";
    const NAME: &str = "unencodable_surrogates_are_truthful_capability_failures";
    if !child(NAME, GUARD, UTF8_MODE_ZERO) {
        return;
    }
    for (invocation_id, profile, action, code) in [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBK",
            r#""\ud800""#,
            r#""a""#,
            "U+D800",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FBM",
            r#""\udc00""#,
            r#""a""#,
            "U+DC00",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FCN",
            r#""\udc7f""#,
            r#""a""#,
            "U+DC7F",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FCP",
            r#""p""#,
            r#""\ud800""#,
            "U+D800",
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FCQ",
            r#""\udc80\ud800""#,
            r#""a""#,
            "U+D800",
        ),
    ] {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let line = started_line(invocation_id, profile, action);
        let record = write_pair(&native, &source, invocation_id, &line);
        let head = native.head();
        let source_head = source.head();

        let reference = oracle_commit(&source.root, invocation_id, "agent");
        assert_eq!(
            reference["committed"], false,
            "{invocation_id}: {reference:?}"
        );
        assert_eq!(source.head(), source_head);

        let mut service = native.service();
        let outcome = service.commit_op_record(&id(invocation_id), OpClosedBy::Agent);
        assert!(!outcome.committed(), "{invocation_id}: {outcome:?}");
        let refusal = outcome.refusal().expect("a truthful native refusal");
        assert!(
            refusal.contains(code) && refusal.contains("not encodable"),
            "{invocation_id}: names the unencodable code point, got {refusal:?}"
        );
        assert!(
            !refusal.contains("Invalid invocation record"),
            "{invocation_id}: the row is valid; got {refusal:?}"
        );
        assert_eq!(native.head(), head);
        assert_eq!(native.record_bytes(invocation_id), record);
    }
}

// ---------------------------------------------------------------------------
// Nested/unused surrogates keep the one canonical repr and still commit.
// ---------------------------------------------------------------------------

#[test]
fn nested_and_unused_surrogates_keep_canonical_repr_bytes() {
    const GUARD: &str = "SK_OP_BYTE_NESTED_CHILD";
    const NAME: &str = "nested_and_unused_surrogates_keep_canonical_repr_bytes";
    if !child(NAME, GUARD, UTF8_MODE_ZERO) {
        return;
    }
    for (invocation_id, profile, action, expected) in [
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FCR",
            r#"["\ud800", "\udc80"]"#,
            r#""a""#,
            b"op(['\\ud800', '\\udc80']): a [01ARZ3ND]\n".as_slice(),
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FCS",
            r#"{"k": "\ud800\u00e9"}"#,
            r#""a""#,
            // A printable non-ASCII scalar stays verbatim in `repr`; the lone
            // surrogate is escaped. One canonical formatter, no `\xe9`.
            "op({'k': '\\ud800\u{00e9}'}): a [01ARZ3ND]\n".as_bytes(),
        ),
    ] {
        let native = Fixture::with_positive_primary();
        native.checkout_new("topic");
        let source = Fixture::with_positive_primary();
        source.checkout_new("topic");
        let line = format!(
            "{{\"event\": \"started\", \"invocation_id\": \"{invocation_id}\", \"profile_id\": {profile}, \"action\": {action}, \"unused\": {{\"\\ud800\": \"\\udc80\"}}}}\n"
        );
        let record = write_pair(&native, &source, invocation_id, &line);

        assert_source_committed(
            &oracle_commit(&source.root, invocation_id, "agent"),
            invocation_id,
        );
        native_commit(&native, invocation_id, OpClosedBy::Agent);

        assert_commit_shape(&native, &source, invocation_id, "topic");
        assert_eq!(
            native.message_bytes(),
            expected.to_vec(),
            "{invocation_id}: one canonical repr, no second formatter"
        );
        assert_eq!(
            source.message_bytes(),
            expected.to_vec(),
            "{invocation_id}: source object body equality for an ASCII repr"
        );
        assert_eq!(source.head(), native.head());
        assert_eq!(native.record_bytes(invocation_id), record);
    }
}

// ---------------------------------------------------------------------------
// Doctor sweep: the closure spine only, raw escaped payload, archive intact.
// ---------------------------------------------------------------------------

#[test]
fn doctor_sweep_commits_only_the_closure_spine_with_low_surrogate_bytes() {
    const GUARD: &str = "SK_OP_BYTE_DOCTOR_CHILD";
    const NAME: &str = "doctor_sweep_commits_only_the_closure_spine_with_low_surrogate_bytes";
    if !child(NAME, GUARD, UTF8_MODE_ZERO) {
        return;
    }
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");

    // A pre-existing archive record carrying a used low surrogate, already
    // committed and byte-frozen before the doctor close.
    let archive = started_line(ARCHIVE, r#""\udc80""#, r#""a""#);
    native.write_record_line(ARCHIVE, &archive);
    source.write_record_line(ARCHIVE, &archive);
    for fixture in [&native, &source] {
        fixture.git_ok(&["add", "--force", &format!("kitty-ops/{ARCHIVE}.jsonl")]);
        fixture.git_ok(&["commit", "-q", "-m", "archive record"]);
    }
    assert_eq!(native.head(), source.head(), "identical archive history");

    // The doctor close is judged from that started row; the spine row is the
    // durable close and is byte-identical on both sides.
    for fixture in [&native, &source] {
        let completed = OpCompletedEvent::new(OpCompletedFields {
            invocation_id: id(ARCHIVE),
            completed_at: "2026-11-02T00:00:00+00:00".to_owned(),
            outcome: OpOutcome::Done,
            closed_by: ClosedBy::DoctorSweep,
            evidence_ref: None,
        })
        .expect("valid closure");
        append_op_closure(&fixture.root, &completed).expect("append closure spine");
    }
    let archive_before = native.record_bytes(ARCHIVE);
    assert_eq!(source.record_bytes(ARCHIVE), archive_before);
    let spine = fs::read(native.root.join("kitty-ops").join(OP_CLOSURES_FILENAME)).unwrap();
    assert_eq!(
        fs::read(source.root.join("kitty-ops").join(OP_CLOSURES_FILENAME)).unwrap(),
        spine
    );

    assert_source_committed(
        &oracle_commit(&source.root, ARCHIVE, "doctor_sweep"),
        ARCHIVE,
    );
    native_commit(&native, ARCHIVE, OpClosedBy::DoctorSweep);

    assert_eq!(
        native.commit_paths(),
        vec!["kitty-ops/op-closures.jsonl".to_owned()]
    );
    assert_eq!(
        source.commit_paths(),
        vec!["kitty-ops/op-closures.jsonl".to_owned()],
        "a doctor close commits the spine only"
    );
    assert_eq!(native.head(), source.head());
    assert_eq!(native.commit_object(), source.commit_object());
    assert_eq!(
        native.reflog_message("topic"),
        b"commit: op(\x80): a [01ARZ3ND] (doctor sweep)".to_vec()
    );
    assert_eq!(
        source.reflog_message("topic"),
        native.reflog_message("topic")
    );
    assert_eq!(
        native.message_bytes(),
        b"op(\xc2\x80): a [01ARZ3ND] (doctor sweep)\n".to_vec()
    );
    assert_eq!(source.message_bytes(), native.message_bytes());
    assert_eq!(
        native.record_bytes(ARCHIVE),
        archive_before,
        "the archive row was not rewritten by the commit"
    );
    assert_eq!(
        fs::read(native.root.join("kitty-ops").join(OP_CLOSURES_FILENAME)).unwrap(),
        spine,
        "the commit did not rewrite the spine"
    );
    assert_eq!(native.status(), "");
    assert_eq!(source.status(), "");
}

// ---------------------------------------------------------------------------
// Git owns `i18n.commitEncoding`: the Op layer contributes pre-conversion bytes.
// ---------------------------------------------------------------------------

#[test]
fn iso8859_1_commit_encoding_keeps_the_preconversion_payload() {
    const GUARD: &str = "SK_OP_BYTE_ENCODING_HEADER_CHILD";
    const NAME: &str = "iso8859_1_commit_encoding_keeps_the_preconversion_payload";
    if !child(NAME, GUARD, UTF8_MODE_ZERO) {
        return;
    }
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FCT";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    for fixture in [&native, &source] {
        fixture.git_ok(&["config", "i18n.commitEncoding", "ISO-8859-1"]);
    }
    let line = started_line(invocation_id, r#""\udc80""#, r#""a""#);
    let record = write_pair(&native, &source, invocation_id, &line);

    assert_source_committed(
        &oracle_commit(&source.root, invocation_id, "agent"),
        invocation_id,
    );
    native_commit(&native, invocation_id, OpClosedBy::Agent);

    assert_commit_shape(&native, &source, invocation_id, "topic");
    // Declaring the payload's encoding stops Git's own conversion on both
    // sides; the exact bytes reach the object unchanged and Git adds the
    // `encoding` header. The Op layer contributed the pre-conversion payload,
    // and Git's `i18n.commitEncoding` owns the rest, so the complete object is
    // still identical.
    let payload = b"op(\x80): a [01ARZ3ND]\n".to_vec();
    assert_eq!(native.message_bytes(), payload);
    assert_eq!(source.message_bytes(), payload);
    for fixture in [&native, &source] {
        assert!(
            fixture
                .commit_object()
                .windows(b"encoding ISO-8859-1\n".len())
                .any(|window| window == b"encoding ISO-8859-1\n"),
            "the commit carries Git's encoding header"
        );
    }
    assert_eq!(native.record_bytes(invocation_id), record);
}

/// R1: a source-valid prepare-commit-msg hook reads
/// `$(git rev-parse --git-path COMMIT_EDITMSG)` and requires the pending
/// message. The source side runs first through the real executor; the native
/// side is the real `OpCommitService`.
#[test]
fn prepare_commit_msg_reads_the_actual_commit_editmsg_source_then_native() {
    const NAME: &str = "prepare_commit_msg_reads_the_actual_commit_editmsg_source_then_native";
    if !child(NAME, "SK_OP_COMMIT_ARTIFACT_R1_CHILD", UTF8_MODE_ZERO) {
        return;
    }
    let invocation_id = "01ARZ3NDEKTSV4RRFFQ69G5FD1";
    let native = Fixture::with_positive_primary();
    native.checkout_new("topic");
    let source = Fixture::with_positive_primary();
    source.checkout_new("topic");
    let line = started_line(invocation_id, r#""implementer-ivan""#, r#""implement""#);
    write_pair(&native, &source, invocation_id, &line);

    let short = &invocation_id[..8];
    let hook = format!(
        r#"d=$(git rev-parse --absolute-git-dir) || exit 1
p=$(git rev-parse --git-path COMMIT_EDITMSG) || exit 1
printf '%s' "$p" > "$d/prepare-artifact-spelling"
if [ -f "$p" ]; then cat "$p" > "$d/prepare-artifact-content"; else printf 'ABSENT' > "$d/prepare-artifact-content"; fi
grep -q '{short}' "$p" || exit 1"#
    );
    native.install_hook("prepare-commit-msg", &hook);
    source.install_hook("prepare-commit-msg", &hook);

    assert!(
        commit_pair(&native, &source, invocation_id),
        "{invocation_id}: the source commits this hook-valid row"
    );
    let source_artifact = source.artifact_bytes();
    assert!(
        String::from_utf8_lossy(&source_artifact).contains(short),
        "the source hook saw the actual artifact"
    );
    assert_eq!(
        native.artifact_bytes(),
        source_artifact,
        "{invocation_id}: both sides leave the same hook-visible artifact"
    );
    let source_spelling =
        String::from_utf8_lossy(&source.hook_log("prepare-artifact-spelling")).into_owned();
    let native_spelling =
        String::from_utf8_lossy(&native.hook_log("prepare-artifact-spelling")).into_owned();
    assert_eq!(
        native_spelling, source_spelling,
        "{invocation_id}: the hook argv path is git's own spelling on both sides"
    );
    assert_eq!(
        native.hook_log("prepare-artifact-content"),
        source.hook_log("prepare-artifact-content"),
        "{invocation_id}: prepare-commit-msg sees the same new artifact bytes"
    );
    assert_eq!(
        native.artifact_bytes(),
        native.hook_log("prepare-artifact-content")
    );
}
