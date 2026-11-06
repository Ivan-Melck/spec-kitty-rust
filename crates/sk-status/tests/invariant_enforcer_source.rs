//! Source-first parity for the negative-invariant ENFORCER and provenance rules.
//!
//! Order: the PINNED Source (`acceptance/matrix.py::enforce_negative_invariants`,
//! `_check_grep_absence`, `_check_custom_command`, `_check_invariant`,
//! `_should_defer`, `_defer_invariant`, `_stamp_provenance`,
//! `validate_invariant_provenance`) runs FIRST on a real working directory whose
//! files the test wrote, and its full row output is asserted; the Native
//! `enforce_negative_invariants` / `validate_invariant_provenance` then run on
//! the same directory and the same rows and must agree field for field.
//!
//! Nothing is fabricated: the grep and the custom command really run, on real
//! files, in a sandbox directory.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::path::Path;
use std::process::Command;

use sk_status::acceptance::{
    DEFERRED_TO_CONSOLIDATION, InvariantJudgeContext, NegativeInvariant, PROVENANCE_RECORDED,
    enforce_negative_invariants, validate_invariant_provenance,
};

const ORACLE: &str = r##"
import json, sys
from pathlib import Path
from mission_runtime.artifacts import TopologySurface
from mission_runtime.lifecycle_phase import LifecyclePhase
from specify_cli.acceptance.execution_context import GateExecutionContext
from specify_cli.acceptance.matrix import (
    NegativeInvariant,
    _validate_invariant_provenance as validate_invariant_provenance,
    enforce_negative_invariants,
)

root = Path(sys.argv[1])
context = GateExecutionContext(
    surface=root,
    surface_kind=TopologySurface.PRIMARY,
    ref="0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f",
    phase=LifecyclePhase.PRE_CONSOLIDATION,
    mission_slug="demo",
)
post = GateExecutionContext(
    surface=root,
    surface_kind=TopologySurface.CONSOLIDATED,
    ref="1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e",
    phase=LifecyclePhase.PUBLISHED,
    mission_slug="demo",
)

def row(**kwargs):
    return NegativeInvariant(**kwargs)

CASES = [
    ("grep-absent", row(invariant_id="NI-1", description="no legacy import", verification_method="grep_absence", verification_command="legacy_import_marker"), "pre"),
    ("grep-present", row(invariant_id="NI-2", description="no forbidden token", verification_method="grep_absence", verification_command="forbidden_token"), "pre"),
    ("grep-scoped-absent-dir", row(invariant_id="NI-3", description="scoped", verification_method="grep_absence", verification_command="forbidden_token", scope="src/missing"), "pre"),
    ("grep-scoped-present-dir", row(invariant_id="NI-4", description="scoped present", verification_method="grep_absence", verification_command="legacy_import_marker", scope="src"), "pre"),
    ("grep-no-pattern", row(invariant_id="NI-5", description="no pattern", verification_method="grep_absence"), "pre"),
    ("custom-ok", row(invariant_id="NI-6", description="command ok", verification_method="custom_command", verification_command="test -f src/clean.py"), "pre"),
    ("custom-fail", row(invariant_id="NI-7", description="command fails", verification_method="custom_command", verification_command="test -f src/absent.py"), "pre"),
    ("route-check", row(invariant_id="NI-8", description="route", verification_method="route_check", verification_command="/health"), "pre"),
    ("terminal-preserved", row(invariant_id="NI-9", description="already judged", verification_method="grep_absence", verification_command="forbidden_token", result="confirmed_absent", evidence="grep found zero matches"), "pre"),
    ("deferred-preserved", row(invariant_id="NI-10", description="already deferred", verification_method="grep_absence", verification_command="forbidden_token", result="deferred_to_consolidation", deferred_to_phase="POST_CONSOLIDATION"), "pre"),
    ("scoped-at-post-consolidation", row(invariant_id="NI-11", description="scoped late", verification_method="grep_absence", verification_command="forbidden_token", scope="src/missing"), "post"),
]

def dump(ni):
    out = {
        "invariant_id": ni.invariant_id,
        "description": ni.description,
        "verification_method": ni.verification_method,
        "verification_command": ni.verification_command,
        "result": ni.result,
        "evidence": ni.evidence,
        "scope": ni.scope,
        "verified_ref": ni.verified_ref,
        "verified_surface_kind": ni.verified_surface_kind,
        "deferred_reason": ni.deferred_reason,
        "deferred_to_phase": ni.deferred_to_phase,
        "provenance_origin": ni.provenance_origin,
    }
    out.update(ni.extras)
    return out

out = []
for name, invariant, phase in CASES:
    judged = enforce_negative_invariants(root, [invariant], context=context if phase == "pre" else post)
    out.append({"name": name, "row": dump(judged[0]), "provenance_errors": validate_invariant_provenance(judged[0])})
# Legacy callers pass no context at all.
legacy = enforce_negative_invariants(root, [row(invariant_id="NI-12", description="legacy", verification_method="grep_absence", verification_command="forbidden_token")], context=None)
out.append({"name": "no-context", "row": dump(legacy[0]), "provenance_errors": validate_invariant_provenance(legacy[0])})

BAD = [
    ("bad-result", row(invariant_id="B-1", description="d", verification_method="grep_absence", result="not_present")),
    ("bad-origin", row(invariant_id="B-2", description="d", verification_method="grep_absence", provenance_origin="mystery")),
    ("recorded-without-refs", row(invariant_id="B-3", description="d", verification_method="grep_absence", result="confirmed_absent", provenance_origin="recorded")),
    ("recorded-with-refs", row(invariant_id="B-4", description="d", verification_method="grep_absence", result="confirmed_absent", provenance_origin="recorded", verified_ref="abc", verified_surface_kind="primary")),
]
for name, invariant in BAD:
    out.append({"name": name, "row": None, "provenance_errors": validate_invariant_provenance(invariant)})

print(json.dumps(out, sort_keys=True))
"##;

struct Sandbox {
    root: std::path::PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("sandbox").keep();
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::write(root.join("src/clean.py"), "print('clean')\n").expect("clean");
        std::fs::write(
            root.join("src/legacy.py"),
            "from pkg import legacy_import_marker\n",
        )
        .expect("legacy");
        std::fs::write(root.join("notes.txt"), "forbidden_token lives here\n").expect("notes");
        Self { root }
    }
}

fn source_output(root: &Path) -> serde_json::Value {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = repo_root.join("target/invariant-enforcer-oracle.py");
    std::fs::write(&script, ORACLE).expect("write the oracle script");
    let output = Command::new(oracle::python())
        .arg(&script)
        .arg(root)
        .current_dir(&repo_root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", repo_root.join("target/invariant-oracle-home"))
        .env(
            "PYTHONPATH",
            format!(
                "{}:{}",
                repo_root.display(),
                repo_root.join("src").display()
            ),
        )
        .env("SPEC_KITTY_PACKS_ROOT", repo_root.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("run the pinned Source enforcer");
    assert!(
        output.status.success(),
        "the pinned Source enforcer failed (a harness/premise failure, not a business red):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle json")
}

fn context(phase: &str) -> InvariantJudgeContext {
    match phase {
        "post" => InvariantJudgeContext {
            reference: "1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e".to_owned(),
            surface: "consolidated".to_owned(),
            phase: "PUBLISHED".to_owned(),
        },
        _ => InvariantJudgeContext {
            reference: "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f".to_owned(),
            surface: "primary".to_owned(),
            phase: "PRE_CONSOLIDATION".to_owned(),
        },
    }
}

fn pending(
    id: &str,
    method: &str,
    command: Option<&str>,
    scope: Option<&str>,
) -> NegativeInvariant {
    let mut row = NegativeInvariant::pending(id, "description", method, command.map(str::to_owned));
    row.scope = scope.map(str::to_owned);
    row
}

fn native_row(name: &str) -> (NegativeInvariant, &'static str) {
    match name {
        "grep-absent" => (
            pending("NI-1", "grep_absence", Some("legacy_import_marker"), None),
            "pre",
        ),
        "grep-present" => (
            pending("NI-2", "grep_absence", Some("forbidden_token"), None),
            "pre",
        ),
        "grep-scoped-absent-dir" => (
            pending(
                "NI-3",
                "grep_absence",
                Some("forbidden_token"),
                Some("src/missing"),
            ),
            "pre",
        ),
        "grep-scoped-present-dir" => (
            pending(
                "NI-4",
                "grep_absence",
                Some("legacy_import_marker"),
                Some("src"),
            ),
            "pre",
        ),
        "grep-no-pattern" => (pending("NI-5", "grep_absence", None, None), "pre"),
        "custom-ok" => (
            pending("NI-6", "custom_command", Some("test -f src/clean.py"), None),
            "pre",
        ),
        "custom-fail" => (
            pending(
                "NI-7",
                "custom_command",
                Some("test -f src/absent.py"),
                None,
            ),
            "pre",
        ),
        "route-check" => (pending("NI-8", "route_check", Some("/health"), None), "pre"),
        "terminal-preserved" => {
            let mut row = pending("NI-9", "grep_absence", Some("forbidden_token"), None);
            row.result = "confirmed_absent".to_owned();
            row.evidence = Some("grep found zero matches".to_owned());
            (row, "pre")
        }
        "deferred-preserved" => {
            let mut row = pending("NI-10", "grep_absence", Some("forbidden_token"), None);
            row.result = DEFERRED_TO_CONSOLIDATION.to_owned();
            row.deferred_to_phase = Some("POST_CONSOLIDATION".to_owned());
            (row, "pre")
        }
        "scoped-at-post-consolidation" => (
            pending(
                "NI-11",
                "grep_absence",
                Some("forbidden_token"),
                Some("src/missing"),
            ),
            "post",
        ),
        "no-context" => (
            pending("NI-12", "grep_absence", Some("forbidden_token"), None),
            "none",
        ),
        other => panic!("unexpected case {other}"),
    }
}

fn row_value(row: &NegativeInvariant) -> serde_json::Value {
    let mut value = row.to_json();
    assert!(value.is_object());
    if let Some(object) = value.as_object_mut() {
        // The Source dump always names every key (its dataclass fields), while
        // `to_dict` omits unset optionals; compare on the union of both shapes.
        for key in [
            "verification_command",
            "evidence",
            "scope",
            "verified_ref",
            "verified_surface_kind",
            "deferred_reason",
            "deferred_to_phase",
            "provenance_origin",
        ] {
            object
                .entry(key.to_owned())
                .or_insert(serde_json::Value::Null);
        }
        if object.get("provenance_origin") == Some(&serde_json::Value::Null) {
            object.insert(
                "provenance_origin".into(),
                serde_json::json!("legacy_unrecorded"),
            );
        }
        object.insert(
            "description".into(),
            serde_json::json!(row.description.clone()),
        );
        object.insert(
            "verification_method".into(),
            serde_json::json!(row.verification_method.clone()),
        );
        object.insert("result".into(), serde_json::json!(row.result.clone()));
    }
    value
}

#[test]
fn negative_invariant_enforcement_matches_the_pinned_source() {
    let sandbox = Sandbox::new();
    let source = source_output(&sandbox.root);
    let rows = source.as_array().expect("rows");
    assert!(rows.len() >= 13, "every case is present: {}", rows.len());

    for expected in rows {
        let name = expected["name"].as_str().expect("name");
        if expected["row"].is_null() {
            // Provenance-only case: the Native rule runs on the same row.
            let (row, _) = match name {
                "bad-result" => {
                    let mut row = pending("B-1", "grep_absence", None, None);
                    row.result = "not_present".to_owned();
                    (row, "")
                }
                "bad-origin" => {
                    let mut row = pending("B-2", "grep_absence", None, None);
                    row.provenance_origin = "mystery".to_owned();
                    (row, "")
                }
                "recorded-without-refs" => {
                    let mut row = pending("B-3", "grep_absence", None, None);
                    row.result = "confirmed_absent".to_owned();
                    row.provenance_origin = PROVENANCE_RECORDED.to_owned();
                    (row, "")
                }
                "recorded-with-refs" => {
                    let mut row = pending("B-4", "grep_absence", None, None);
                    row.result = "confirmed_absent".to_owned();
                    row.provenance_origin = PROVENANCE_RECORDED.to_owned();
                    row.verified_ref = Some("abc".to_owned());
                    row.verified_surface_kind = Some("primary".to_owned());
                    (row, "")
                }
                other => panic!("unexpected provenance case {other}"),
            };
            let expected_errors: Vec<String> = expected["provenance_errors"]
                .as_array()
                .expect("errors")
                .iter()
                .map(|value| value.as_str().expect("error").to_owned())
                .collect();
            assert_eq!(
                validate_invariant_provenance(&row),
                expected_errors,
                "{name}: provenance errors match Source"
            );
            continue;
        }
        let (mut row, phase) = native_row(name);
        // `description` is an INPUT carried through verbatim; take Source's so the
        // comparison is about the judgement, not the fixture's placeholder.
        row.description = expected["row"]["description"]
            .as_str()
            .expect("description")
            .to_owned();
        let context = (phase != "none").then(|| context(phase));
        let native = enforce_negative_invariants(&sandbox.root, &[row], context.as_ref());
        assert_eq!(native.len(), 1);
        let native_value = row_value(&native[0]);
        let expected_value = &expected["row"];
        for key in [
            "invariant_id",
            "verification_method",
            "verification_command",
            "result",
            "evidence",
            "scope",
            "verified_ref",
            "verified_surface_kind",
            "deferred_reason",
            "deferred_to_phase",
            "provenance_origin",
        ] {
            assert_eq!(
                &native_value[key], &expected_value[key],
                "{name}: field {key} matches Source"
            );
        }
        let expected_errors: Vec<String> = expected["provenance_errors"]
            .as_array()
            .expect("errors")
            .iter()
            .map(|value| value.as_str().expect("error").to_owned())
            .collect();
        assert_eq!(
            validate_invariant_provenance(&native[0]),
            expected_errors,
            "{name}: provenance errors match Source"
        );
    }
}
