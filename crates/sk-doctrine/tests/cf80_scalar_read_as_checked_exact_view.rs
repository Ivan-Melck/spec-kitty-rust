//! CF80 — the scalar activation-ledger read as a CHECKED VIEW of the canonical
//! exact read, paired against the pinned reference in TWO real children.
//!
//! The defect this lane removes: the scalar reader had its own `YamlDocument`
//! execution path whose `get_string_list` refuses any legacy list member that is
//! not a string. The pinned reference's legacy (non-strict) read is
//! `tuple(str(item) for item in value)`, so `[true, 123, null, [1, 2], {a: 1},
//! 1.5]` is Source-valid and becomes `("True", "123", "None", "[1, 2]",
//! "{'a': 1}", "1.5")`. The exact reader already owns that coercion
//! (`NodeRef.python_str`); the scalar read now delegates to it and the existing
//! checked `to_scalar_view`, so there is ONE read truth.
//!
//! Phase 1 (written before the change) drives ONLY the pre-existing public API,
//! so the missing Source-valid acceptance is a genuine business red: the Source
//! child reads the mixed legacy lists and the Native child refuses them.
//!
//! Every fixture is a fresh temporary business tree per side; both children
//! assert their own isolated CWD/HOME/XDG/TMP/TEMP and absolute Source/PYTHONPATH/
//! packs/hash-seed before their first business read. All business arguments are
//! identical RELATIVE paths under independent CWDs, so receipts are comparable
//! without rewriting or normalising any output.

#![cfg(unix)]

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value as Json, json};

use sk_doctrine::{
    ActivationLedger, ActivationReadError, DoctrineError, load_activation_ledger,
    load_activation_ledger_exact, load_activation_ledger_inspection, project_activation_state,
};

/// Set only inside the Native child (the sandbox root).
const CHILD_ENV: &str = "SK_CF80_PAIR_ROOT";
/// Set only inside the Native child: where to write its receipt JSON.
const CHILD_RECEIPT_ENV: &str = "SK_CF80_NATIVE_RECEIPT";

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
        // The actual startup filesystem codec this child reads files through.
        assert_eq!(
            sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
            "utf-8, surrogateescape"
        );
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
///
/// The bodies are byte-identical to the Source-first probe
/// (`probes/cf80-source-probe.py`) so Source and Native read the same documents
/// through independent copies.
fn seed(root: &Path) {
    // a: the defect case — a legacy allow list of non-string members.
    write(
        root,
        &format!("a/{CONFIG}"),
        "activated_directives: [true, 123, null, [1, 2], {a: 1}, 1.5]\n",
    );

    // b: a Source-admitted date/timestamp item (round-trip loader constructs it).
    write(
        root,
        &format!("b/{CONFIG}"),
        "activated_directives: [!!timestamp 2021-01-02]\n",
    );

    // c: a zero-byte config (Source: empty mapping, file-exists fact kept).
    write(root, &format!("c/{CONFIG}"), "");

    // d: an explicit null root.
    write(root, &format!("d/{CONFIG}"), "null\n");

    // e: modern strict string list with duplicates and a non-sorted order.
    write(
        root,
        &format!("e/{LEDGER}"),
        "activated_directives: [\"010-x\", \"002-y\", \"010-x\"]\n",
    );

    // f: modern strict list with a non-string member -> refusal.
    write(
        root,
        &format!("f/{LEDGER}"),
        "activated_directives: [true]\n",
    );

    // g: modern unknown non-deny key -> refusal.
    write(root, &format!("g/{LEDGER}"), "bogus_key: []\n");

    // h: modern null vs [] three-state.
    write(
        root,
        &format!("h/{LEDGER}"),
        "activated_directives: null\nactivated_tactics: []\n",
    );

    // i: legacy non-list -> refusal.
    write(
        root,
        &format!("i/{CONFIG}"),
        "activated_directives: notalist\n",
    );

    // j: legacy string list order + duplicates preserved.
    write(
        root,
        &format!("j/{CONFIG}"),
        "activated_directives: [\"010-b\", \"010-a\", \"010-b\"]\n",
    );

    // k: fresh project (no config, no ledger).
    fs::create_dir_all(root.join("k")).expect("fresh project");

    // l: legacy empty list is distinct from absent (raw carries the key).
    write(root, &format!("l/{CONFIG}"), "activated_directives: []\n");

    // m: legacy unknown key beside an allow key (non-strict: ignored).
    write(
        root,
        &format!("m/{CONFIG}"),
        "activated_directives: [010-x]\nbogus_key: []\n",
    );

    // n: a constructor abort inside the modern ledger document itself: the
    // pinned loader raises `ValueError` out of the read (never a healthy read).
    write(
        root,
        &format!("n/{LEDGER}"),
        "activated_directives: [!!timestamp 2021-13-45]\n",
    );
}

/// One recorded business step, identical in both children.
fn step_record(ledger: Result<ActivationLedger, DoctrineError>) -> Json {
    match ledger {
        Ok(ledger) => {
            // Source's `raw()` carries only the PRESENT keys.
            let raw: BTreeMap<String, Json> = ledger
                .raw()
                .iter()
                .filter_map(|(key, value)| value.as_ref().map(|ids| (key.clone(), json!(ids))))
                .collect();
            json!({
                "outcome": "ok",
                "source": match ledger.source {
                    sk_doctrine::ActivationSource::Ledger => "LEDGER",
                    sk_doctrine::ActivationSource::Legacy => "LEGACY",
                },
                "exists": ledger.exists,
                "raw_keys": raw,
            })
        }
        Err(_) => json!({"outcome": "refused"}),
    }
}

/// The Native child's receipt: the same sequence of business reads the Source
/// child performs, through the same pre-existing public API.
fn native_receipt() -> Json {
    let mut out = Map::new();
    for name in [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n",
    ] {
        let step = format!("case_{name}");
        out.insert(step, step_record(load_activation_ledger(Path::new(name))));
    }
    // The inspection read shares the same one read truth and stays silent.
    out.insert(
        "inspection_a".to_owned(),
        step_record(load_activation_ledger_inspection(Path::new("a"))),
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
assert sys.getfilesystemencoding() == "utf-8"
assert sys.getfilesystemencodeerrors() == "surrogateescape"
from charter.activation import activation_ledger as al
assert Path(al.__file__).is_relative_to(repo / "src")

# All business arguments are identical relative Paths, but CWD, fixtures and
# every environment directory belong to this child alone. No output rewriting.
CONFIG = ".kittify/config.yaml"
LEDGER = ".kittify/charter/activation.yaml"

def write(root, relative, body):
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")

sandbox = Path(".")
write(sandbox, f"a/{CONFIG}", "activated_directives: [true, 123, null, [1, 2], {a: 1}, 1.5]\n")
write(sandbox, f"b/{CONFIG}", "activated_directives: [!!timestamp 2021-01-02]\n")
write(sandbox, f"c/{CONFIG}", "")
write(sandbox, f"d/{CONFIG}", "null\n")
write(sandbox, f"e/{LEDGER}", 'activated_directives: ["010-x", "002-y", "010-x"]\n')
write(sandbox, f"f/{LEDGER}", "activated_directives: [true]\n")
write(sandbox, f"g/{LEDGER}", "bogus_key: []\n")
write(sandbox, f"h/{LEDGER}", "activated_directives: null\nactivated_tactics: []\n")
write(sandbox, f"i/{CONFIG}", "activated_directives: notalist\n")
write(sandbox, f"j/{CONFIG}", 'activated_directives: ["010-b", "010-a", "010-b"]\n')
(sandbox / "k").mkdir(parents=True, exist_ok=True)
write(sandbox, f"l/{CONFIG}", "activated_directives: []\n")
write(sandbox, f"m/{CONFIG}", "activated_directives: [010-x]\nbogus_key: []\n")
write(sandbox, f"n/{LEDGER}", "activated_directives: [!!timestamp 2021-13-45]\n")

out = {}

def record(step, root, report_stray=True):
    try:
        ledger = al.load_activation_ledger(root, report_stray=report_stray)
    except BaseException:  # noqa: BLE001 - a refusal is the recorded datum
        out[step] = {"outcome": "refused"}
        return
    out[step] = {
        "outcome": "ok",
        "source": ledger.source.name,
        "exists": ledger.exists,
        "raw_keys": ledger.raw(),
    }

for name in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n"]:
    record(f"case_{name}", sandbox / name)
record("inspection_a", sandbox / "a", report_stray=False)

Path(sys.argv[2]).write_text(json.dumps(out, sort_keys=True), encoding="utf-8")
"##
}

/// Run the Source child; return its receipt.
fn run_source(sandbox: &Sandbox) -> Json {
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
    serde_json::from_slice(&fs::read(&receipt).expect("source receipt file"))
        .expect("source receipt json")
}

/// Re-exec this test binary as the Native child and return its receipt.
fn run_native(sandbox: &Sandbox) -> Json {
    let receipt = sandbox.path("native-receipt.json");
    let _ = fs::remove_file(&receipt);
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args([
            "--exact",
            "the_scalar_read_is_the_exact_read_for_source_valid_legacy_lists",
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
    serde_json::from_slice(&fs::read(&receipt).expect("native receipt file"))
        .expect("native receipt json")
}

#[test]
fn the_scalar_read_is_the_exact_read_for_source_valid_legacy_lists() {
    if let Some(root) = std::env::var_os(CHILD_ENV) {
        let sandbox = Sandbox {
            root: PathBuf::from(root),
        };
        sandbox.assert_isolation();
        write_receipt(&native_receipt());
        return;
    }

    // Fail closed on the oracle before either child runs.
    let _ = oracle::python();
    let source_sandbox = Sandbox::new();
    let native_sandbox = Sandbox::new();
    assert_ne!(source_sandbox.root, native_sandbox.root);
    seed(&source_sandbox.path("cwd"));
    seed(&native_sandbox.path("cwd"));

    let source = run_source(&source_sandbox);
    let native = run_native(&native_sandbox);
    assert_eq!(
        native,
        source,
        "the scalar read must be the same read truth as the reference\nnative={}\nsource={}",
        serde_json::to_string_pretty(&native).unwrap(),
        serde_json::to_string_pretty(&source).unwrap()
    );

    // Non-vacuity: the defect case really carries the reference's `str()` ids,
    // and the strict modern cases really refuse. Spelled here independently of
    // the implementation, not copied from the Native output.
    assert_eq!(
        source["case_a"]["raw_keys"]["activated_directives"],
        json!(["True", "123", "None", "[1, 2]", "{'a': 1}", "1.5"])
    );
    assert_eq!(
        source["case_b"]["raw_keys"]["activated_directives"],
        json!(["2021-01-02"])
    );
    assert_eq!(
        source["case_e"]["raw_keys"]["activated_directives"],
        json!(["010-x", "002-y", "010-x"])
    );
    assert_eq!(
        source["case_j"]["raw_keys"]["activated_directives"],
        json!(["010-b", "010-a", "010-b"])
    );
    for case in ["f", "g", "i", "n"] {
        assert_eq!(
            source[format!("case_{case}")]["outcome"],
            "refused",
            "reference must refuse case {case}"
        );
        assert_eq!(
            native[format!("case_{case}")]["outcome"],
            "refused",
            "Native must refuse case {case}"
        );
    }
    assert_eq!(source["case_k"]["exists"], false);
    assert_eq!(source["case_k"]["source"], "LEDGER");
    assert_eq!(
        source["case_h"]["raw_keys"],
        json!({"activated_tactics": []})
    );
}

/// The checked projection is literal: on a scalar-expressible ledger the scalar
/// read IS `exact.to_scalar_view()`, field for field.
#[test]
fn scalar_read_equals_the_checked_projection_of_the_exact_read() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        CONFIG,
        "activated_directives: [true, 123]\nactivated_tactics: [t-1]\n",
    );
    let exact = load_activation_ledger_exact(root).unwrap();
    let scalar = load_activation_ledger(root).unwrap();
    assert_eq!(exact.to_scalar_view().unwrap(), scalar);
    assert_eq!(
        scalar.raw()["activated_directives"],
        Some(vec!["True".to_owned(), "123".to_owned()])
    );
}

/// Strictness is not traded away: modern refusals stay the SAME existing
/// Doctrine variants (the exact reader already classifies them as Doctrine and
/// the scalar view must not rewrap them into the new typed cause).
#[test]
fn modern_refusals_keep_their_existing_doctrine_variants() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, LEDGER, "activated_directives: [true]\n");
    match load_activation_ledger(root) {
        Err(DoctrineError::InvalidLedgerValue { key, .. }) => {
            assert_eq!(key, "activated_directives");
        }
        other => panic!("expected the existing InvalidLedgerValue refusal, got {other:?}"),
    }

    write(root, LEDGER, "bogus_key: []\n");
    match load_activation_ledger(root) {
        Err(DoctrineError::UnknownLedgerKey { key, .. }) => assert_eq!(key, "bogus_key"),
        other => panic!("expected the existing UnknownLedgerKey refusal, got {other:?}"),
    }

    // The legacy surface is a different source: drop the modern ledger so the
    // pointed/legacy config is what the resolver reads.
    fs::remove_file(root.join(LEDGER)).expect("drop the ledger fixture");
    write(root, CONFIG, "activated_directives: notalist\n");
    match load_activation_ledger(root) {
        Err(DoctrineError::InvalidLedgerValue { key, .. }) => {
            assert_eq!(key, "activated_directives");
        }
        other => panic!("expected the existing InvalidLedgerValue refusal, got {other:?}"),
    }
}

/// The one boxed typed cause: a constructor abort and an unprojectable token are
/// carried with their real phase/class/message, never a healthy read and never a
/// fake IO error.
#[test]
fn non_doctrine_failures_keep_their_typed_cause() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        LEDGER,
        "activated_directives: [!!timestamp 2021-13-45]\n",
    );
    match load_activation_ledger(root) {
        Err(DoctrineError::ActivationRead { source }) => match *source {
            ActivationReadError::Aborted {
                phase,
                python,
                message,
                ..
            } => {
                assert!(phase.contains("activation document"), "phase: {phase}");
                assert_eq!(python, "ValueError");
                assert!(
                    message.contains("month must be in 1..12"),
                    "real cause retained: {message}"
                );
            }
            other => panic!("expected the runtime abort, got {other:?}"),
        },
        other => panic!("expected the typed abort cause, got {other:?}"),
    }

    // A lone surrogate is Source-valid data with no scalar projection: the
    // checked view refuses explicitly instead of dropping or substituting it.
    write(root, LEDGER, "deactivated_directives: [\"\\uD800\"]\n");
    match load_activation_ledger(root) {
        Err(DoctrineError::ActivationRead { source }) => {
            assert!(matches!(*source, ActivationReadError::ScalarProjection(_)));
        }
        other => panic!("expected the scalar projection refusal, got {other:?}"),
    }
}

/// One actual existing scalar consumer: a preset/project activation state built
/// from a Source-valid mixed legacy list. The coerced id resolves through the
/// shared identity index and really gates admission; the unresolvable coerced
/// tokens are carried as unknown rather than refusing the read.
#[test]
fn existing_project_activation_consumer_reads_the_coerced_legacy_ids() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let built_in = root.join("built-in");
    write(
        &built_in,
        "directives/123-int.directive.yaml",
        "id: \"123\"\nname: Legacy numeric id\n",
    );
    write(root, CONFIG, "activated_directives: [true, 123]\n");
    let state = project_activation_state(root, &built_in, &[], None).unwrap();
    assert!(
        state.admits("directive", "123"),
        "the coerced int id must gate admission through the real consumer"
    );
    assert!(
        !state.admits("directive", "999-missing"),
        "a configured allow set still excludes unlisted ids"
    );
    assert_eq!(
        state.unknown_allow_tokens.get("directive"),
        Some(&vec!["True".to_owned()]),
        "the unresolvable coerced token is reported, never silently applied"
    );
}
