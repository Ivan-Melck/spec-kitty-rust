//! Source-first proof of the EXACT retrospective-YAML wire, through the ONE
//! canonical emitter.
//!
//! Source writes `retrospective.yaml` with
//! `kernel.yaml_io.write_mapping_atomic(data, path, width=120)`, i.e.
//! `ruamel.yaml YAML(typ="rt")`, `default_flow_style=False`,
//! `preserve_quotes=True`, `yaml.width = 120`, then the #3057
//! trailing-whitespace normalizer. The canonical Rust emitter for that flavour is
//! `sk_charter::ordered_yaml_width(body, width)` — one emitter
//! with an explicit width parameter, which already documents the same
//! `preserve_quotes = True` / `width` settings.
//!
//! This test binds the two BYTE for byte over the document shapes the
//! retrospective record actually carries: long prose that must wrap at 120
//! columns, strings that look like YAML scalars, unicode, control characters,
//! quote styles, empty values, nested mappings and sequences, and a long
//! unbreakable token that must NOT be folded. Both sides load the SAME document
//! text, so the comparison isolates the emitter (no hand-built structure can
//! drift between the two sides).
//!
//! If this is green, the remaining wiring is a manifest edge
//! (`sk-charter` → `sk-workspace`) or a `sk-charter`-owned retrospective API —
//! NOT a missing emitter and NOT a reason to copy one.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ORACLE: &str = r#"
import json, os, sys
from pathlib import Path
source = Path(sys.argv[1])
assert Path.cwd() == source / 'cwd'
assert sys.version_info[:3] == (3, 11, 15)
for key, name in [('HOME','home'),('XDG_CACHE_HOME','cache'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == source / name, (key, os.environ.get(key))
from kernel.yaml_io import write_mapping_atomic
from ruamel.yaml import YAML
document_text = Path(sys.argv[2]).read_text(encoding="utf-8")
loader = YAML(typ="safe")
data = loader.load(document_text)
out = source / "retrospective.source.yaml"
write_mapping_atomic(data, out, width=120)
sys.stdout.write(out.read_text(encoding="utf-8"))
"#;

/// The document, written once as YAML text so both sides parse the identical
/// value (order included).
const DOCUMENT: &str = r##"---
mission_slug: fx-01M6YAMLWIRE
mission_id: 01M6YAMLWIRE0000000000000000
created_at: "2026-11-05T00:00:00Z"
status: completed
findings_status: recorded
generator_version: 3.2.0
provenance:
  kind: runtime_post_completion
  invoked_at: "2026-11-05T00:00:00Z"
  actor: operator
summary:
  headline: >-
    A long prose field that the emitter must wrap at one hundred and twenty columns without
    losing a single character or leaving a trailing space on any wrapped continuation line.
  nested:
    deeper:
      value: 1
      other: null
findings:
  - finding_id: F-001
    category: process
    severity: high
    note: >-
      This note is deliberately much longer than one hundred and twenty characters so that the
      emitter's folding behaviour is exercised on a real prose scalar, exactly as the retrospective
      writer's 2000-character `Finding.note` field would be in a genuine record.
    evidence_refs:
      - kitty-specs/fx-01M6YAMLWIRE/status.events.jsonl
      - kitty-specs/fx-01M6YAMLWIRE/traces/trace-1.md
  - finding_id: F-002
    category: friction
    severity: low
    note: short
    unscaled_token: abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz
scalar_torture:
  looks_boolean: "yes"
  looks_null: "null"
  looks_number: "123"
  looks_float: "1.5"
  looks_empty: ""
  null_value: null
  empty_list: []
  empty_mapping: {}
  hash: "# not a comment"
  colon: "a: b"
  star: "*star"
  anchor: "&anchor"
  tag: "!tag"
  percent: "%percent"
  at: "@at"
  tick: "`tick`"
  quote_single: "it's fine"
  quote_double: 'she said "hi"'
  unicode: "café 日本語 🎉"
  control: "\u0001"
  newline: "line one\nline two\n"
  tab: "a\tb"
  datetime_like: "2026-11-05"
numbers:
  integer: 42
  negative: -7
  float: 1.25
  big: 12345678901234567890
  boolean_true: true
  boolean_false: false
"weird key with spaces": value
"unicode_key_é": 1
"#hashkey": 2
"empty_value_key":
proposals: []
"##;

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
    String::from_utf8(bytes.to_vec()).expect("observed YAML/text boundary must be valid UTF-8")
}

fn source_bytes(sandbox: &Sandbox, document_text: &str) -> String {
    let script = sandbox.path().join("yaml-wire-oracle.py");
    fs::write(&script, ORACLE).expect("write oracle");
    let document = sandbox.path().join("document.yaml");
    fs::write(&document, document_text).expect("write document");
    let output: Output = Command::new(oracle::python())
        .arg(&script)
        .arg(sandbox.path())
        .arg(&document)
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
        .expect("run the pinned Source YAML writer");
    assert!(
        output.status.success(),
        "the pinned Source YAML writer failed (a harness/premise failure, not a business red):\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stdout)
}

#[test]
fn the_canonical_emitter_reproduces_the_source_retrospective_yaml_bytes_at_width_120() {
    let sandbox = Sandbox::new();
    let source = source_bytes(&sandbox, DOCUMENT);
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(DOCUMENT).expect("document parses");
    let native = text(&sk_charter::ordered_yaml_width(&value, 120));
    // Source's writer is the emitter PLUS `kernel.yaml_io`'s #3057
    // `_normalize_nonsemantic_trailing_whitespace`: every line is right-trimmed
    // of spaces/tabs/CR only when a re-parse proves the meaning is unchanged.
    // The rule is reproduced here (nothing else) so the test can state exactly
    // what the canonical API still needs; the safety re-parse is asserted below.
    let normalize = |text: &str| -> String {
        text.split('\n')
            .map(|line| line.trim_end_matches([' ', '\t', '\r']))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let source_normalized = normalize(&source);
    let native_normalized = normalize(&native);
    if source == normalize(&source) && native != source {
        // The source bytes carry no trailing whitespace, so a difference here
        // would be a real emitter difference, not the #3057 artifact.
    }
    if native_normalized != source_normalized {
        // Report the first differing line with context: a byte diff on a long
        // line is otherwise unreadable.
        let source_lines: Vec<&str> = source.lines().collect();
        let native_lines: Vec<&str> = native.lines().collect();
        let mut report = String::new();
        for index in 0..source_lines.len().max(native_lines.len()) {
            let left = source_lines.get(index).copied().unwrap_or("<absent>");
            let right = native_lines.get(index).copied().unwrap_or("<absent>");
            if left != right {
                report.push_str(&format!(
                    "line {}:\n  source: {left:?}\n  native: {right:?}\n",
                    index + 1
                ));
                if report.lines().count() > 24 {
                    break;
                }
            }
        }
        panic!(
            "retrospective.yaml BYTES differ from Source beyond the #3057 trailing-whitespace artifact at width=120:\n{report}\nsource len={} native len={}",
            source.len(),
            native.len()
        );
    }
    assert_eq!(
        native_normalized, source,
        "the canonical emitter + Source's #3057 trailing-whitespace normalization reproduces \
         the retrospective YAML BYTES exactly"
    );
    // The normalization is semantics-preserving: both forms parse to the same
    // document (Source refuses to strip when its own re-parse disagrees; this is
    // the same assertion made with the shared Rust reader).
    let parsed_source: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&source).expect("source yaml re-parses");
    let parsed_native: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&normalize(&native)).expect("normalized native yaml re-parses");
    assert_eq!(
        parsed_native, parsed_source,
        "the normalized bytes carry the same YAML document"
    );
    // And the pre-normalization difference is ONLY trailing whitespace on
    // wrapped/continued lines — the exact, named gap this test exists to record.
    let trailing = source
        .lines()
        .zip(native.lines())
        .filter(|(left, right)| left.trim_end() != *right && left.trim_end() == right.trim_end())
        .count();
    assert!(
        trailing > 0,
        "the emitter's only difference is the #3057 trailing-whitespace artifact"
    );

    // Bind the facts the record actually needs, independent of the diff loop.
    let long_token = "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz";
    for line in source.lines() {
        if line.contains(long_token) {
            // An unbreakable token has no split point: the emitter must NOT
            // invent one (that would change the scalar).
            continue;
        }
        assert!(
            line.chars().count() <= 120,
            "every breakable emitted line respects the 120-column width: {line:?}"
        );
    }
    assert!(
        source.contains("café 日本語 🎉"),
        "unicode scalars stay raw UTF-8 (no ASCII escapes)"
    );
    assert!(
        source.contains("unchanged") || source.contains("line one"),
        "multi-line scalar present"
    );
    assert!(
        source.contains(long_token),
        "an unbreakable token longer than the width is emitted on one line, never folded"
    );
    // Source (ruamel round-trip, YAML 1.2) emits `yes` PLAIN but quotes a
    // string that would otherwise read as null/number, and double-quotes a
    // control character. The byte equality above is the authority; these bind
    // the concrete shapes so a future emitter change names the field.
    assert!(
        source.contains("looks_boolean: yes"),
        "a YAML-1.2 plain string stays plain: {source}"
    );
    assert!(
        source.contains("looks_null: 'null'") && source.contains("looks_number: '123'"),
        "a string that would read as null/number keeps its quotes: {source}"
    );
    assert!(
        source.contains("control: \"\\x01\""),
        "a control character is double-quoted and escaped: {source}"
    );
}

#[test]
fn the_shared_normalized_api_matches_source_without_a_consumer_postprocessor() {
    for document in [
        DOCUMENT.to_owned(),
        format!("{DOCUMENT}nan_value: .nan\ninfinity: .inf\nnegative_infinity: -.inf\n"),
        format!("{DOCUMENT}literal_trailing: |\n  keep these trailing spaces  \n"),
    ] {
        let sandbox = Sandbox::new();
        // Source executes and asserts its actual environment before Native.
        let source = source_bytes(&sandbox, &document);
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&document).unwrap();
        let native = sk_charter::ordered_yaml_width_normalized(&value, 120);
        assert_eq!(
            native,
            source.as_bytes(),
            "canonical API, no caller normalization"
        );
    }
}
