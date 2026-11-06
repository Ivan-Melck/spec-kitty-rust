//! CF79 — Source activation-reader warning/report delivery, paired against the
//! pinned reference in TWO real children (the reference first).
//!
//! The pinned source emits two plain-stderr lines from its activation read:
//!
//! * `report_stray_legacy_keys` — legacy activation keys beside a ledger, one
//!   line per distinct `((Path, (key, ...)), ...)` report, suppressed for the
//!   rest of the process by ONE module-level latch that both reporters share;
//! * `report_legacy_deny_keys` — the sorted `deactivated_<kind>` keys of a
//!   legacy surface, which are reported and never applied.
//!
//! Both are `print(f"Warning: {message}", file=sys.stderr)`: a CLI line, not a
//! Python `UserWarning`, never on stdout, and printed at exact read stages (the
//! stray report precedes the modern ledger read; the legacy deny report follows
//! the raw legacy read and precedes projection/validation). A failed write still
//! consumes the latch because the report is recorded before the print.
//!
//! Phase 1 (this file's first delivery) drives ONLY the pre-existing public API,
//! so the missing side effect is a genuine business red: the reference children
//! write warning bytes and the Native child writes none.

#![cfg(unix)]

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value as Json, json};

use sk_doctrine::{
    ActivationLedger, LoadDeniedOptions, load_activation_ledger, load_activation_ledger_exact,
    load_activation_ledger_inspection, load_denied,
};

/// Set only inside the Native child (the sandbox root).
const CHILD_ENV: &str = "SK_CF79_PAIR_ROOT";
/// Set only inside the Native child: where to write its receipt JSON.
const CHILD_RECEIPT_ENV: &str = "SK_CF79_NATIVE_RECEIPT";
/// The pinned interpreter's stream settings each pair is exercised under.
const CODEC_SETTINGS: [Option<&str>; 3] = [None, Some("ascii"), Some("utf-16")];

const CONFIG: &str = ".kittify/config.yaml";
const LEDGER: &str = ".kittify/charter/activation.yaml";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in [
            "home",
            "cache",
            "config",
            "data",
            "state",
            "runtime",
            "config-dirs",
            "data-dirs",
            "tmp",
            "cwd",
        ] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox dir");
        }
        Self { root: temp.keep() }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn env_pairs(&self) -> [(&'static str, &'static str); 11] {
        [
            ("HOME", "home"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("XDG_CONFIG_DIRS", "config-dirs"),
            ("XDG_DATA_DIRS", "data-dirs"),
            ("TMPDIR", "tmp"),
            ("TMP", "tmp"),
            ("TEMP", "tmp"),
        ]
    }

    /// Asserted as the FIRST statement of the Native child, before any business
    /// read: an inherited or shared environment is not a paired execution.
    fn assert_isolation(&self) {
        assert_eq!(std::env::current_dir().unwrap(), self.path("cwd"));
        for (key, name) in self.env_pairs() {
            assert_eq!(
                std::env::var_os(key).map(PathBuf::from),
                Some(self.path(name)),
                "child {key} leaked"
            );
        }
        assert_eq!(
            std::env::var_os("PYTHONPATH").map(PathBuf::from),
            Some(repo_root().join("src"))
        );
        assert_eq!(
            std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
            Some(repo_root().join("packs"))
        );
        assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
        let expected_encoding =
            std::env::var("PYTHONIOENCODING").unwrap_or_else(|_| "utf-8".to_owned());
        let policy = sk_kernel::python_stream::StreamPolicy::for_stderr()
            .expect("actual Native stderr startup policy");
        assert_eq!(policy.encoding(), expected_encoding);
        assert_eq!(policy.errors(), "backslashreplace");
    }
}

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, body).expect("fixture write");
}

/// Seed every fixture root the step plan reads. All are fresh temporary trees.
fn seed(sandbox: &Sandbox) {
    // a/: two stray allow keys beside a modern ledger (report order as authored).
    write(
        &sandbox.path("a"),
        CONFIG,
        "activated_directives: [010-x]\nactivated_tactics: [t-1]\n",
    );
    write(&sandbox.path("a"), LEDGER, "{}\n");

    // b/: a different path, so its stray report is distinct from a/'s.
    write(
        &sandbox.path("b"),
        CONFIG,
        "activated_directives: [010-x]\n",
    );
    write(&sandbox.path("b"), LEDGER, "{}\n");

    // c/: the already-read ledger handed to `load_denied`.
    write(&sandbox.path("c"), CONFIG, "activated_tactics: [t-1]\n");
    write(&sandbox.path("c"), LEDGER, "{}\n");

    // d/: an inspection read (Source `report_stray=False`) must be silent even
    // though this exact report was never seen before.
    write(&sandbox.path("d"), CONFIG, "activated_kinds: [directive]\n");
    write(&sandbox.path("d"), LEDGER, "{}\n");

    // ptr/: the stray keys sit on the POINTED charter file, reached through a
    // pointer whose text carries dot/duplicate-slash components.
    write(
        &sandbox.path("ptr"),
        CONFIG,
        "charter: ./sub//charter.yaml\n",
    );
    write(
        &sandbox.path("ptr"),
        "sub/charter.yaml",
        "activated_directives: [010-x]\n",
    );
    write(&sandbox.path("ptr"), LEDGER, "{}\n");

    // legacy/: a hand-written deny key, a non-string top-level key and an allow
    // key. The deny key is reported and never applied; the numeric key is not
    // stringified into a deny.
    write(
        &sandbox.path("legacy"),
        CONFIG,
        "activated_directives: []\ndeactivated_directives: [900-unknown]\n1: [x]\n",
    );

    // legacy-b/: a second legacy surface whose deny report must be suppressed by
    // an inspection read and then delivered by the ordinary read.
    write(
        &sandbox.path("legacy-b"),
        CONFIG,
        "activated_tactics: []\ndeactivated_tactics: [900-unknown]\n",
    );

    // bad-ledger/: an unknown non-deny ledger key refuses AFTER the stray report.
    write(
        &sandbox.path("bad-ledger"),
        CONFIG,
        "activated_directives: [010-x]\n",
    );
    write(&sandbox.path("bad-ledger"), LEDGER, "bogus_key: []\n");

    // bad-legacy/: a malformed allow list refuses AFTER the legacy deny report.
    write(
        &sandbox.path("bad-legacy"),
        CONFIG,
        "activated_directives: notalist\ndeactivated_directives: [900-unknown]\n",
    );
}

/// One recorded business step, identical in both children.
fn step_record(ledger: Option<&ActivationLedger>, denied: usize) -> Json {
    match ledger {
        Some(ledger) => {
            // Source's `raw()` carries only the PRESENT keys; absent keys are not
            // in the mapping at all (the Native accessor materialises the two
            // three-state keys as None, which is dropped here so the compared
            // datum is Source's shape, not a Native convenience).
            let raw: BTreeMap<String, Json> = ledger
                .raw()
                .iter()
                .filter_map(|(key, value)| value.as_ref().map(|ids| (key.clone(), json!(ids))))
                .collect();
            json!({
                "outcome": "ok",
                "raw_keys": raw,
                "denied": denied,
                "stray_count": ledger.stray_legacy_keys.len(),
                "stray_keys": ledger
                    .stray_legacy_keys
                    .iter()
                    .map(|(_path, keys)| keys.clone())
                    .collect::<Vec<_>>(),
            })
        }
        None => json!({"outcome": "refused"}),
    }
}

/// The Native child's receipt: the same sequence of business reads the Source
/// child performs, without any test-only reporting hook.
fn native_receipt(sandbox: &Sandbox) -> Json {
    let mut out = Map::new();

    // Each child starts from the seeded fixture: the order-change step below
    // rewrites this file, so the next pair must not inherit that edit.
    write(
        &sandbox.path("a"),
        CONFIG,
        "activated_directives: [010-x]\nactivated_tactics: [t-1]\n",
    );

    let first = load_activation_ledger(&sandbox.path("a"));
    out.insert("stray_a".to_owned(), step_record(first.as_ref().ok(), 0));
    let second = load_activation_ledger(&sandbox.path("a"));
    out.insert(
        "stray_a_again".to_owned(),
        step_record(second.as_ref().ok(), 0),
    );
    let legacy = load_activation_ledger(&sandbox.path("legacy"));
    out.insert(
        "legacy_deny".to_owned(),
        step_record(legacy.as_ref().ok(), 0),
    );
    out.insert(
        "bad_ledger".to_owned(),
        step_record(
            load_activation_ledger(&sandbox.path("bad-ledger"))
                .as_ref()
                .ok(),
            0,
        ),
    );
    out.insert(
        "bad_legacy".to_owned(),
        step_record(
            load_activation_ledger(&sandbox.path("bad-legacy"))
                .as_ref()
                .ok(),
            0,
        ),
    );
    let denied_b = load_denied(&sandbox.path("b"), &LoadDeniedOptions::default());
    out.insert(
        "stray_b".to_owned(),
        json!({
            "outcome": if denied_b.is_ok() { "ok" } else { "refused" },
            "denied": denied_b.as_ref().map(Vec::len).unwrap_or(0),
        }),
    );
    // The caller-read ledger, then the supplied-ledger read, which must bypass
    // the probe/read/report. The only warning this step can carry is the one
    // from the caller's own read, exactly as Source's sequence does.
    let supplied = load_activation_ledger_exact(&sandbox.path("c"));
    let supplied_denied = match &supplied {
        Ok(ledger) => load_denied(
            &sandbox.path("c"),
            &LoadDeniedOptions {
                ledger: Some(ledger),
                ..LoadDeniedOptions::default()
            },
        )
        .map(|denied| denied.len())
        .unwrap_or(0),
        Err(_) => 0,
    };
    out.insert(
        "supplied_bypass".to_owned(),
        json!({
            "outcome": if supplied.is_ok() { "ok" } else { "refused" },
            "denied": supplied_denied,
        }),
    );
    let pointer = load_activation_ledger(&sandbox.path("ptr"));
    out.insert(
        "pointer_alias".to_owned(),
        step_record(pointer.as_ref().ok(), 0),
    );
    out.insert(
        "inspection_d".to_owned(),
        step_record(
            load_activation_ledger_inspection(&sandbox.path("d"))
                .as_ref()
                .ok(),
            0,
        ),
    );
    out.insert(
        "normal_d".to_owned(),
        step_record(load_activation_ledger(&sandbox.path("d")).as_ref().ok(), 0),
    );
    // A changed key SET is a different Source report tuple: it prints again. The
    // resolver sorts each file's keys, so document order alone is not a change;
    // the added key also changes the probe cache's size key, so both readers
    // really re-read the file.
    write(
        &sandbox.path("a"),
        CONFIG,
        "activated_tactics: [t-1]\nactivated_directives: [010-x]\nactivated_kinds: [directive]\n",
    );
    out.insert(
        "order_change_a".to_owned(),
        step_record(load_activation_ledger(&sandbox.path("a")).as_ref().ok(), 0),
    );
    out.insert(
        "inspection_legacy_b".to_owned(),
        step_record(
            load_activation_ledger_inspection(&sandbox.path("legacy-b"))
                .as_ref()
                .ok(),
            0,
        ),
    );
    out.insert(
        "normal_legacy_b".to_owned(),
        step_record(
            load_activation_ledger(&sandbox.path("legacy-b"))
                .as_ref()
                .ok(),
            0,
        ),
    );
    Json::Object(out)
}

/// Write the child's receipt where the parent expects it.
fn write_receipt(receipt: &Json) {
    let path = std::env::var_os(CHILD_RECEIPT_ENV).expect("receipt path");
    fs::write(
        path,
        serde_json::to_vec_pretty(receipt).expect("receipt json"),
    )
    .expect("receipt write");
}

/// The Source child: the same steps through the pinned reference.
fn source_script() -> &'static str {
    r##"
import json
import os
import sys
from pathlib import Path

sandbox = Path(__file__).resolve().parent
assert Path.cwd() == sandbox / "cwd", Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
assert os.environ.get("PYTHONHASHSEED") == "0", os.environ.get("PYTHONHASHSEED")
for key, name in (
    ("HOME", "home"), ("XDG_CACHE_HOME", "cache"), ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
):
    assert Path(os.environ[key]) == sandbox / name, (key, os.environ[key])
repo = Path(sys.argv[1])
assert repo.is_absolute()
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
from charter.activation import activation_ledger as al
from charter.activation.pack_context import load_denied
assert Path(al.__file__).is_relative_to(repo / "src")
assert sys.stderr.encoding == os.environ.get("PYTHONIOENCODING", "utf-8")
assert sys.stderr.errors == "backslashreplace"
assert sys.getfilesystemencoding() == "utf-8"
assert sys.getfilesystemencodeerrors() == "surrogateescape"

# All business arguments are identical relative Paths, but CWD, fixtures and
# every environment directory belong to this child alone. No output rewriting.
sandbox = Path(".")
out = {}

# Each child starts from the seeded fixture: the order-change step below rewrites
# this file, so the next pair must not inherit that edit.
(sandbox / "a" / ".kittify" / "config.yaml").write_text(
    "activated_directives: [010-x]\nactivated_tactics: [t-1]\n", encoding="utf-8"
)

def record_ledger(step, ledger):
    raw = {key: value for key, value in ledger.raw().items()}
    out[step] = {
        "outcome": "ok",
        "raw_keys": raw,
        "denied": 0,
        "stray_count": len(ledger.stray_legacy_keys),
        "stray_keys": [list(keys) for _path, keys in ledger.stray_legacy_keys],
    }

def record_call(step, call, deny=False):
    try:
        value = call()
    except BaseException:  # noqa: BLE001 - a refusal is the recorded datum
        out[step] = {"outcome": "refused"}
        return None
    if deny:
        out[step] = {"outcome": "ok", "denied": len(value)}
    else:
        record_ledger(step, value)
    return value

record_call("stray_a", lambda: al.load_activation_ledger(sandbox / "a"))
record_call("stray_a_again", lambda: al.load_activation_ledger(sandbox / "a"))
record_call("legacy_deny", lambda: al.load_activation_ledger(sandbox / "legacy"))
record_call("bad_ledger", lambda: al.load_activation_ledger(sandbox / "bad-ledger"))
record_call("bad_legacy", lambda: al.load_activation_ledger(sandbox / "bad-legacy"))
record_call("stray_b", lambda: load_denied(sandbox / "b"), deny=True)
supplied = record_call("supplied_bypass", lambda: al.load_activation_ledger(sandbox / "c"))
record_call("supplied_bypass", lambda: load_denied(sandbox / "c", ledger=supplied), deny=True)
record_call("pointer_alias", lambda: al.load_activation_ledger(sandbox / "ptr"))
record_call(
    "inspection_d",
    lambda: al.load_activation_ledger(sandbox / "d", report_stray=False),
)
record_call("normal_d", lambda: al.load_activation_ledger(sandbox / "d"))
(sandbox / "a" / ".kittify" / "config.yaml").write_text(
    "activated_tactics: [t-1]\nactivated_directives: [010-x]\nactivated_kinds: [directive]\n",
    encoding="utf-8",
)
record_call("order_change_a", lambda: al.load_activation_ledger(sandbox / "a"))
record_call(
    "inspection_legacy_b",
    lambda: al.load_activation_ledger(sandbox / "legacy-b", report_stray=False),
)
record_call("normal_legacy_b", lambda: al.load_activation_ledger(sandbox / "legacy-b"))

Path(sys.argv[2]).write_text(json.dumps(out, sort_keys=True), encoding="utf-8")
"##
}

/// Run the Source child for one stream-codec setting; return its receipt and raw
/// stderr bytes.
fn run_source(sandbox: &Sandbox, setting: Option<&str>) -> (Json, Vec<u8>) {
    let script = sandbox.path("source_receipt.py");
    fs::write(&script, source_script()).expect("source script");
    let receipt = sandbox.path("source-receipt.json");
    let _ = fs::remove_file(&receipt);
    let mut command = Command::new(oracle::python());
    command
        .arg(&script)
        .arg(repo_root())
        .arg(&receipt)
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PYTHONPATH", repo_root().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("LC_ALL", "C.UTF-8");
    if let Some(setting) = setting {
        command.env("PYTHONIOENCODING", setting);
    }
    for (key, name) in sandbox.env_pairs() {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().expect("run the pinned reference");
    assert!(
        output.status.success(),
        "reference receipt failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = serde_json::from_slice(&fs::read(&receipt).expect("source receipt file"))
        .expect("source receipt json");
    (receipt, output.stderr)
}

/// Re-exec this test binary as the Native child and return (receipt, stderr).
fn run_native(sandbox: &Sandbox, setting: Option<&str>) -> (Json, Vec<u8>) {
    let receipt = sandbox.path("native-receipt.json");
    let _ = fs::remove_file(&receipt);
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args([
            "--exact",
            "the_source_and_native_children_emit_the_same_activation_report_lines",
            "--nocapture",
        ])
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(CHILD_ENV, &sandbox.root)
        .env(CHILD_RECEIPT_ENV, &receipt)
        .env("PYTHONPATH", repo_root().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("LC_ALL", "C.UTF-8");
    if let Some(setting) = setting {
        command.env("PYTHONIOENCODING", setting);
    }
    for (key, name) in sandbox.env_pairs() {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().expect("run the Native child");
    assert!(
        output.status.success(),
        "Native child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = serde_json::from_slice(&fs::read(&receipt).expect("native receipt file"))
        .expect("native receipt json");
    (receipt, output.stderr)
}

#[test]
fn the_source_and_native_children_emit_the_same_activation_report_lines() {
    if let Some(root) = std::env::var_os(CHILD_ENV) {
        let sandbox = Sandbox {
            root: PathBuf::from(root),
        };
        sandbox.assert_isolation();
        // The actual business reads of this lane; the child's own stderr is the
        // measured delivery channel.
        write_receipt(&native_receipt(&Sandbox {
            root: PathBuf::new(),
        }));
        return;
    }

    // Fail closed on the oracle before either child runs.
    let _ = oracle::python();
    let mut source_expected: Option<Vec<u8>> = None;
    for setting in CODEC_SETTINGS {
        let source_sandbox = Sandbox::new();
        let native_sandbox = Sandbox::new();
        assert_ne!(source_sandbox.root, native_sandbox.root);
        for sandbox in [&source_sandbox, &native_sandbox] {
            seed(&Sandbox {
                root: sandbox.path("cwd"),
            });
        }
        let (source, source_stderr) = run_source(&source_sandbox, setting);
        let (native, native_stderr) = run_native(&native_sandbox, setting);

        // The business data must already agree; the delivered bytes are the new
        // capability. Both are compared, so an empty-native pass is impossible.
        assert_eq!(native, source, "{setting:?}: receipt");
        assert!(
            !source_stderr.is_empty(),
            "{setting:?}: the reference must emit warning bytes"
        );
        assert_eq!(
            native_stderr,
            source_stderr,
            "{setting:?}: Native stderr bytes\nnative={}\nsource={}",
            String::from_utf8_lossy(&native_stderr),
            String::from_utf8_lossy(&source_stderr)
        );
        if setting.is_none() {
            source_expected = Some(source_stderr.clone());
        }
    }

    // Non-vacuity AND an independent statement of the policy: the default
    // (UTF-8) stream carries BOTH warning kinds, in Source's stage order, exactly
    // ten latch-deduplicated lines. The expected text is spelled here from the
    // Source literals, not copied from the Native output.
    let expected = String::from_utf8(source_expected.expect("default receipt")).expect("utf-8");
    assert_eq!(
        expected,
        expected_lines(&Sandbox {
            root: PathBuf::new(),
        })
        .join("")
    );
    for line in expected.lines() {
        assert!(line.starts_with("Warning: "), "prefix: {line}");
    }
}

/// The stray-keys line for one reported file, in the reference's spelling.
fn stray_line(listed: &str, path: &Path) -> String {
    format!(
        "Warning: Legacy activation keys beside .kittify/charter/activation.yaml are ignored; \
         the activation ledger (activation.yaml) wins: {listed} in {}. Remove them, or run \
         `spec-kitty doctor charter-packs`.\n",
        path.display()
    )
}

/// The legacy deny line for one legacy surface.
fn deny_line(keys: &str, path: &Path) -> String {
    format!(
        "Warning: {keys} in {} is not applied: a deactivation is recorded only in \
         .kittify/charter/activation.yaml. Remove the key by hand, then run `spec-kitty upgrade` \
         (or `spec-kitty migrate activation-ledger`).\n",
        path.display()
    )
}

/// The exact ordered lines both children must deliver, in Source's stages.
fn expected_lines(sandbox: &Sandbox) -> Vec<String> {
    let config = |name: &str| sandbox.path(name).join(CONFIG);
    vec![
        stray_line("activated_directives, activated_tactics", &config("a")),
        deny_line("deactivated_directives", &config("legacy")),
        stray_line("activated_directives", &config("bad-ledger")),
        deny_line("deactivated_directives", &config("bad-legacy")),
        stray_line("activated_directives", &config("b")),
        stray_line("activated_tactics", &config("c")),
        stray_line(
            "activated_directives",
            &sandbox.path("ptr").join("sub/charter.yaml"),
        ),
        stray_line("activated_kinds", &config("d")),
        stray_line(
            "activated_directives, activated_kinds, activated_tactics",
            &config("a"),
        ),
        deny_line("deactivated_tactics", &config("legacy-b")),
    ]
}
