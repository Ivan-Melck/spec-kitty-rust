//! Source-first proof for the RETROSPECTIVE WRITER (bytes, phase, error, mode).
//!
//! Source writes the record with
//! `kernel.yaml_io.write_mapping_atomic(record.to_dict(), path, width=120)`:
//! ruamel round-trip dump at 120 columns, the guarded ASCII trailing-space/tab/CR
//! normalization, a temp file created with mode `0o644` (so the process UMASK
//! applies), the file fsynced, the rename atomic, and a best-effort parent
//! directory fsync.
//!
//! The adapter now serializes through the ONE canonical emitter
//! (`sk_charter::ordered_yaml_width_normalized`) and publishes with the kernel's
//! atomic writer; this module performs NO post-processing of the emitter's
//! bytes. This test binds:
//!
//! * BYTES for a fixed record (no clock): long prose wrapped at 120 columns, a
//!   timestamp `str` (which ruamel quotes because its resolver would type it),
//!   unicode, an embedded newline, an empty optional, and the full nested
//!   `created_by` / `provenance` / finding / evidence shapes;
//! * PHASE: the Native writer fails on a directory destination and leaves no
//!   temporary file; paired Source publication/error cases are additionally
//!   exercised by Kernel's `requested_atomic_source` integration test;
//! * MODE: Source's resulting mode is `0o644 & ~umask` (measured inside the
//!   Source process, which reads and restores its own umask). The Unix adapter
//!   must match that formula exactly; a legacy 0600 result is not allowed.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_workspace::retrospective::{
    EvidenceRef, Finding, RetrospectiveRecord, write_retrospective_record,
};

const SLUG: &str = "fx-01M6WRITER";

const ORACLE: &str = r#"
import json, os, sys
from pathlib import Path
root = Path(sys.argv[1])
assert Path.cwd() == root / 'cwd'
assert sys.version_info[:3] == (3, 11, 15)
for key,name in [('HOME','home'),('XDG_CACHE_HOME','cache'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == root / name, (key, os.environ.get(key))
from specify_cli.retrospective.schema import (
    GenActor, GenEvidenceRef, GenFinding, GenProvenance, GenRetrospectiveRecord,
)
from specify_cli.retrospective.writer import write_gen_record

root = Path(sys.argv[1])
slug = sys.argv[2]
detail_kind = sys.argv[3] if len(sys.argv) > 3 else "plain"
if detail_kind == "newline":
    details = (
        "This detail is deliberately longer than one hundred and twenty characters so the "
        "emitter folds it exactly as Source folds a real Finding.details field, and it also "
        "carries an embedded newline:\nsecond paragraph with trailing words."
    )
else:
    details = (
        "This detail is deliberately longer than one hundred and twenty characters so the "
        "emitter folds it exactly as Source folds a real Finding.details field, and it ends "
        "with ordinary words."
    )
before = os.umask(0)
os.umask(before)
record = GenRetrospectiveRecord(
    mission_id="01M6WRITER0000000000000000",
    mission_slug=slug,
    mission_number=7,
    friendly_name="café 日本語 🎉",
    mission_type="software-dev",
    target_branch="main",
    created_at="2026-11-05T00:00:00Z",
    created_by=GenActor(kind="runtime", id="spec-kitty-generator", display=None),
    provenance=GenProvenance(
        kind="runtime_post_completion",
        invoked_at="2026-11-05T00:00:01Z",
        policy_resolved_from={"enabled": "<default>", "timing": "<default>", "precedence": "<default>"},
    ),
    policy_source={"enabled": "<default>", "timing": "<default>", "precedence": "<default>"},
    generator_version="1.0",
    findings_status="has_findings",
    helped=[GenFinding(
        id="h-001",
        category="process",
        summary="a short helped summary",
        evidence_refs=["e-001"],
        details=details,
    )],
    not_helpful=[],
    gaps=[],
    proposals=[],
    evidence_refs=[GenEvidenceRef(id="e-001", kind="file", path="kitty-specs/%s/status.events.jsonl" % slug)],
)
path = write_gen_record(record, mode="error", repo_root=root)
data = path.read_bytes()
print(json.dumps({
    "path": str(path),
    "mode": path.stat().st_mode & 0o777,
    "umask": before,
    "bytes": data.decode("utf-8"),
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
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("observed text must be UTF-8, never lossy")
}

fn isolated_native_test(name: &str) -> bool {
    let variables = [
        ("HOME", "home"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ];
    if let Some(root) = std::env::var_os("SK_RETROSPECTIVE_NATIVE_ROOT") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root.join("cwd"));
        for (key, directory) in variables {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(directory));
        }
        return false;
    }
    let sandbox = Sandbox::new();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(sandbox.path().join("cwd"))
        .env("SK_RETROSPECTIVE_NATIVE_ROOT", sandbox.path())
        .env(
            "PYTHONPATH",
            format!(
                "{}:{}",
                repo_root().display(),
                repo_root().join("src").display()
            ),
        )
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"));
    for (key, directory) in variables {
        command.env(key, sandbox.path().join(directory));
    }
    let output = command.output().unwrap();
    fs::write(sandbox.path().join("native.stdout"), &output.stdout).unwrap();
    fs::write(sandbox.path().join("native.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "isolated Native: {} {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    true
}

fn source_record(sandbox: &Sandbox, detail_kind: &str) -> serde_json::Value {
    let script = sandbox.path().join("retrospective-writer-oracle.py");
    fs::write(&script, ORACLE).expect("write oracle");
    let output: Output = Command::new(oracle::python())
        .arg(&script)
        .arg(sandbox.path())
        .arg(SLUG)
        .arg(detail_kind)
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
        .expect("run the pinned Source retrospective writer");
    assert!(
        output.status.success(),
        "the pinned Source retrospective writer failed (a harness/premise failure, not a business red):\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("source json")
}

fn native_record(detail_kind: &str) -> RetrospectiveRecord {
    let mut policy_source = std::collections::BTreeMap::new();
    policy_source.insert("enabled".to_owned(), "<default>".to_owned());
    policy_source.insert("timing".to_owned(), "<default>".to_owned());
    policy_source.insert("precedence".to_owned(), "<default>".to_owned());
    RetrospectiveRecord {
        schema_version: 1,
        mission_id: "01M6WRITER0000000000000000".to_owned(),
        mission_slug: SLUG.to_owned(),
        mission_number: Some(7),
        friendly_name: "café 日本語 🎉".to_owned(),
        mission_type: "software-dev".to_owned(),
        target_branch: "main".to_owned(),
        created_at: "2026-11-05T00:00:00Z".to_owned(),
        created_by_kind: "runtime".to_owned(),
        created_by_id: "spec-kitty-generator".to_owned(),
        created_by_display: None,
        provenance_kind: "runtime_post_completion".to_owned(),
        provenance_invoked_at: "2026-11-05T00:00:01Z".to_owned(),
        policy_source,
        findings_status: "has_findings",
        helped: vec![Finding {
            id: "h-001".to_owned(),
            category: "process".to_owned(),
            summary: "a short helped summary".to_owned(),
            details: Some(if detail_kind == "newline" {
                "This detail is deliberately longer than one hundred and twenty characters so the \
                 emitter folds it exactly as Source folds a real Finding.details field, and it also \
                 carries an embedded newline:\nsecond paragraph with trailing words."
                    .to_owned()
            } else {
                "This detail is deliberately longer than one hundred and twenty characters so the \
                 emitter folds it exactly as Source folds a real Finding.details field, and it ends \
                 with ordinary words."
                    .to_owned()
            }),
            evidence_refs: vec!["e-001".to_owned()],
        }],
        not_helpful: Vec::new(),
        gaps: Vec::new(),
        generator_version: "1.0".to_owned(),
        evidence_refs: vec![EvidenceRef {
            id: "e-001".to_owned(),
            kind: "file".to_owned(),
            path: Some(format!("kitty-specs/{SLUG}/status.events.jsonl")),
            range: None,
            url: None,
        }],
    }
}

#[test]
fn the_retrospective_writer_emits_the_source_bytes_and_phase() {
    if isolated_native_test("the_retrospective_writer_emits_the_source_bytes_and_phase") {
        return;
    }
    let sandbox = Sandbox::new();
    let source = source_record(&sandbox, "plain");
    let source_bytes = source["bytes"].as_str().expect("source bytes").to_owned();

    let feature_dir = sandbox.path().join("native").join("kitty-specs").join(SLUG);
    let path = write_retrospective_record(&native_record("plain"), &feature_dir)
        .expect("the Native writer publishes the record");
    let native_bytes = fs::read_to_string(&path).expect("native bytes");

    if native_bytes != source_bytes {
        let source_lines: Vec<&str> = source_bytes.lines().collect();
        let native_lines: Vec<&str> = native_bytes.lines().collect();
        let mut report = String::new();
        for index in 0..source_lines.len().max(native_lines.len()) {
            let left = source_lines.get(index).copied().unwrap_or("<absent>");
            let right = native_lines.get(index).copied().unwrap_or("<absent>");
            if left != right {
                report.push_str(&format!(
                    "line {}:\n  source: {left:?}\n  native: {right:?}\n",
                    index + 1
                ));
                if report.lines().count() > 30 {
                    break;
                }
            }
        }
        panic!(
            "retrospective.yaml BYTES differ from Source:\n{report}\nsource len={} native len={}",
            source_bytes.len(),
            native_bytes.len()
        );
    }

    // The timestamp `str` is quoted the way ruamel quotes it (its resolver would
    // otherwise type-resolve it), and the long prose is folded at the width.
    assert!(
        source_bytes.contains("created_at: '2026-11-05T00:00:00Z'"),
        "a timestamp string is quoted exactly as Source quotes it: {source_bytes}"
    );
    assert!(
        source_bytes.contains("invoked_at: '2026-11-05T00:00:01Z'"),
        "the provenance timestamp is quoted too"
    );
    assert!(
        source_bytes.contains("café 日本語 🎉"),
        "unicode scalars stay raw UTF-8"
    );
    assert!(
        source_bytes.lines().all(|line| line.chars().count() <= 120),
        "every emitted line respects the 120-column width"
    );

    // ---- MODE, measured on both sides against Source's own umask ----
    let source_mode = source["mode"].as_u64().expect("source mode");
    let source_umask = source["umask"].as_u64().expect("source umask");
    let expected_mode = 0o644u64 & !source_umask;
    assert_eq!(
        source_mode, expected_mode,
        "Source's file is 0o644 with the process umask applied (umask {source_umask:o})"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let native_mode = u64::from(
            fs::metadata(&path)
                .expect("native metadata")
                .permissions()
                .mode()
                & 0o777,
        );
        assert_eq!(
            native_mode, expected_mode,
            "requested creation mode must match Source's actual umask, not a 0600 allowance"
        );
    }
}

#[test]
fn a_failed_publish_leaves_no_temporary_file_and_no_record() {
    if isolated_native_test("a_failed_publish_leaves_no_temporary_file_and_no_record") {
        return;
    }
    let sandbox = Sandbox::new();
    let feature_dir = sandbox
        .path()
        .join("blocked")
        .join("kitty-specs")
        .join(SLUG);
    fs::create_dir_all(&feature_dir).expect("feature dir");
    // A DIRECTORY where the record should be: the atomic publish cannot replace
    // it, and the writer must report the IO failure.
    fs::create_dir_all(feature_dir.join("retrospective.yaml")).expect("blocking dir");
    let error = write_retrospective_record(&native_record("plain"), &feature_dir)
        .expect_err("a directory at the record path refuses the publish");
    assert!(
        error.to_string().contains("retrospective.yaml"),
        "the refusal names the record path: {error}"
    );
    let leftovers: Vec<String> = fs::read_dir(&feature_dir)
        .expect("read feature dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "retrospective.yaml")
        .collect();
    assert!(
        leftovers.is_empty(),
        "a failed publish leaves NO temporary file behind: {leftovers:?}"
    );
    assert!(
        feature_dir.join("retrospective.yaml").is_dir(),
        "the blocking entry is untouched (nothing was partially published)"
    );
}

/// Escaped newlines must obey the requested width in the canonical emitter.
#[test]
fn an_escaped_newline_scalar_matches_source_bytes() {
    if isolated_native_test("an_escaped_newline_scalar_matches_source_bytes") {
        return;
    }
    let sandbox = Sandbox::new();
    let source = source_record(&sandbox, "newline");
    let source_bytes = source["bytes"].as_str().expect("source bytes").to_owned();
    let feature_dir = sandbox
        .path()
        .join("native-newline")
        .join("kitty-specs")
        .join(SLUG);
    let path = write_retrospective_record(&native_record("newline"), &feature_dir)
        .expect("the Native writer publishes the record");
    let native_bytes = fs::read_to_string(&path).expect("native bytes");

    let source_detail = source_bytes
        .lines()
        .position(|line| line.trim_start().starts_with("details:"))
        .expect("source details line");
    let native_detail = native_bytes
        .lines()
        .position(|line| line.trim_start().starts_with("details:"))
        .expect("native details line");
    assert_eq!(
        source_detail, native_detail,
        "everything before the escaped scalar is byte-identical"
    );
    assert!(
        source_bytes
            .lines()
            .nth(source_detail)
            .expect("source line")
            .chars()
            .count()
            > 120,
        "Source's double-quoted scalar breaks only after the width: {:?}",
        source_bytes.lines().nth(source_detail)
    );
    assert_eq!(
        native_bytes, source_bytes,
        "whole Source bytes, no folding waiver"
    );
    // Re-parsing is an additional safety check, not a substitute for bytes.
    let source_value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&source_bytes).expect("source parses");
    let native_value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&native_bytes).expect("native parses");
    assert_eq!(
        native_value, source_value,
        "byte-identical documents also retain the same semantic value"
    );
}
