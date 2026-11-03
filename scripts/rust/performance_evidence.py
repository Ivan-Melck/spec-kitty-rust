#!/usr/bin/env python3
"""Completion-checker adapter for the benchmark evidence bundle.

``scripts/rust/check_completion.py`` already re-derives a speedup from the raw
sample arrays in ``coverage.json``. What it could not do is prove those arrays
belong to a real measurement: the first harness wrote a different id set, and a
hand-edited report could satisfy every arithmetic check.

This module closes that hole with one additive call from the checker:

    import performance_evidence
    performance_evidence.check_bundle(report, performance, root=ROOT)

``check_bundle`` loads the evidence bundle referenced by
``performance.aggregate.evidence``, verifies its hash and self-hash, cross-checks
the coverage section against it, and then runs the full
:mod:`spkbench.certify` verifier: stored payloads and post-state envelopes, the
registry's post-state contract re-derived from stored bytes, every raw sample and
its global observation order, the reconstructed pairs, the aggregate statistics,
the regenerated fixtures (the same check ``bench.py gate`` runs), and the *live*
native source (HEAD or a proven evidence-only descendant, build-input digest,
``Cargo.lock``, toolchain, both binaries, the re-verified build receipt) plus the
harness / adapter code that produced the bundle. Any divergence is reported
through the checker's own ``Report.fail`` surface, so the completion gate stays
non-vacuous. Malformed evidence is reported as a typed failure, never raised.

It never writes ``coverage.json``. ``emit`` verifies first and prints the exact
replacement ``performance`` section only for a bundle whose verdict recomputes
as ``PASS``; ``validate`` does the same and exits non-zero otherwise.

Usage::

    <oracle-python> scripts/rust/performance_evidence.py validate \\
        --bundle benchmarks/rust/evidence/<run>/performance-evidence.json
    <oracle-python> scripts/rust/performance_evidence.py emit \\
        --bundle benchmarks/rust/evidence/<run>/performance-evidence.json \\
        --out /tmp/performance-section.json
    <oracle-python> scripts/rust/performance_evidence.py registry
"""
from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from pathlib import Path

sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BENCH = ROOT / "benchmarks" / "rust"
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(HERE))

from spkbench import certify  # noqa: E402
from spkbench.registry import PerformanceRegistry, RegistryError  # noqa: E402

DEFAULT_REGISTRY = ROOT / "docs/rust/inventory/performance-registry.json"

#: Fixture observation seam. ``None`` means "regenerate with the frozen recipes",
#: which is what the checker always does. Tests that exercise a *unit-scoped*
#: synthetic registry inject a regenerator with the same shape; there is no
#: bundle flag or asserted verdict that can turn the fixture check off.
FIXTURE_OBSERVER = None

# Exact mirror of inventory_model.PERF_POLICY. The test suite asserts equality,
# so a curated-model change cannot silently drift from the harness.
PERF_POLICY = {
    "gate": "geometric_mean_speedup",
    "minimum_speedup": 3.0,
    "max_per_workload_regression_fraction": 0.10,
    "minimum_strict_workloads": 14,
    "minimum_samples_per_side": 20,
    "startup_is_separate": True,
    "startup_counts_toward_gate": False,
    "projection_counts_toward_gate": False,
    "require_result_equivalence": True,
    "require_raw_samples": True,
    "require_interleaved_repetitions": True,
    "require_machine_and_load_context": True,
    "require_toolchain_identity": True,
    "require_fixture_identity": True,
    "notes": (
        "Aggregate = geometric mean of per-workload median(Python)/median(Rust) over strict, "
        "required work workloads. Startup and projection workloads are reported separately and "
        "cannot substitute."
    ),
}

REQUIRED_GATE_IDS = frozenset(
    {
        "status.replay",
        "state.event-append",
        "state.move-task",
        "state.decision.resolve",
        "runtime.next.governed",
        "governance.charter",
        "glossary.activation",
        "workflow.mission-create",
        "workflow.specify",
        "workflow.plan",
        "workflow.tasks",
        "workflow.implement",
        "workflow.review",
        "workflow.accept",
        "workflow.consolidate",
        "workflow.merge",
        "workflow.recovery",
    }
)
REQUIRED_STARTUP_IDS = frozenset({"startup.version", "startup.help"})
REQUIRED_ALL_IDS = REQUIRED_GATE_IDS | REQUIRED_STARTUP_IDS | {"cli.mission.list"}

MIN_SAMPLES = 20
MIN_SPEEDUP = 3.0
MAX_REGRESSION = 0.10
MIN_STRICT_WORKLOADS = 14
TOLERANCE = 0.02


# ---------------------------------------------------------------------------
# Checker surface
# ---------------------------------------------------------------------------


class _Collector:
    """Adapter so this module works with or without the checker's Report."""

    def __init__(self, report=None, gate: str = "performance"):
        self.report = report
        self.gate = gate
        self.errors: list[dict] = []
        self.warnings: list[dict] = []

    def fail(self, code: str, message: str, *, entry_id: str | None = None) -> None:
        self.errors.append({"code": code, "message": message, "entry_id": entry_id})
        if self.report is not None:
            self.report.fail(code, message, entry_id=entry_id, gate=self.gate)

    def warn(self, code: str, message: str, *, entry_id: str | None = None) -> None:
        self.warnings.append({"code": code, "message": message, "entry_id": entry_id})
        if self.report is not None and hasattr(self.report, "warn"):
            self.report.warn(code, message, entry_id=entry_id, gate=self.gate)


def _median(values: list[float]) -> float:
    return statistics.median(values)


def _geomean(values: list[float]) -> float:
    return math.exp(sum(math.log(value) for value in values) / len(values))


def _sha256(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _registry(registry_path: Path | None = None) -> PerformanceRegistry:
    """Load the frozen registry; an explicitly supplied path is audit-only.

    The default path must pass full structural validation. A caller-supplied
    path (used by the test suite and by ``--registry`` audits) still has to be
    digest-consistent but need not carry the 20 frozen ids.
    """
    if registry_path is None:
        return PerformanceRegistry.load(DEFAULT_REGISTRY)
    import hashlib

    target = Path(registry_path)
    raw = target.read_bytes()
    data = json.loads(raw.decode("utf-8"))
    registry = PerformanceRegistry(target, data, file_sha256=hashlib.sha256(raw).hexdigest())
    from spkbench.registry import registry_digest

    if registry.declared_digest != "sha256:" + registry_digest(data):
        raise RegistryError("supplied registry digest does not match its contents")
    return registry


def _close(recorded, recomputed, *, tolerance: float = TOLERANCE) -> bool:
    for value in (recorded, recomputed):
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return False
    return abs(float(recorded) - float(recomputed)) <= max(
        tolerance, abs(float(recomputed)) * tolerance
    )


def _check_projection(
    out: _Collector, performance: dict, verification: dict, bundle: dict
) -> None:
    """Every projected aggregate number must equal the recomputed value."""

    recomputed = verification.get("recomputed") or {}
    aggregate = performance.get("aggregate") or {}
    if recomputed.get("geometric_mean_speedup") is None:
        if aggregate.get("geometric_mean_speedup") is not None:
            out.fail(
                "PERF_PROJECTION_MISMATCH",
                "performance.aggregate.geometric_mean_speedup is recorded but no strict workload "
                "recomputes",
            )
    elif not _close(aggregate.get("geometric_mean_speedup"), recomputed["geometric_mean_speedup"]):
        out.fail(
            "PERF_PROJECTION_MISMATCH",
            f"performance.aggregate.geometric_mean_speedup {aggregate.get('geometric_mean_speedup')!r} "
            f"does not match the recomputed {recomputed['geometric_mean_speedup']:.6f}",
        )
    recorded_ci = aggregate.get("geometric_mean_ci95")
    recomputed_ci = recomputed.get("geometric_mean_ci95")
    if recomputed_ci is None:
        if recorded_ci is not None:
            out.fail(
                "PERF_PROJECTION_MISMATCH",
                "performance.aggregate.geometric_mean_ci95 is recorded but could not be recomputed",
            )
    elif (
        not isinstance(recorded_ci, list)
        or len(recorded_ci) != 2
        or not _close(recorded_ci[0], recomputed_ci[0])
        or not _close(recorded_ci[1], recomputed_ci[1])
    ):
        out.fail(
            "PERF_PROJECTION_MISMATCH",
            "performance.aggregate.geometric_mean_ci95 does not match the recomputed interval",
        )
    recorded_work = aggregate.get("work_excess_geometric_mean")
    recomputed_work = recomputed.get("work_excess_geometric_mean")
    if recomputed_work is None:
        if recorded_work is not None:
            out.fail(
                "PERF_PROJECTION_MISMATCH",
                "performance.aggregate.work_excess_geometric_mean is recorded but could not be "
                "recomputed",
            )
    elif not _close(recorded_work, recomputed_work):
        out.fail(
            "PERF_PROJECTION_MISMATCH",
            "performance.aggregate.work_excess_geometric_mean does not match the recomputed value",
        )
    if aggregate.get("strict_workload_count") != recomputed.get("strict_workload_count"):
        out.fail(
            "PERF_PROJECTION_MISMATCH",
            f"performance.aggregate.strict_workload_count "
            f"{aggregate.get('strict_workload_count')!r} does not match the recomputed "
            f"{recomputed.get('strict_workload_count')!r}",
        )
    if aggregate.get("sample_count") != recomputed.get("sample_count"):
        out.fail(
            "PERF_PROJECTION_MISMATCH",
            f"performance.aggregate.sample_count {aggregate.get('sample_count')!r} does not match "
            f"the recomputed {recomputed.get('sample_count')!r}",
        )
    bundle_aggregate = bundle.get("aggregate") or {}
    if bundle_aggregate.get("geometric_mean_speedup") is not None and not _close(
        bundle_aggregate.get("geometric_mean_speedup"), aggregate.get("geometric_mean_speedup")
    ):
        out.fail(
            "PERF_PROJECTION_MISMATCH",
            "bundle aggregate and projected aggregate disagree on the geometric mean",
        )


def check_bundle(
    report,
    performance: dict,
    *,
    root: Path = ROOT,
    gate: str = "performance",
    registry_path: Path | None = None,
    enforce_checker_ids: bool = True,
    live_probe=None,
) -> dict:
    """Verify that ``performance`` is bound to a genuine, passing bundle.

    Call this from ``_check_performance`` after the checker's own arithmetic
    checks. It is additive: it never relaxes a checker rule, it always observes
    the live native source, and malformed evidence becomes a reported failure.
    """
    out = _Collector(report, gate)
    try:
        return _check_bundle(
            out,
            performance,
            root=Path(root),
            registry_path=registry_path,
            enforce_checker_ids=enforce_checker_ids,
            live_probe=live_probe,
        )
    except (KeyboardInterrupt, SystemExit):
        raise
    except Exception as exc:  # noqa: BLE001 - evidence boundary must not crash the checker
        out.fail(
            "PERF_EVIDENCE_MALFORMED",
            f"evidence bundle could not be judged: {type(exc).__name__}: {exc}",
        )
        return {
            "ok": False,
            "verdict": "BLOCKED",
            "errors": out.errors,
            "warnings": out.warnings,
            "bundle": None,
        }


def _check_bundle(
    out: _Collector,
    performance: dict,
    *,
    root: Path,
    registry_path: Path | None,
    enforce_checker_ids: bool,
    live_probe,
) -> dict:
    if registry_path is None:
        # Audit from another checkout: prefer that checkout's frozen registry so
        # the ids/floors checked are the ones the cited evidence was produced
        # against. The default checkout still resolves to its own file.
        candidate = Path(root) / "docs/rust/inventory/performance-registry.json"
        if candidate.is_file() and Path(root).resolve() != ROOT.resolve():
            registry_path = candidate
    try:
        registry = _registry(registry_path)
    except RegistryError as exc:
        out.fail("PERF_REGISTRY_UNREADABLE", f"frozen performance registry is invalid: {exc}")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}

    # The registry must still agree with the checker's hard-coded floor.
    if enforce_checker_ids and set(registry.required_ids) != REQUIRED_ALL_IDS:
        out.fail(
            "PERF_REGISTRY_DRIFT",
            "frozen registry ids differ from the completion checker's required performance ids",
        )
    if enforce_checker_ids and set(registry.gate_ids) != REQUIRED_GATE_IDS:
        out.fail(
            "PERF_REGISTRY_DRIFT",
            "frozen registry gate ids differ from the completion checker's strict gate ids",
        )
    if enforce_checker_ids and registry.policy.get("minimum_speedup") != MIN_SPEEDUP:
        out.fail("PERF_REGISTRY_DRIFT", "frozen registry lowered the 3.0x floor")
    if enforce_checker_ids and registry.policy.get("minimum_samples_per_side") != MIN_SAMPLES:
        out.fail("PERF_REGISTRY_DRIFT", "frozen registry lowered the 20-sample floor")

    if not isinstance(performance, dict):
        out.fail("PERF_EVIDENCE_MALFORMED", "performance section is not an object")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}

    policy = performance.get("policy")
    if policy is not None:
        if not isinstance(policy, dict):
            out.fail("PERF_EVIDENCE_MALFORMED", "performance.policy is not an object")
        else:
            for key, expected in PERF_POLICY.items():
                if policy.get(key) != expected:
                    out.fail(
                        "PERF_POLICY_MISMATCH",
                        f"performance.policy.{key} must be {expected!r}",
                    )

    evidence = (performance.get("aggregate") or {}).get("evidence") or {}
    if not isinstance(evidence, dict):
        out.fail("PERF_EVIDENCE_MALFORMED", "performance.aggregate.evidence is not an object")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}
    if not evidence.get("path"):
        out.fail("PERF_EVIDENCE_BUNDLE_MISSING", "no evidence bundle path is recorded")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}
    path = Path(evidence["path"])
    path = path if path.is_absolute() else root / path
    if not path.is_file():
        out.fail("PERF_EVIDENCE_BUNDLE_MISSING", f"evidence bundle does not exist: {path}")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}
    if evidence.get("sha256") != _sha256(path):
        out.fail("PERF_EVIDENCE_HASH", "evidence bundle sha256 does not match the recorded hash")
    try:
        bundle = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        out.fail("PERF_EVIDENCE_UNREADABLE", f"evidence bundle is not valid JSON: {exc}")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}
    if not isinstance(bundle, dict):
        out.fail("PERF_EVIDENCE_MALFORMED", "evidence bundle is not an object")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}

    if bundle.get("schema") != certify.SCHEMA:
        out.fail(
            "PERF_EVIDENCE_SCHEMA",
            f"evidence bundle schema {bundle.get('schema')!r} is not {certify.SCHEMA!r}",
        )
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}
    if bundle.get("self_sha256") != certify.self_digest(bundle):
        out.fail("PERF_BUNDLE_SELF_HASH", "evidence bundle self-hash does not match its contents")

    declared = bundle.get("registry") or {}
    if not isinstance(declared, dict):
        out.fail("PERF_EVIDENCE_MALFORMED", "bundle registry block is not an object")
        return {"ok": False, "verdict": "BLOCKED", "errors": out.errors, "warnings": out.warnings}
    if declared.get("digest") != registry.declared_digest:
        out.fail("PERF_BUNDLE_REGISTRY", "bundle cites a different frozen registry digest")
    rows = _rows_by_id(bundle.get("workloads"), out, "bundle")
    section_rows = _rows_by_id(performance.get("workloads"), out, "coverage")
    required_ids = REQUIRED_ALL_IDS if enforce_checker_ids else set(registry.required_ids)
    gate_ids = REQUIRED_GATE_IDS if enforce_checker_ids else set(registry.gate_ids)
    missing = sorted(required_ids - set(rows))
    if missing:
        out.fail(
            "PERF_EVIDENCE_INCOMPLETE",
            "evidence bundle omits required workloads: " + ", ".join(missing),
        )
    extra = sorted(
        wid for wid, row in rows.items() if row.get("required") and wid not in required_ids
    )
    if extra:
        out.fail(
            "PERF_EVIDENCE_UNEXPECTED",
            "bundle marks non-frozen workloads required: " + ", ".join(extra),
        )

    # The coverage section must describe the same samples as the bundle. The
    # checker's own loop trusts the section arrays; without this cross-check a
    # section could carry different numbers than the artifact it cites.
    for wid in sorted(required_ids & set(section_rows) & set(rows)):
        section_row = section_rows[wid]
        bundle_row = rows[wid]
        for side in ("python", "rust"):
            recorded = [
                round(float(value), 6) for value in (section_row.get("samples") or {}).get(side) or []
            ]
            actual = [
                round(float(value), 6) for value in (bundle_row.get("samples") or {}).get(side) or []
            ]
            if sorted(recorded) != sorted(actual):
                out.fail(
                    "PERF_SECTION_BUNDLE_MISMATCH",
                    f"coverage samples for {side} do not match the evidence bundle",
                    entry_id=wid,
                )
        expected_status = "pass" if bundle_row.get("status") == "ok" else "pending"
        if section_row.get("status") != expected_status:
            out.fail(
                "PERF_SECTION_BUNDLE_MISMATCH",
                f"coverage status {section_row.get('status')!r} does not match the bundle "
                f"(bundle row is {bundle_row.get('status')!r})",
                entry_id=wid,
            )
        if performance.get("status") == "pass":
            recorded_command = section_row.get("command_argv")
            if isinstance(recorded_command, str):
                try:
                    recorded_command = json.loads(recorded_command)
                except ValueError:
                    recorded_command = None
            if recorded_command != bundle_row.get("command_argv"):
                out.fail(
                    "PERF_PROJECTION_MISMATCH",
                    "coverage command_argv does not match the evidence bundle",
                    entry_id=wid,
                )
            # The coverage section is what humans read; its per-workload
            # statistics must be the recomputed ones, not a second projection.
            bundle_post = (bundle_row.get("post_state") or {}).get("equal")
            for field, expected_value in (
                ("fixture_id", bundle_row.get("fixture_id")),
                ("result_equivalent", bundle_row.get("result_equivalent")),
                ("post_state_equal", bundle_post),
            ):
                if section_row.get(field) != expected_value:
                    out.fail(
                        "PERF_PROJECTION_MISMATCH",
                        f"coverage {field} does not match the evidence bundle",
                        entry_id=wid,
                    )
            bundle_work_ratio = (bundle_row.get("work_excess") or {}).get("work_excess_ratio")
            for field, expected in (
                ("speedup", bundle_row.get("speedup")),
                ("work_excess_ratio", bundle_work_ratio),
            ):
                recorded = section_row.get(field)
                if expected is None and recorded is None:
                    continue
                if not _close(recorded, expected):
                    out.fail(
                        "PERF_PROJECTION_MISMATCH",
                        f"coverage {field} {recorded!r} does not match the recomputed {expected!r}",
                        entry_id=wid,
                    )
            recorded_ci = section_row.get("paired_ci95")
            expected_ci = bundle_row.get("paired_ci95")
            if expected_ci is None and recorded_ci is None:
                pass
            elif (
                not isinstance(recorded_ci, list)
                or len(recorded_ci) != 2
                or not isinstance(expected_ci, list)
                or len(expected_ci) != 2
                or not _close(recorded_ci[0], expected_ci[0])
                or not _close(recorded_ci[1], expected_ci[1])
            ):
                out.fail(
                    "PERF_PROJECTION_MISMATCH",
                    "coverage paired_ci95 does not match the recomputed interval",
                    entry_id=wid,
                )

    # Re-derive every gate workload's speedup from its raw samples.
    recomputed: dict[str, float] = {}
    for wid in sorted(gate_ids & set(rows)):
        row = rows[wid]
        if row.get("status") != "ok":
            out.fail("PERF_WORKLOAD_NOT_PASSING", f"bundle status={row.get('status')!r}", entry_id=wid)
            continue
        if row.get("strict_equivalence") is not True or row.get("equivalence") != "strict":
            out.fail("PERF_NOT_STRICT", "bundle row is not strict-equivalence", entry_id=wid)
        if row.get("result_equivalent") is not True:
            out.fail("PERF_RESULT_MISMATCH", "bundle row is not result-equivalent", entry_id=wid)
        samples = row.get("samples") or {}
        python = samples.get("python")
        rust = samples.get("rust")
        if not isinstance(python, list) or not isinstance(rust, list):
            out.fail("PERF_SAMPLES_MISSING", "bundle row has no raw samples", entry_id=wid)
            continue
        if len(python) < MIN_SAMPLES or len(rust) < MIN_SAMPLES:
            out.fail(
                "PERF_SAMPLES_TOO_FEW",
                f"python={len(python)} rust={len(rust)}; need >= {MIN_SAMPLES} each",
                entry_id=wid,
            )
            continue
        try:
            python_values = [float(value) for value in python]
            rust_values = [float(value) for value in rust]
        except (TypeError, ValueError):
            out.fail("PERF_SAMPLES_NOT_NUMERIC", "samples are not numeric", entry_id=wid)
            continue
        if any(value <= 0 for value in python_values + rust_values):
            out.fail("PERF_SAMPLES_NONPOSITIVE", "samples must be positive latencies", entry_id=wid)
            continue
        speedup = _median(python_values) / _median(rust_values)
        recomputed[wid] = speedup
        recorded = row.get("speedup")
        if not isinstance(recorded, (int, float)) or abs(float(recorded) - speedup) > max(
            0.01, speedup * 0.01
        ):
            out.fail(
                "PERF_SPEEDUP_MISMATCH",
                f"recorded={recorded!r} but samples give {speedup:.4f}",
                entry_id=wid,
            )
        if speedup < 1.0 - MAX_REGRESSION:
            out.fail(
                "PERF_WORKLOAD_REGRESSION",
                f"speedup={speedup:.4f} is more than {MAX_REGRESSION:.0%} slower than Python",
                entry_id=wid,
            )
        for field_name in ("fixture_id", "command_argv", "environment", "evidence_refs"):
            if not row.get(field_name):
                out.fail("PERF_WORKLOAD_METADATA", f"bundle row lacks {field_name}", entry_id=wid)
        for ref in row.get("evidence_refs") or []:
            ref_path = Path(ref)
            ref_path = ref_path if ref_path.is_absolute() else root / ref_path
            if not ref_path.exists():
                out.fail("PERF_EVIDENCE_MISSING", f"evidence reference does not exist: {ref}", entry_id=wid)
        post = row.get("post_state") or {}
        if (row.get("measurement") == "mutation") and (
            post.get("compared") is not True or post.get("equal") is not True
        ):
            out.fail("PERF_POST_STATE", "mutation post-state is not proven equal", entry_id=wid)
        wex = (row.get("work_excess") or {}).get("work_excess_ratio")
        if wex is None:
            wex = row.get("work_excess_ratio")
        if not isinstance(wex, (int, float)) or wex <= 0:
            out.fail("PERF_WORK_EXCESS", "work-excess ratio is missing", entry_id=wid)

    min_strict = MIN_STRICT_WORKLOADS if enforce_checker_ids else int(
        registry.policy.get("minimum_strict_workloads") or MIN_STRICT_WORKLOADS
    )
    if len(recomputed) < min_strict:
        out.fail(
            "PERF_INSUFFICIENT_WORKLOADS",
            f"{len(recomputed)} strict gate workloads recompute; need {min_strict}",
        )

    baseline_commit = (performance.get("baseline") or {}).get("commit")
    candidate_commit = (performance.get("candidate") or {}).get("commit")
    for label, commit in (("baseline", baseline_commit), ("candidate", candidate_commit)):
        if not isinstance(commit, str) or len(commit) != 40 or any(
            char not in "0123456789abcdef" for char in commit
        ):
            out.fail(f"PERF_{label.upper()}_COMMIT", f"{label} commit is not a 40-hex commit id")
    if (bundle.get("baseline") or {}).get("commit") != baseline_commit:
        out.fail("PERF_EVIDENCE_BASELINE", "bundle baseline does not match the recorded baseline")
    if (bundle.get("candidate") or {}).get("commit") != candidate_commit:
        out.fail("PERF_EVIDENCE_CANDIDATE", "bundle candidate does not match the recorded candidate")

    startup = performance.get("startup") or {}
    if startup.get("status") != "pass" or not startup.get("measurements"):
        out.fail("PERF_STARTUP_NOT_MEASURED", f"startup status={startup.get('status')!r}")
    if (bundle.get("startup") or {}).get("status") != "pass":
        out.fail("PERF_STARTUP_NOT_MEASURED", "bundle startup measurement did not pass")

    # Full verifier: stored evidence, statistics, live identity, build receipt,
    # fixtures, harness code. This is the same single implementation `bench.py
    # gate` uses; there is no path here that trusts a recorded identity.
    probe = live_probe or certify.NATIVE_PROBE_FACTORY(root)
    verification = certify.evaluate_bundle(
        bundle,
        registry,
        repo=root,
        live_probe=probe,
        verify_fixtures=True,
        fixture_observer=FIXTURE_OBSERVER,
        artifacts_root=path.parent,
        require_artifacts=True,
        require_manifest=True,
    )
    bundle_gate = verification["gate"]
    for message in (bundle_gate.get("blockers") or [])[:20]:
        out.fail("PERF_BUNDLE_NOT_CERTIFIED", f"bundle blocked: {message}")
    for message in (bundle_gate.get("failures") or [])[:20]:
        out.fail("PERF_BUNDLE_NOT_CERTIFIED", f"bundle failed: {message}")
    if performance.get("status") == "pass" and bundle_gate["verdict"] != "PASS":
        out.fail(
            "PERF_STATUS_NOT_BACKED",
            f"performance.status is 'pass' but the bundle verdict is {bundle_gate['verdict']}",
        )
    _check_projection(out, performance, verification, bundle)

    return {
        "ok": not out.errors,
        "verdict": (
            "FAIL" if out.errors and bundle_gate["verdict"] == "PASS" else bundle_gate["verdict"]
        ),
        "errors": out.errors,
        "warnings": out.warnings,
        "recomputed": {
            "speedups": recomputed,
            "geometric_mean_speedup": (_geomean(list(recomputed.values())) if recomputed else None),
            "sample_count": sum(
                len(rows[wid].get("samples", {}).get("rust") or []) for wid in recomputed
            ),
            **{
                key: value
                for key, value in (verification.get("recomputed") or {}).items()
                if key != "geometric_mean_speedup"
            },
        },
        "bundle": str(path),
    }


def _rows_by_id(value, out: _Collector, label: str) -> dict[str, dict]:
    if not isinstance(value, list):
        out.fail("PERF_EVIDENCE_MALFORMED", f"{label} workloads is not a list")
        return {}
    rows: dict[str, dict] = {}
    for row in value:
        if not isinstance(row, dict):
            out.fail("PERF_EVIDENCE_MALFORMED", f"{label} workloads contains a non-object row")
            continue
        wid = row.get("id")
        if not isinstance(wid, str) or not wid:
            out.fail("PERF_EVIDENCE_MALFORMED", f"{label} workload row has no string id")
            continue
        rows[wid] = row
    return rows


def check_performance(report, performance: dict, *, root: Path = ROOT, gate: str = "performance") -> dict:
    """Alias used by the checker integration patch."""
    return check_bundle(report, performance, root=root, gate=gate)


# ---------------------------------------------------------------------------
# Section projection / CLI
# ---------------------------------------------------------------------------


def _verify_bundle(bundle_path: Path, registry_path: Path | None, *, live_probe=None) -> tuple[dict, dict, dict]:
    """Load a bundle and recompute its verdict from stored + live evidence."""
    registry = _registry(registry_path)
    bundle = certify.load_bundle(bundle_path)
    if not isinstance(bundle, dict):
        raise SystemExit("evidence bundle is not an object")
    if bundle.get("schema") != certify.SCHEMA:
        raise SystemExit(
            f"evidence schema {bundle.get('schema')!r} predates the certification bundle; "
            "historical evidence cannot be re-emitted as a passing section"
        )
    repo = _repo_root(bundle_path)
    probe = live_probe or certify.NATIVE_PROBE_FACTORY(repo)
    verification = certify.evaluate_bundle(
        bundle,
        registry,
        repo=repo,
        live_probe=probe,
        verify_fixtures=True,
        fixture_observer=FIXTURE_OBSERVER,
        artifacts_root=Path(bundle_path).resolve().parent,
        require_artifacts=True,
        require_manifest=True,
    )
    return bundle, verification, registry


def _repo_root(bundle_path: Path) -> Path:
    """The checkout an out-of-tree bundle was written from, else this repo."""
    bundle = certify.load_bundle(bundle_path) if Path(bundle_path).is_file() else {}
    manifest = (bundle.get("candidate") or {}).get("build_receipt") or {}
    repo = manifest.get("repo")
    if isinstance(repo, str) and Path(repo).is_dir():
        return Path(repo)
    return ROOT


def performance_section(bundle_path: Path, *, registry_path: Path | None = None) -> dict:
    """Project a bundle only after re-verifying it. Raises for a non-PASS bundle."""
    bundle, verification, _registry_obj = _verify_bundle(bundle_path, registry_path)
    section = certify.performance_section(bundle, bundle_path, verification=verification)
    # The checker compares performance.policy exactly against
    # inventory_model.PERF_POLICY; include that curated dict so a wholesale
    # application of this section is valid.
    section["policy"] = dict(PERF_POLICY)
    return section


def _cmd_emit(args: argparse.Namespace) -> int:
    bundle_path = Path(args.bundle).resolve()
    try:
        bundle, verification, _registry_obj = _verify_bundle(
            bundle_path, Path(args.registry) if args.registry else None
        )
    except (OSError, ValueError, SystemExit, certify.EvidenceError) as exc:
        print(json.dumps({"ok": False, "error": str(exc), "status": "blocked"}, indent=2))
        return 2
    verdict = verification["gate"]["verdict"]
    section = certify.performance_section(bundle, bundle_path, verification=verification)
    section["policy"] = dict(PERF_POLICY)
    text = json.dumps(section, indent=2, sort_keys=True) + "\n"
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
        print(
            json.dumps(
                {
                    "out": str(Path(args.out).resolve()),
                    "status": section["status"],
                    "verdict": verdict,
                    "geometric_mean_speedup": section["aggregate"]["geometric_mean_speedup"],
                },
                indent=2,
            )
        )
    else:
        print(text, end="")
    return 0 if (verdict == "PASS" and section["status"] == "pass") else 2


def _cmd_review(args: argparse.Namespace) -> int:
    """Check an existing coverage.json performance section against a bundle."""
    coverage = json.loads(Path(args.coverage).read_text(encoding="utf-8"))
    section = coverage.get("performance") or {}
    if args.bundle:
        bundle_path = Path(args.bundle).resolve()
        bundle = certify.load_bundle(bundle_path)
        recorded = (section.get("aggregate") or {}).get("evidence") or {}
        if recorded.get("sha256") != _sha256(bundle_path):
            print(
                json.dumps(
                    {
                        "ok": False,
                        "error": "coverage.json performance.aggregate.evidence does not point at this bundle",
                    },
                    indent=2,
                )
            )
            return 2
        del bundle
    result = check_bundle(
        None,
        section,
        root=Path(args.root).resolve(),
        registry_path=Path(args.registry) if args.registry else None,
    )
    print(json.dumps(result, indent=2, sort_keys=True, default=str))
    if result["verdict"] != "PASS":
        return 2
    return 0 if result["ok"] else 1


def _cmd_validate(args: argparse.Namespace) -> int:
    path = Path(args.bundle).resolve()
    try:
        bundle, verification, registry = _verify_bundle(
            path, Path(args.registry) if args.registry else None
        )
    except (OSError, ValueError, SystemExit, certify.EvidenceError) as exc:
        print(json.dumps({"ok": False, "error": str(exc), "verdict": "BLOCKED"}, indent=2))
        return 2
    section = certify.performance_section(bundle, path, verification=verification)
    section["policy"] = dict(PERF_POLICY)
    del registry
    result = check_bundle(
        None,
        section,
        root=Path(args.root).resolve(),
        registry_path=Path(args.registry) if args.registry else None,
        enforce_checker_ids=not args.allow_synthetic_registry,
    )
    print(json.dumps(result, indent=2, sort_keys=True, default=str))
    if result["verdict"] != "PASS":
        return 2
    return 0 if result["ok"] else 1


def _cmd_registry(args: argparse.Namespace) -> int:
    registry = _registry(Path(args.registry) if args.registry else None)
    print(
        json.dumps(
            {
                "identity": registry.identity(),
                "policy": registry.policy,
                "checker_ids_match": set(registry.required_ids) == REQUIRED_ALL_IDS,
                "gate_ids_match": set(registry.gate_ids) == REQUIRED_GATE_IDS,
                "checker_policy": PERF_POLICY,
            },
            indent=2,
        )
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--registry", default=None)
    sub = parser.add_subparsers(dest="command", required=True)

    registry = sub.add_parser("registry", help="show the frozen registry and checker agreement")
    registry.set_defaults(func=_cmd_registry)

    validate = sub.add_parser("validate", help="verify a bundle against the checker contract")
    validate.add_argument("--bundle", required=True)
    validate.add_argument("--root", default=str(ROOT))
    validate.add_argument(
        "--allow-synthetic-registry",
        action="store_true",
        help="audit-only: skip the checker-id equality assertion (never for CI)",
    )
    validate.set_defaults(func=_cmd_validate)

    review = sub.add_parser("review", help="check coverage.json against a bundle")
    review.add_argument("--coverage", default=str(ROOT / "docs/rust/inventory/coverage.json"))
    review.add_argument("--bundle", default=None)
    review.add_argument("--root", default=str(ROOT))
    review.set_defaults(func=_cmd_review)

    emit = sub.add_parser("emit", help="print the coverage.json performance section proposal")
    emit.add_argument("--bundle", required=True)
    emit.add_argument("--out", default=None)
    emit.set_defaults(func=_cmd_emit)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
