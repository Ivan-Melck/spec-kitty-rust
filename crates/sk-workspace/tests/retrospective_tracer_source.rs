//! Source-first parity for the retrospective TRACER ingestor (FR-007).
//!
//! The PINNED Source functions (`retrospective/generator.py::_parse_trace_entries`,
//! `_classify_trace_disposition`, `_build_trace_findings`) run FIRST on a set of
//! real tracer documents and their output is asserted; the Native port then
//! produces the same entries, the same channels and the same findings (id
//! sequence, category, summary, details, evidence ids and paths).
//!
//! The documents are the shapes the Source tracer writer emits: a numbered
//! `1. **[phase] symptom**` entry, a bullet `- **symptom**` entry, a
//! multi-paragraph entry, `**Fixed —**`/`no gap` dispositions, `candidate gap`
//! markers, unstructured prose (no entries) and a bold span split across lines
//! (not an entry: Source's `(.+?)` never crosses a newline).

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::process::Command;

use sk_workspace::retrospective::{classify_trace_disposition, load_traces, parse_trace_entries};

/// `(id, category, summary, details, evidence refs)` — the finding fields the
/// Source builder produces and this test compares.
type TracerRow = (String, String, String, Option<String>, Vec<String>);

const ORACLE: &str = r##"
import json
from specify_cli.retrospective import generator

TRACERS = {
    "kitty-specs/demo/traces/one.md": (
        "# Tracer\n\n"
        "1. **[implement] code review flagged a missing guard**\n"
        "   The fixture exercised the path and no gap appeared.\n\n"
        "2. **[review] rejection cycle**\n"
        "   candidate gap: the reviewer had to be told twice.\n"
    ),
    "kitty-specs/demo/traces/two.md": (
        "- **build broke on a stale cache**\n"
        "  **Fixed —** by clearing it.\n"
        "- **manual step**\n"
        "  This was expected.\n"
        "- **unknown**\n"
        "  No disposition marker at all.\n"
    ),
    "kitty-specs/demo/traces/three.md": (
        "Just prose, no structured entries here.\n"
        "**bold but not a list item**\n"
    ),
}

out = {"entries": {}, "dispositions": {}, "findings": {}}
reserved = {}
helped, not_helpful, gaps = generator._build_trace_findings(
    traces=sorted(TRACERS.items(), key=lambda item: (item[1], item[0])),
    finding_id_counters={},
    ev_reg=generator._EvidenceRegistry(),
)
for label, bucket in (("helped", helped), ("not_helpful", not_helpful), ("gaps", gaps)):
    out["findings"][label] = [
        {
            "id": f.id,
            "category": f.category,
            "summary": f.summary,
            "details": f.details,
            "evidence_refs": list(f.evidence_refs),
        }
        for f in bucket
    ]

counters = {}
registry = generator._EvidenceRegistry()
for rel, text in TRACERS.items():
    out["entries"][rel] = [list(pair) for pair in generator._parse_trace_entries(text)]
    out["dispositions"][rel] = [
        generator._classify_trace_disposition(body) for _summary, body in generator._parse_trace_entries(text)
    ]
out["evidence"] = [{"id": r.id, "kind": r.kind, "path": r.path} for r in registry.refs]
print(json.dumps(out, sort_keys=True))
"##;

const TRACERS: [(&str, &str); 3] = [
    (
        "kitty-specs/demo/traces/one.md",
        "# Tracer\n\n1. **[implement] code review flagged a missing guard**\n   The fixture exercised the path and no gap appeared.\n\n2. **[review] rejection cycle**\n   candidate gap: the reviewer had to be told twice.\n",
    ),
    (
        "kitty-specs/demo/traces/two.md",
        "- **build broke on a stale cache**\n  **Fixed —** by clearing it.\n- **manual step**\n  This was expected.\n- **unknown**\n  No disposition marker at all.\n",
    ),
    (
        "kitty-specs/demo/traces/three.md",
        "Just prose, no structured entries here.\n**bold but not a list item**\n",
    ),
];

fn source_output() -> serde_json::Value {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = root.join("target/tracer-ingestor-oracle.py");
    std::fs::write(&script, ORACLE).expect("write the oracle script");
    let output = Command::new(oracle::python())
        .arg(&script)
        .current_dir(&root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("target/tracer-oracle-home"))
        .env(
            "PYTHONPATH",
            format!("{}:{}", root.display(), root.join("src").display()),
        )
        .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("run the pinned Source tracer ingestor");
    assert!(
        output.status.success(),
        "the pinned Source tracer ingestor failed (a harness/premise failure, not a business red):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle json")
}

#[test]
fn tracer_entry_parsing_and_disposition_match_the_pinned_source() {
    let source = source_output();
    for (rel, text) in TRACERS {
        let expected_entries: Vec<(String, String)> = source["entries"][rel]
            .as_array()
            .expect("entries")
            .iter()
            .map(|pair| {
                let pair = pair.as_array().expect("pair");
                (
                    pair[0].as_str().expect("summary").to_owned(),
                    pair[1].as_str().expect("body").to_owned(),
                )
            })
            .collect();
        let native_entries = parse_trace_entries(text);
        assert_eq!(
            native_entries, expected_entries,
            "{rel}: parsed tracer entries match Source"
        );
        let expected_dispositions: Vec<String> = source["dispositions"][rel]
            .as_array()
            .expect("dispositions")
            .iter()
            .map(|value| value.as_str().expect("disposition").to_owned())
            .collect();
        let native_dispositions: Vec<String> = native_entries
            .iter()
            .map(|(_, body)| classify_trace_disposition(body).to_owned())
            .collect();
        assert_eq!(
            native_dispositions, expected_dispositions,
            "{rel}: dispositions match Source"
        );
    }
}

/// The findings Source's `_build_trace_findings` builds, bucket by bucket.
///
/// This is asserted through the PUBLIC generator entry point, so it binds the
/// routing, the id sequence (`h-`/`n-`/`g-`), the 200/1000-character
/// truncation, and the evidence registration — not just the parser.
#[test]
fn tracer_findings_buckets_ids_and_truncation_match_the_pinned_source() {
    let source = source_output();
    let root = tempfile::tempdir().expect("sandbox").keep();
    let feature_dir = root.join("kitty-specs/demo");
    std::fs::create_dir_all(feature_dir.join("traces")).expect("traces dir");
    for (rel, text) in TRACERS {
        let name = rel.rsplit('/').next().expect("name");
        std::fs::write(feature_dir.join("traces").join(name), text).expect("tracer");
    }
    // A non-markdown neighbour and an unreadable tracer must both be skipped.
    std::fs::write(feature_dir.join("traces/notes.txt"), "ignored\n").expect("txt");
    std::fs::write(feature_dir.join("traces/broken.md"), [0xff, 0xfe, 0xfd]).expect("bad utf8");

    let traces = load_traces(&feature_dir);
    let names: Vec<&str> = traces.iter().map(|(rel, _)| rel.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "kitty-specs/demo/traces/one.md",
            "kitty-specs/demo/traces/three.md",
            "kitty-specs/demo/traces/two.md",
        ],
        "only readable markdown tracers, sorted by name"
    );

    // The same three documents the oracle used, in the same order.
    let mut ordered: Vec<(String, String)> = TRACERS
        .iter()
        .map(|(rel, text)| ((*rel).to_owned(), (*text).to_owned()))
        .collect();
    ordered.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
    let (helped, not_helpful, gaps) = sk_workspace::retrospective::trace_findings(&ordered);
    let native: [(&str, &Vec<sk_workspace::retrospective::Finding>); 3] = [
        ("helped", &helped),
        ("not_helpful", &not_helpful),
        ("gaps", &gaps),
    ];

    for (bucket, rows) in native {
        let expected: Vec<TracerRow> = source["findings"][bucket]
            .as_array()
            .expect("bucket")
            .iter()
            .map(|row| {
                (
                    row["id"].as_str().expect("id").to_owned(),
                    row["category"].as_str().expect("category").to_owned(),
                    row["summary"].as_str().expect("summary").to_owned(),
                    row["details"].as_str().map(str::to_owned),
                    row["evidence_refs"]
                        .as_array()
                        .expect("refs")
                        .iter()
                        .map(|value| value.as_str().expect("ref").to_owned())
                        .collect(),
                )
            })
            .collect();
        let actual: Vec<TracerRow> = rows
            .iter()
            .map(|finding| {
                (
                    finding.id.clone(),
                    finding.category.clone(),
                    finding.summary.clone(),
                    finding.details.clone(),
                    finding.evidence_refs.clone(),
                )
            })
            .collect();
        assert_eq!(actual, expected, "{bucket}: findings match Source");
    }
}
