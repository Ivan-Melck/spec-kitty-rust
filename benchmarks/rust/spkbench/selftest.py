"""Negative tests for the harness itself.

A benchmark harness that can only say "fast" is useless. These tests exercise
the ways a naive harness turns green incorrectly: failure hidden by a fast run,
empty output treated as success, faster-but-wrong payloads, pending workloads
counted as done, projections/startup substituting for required work, noisy or
truncated runs, and identity drift.
"""

from __future__ import annotations

import copy
import json
from dataclasses import dataclass

from .gate import GateOptions, evaluate_gate
from .runner import _exit_matches
from .workloads import drop_paths, first_difference


def _exact_identity() -> dict:
    return {
        "status": "exact",
        "problems": [],
        "source": {"oracle_imports_from_repo_src": True, "repo_short_head": {"stdout": "abc1234"}},
        "dependencies": {"status": "exact", "mismatches": [], "installed_not_in_lock": []},
        "rust_binary": {"exists": True, "sha256": "a" * 64},
    }


def _timing(ratio: float, *, rust_n: int = 5, oracle_n: int = 5, cv: float = 0.01, noise: str = "low") -> dict:
    return {
        "rust": {"n": rust_n, "median_s": 0.01, "cv": cv},
        "oracle": {"n": oracle_n, "median_s": 0.01 * ratio, "cv": cv},
        "median_ratio_oracle_over_rust": ratio,
        "median_ratio_ci95": [ratio * 0.95, ratio * 1.05],
        "noise": noise,
    }


def _workload(
    workload_id: str,
    *,
    ratio: float = 4.0,
    status: str = "ok",
    equivalence: str = "strict",
    klass: str = "work",
    required: bool = True,
) -> dict:
    gate_eligible = required and equivalence == "strict" and klass == "work"
    record = {
        "id": workload_id,
        "class": klass,
        "equivalence": equivalence,
        "required": required,
        "gate_eligible": gate_eligible,
        "status": status,
        "reason": "",
        "equivalence_diff": "",
        "timing": _timing(ratio) if status == "ok" else None,
    }
    return record


def _run(workloads: list[dict], *, truncated: bool = False) -> dict:
    return {
        "schema": "spkbench.run.v1",
        "truncated": truncated,
        "workloads": workloads,
    }


@dataclass
class FakeObservation:
    exit_code: int | None
    stdout: bytes
    timed_out: bool = False


class CheckFailure(AssertionError):
    pass


def _check(condition: bool, message: str) -> None:
    if not condition:
        raise CheckFailure(message)


def test_positive_control_passes() -> str:
    run = _run([_workload("a", ratio=3.5), _workload("b", ratio=4.0), _workload("c", ratio=5.0)])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "PASS", f"expected PASS, got {gate.verdict}: {gate.reasons}")
    return "3 strict workloads ≥3x with exact identity passes"


def test_pending_blocks_even_with_fast_pairs() -> None:
    pending = _workload("status.replay", status="pending", equivalence="pending", required=True)
    run = _run([_workload("a", ratio=99.0), pending])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "BLOCKED", f"pending required workload must block, got {gate.verdict}")
    _check(any("pending" in r for r in gate.reasons), "block reason should name pending work")
    return "pending required workload blocks despite a 99x measured pair"


def test_faster_but_wrong_fails() -> None:
    wrong = _workload("a", ratio=100.0)
    wrong["status"] = "equivalence_failed"
    wrong["equivalence_diff"] = "$.step_id: 'step-0020' != 'step-0021'"
    run = _run([wrong, _workload("b", ratio=5.0)])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "FAIL", f"faster-but-wrong must FAIL, got {gate.verdict}")
    return "100x speed with a different payload fails on equivalence"


def test_empty_output_is_not_equivalence() -> None:
    workload = _workload("a")
    rust = FakeObservation(0, b"", False)
    oracle = FakeObservation(0, b"{}", False)
    # Contract workloads require non-empty stdout.
    workload_contract = copy.deepcopy(workload)
    workload_contract["equivalence"] = "contract"
    from .workloads import Workload

    spec = Workload(
        id="contract",
        description="",
        fixture=None,
        klass="startup",
        equivalence="contract",
        required=False,
    )
    ok, reason = _exit_matches(spec, rust, oracle)  # type: ignore[arg-type]
    _check(not ok and "empty stdout" in reason, f"empty stdout must fail contract check, got {ok} {reason}")
    return "empty stdout rejected for contract-class workloads"


def test_projection_cannot_rescue_gate() -> None:
    projection = _workload("mission.list.projection", ratio=1000.0, equivalence="projection", required=False)
    run = _run([projection])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "BLOCKED", f"projection-only run must be BLOCKED, got {gate.verdict}")
    _check(gate.gate_workload_count == 0, "projection must not enter the gate population")
    return "a 1000x projection still leaves the gate BLOCKED (no strict work)"


def test_startup_cannot_rescue_gate() -> None:
    startup = _workload("startup.version", ratio=500.0, equivalence="contract", klass="startup", required=False)
    run = _run([startup])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "BLOCKED", f"startup-only run must be BLOCKED, got {gate.verdict}")
    return "a 500x startup claim cannot stand in for workload evidence"


def test_micro_cannot_rescue_gate() -> None:
    micro = _workload("transition.check.x", ratio=50.0, equivalence="projection", klass="micro", required=False)
    run = _run([micro])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "BLOCKED", f"micro-only run must be BLOCKED, got {gate.verdict}")
    return "launch-dominated micro results cannot rescue the gate"


def test_missing_required_fails() -> None:
    missing = _workload("runtime.query.step", status="not_measured", required=True)
    missing["timing"] = None
    run = _run([_workload("b", ratio=4.0), missing])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "FAIL", f"missing required workload must FAIL, got {gate.verdict}")
    return "a required workload with no samples fails the gate"


def test_slowdown_fails() -> None:
    slow = _workload("a", ratio=0.5)
    run = _run([slow, _workload("b", ratio=4.0)])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "FAIL", f"a 2x slowdown must FAIL, got {gate.verdict}")
    _check(any("regression >10%" in r for r in gate.reasons), "slowdown reason should be explicit")
    return "a required workload slower by >10% fails even when the geomean is high"


def test_geomean_below_target_fails() -> None:
    run = _run([_workload("a", ratio=2.0), _workload("b", ratio=2.5)])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "FAIL", f"2.2x geomean must FAIL, got {gate.verdict}")
    return "sub-3x geometric mean fails"


def test_noisy_blocks() -> None:
    noisy = _workload("a", ratio=10.0)
    noisy["timing"] = _timing(10.0, cv=0.5, noise="high")
    noisy["timing"]["median_ratio_ci95"] = [1.5, 40.0]
    run = _run([noisy, _workload("b", ratio=4.0)])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "BLOCKED", f"low-confidence ratio must BLOCK, got {gate.verdict}")
    return "a ratio whose CI reaches below target blocks rather than passing"


def test_confident_ratio_tolerates_jitter() -> None:
    jittery = _workload("a", ratio=100.0)
    jittery["timing"] = _timing(100.0, cv=0.45, noise="high")
    jittery["timing"]["median_ratio_ci95"] = [60.0, 160.0]
    run = _run([jittery, _workload("b", ratio=4.0)])
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "PASS", f"confident 100x ratio should pass, got {gate.verdict}: {gate.reasons}")
    return "large but confident ratios pass despite relative jitter on tiny absolutes"


def test_insufficient_samples_blocks() -> None:
    thin = _workload("a", ratio=10.0)
    thin["timing"] = _timing(10.0, rust_n=1, oracle_n=1)
    run = _run([thin, _workload("b", ratio=4.0)])
    gate = evaluate_gate(run, _exact_identity(), GateOptions(min_samples_per_side=5))
    _check(gate.verdict == "BLOCKED", f"insufficient samples must BLOCK, got {gate.verdict}")
    return "too few samples blocks the gate"


def test_truncated_blocks() -> None:
    run = _run([_workload("a", ratio=10.0), _workload("b", ratio=4.0)], truncated=True)
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "BLOCKED", f"truncated run must BLOCK, got {gate.verdict}")
    return "a time-budget-truncated run blocks"


def test_fixture_mutation_fails() -> None:
    run = _run([_workload("a", ratio=10.0), _workload("b", ratio=4.0)])
    run["fixtures_unchanged"] = False
    gate = evaluate_gate(run, _exact_identity())
    _check(gate.verdict == "FAIL", f"fixture mutation must FAIL, got {gate.verdict}")
    return "a mutated read-only fixture fails post-state equivalence"


def test_identity_drift_blocks() -> None:
    identity = _exact_identity()
    identity["dependencies"] = {
        "status": "drift",
        "mismatches": [{"name": "pydantic", "lock": "2.13.3", "installed": "2.14.2"}],
        "installed_not_in_lock": [],
    }
    run = _run([_workload("a", ratio=10.0), _workload("b", ratio=4.0)])
    gate = evaluate_gate(run, identity)
    _check(gate.verdict == "BLOCKED", f"identity drift must BLOCK, got {gate.verdict}")
    return "dependency drift from uv.lock blocks the measurement"


def test_equivalence_diff_detects_nested_change() -> None:
    left = {"decision": {"kind": "step", "step_id": "step-0001", "context": {"options": ["a", "b"]}}}
    right = {"decision": {"kind": "step", "step_id": "step-0001", "context": {"options": ["a", "c"]}}}
    diff = first_difference(left, right)
    _check(diff is not None and "options[1]" in diff, f"nested diff missed: {diff}")
    return "nested payload changes are detected with a field path"


def test_drop_paths_is_narrow() -> None:
    payload = {"result": {"status": "resolved", "matches": [{"surface": "x"}]}}
    dropped = drop_paths(payload, ["result.status"])
    _check("status" not in dropped["result"], "declared derived field should be dropped")
    _check(dropped["result"]["matches"][0]["surface"] == "x", "drop must not remove unrelated fields")
    return "only the declared derived field is removed"


def test_unparseable_rust_envelope_fails() -> None:
    from .workloads import rust_payload

    payload = rust_payload(b"not json")
    _check("_unparseable" in payload, "garbage stdout must be marked unparseable")
    return "non-JSON output is rejected, not treated as an empty payload"


TESTS = [
    test_positive_control_passes,
    test_pending_blocks_even_with_fast_pairs,
    test_faster_but_wrong_fails,
    test_empty_output_is_not_equivalence,
    test_projection_cannot_rescue_gate,
    test_startup_cannot_rescue_gate,
    test_micro_cannot_rescue_gate,
    test_missing_required_fails,
    test_slowdown_fails,
    test_geomean_below_target_fails,
    test_noisy_blocks,
    test_confident_ratio_tolerates_jitter,
    test_insufficient_samples_blocks,
    test_truncated_blocks,
    test_fixture_mutation_fails,
    test_identity_drift_blocks,
    test_equivalence_diff_detects_nested_change,
    test_drop_paths_is_narrow,
    test_unparseable_rust_envelope_fails,
]


def run_selftest() -> dict:
    results = []
    failures = 0
    for test in TESTS:
        try:
            message = test()
            results.append({"test": test.__name__, "ok": True, "message": message})
        except Exception as exc:  # noqa: BLE001 - self-test runner
            failures += 1
            results.append({"test": test.__name__, "ok": False, "error": f"{type(exc).__name__}: {exc}"})
    return {
        "ok": failures == 0,
        "failures": failures,
        "total": len(TESTS),
        "results": results,
    }


def main() -> int:
    report = run_selftest()
    print(json.dumps(report, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
