//! Source-first acceptance matrix for the review/acceptance decision surface.
//!
//! Every case is evaluated against the pinned Source implementation **first**,
//! then the native implementation is asserted to produce the same answer. A
//! missing or drifted oracle fails the test before any native assertion (the
//! support module is fail-closed), so an unavailable reference can never become
//! a passing zero-comparison.

mod support;

use serde_json::{Value, json};
use sk_mission::{MissionReviewMode, diagnostic, resolve_mode};
use sk_status::acceptance::{AcceptanceCriterion, AcceptanceMatrix, NegativeInvariant};

const MODE_SCRIPT: &str = r#"
import json
from specify_cli.cli.commands.review._mode import resolve_mode, ModeMismatchError

cases = [(None, None), (None, "abc1234"), ("lightweight", "abc1234"),
         ("post-merge", "abc1234"), ("post-merge", None), ("bogus-mode", "abc1234")]
out = []
for flag, baseline in cases:
    try:
        mode, auto = resolve_mode(cli_flag=flag, baseline_merge_commit=baseline)
        out.append({"mode": str(mode.value), "auto": auto})
    except ModeMismatchError as exc:
        out.append({"code": str(exc.diagnostic_code)})
    except Exception as exc:  # a ValueError for an unknown mode
        out.append({"error": type(exc).__name__})
print(json.dumps(out))
"#;

const BLE001_SCRIPT: &str = r#"
import json
from specify_cli.cli.commands.review._ble001_audit import audit_auth_storage_ble001_line

lines = [
    "except Exception:  # noqa: BLE001",
    "except Exception as exc:  # noqa: BLE001 - clean up the socket boundary",
    "except Exception:  # noqa: BLE001, E501 - translate the transport error",
    "except Exception:  # noqa: BLE001 - TODO",
    "except (ValueError, Exception) as exc:  # noqa: BLE001",
    "except ValueError:  # noqa: BLE001",
    "except SomeError:  # noqa: BLE001 - narrow reason here",
    "except Exception:  # noqa: BLE001 - Broad-Exception!",
    "except Exception:",
    "value = 1  # noqa: BLE001 - not an except clause",
]
out = []
for line in lines:
    finding = audit_auth_storage_ble001_line(
        "src/specify_cli/auth/store.py", 7, line, repo_root=None
    )
    out.append(None if finding is None else {
        "line": finding.line,
        "suppression": finding.suppression,
        "reason": finding.reason,
        "remediation": finding.remediation,
    })
print(json.dumps(out))
"#;

const ISSUE_MATRIX_SCRIPT: &str = r#"
import json
from pathlib import Path
from specify_cli.cli.commands.review._issue_matrix import validate_issue_matrix

texts = [
    "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | fixed | abc123 |\n",
    "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | maybe | abc123 |\n",
    "| Issue | Verdict |\n|---|---|\n| #1 | fixed |\n",
    "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | not-applicable | abc |\n| #2 | in-mission | def |\n",
    "no table at all\n",
    "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n",
]
out = []
for text in texts:
    result = validate_issue_matrix(Path("/nonexistent/issue-matrix.md"), content=text)
    out.append({
        "passed": result.passed,
        "rows": len(result.rows),
        "codes": sorted({d["diagnostic_code"] for d in result.diagnostics}),
    })
print(json.dumps(out))
"#;

const UNCHECKED_SCRIPT: &str = r#"
import json
from specify_cli.core.subtask_rows import iter_unchecked_subtask_rows

texts = [
    "- [ ] T001 real\n- [ ] prose\n",
    "```\n- [ ] T002 fenced\n```\n",
    "- [] T003 tight\n- [ ] T12 short\n- [ ] T0004 four\n",
    "- [ ] T005\n  - [ ] T006 nested\n",
    "",
]
print(json.dumps([list(iter_unchecked_subtask_rows(text)) for text in texts]))
"#;

const VERDICT_SCRIPT: &str = r#"
import json
from specify_cli.acceptance.matrix import AcceptanceMatrix

def row(cid, verdict):
    return {"criterion_id": cid, "description": cid, "proof_type": "code_review",
            "pass_fail": verdict}

def invariant(iid, result):
    return {"invariant_id": iid, "description": iid,
            "verification_method": "grep_absence", "result": result}

cases = [
    ([], []),
    ([row("a", "pass")], []),
    ([row("a", "pending")], []),
    ([row("a", "fail")], []),
    ([row("a", "pass")], [invariant("i", "still_present")]),
    ([row("a", "pass")], [invariant("i", "deferred_to_consolidation")]),
    ([row("a", "pass")], [invariant("i", "pending")]),
    ([row("a", "bogus")], []),
]
print(json.dumps([
    AcceptanceMatrix.from_dict({
        "mission_slug": "demo",
        "criteria": criteria,
        "negative_invariants": invariants,
    }).overall_verdict
    for criteria, invariants in cases
]))
"#;

#[test]
fn review_mode_resolution_matches_source_row_for_row() {
    // Source first.
    let source = support::oracle::run_json(MODE_SCRIPT, &[]);
    let source = source.as_array().expect("oracle returned a list");

    let cases: [(Option<&str>, Option<&str>); 6] = [
        (None, None),
        (None, Some("abc1234")),
        (Some("lightweight"), Some("abc1234")),
        (Some("post-merge"), Some("abc1234")),
        (Some("post-merge"), None),
        (Some("bogus-mode"), Some("abc1234")),
    ];
    assert_eq!(source.len(), cases.len(), "one Source answer per case");

    for (index, (flag, baseline)) in cases.iter().enumerate() {
        let expected = &source[index];
        let native = resolve_mode(*flag, *baseline);
        if let Some(code) = expected.get("code").and_then(Value::as_str) {
            let error = native.expect_err("Source refused this case");
            assert!(
                error.to_string().contains(code)
                    || matches!(error, sk_mission::MissionReviewError::ModeMismatch),
                "case {index}: Source code {code}, native {error}"
            );
            continue;
        }
        if expected.get("error").is_some() {
            assert!(
                native.is_err(),
                "case {index}: Source raised for an unknown mode, native accepted it"
            );
            continue;
        }
        let (mode, auto) = native.expect("Source accepted this case");
        assert_eq!(
            mode.as_str(),
            expected["mode"].as_str().unwrap(),
            "case {index}"
        );
        assert_eq!(auto, expected["auto"].as_bool().unwrap(), "case {index}");
        let expected_mode = if expected["mode"] == json!("post-merge") {
            MissionReviewMode::PostMerge
        } else {
            MissionReviewMode::Lightweight
        };
        assert_eq!(mode, expected_mode, "case {index}");
    }
}

#[test]
fn ble001_suppression_audit_matches_source_row_for_row() {
    let source = support::oracle::run_json(BLE001_SCRIPT, &[]);
    let source = source.as_array().expect("oracle returned a list");

    let lines = [
        "except Exception:  # noqa: BLE001",
        "except Exception as exc:  # noqa: BLE001 - clean up the socket boundary",
        "except Exception:  # noqa: BLE001, E501 - translate the transport error",
        "except Exception:  # noqa: BLE001 - TODO",
        "except (ValueError, Exception) as exc:  # noqa: BLE001",
        "except ValueError:  # noqa: BLE001",
        "except SomeError:  # noqa: BLE001 - narrow reason here",
        "except Exception:  # noqa: BLE001 - Broad-Exception!",
        "except Exception:",
        "value = 1  # noqa: BLE001 - not an except clause",
    ];
    assert_eq!(source.len(), lines.len());

    for (index, line) in lines.iter().enumerate() {
        let expected = &source[index];
        let native = sk_mission::audit_ble001_line("src/specify_cli/auth/store.py", 7, line);
        match (expected.is_null(), native) {
            (true, None) => {}
            (true, Some(finding)) => {
                panic!("case {index}: Source has no finding, native reported {finding:?}")
            }
            (false, None) => {
                panic!("case {index}: Source reported a finding, native reported none")
            }
            (false, Some(finding)) => {
                assert_eq!(finding.line, expected["line"].as_u64().unwrap() as usize);
                assert_eq!(
                    finding.suppression,
                    expected["suppression"].as_str().unwrap()
                );
                assert_eq!(
                    finding.reason,
                    expected["reason"].as_str().unwrap(),
                    "case {index}"
                );
                assert_eq!(
                    finding.remediation,
                    expected["remediation"].as_str().unwrap(),
                    "case {index}"
                );
            }
        }
    }
}

#[test]
fn issue_matrix_validation_matches_source_row_for_row() {
    let source = support::oracle::run_json(ISSUE_MATRIX_SCRIPT, &[]);
    let source = source.as_array().expect("oracle returned a list");

    let texts = [
        "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | fixed | abc123 |\n",
        "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | maybe | abc123 |\n",
        "| Issue | Verdict |\n|---|---|\n| #1 | fixed |\n",
        "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | not-applicable | abc |\n| #2 | in-mission | def |\n",
        "no table at all\n",
        "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n",
    ];
    assert_eq!(source.len(), texts.len());

    for (index, text) in texts.iter().enumerate() {
        let expected = &source[index];
        let native = sk_mission::validate_issue_matrix_text(text);
        let expected_passed = expected["passed"].as_bool().unwrap();
        assert_eq!(
            native.is_ok(),
            expected_passed,
            "case {index}: Source passed={expected_passed}, native={native:?}"
        );
        if expected_passed {
            assert_eq!(
                native.unwrap(),
                expected["rows"].as_u64().unwrap() as usize,
                "case {index}: row count"
            );
        } else {
            let codes: Vec<String> = native
                .unwrap_err()
                .into_iter()
                .map(|(code, _)| code)
                .collect();
            let expected_codes: Vec<String> = expected["codes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect();
            assert!(
                codes.iter().any(|code| expected_codes.contains(code)),
                "case {index}: Source codes {expected_codes:?}, native {codes:?}"
            );
            // Every native code must be one this port declares (the Source
            // vocabulary), so a drifted code name is a real failure.
            let vocabulary = [
                diagnostic::ISSUE_MATRIX_MISSING,
                diagnostic::ISSUE_MATRIX_INVALID,
                diagnostic::ISSUE_MATRIX_SCHEMA_DRIFT,
                diagnostic::ISSUE_MATRIX_VERDICT_UNKNOWN,
                diagnostic::ISSUE_MATRIX_MULTI_TABLE,
                diagnostic::ISSUE_MATRIX_EVIDENCE_EMPTY,
                diagnostic::ISSUE_MATRIX_DEFERRED_WITHOUT_HANDLE,
            ];
            for code in &codes {
                assert!(
                    vocabulary.contains(&code.as_str()),
                    "case {index}: native code {code} is outside the Source vocabulary"
                );
            }
        }
    }
}

#[test]
fn unchecked_subtask_rows_match_source_row_for_row() {
    let source = support::oracle::run_json(UNCHECKED_SCRIPT, &[]);
    let source = source.as_array().expect("oracle returned a list");

    let texts = [
        "- [ ] T001 real\n- [ ] prose\n",
        "```\n- [ ] T002 fenced\n```\n",
        "- [] T003 tight\n- [ ] T12 short\n- [ ] T0004 four\n",
        "- [ ] T005\n  - [ ] T006 nested\n",
        "",
    ];
    assert_eq!(source.len(), texts.len());

    for (index, text) in texts.iter().enumerate() {
        let expected: Vec<String> = source[index]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        let native = sk_status::acceptance::iter_unchecked_subtask_rows(text);
        assert_eq!(native, expected, "case {index}");
    }
}

#[test]
fn matrix_overall_verdict_matches_source_row_for_row() {
    let source = support::oracle::run_json(VERDICT_SCRIPT, &[]);
    let source = source.as_array().expect("oracle returned a list");

    let criteria = |verdict: &str| {
        let mut row = AcceptanceCriterion::code_review("a", "a");
        row.pass_fail = verdict.to_owned();
        vec![row]
    };
    let invariant = |result: &str| {
        let mut row = NegativeInvariant::pending("i", "i", "grep_absence", None);
        row.result = result.to_owned();
        row
    };

    let cases: Vec<(Vec<AcceptanceCriterion>, Vec<NegativeInvariant>)> = vec![
        (Vec::new(), Vec::new()),
        (criteria("pass"), Vec::new()),
        (criteria("pending"), Vec::new()),
        (criteria("fail"), Vec::new()),
        (criteria("pass"), vec![invariant("still_present")]),
        (
            criteria("pass"),
            vec![invariant("deferred_to_consolidation")],
        ),
        (criteria("pass"), vec![invariant("pending")]),
        (criteria("bogus"), Vec::new()),
    ];
    assert_eq!(source.len(), cases.len());

    for (index, (rows, invariants)) in cases.into_iter().enumerate() {
        let mut matrix = AcceptanceMatrix::new("demo", rows);
        matrix.negative_invariants = invariants;
        assert_eq!(
            matrix.overall_verdict(),
            source[index].as_str().unwrap(),
            "case {index}"
        );
    }
}
