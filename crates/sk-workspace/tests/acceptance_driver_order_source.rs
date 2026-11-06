//! Source-first proof for the ACCEPTANCE-MATRIX driver's `extras` ORDER and its
//! non-finite / big-integer numbers, through the ONE J1 Kernel codec.
//!
//! Source reads each side with `json.loads` (dict insertion order, `NaN` /
//! `Infinity` accepted), merges each row with `dict.fromkeys((*base, *ours,
//! *theirs))`, projects `AcceptanceMatrix.from_dict(...).to_dict()` (declared
//! fields then `model_extra` in that insertion order) and writes
//! `json.dumps(merged, indent=2) + "\n"` — `sort_keys=False`.
//!
//! The adapter used to lose that order at `serde_json` parse time (a sorted
//! `Map`), so unknown row keys came out sorted instead of in the file's order.
//! H4 reads the sides with the ordered Kernel parser (`parse` →
//! `ExactObject`), carries the order beside the merge, and renders through
//! `render_python_with`.
//!
//! Cases:
//! * a criterion row whose unknown keys are stored out of alphabetical order;
//! * a document-level unknown key;
//! * an extras union across base/ours/theirs, which must follow
//!   `dict.fromkeys` (base keys first, then ours-only, then theirs-only);
//! * a non-finite `Infinity` extras value (Source accepts and re-emits it);
//! * a 40-digit integer, emitted exactly.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_workspace::merge_drivers::run_driver_paths;

const ORACLE: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.consolidation.drivers import run_acceptance_matrix_driver

root = Path(sys.argv[1])
run_acceptance_matrix_driver(
    str(root / "base.json"), str(root / "ours.json"), str(root / "theirs.json")
)
sys.stdout.write((root / "ours.json").read_text(encoding="utf-8"))
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
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn source_bytes(sandbox: &Sandbox, base: &str, ours: &str, theirs: &str) -> String {
    let script = sandbox.path().join("acceptance-driver-oracle.py");
    fs::write(&script, ORACLE).expect("write oracle");
    fs::write(sandbox.path().join("base.json"), base).expect("base");
    fs::write(sandbox.path().join("ours.json"), ours).expect("ours");
    fs::write(sandbox.path().join("theirs.json"), theirs).expect("theirs");
    let output: Output = Command::new(oracle::python())
        .arg(&script)
        .arg(sandbox.path())
        .current_dir(sandbox.path().join("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", sandbox.path().join("home"))
        .env("XDG_CACHE_HOME", sandbox.path().join("cache"))
        .env("XDG_CONFIG_HOME", sandbox.path().join("config"))
        .env("XDG_DATA_HOME", sandbox.path().join("data"))
        .env("XDG_STATE_HOME", sandbox.path().join("state"))
        .env("XDG_RUNTIME_DIR", sandbox.path().join("runtime"))
        .env("TMPDIR", sandbox.path().join("tmp"))
        .env("TMP", sandbox.path().join("tmp"))
        .env("TEMP", sandbox.path().join("tmp"))
        .env(
            "PYTHONPATH",
            format!(
                "{}:{}",
                repo_root().display(),
                repo_root().join("src").display()
            ),
        )
        .env(
            "SPEC_KITTY_PACKS_ROOT",
            repo_root().join("packs").display().to_string(),
        )
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .output()
        .expect("run the pinned Source acceptance driver");
    assert!(
        output.status.success(),
        "the pinned Source acceptance driver failed (a harness/premise failure, not a business red):\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stdout)
}

/// Run the NATIVE driver through the real path entry, so the physical write
/// (open/truncate → encode → write) is exercised exactly as a git merge driver
/// would, and read back the bytes it actually published.
fn native_bytes(base: &str, ours: &str, theirs: &str) -> String {
    let dir = tempfile::tempdir().expect("driver sandbox");
    let base_path = dir.path().join("base");
    let ours_path = dir.path().join("ours");
    let theirs_path = dir.path().join("theirs");
    fs::write(&base_path, base).expect("base");
    fs::write(&ours_path, ours).expect("ours");
    fs::write(&theirs_path, theirs).expect("theirs");
    run_driver_paths(
        "merge-driver-acceptance-matrix",
        &base_path,
        &ours_path,
        &theirs_path,
    )
    .expect("the Native acceptance driver resolves");
    fs::read_to_string(&ours_path).expect("the driver published the matrix")
}

const MATRIX_BASE: &str = r#"{
  "mission_slug": "fx-01M6ORDER",
  "mission_number": 7,
  "mission_type": "software-dev",
  "criteria": [
    {
      "criterion_id": "FR-001",
      "description": "works",
      "proof_type": "test",
      "evidence": "cargo test",
      "pass_fail": "pass",
      "verified_by": "reviewer",
      "verified_at": "2026-11-05T00:00:00Z",
      "notes": null
    }
  ],
  "negative_invariants": []
}
"#;

#[test]
fn unknown_row_keys_keep_the_source_insertion_order_and_exact_numbers() {
    let sandbox = Sandbox::new();
    // ours carries the unknown keys OUT of alphabetical order, plus a document
    // extra, a non-finite float and a 40-digit integer.
    let ours = r#"{
  "mission_slug": "fx-01M6ORDER",
  "mission_number": 7,
  "mission_type": "software-dev",
  "zeta_document_extra": "doc",
  "criteria": [
    {
      "criterion_id": "FR-001",
      "description": "works",
      "proof_type": "test",
      "evidence": "cargo test",
      "pass_fail": "pass",
      "verified_by": "reviewer",
      "verified_at": "2026-11-05T00:00:00Z",
      "notes": null,
      "zzz_first": 1,
      "aaa_second": Infinity,
      "mmm_third": 1234567890123456789012345678901234567890
    }
  ],
  "negative_invariants": []
}
"#;
    let theirs = MATRIX_BASE;

    let source = source_bytes(&sandbox, MATRIX_BASE, ours, theirs);
    let native = native_bytes(MATRIX_BASE, ours, theirs);
    assert_eq!(
        native, source,
        "the merged acceptance matrix is BYTE-identical to Source"
    );
    // The order assertions are made on the output itself, not implied by the
    // byte equality, so a future divergence names the field.
    let position = |needle: &str| {
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} is absent from {source}"))
    };
    assert!(
        position("zzz_first") < position("aaa_second"),
        "unknown row keys keep the file's order (zzz before aaa): {source}"
    );
    assert!(
        position("aaa_second") < position("mmm_third"),
        "unknown row keys keep the file's order (aaa before mmm): {source}"
    );
    assert!(
        source.contains("\"aaa_second\": Infinity"),
        "a non-finite extras value is written exactly as Source writes it: {source}"
    );
    assert!(
        source.contains("\"mmm_third\": 1234567890123456789012345678901234567890"),
        "a 40-digit integer keeps its exact token: {source}"
    );
    // Source's `AcceptanceMatrix` model takes extras on ROWS (`criteria` /
    // `negative_invariants`) only; a document-level unknown key is dropped by
    // `from_dict(...).to_dict()`. The byte equality above already binds that.
    assert!(
        !source.contains("zeta_document_extra"),
        "a document-level unknown key is dropped exactly as Source drops it: {source}"
    );
}

#[test]
fn a_row_added_on_both_sides_orders_its_extras_as_dict_fromkeys_does() {
    let sandbox = Sandbox::new();
    // The row is absent from base and present on BOTH sides with different
    // extras: Source's `dict.fromkeys((*base, *ours, *theirs))` puts ours'
    // keys first, then theirs-only keys.
    let ours = r#"{
  "mission_slug": "fx-01M6ORDER",
  "mission_number": 7,
  "mission_type": "software-dev",
  "criteria": [
    {
      "criterion_id": "FR-009",
      "description": "new",
      "proof_type": "test",
      "evidence": "shared-evidence",
      "pass_fail": "pass",
      "verified_by": "reviewer",
      "verified_at": "2026-11-05T00:00:00Z",
      "notes": null,
      "ours_extra_z": "z",
      "ours_extra_a": "a"
    }
  ],
  "negative_invariants": []
}
"#;
    let theirs = r#"{
  "mission_slug": "fx-01M6ORDER",
  "mission_number": 7,
  "mission_type": "software-dev",
  "criteria": [
    {
      "criterion_id": "FR-009",
      "description": "new",
      "proof_type": "test",
      "evidence": "shared-evidence",
      "pass_fail": "pass",
      "verified_by": "reviewer",
      "verified_at": "2026-11-05T00:00:00Z",
      "notes": "theirs-note",
      "theirs_extra_m": "m",
      "ours_extra_a": "a"
    }
  ],
  "negative_invariants": []
}
"#;

    let source = source_bytes(&sandbox, MATRIX_BASE, ours, theirs);
    let native = native_bytes(MATRIX_BASE, ours, theirs);
    assert_eq!(native, source, "the merged row is BYTE-identical to Source");
    let position = |needle: &str| {
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} is absent from {source}"))
    };
    assert!(
        position("ours_extra_z") < position("ours_extra_a"),
        "ours' own key order is preserved: {source}"
    );
    assert!(
        position("ours_extra_a") < position("theirs_extra_m"),
        "a theirs-only key is appended after ours' keys: {source}"
    );
}
