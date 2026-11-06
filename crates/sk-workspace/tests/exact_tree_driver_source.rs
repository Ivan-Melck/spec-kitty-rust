//! Source-first proof for the exact-tree drivers: equality, Unicode digits, the
//! full Cs sweep and the PHYSICAL write phase.
//!
//! Every case runs the PINNED Source driver FIRST inside its own fresh sandbox
//! (the ORACLE asserts its actual cwd/HOME/XDG/TMP/PYTHONPATH/packs), then the
//! Native driver in its OWN fresh sandbox through the real path entry
//! (`run_driver_paths`), so the physical open/truncate → encode → write is what
//! the comparison binds. Captured stdout/stderr/exit are retained on failure.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_workspace::merge_drivers::run_driver_paths;

const ORACLE: &str = r#"
import json, os, sys
from pathlib import Path

root = Path(sys.argv[1])
name = sys.argv[2]
payload = json.loads(Path(sys.argv[3]).read_text(encoding="utf-8"))

# ASSERT the actual execution environment (a harness failure here, not a red).
for key in ("HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
            "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"):
    value = os.environ.get(key, "")
    if not value or not Path(value).is_relative_to(root):
        raise SystemExit(f"ENV-FAIL {key}={value!r} not under {root}")
if Path.cwd() != root / "cwd":
    raise SystemExit(f"CWD-FAIL {Path.cwd()}")
pythonpath = os.environ.get("PYTHONPATH", "")
if str(Path.cwd()) or True:
    for entry in pythonpath.split(":"):
        if not Path(entry).is_absolute():
            raise SystemExit(f"PYTHONPATH-FAIL {pythonpath}")

drivers = {
    "merge-driver-event-log": "run_event_log_driver",
    "merge-driver-meta": "run_meta_driver",
    "merge-driver-issue-matrix": "run_issue_matrix_driver",
    "merge-driver-acceptance-matrix": "run_acceptance_matrix_driver",
    "merge-driver-decision-index": "run_decision_index_driver",
}
from specify_cli.consolidation import drivers as module
base = root / "base"
ours = root / "ours"
theirs = root / "theirs"
base.write_bytes(payload["base"].encode("utf-8"))
ours.write_bytes(payload["ours"].encode("utf-8"))
theirs.write_bytes(payload["theirs"].encode("utf-8"))
before = {p.name: p.read_bytes() for p in (base, theirs)}
try:
    getattr(module, drivers[name])(str(base), str(ours), str(theirs))
except Exception as exc:
    result = {
        "outcome": "error",
        "kind": type(exc).__name__,
        "message": str(exc),
        "ours": ours.read_bytes().hex(),
        "ours_mode": ours.stat().st_mode & 0o777,
        "base_unchanged": base.read_bytes() == before["base"],
        "theirs_unchanged": theirs.read_bytes() == before["theirs"],
    }
    print(json.dumps(result))
    raise SystemExit(0)
print(json.dumps({
    "outcome": "ok",
    "bytes": ours.read_bytes().hex(),
    "ours_mode": ours.stat().st_mode & 0o777,
    "base_unchanged": base.read_bytes() == before["base"],
    "theirs_unchanged": theirs.read_bytes() == before["theirs"],
}))
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python() -> PathBuf {
    oracle::python()
}

/// One sandbox per side/case, with the whole asserted environment.
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
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
    }
}

#[derive(Debug)]
struct SourceRun {
    outcome: String,
    bytes: String,
    ours_hex: String,
    ours_mode: u64,
    kind: String,
    base_unchanged: bool,
    theirs_unchanged: bool,
    stdout: String,
    stderr: String,
}

fn source_driver(sandbox: &Sandbox, name: &str, base: &str, ours: &str, theirs: &str) -> SourceRun {
    let script = sandbox.path().join("exact-driver-oracle.py");
    fs::write(&script, ORACLE).expect("oracle");
    let payload = serde_json::json!({ "base": base, "ours": ours, "theirs": theirs });
    let payload_path = sandbox.path().join("payload.json");
    fs::write(
        &payload_path,
        serde_json::to_vec(&payload).expect("payload json"),
    )
    .expect("payload");
    let mut command = Command::new(python());
    command
        .arg(&script)
        .arg(sandbox.path())
        .arg(name)
        .arg(&payload_path)
        .current_dir(sandbox.path().join("cwd"))
        .env_clear();
    for (key, value) in sandbox.pairs() {
        command.env(key, value);
    }
    let output = command.output().expect("run Source driver");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "the pinned Source driver failed as a HARNESS/PREMISE failure:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("source json ({error}): {stdout}"));
    SourceRun {
        outcome: value["outcome"].as_str().unwrap_or_default().to_owned(),
        bytes: value["bytes"].as_str().unwrap_or_default().to_owned(),
        ours_hex: value["ours"].as_str().unwrap_or_default().to_owned(),
        ours_mode: value["ours_mode"].as_u64().unwrap_or(0),
        kind: value["kind"].as_str().unwrap_or_default().to_owned(),
        base_unchanged: value["base_unchanged"].as_bool().unwrap_or(false),
        theirs_unchanged: value["theirs_unchanged"].as_bool().unwrap_or(false),
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The Native driver through the real path entry, in its OWN fresh sandbox.
struct NativeRun {
    result: Result<(), String>,
    ours: Vec<u8>,
    ours_mode: u64,
    base_unchanged: bool,
    theirs_unchanged: bool,
}

fn native_driver(name: &str, base: &str, ours: &str, theirs: &str) -> NativeRun {
    let dir = tempfile::tempdir().expect("native sandbox");
    let base_path = dir.path().join("base");
    let ours_path = dir.path().join("ours");
    let theirs_path = dir.path().join("theirs");
    fs::write(&base_path, base).expect("base");
    fs::write(&ours_path, ours).expect("ours");
    fs::write(&theirs_path, theirs).expect("theirs");
    let result = run_driver_paths(name, &base_path, &ours_path, &theirs_path).map(|_| ());
    #[cfg(unix)]
    let ours_mode = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&ours_path)
            .map(|metadata| u64::from(metadata.permissions().mode() & 0o777))
            .unwrap_or(0)
    };
    #[cfg(not(unix))]
    let ours_mode = 0;
    NativeRun {
        result: result.map_err(|error| error.to_string()),
        ours: fs::read(&ours_path).expect("ours after"),
        ours_mode,
        base_unchanged: fs::read(&base_path).expect("base after") == base.as_bytes(),
        theirs_unchanged: fs::read(&theirs_path).expect("theirs after") == theirs.as_bytes(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const MATRIX_BASE: &str = r#"{"mission_slug":"fx","mission_number":1,"mission_type":"software-dev","criteria":[],"negative_invariants":[]}"#;

fn acceptance_doc(extra_key: &str, extra_value: &str) -> String {
    format!(
        r#"{{"mission_slug":"fx","mission_number":1,"mission_type":"software-dev","criteria":[{{"criterion_id":"C1","description":"d","proof_type":"code_review","pass_fail":"pass","{extra_key}":"{extra_value}"}}],"negative_invariants":[]}}"#
    )
}

/// A driver comparison for one document triple: Source bytes/mode/inputs, then
/// Native bytes/mode/inputs.
fn compare_ok(name: &str, base: &str, ours: &str, theirs: &str, label: &str) {
    let sandbox = Sandbox::new();
    let source = source_driver(&sandbox, name, base, ours, theirs);
    assert_eq!(
        source.outcome, "ok",
        "{label}: Source accepts the document: {source:?}\nstdout={}\nstderr={}",
        source.stdout, source.stderr
    );
    let native = native_driver(name, base, ours, theirs);
    native
        .result
        .unwrap_or_else(|error| panic!("{label}: Native driver failed: {error}"));
    assert_eq!(
        hex(&native.ours),
        source.bytes,
        "{label}: the published bytes match Source, exact code points included"
    );
    assert_eq!(
        native.ours_mode, source.ours_mode,
        "{label}: the published file mode matches Source"
    );
    assert!(
        source.base_unchanged && native.base_unchanged,
        "{label}: base untouched"
    );
    assert!(
        source.theirs_unchanged && native.theirs_unchanged,
        "{label}: theirs untouched"
    );
}

#[test]
fn numeric_and_nan_equality_follows_source() {
    // `1` and `1.0` are EQUAL in Python, so the field is "unchanged" and the
    // merged value keeps `ours`; a structural comparison would call it a change.
    let base = r#"{"mission_slug":"fx","n":1}"#;
    let ours = r#"{"mission_slug":"fx","n":1.0}"#;
    let theirs = r#"{"mission_slug":"fx","n":1.0}"#;
    compare_ok("merge-driver-meta", base, ours, theirs, "int-vs-float");

    // A NaN inside a container compares equal to an equal NaN container in
    // Source's default `json.loads` domain (the identity shortcut).
    let base = r#"{"mission_slug":"fx","v":[NaN]}"#;
    let ours = r#"{"mission_slug":"fx","v":[NaN]}"#;
    let theirs = r#"{"mission_slug":"fx","v":[Infinity]}"#;
    compare_ok("merge-driver-meta", base, ours, theirs, "container-nan");

    // A huge integer is never rounded through f64 and equals itself.
    let base = r#"{"mission_slug":"fx","n":1}"#;
    let ours = r#"{"mission_slug":"fx","n":123456789012345678901234567890}"#;
    let theirs = r#"{"mission_slug":"fx","n":123456789012345678901234567890}"#;
    compare_ok("merge-driver-meta", base, ours, theirs, "big-int");
}

#[test]
fn unicode_decimal_issue_refs_canonicalize_like_source() {
    // U+0663 ARABIC-INDIC DIGIT THREE is Unicode Nd (Python `isdecimal()`),
    // so `#٦٦٣` and `٦٦٣` are the same row key; an ASCII-only predicate would
    // keep them apart.
    let base = r##"{"schema_version":1,"rows":{}}"##;
    let ours = r##"{"schema_version":1,"rows":{"#٦٦٣":{"verdict":"pass"}}}"##;
    let theirs = r##"{"schema_version":1,"rows":{"٦٦٣":{"evidence":"x"}}}"##;
    compare_ok(
        "merge-driver-issue-matrix",
        base,
        ours,
        theirs,
        "unicode-nd-issue-ref",
    );
}

#[test]
fn the_full_cs_sweep_keeps_every_surrogate_key_and_value() {
    // ALL 2048 surrogate code points as keys AND as values in one document.
    let mut entries = Vec::new();
    for unit in 0xD800u32..=0xDFFF {
        entries.push(format!(r#""k\u{unit:04x}":"\u{unit:04x}""#));
    }
    let extras = entries.join(",");
    let ours = format!(
        r#"{{"mission_slug":"fx","mission_number":1,"mission_type":"software-dev","criteria":[{{"criterion_id":"C1","description":"d","proof_type":"code_review","pass_fail":"pass",{extras}}}],"negative_invariants":[]}}"#
    );
    compare_ok(
        "merge-driver-acceptance-matrix",
        MATRIX_BASE,
        &ours,
        MATRIX_BASE,
        "full-cs-sweep",
    );
    // And as event-log keys/values.
    let mut rows = Vec::new();
    for unit in 0xD800u32..=0xDFFF {
        rows.push(format!(
            r#"{{"event_id":"e\u{unit:04x}","at":"t","k\u{unit:04x}":"\u{unit:04x}"}}"#
        ));
    }
    let rows = rows.join("\n") + "\n";
    compare_ok(
        "merge-driver-event-log",
        "",
        &rows,
        "",
        "full-cs-event-rows",
    );
}

#[test]
fn a_surrogate_in_meta_truncates_the_target_at_the_physical_write_like_source() {
    // The parent's canonical Source fixture: a non-empty ancestor selects the
    // THREE-WAY rule, so `ours`'s surrogate-bearing `note` survives the merge and
    // reaches the write. (An EMPTY base selects the two-way rule, which DROPS the
    // unrelated ours-only key — two earlier probes expecting a failure there were
    // premise errors, not Source reds. That premise is pinned below.)
    let base = r#"{"anchor":0}"#;
    let ours = r#"{"anchor":0,"note":"\ud800"}"#;
    let theirs = r#"{"anchor":0,"incoming":1}"#;
    let sandbox = Sandbox::new();
    let source = source_driver(&sandbox, "merge-driver-meta", base, ours, theirs);
    assert_eq!(
        source.outcome, "error",
        "Source fails the meta write: {source:?}"
    );
    assert!(
        source.kind.contains("UnicodeEncodeError"),
        "Source's failure is the physical encode: {source:?}"
    );
    assert_eq!(
        source.ours_hex, "",
        "the destination is TRUNCATED (0 bytes) before the encode raises: {source:?}"
    );
    assert!(
        source.base_unchanged && source.theirs_unchanged,
        "Source leaves the non-output inputs untouched: {source:?}"
    );

    let native = native_driver("merge-driver-meta", base, ours, theirs);
    let message = native.result.err().unwrap_or_default();
    assert!(
        message.contains("U+") || message.contains("surrogate"),
        "the Native failure names the code point: {message}"
    );
    assert_eq!(
        native.ours,
        Vec::<u8>::new(),
        "the Native destination is truncated at the same phase (no stale bytes, no fabricated success)"
    );
    assert!(
        native.base_unchanged && native.theirs_unchanged,
        "the Native write touches only its destination"
    );
    assert_eq!(
        native.ours_mode, source.ours_mode,
        "the truncated destination keeps the Source mode"
    );

    // The premise itself, pinned on BOTH sides: an EMPTY base selects the
    // two-way rule, where `theirs` wins the unrelated key and the ours-only
    // surrogate is dropped, so the write SUCCEEDS. Never force Source to match an
    // assumed cause.
    let two_way = source_driver(
        &sandbox,
        "merge-driver-meta",
        "",
        r#"{"anchor":0,"note":"\ud800"}"#,
        r#"{"anchor":0,"incoming":1}"#,
    );
    assert_eq!(
        two_way.outcome, "ok",
        "empty base selects two-way and drops the ours-only surrogate: {two_way:?}"
    );
    let native_two_way = native_driver(
        "merge-driver-meta",
        "",
        r#"{"anchor":0,"note":"\ud800"}"#,
        r#"{"anchor":0,"incoming":1}"#,
    );
    native_two_way
        .result
        .expect("the two-way rule succeeds on both sides");
    assert_eq!(
        hex(&native_two_way.ours),
        two_way.bytes,
        "the two-way merge bytes match Source"
    );

    // RETRY on the three-way input with a valid note publishes normally.
    let native_retry = native_driver(
        "merge-driver-meta",
        base,
        r#"{"anchor":0,"note":"ok"}"#,
        theirs,
    );
    native_retry.result.expect("a valid retry succeeds");
    assert!(!native_retry.ours.is_empty(), "the retry publishes content");
}

#[test]
fn a_surrogate_in_an_ascii_body_is_escaped_not_refused() {
    // The acceptance driver writes `ensure_ascii=True`; the surrogate must be
    // ESCAPED and written, never refused (regression guard for the new phase).
    compare_ok(
        "merge-driver-acceptance-matrix",
        MATRIX_BASE,
        &acceptance_doc("extra_cs", r"\ud83d\ude00\udc80"),
        MATRIX_BASE,
        "ascii-body-surrogate",
    );
}

const ASSIGNED_ORACLE: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.consolidation.mission_number import is_assigned_mission_number

values = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
print(json.dumps([bool(is_assigned_mission_number(value)) for value in values]))
"#;

#[test]
fn assigned_mission_number_matches_source_for_every_boundary() {
    // Source's helper is `isinstance(value, int) and not isinstance(value, bool)
    // and value >= 1` — no machine-width bound, so a huge positive int is
    // assigned. The adapter previously parsed through i128 and rejected it.
    let sandbox = Sandbox::new();
    let script = sandbox.path().join("assigned-oracle.py");
    fs::write(&script, ASSIGNED_ORACLE).expect("oracle");
    // `values` are JSON texts; the huge integer must survive as an int.
    let cases = [
        "1",
        "0",
        "-1",
        "true",
        "false",
        "null",
        "\"3\"",
        "1.0",
        "123456789012345678901234567890123456789012345678901234567890",
    ];
    let payload_path = sandbox.path().join("values.json");
    fs::write(&payload_path, format!("[{}]", cases.join(","))).expect("values");
    let mut command = Command::new(python());
    command
        .arg(&script)
        .arg(&payload_path)
        .current_dir(sandbox.path().join("cwd"))
        .env_clear();
    for (key, value) in sandbox.pairs() {
        command.env(key, value);
    }
    let output = command.output().expect("run Source helper");
    assert!(
        output.status.success(),
        "the pinned Source helper failed (premise): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<bool> = serde_json::from_slice(&output.stdout).expect("source json");
    for (case, expected) in cases.iter().zip(expected) {
        let value: serde_json::Value = serde_json::from_str(case).expect("case json");
        let actual = sk_workspace::merge_drivers::is_assigned_mission_number(Some(&value));
        assert_eq!(
            actual, expected,
            "is_assigned_mission_number({case}) matches Source"
        );
    }
}
