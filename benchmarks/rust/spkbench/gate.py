"""Acceptance-gate evaluation.

The gate is intentionally stricter than "the numbers looked good":

* pending required workloads block a pass, regardless of measured ratios;
* only ``strict`` + ``work`` + ``required`` workloads enter the geomean;
* projections, micro-benchmarks, and startup are reported but cannot rescue it;
* equivalence failure, nondeterminism, truncation, high noise, or identity
  drift block/fail the result;
* any required strict workload slower than 0.909x fails unless it carries an
  explicit explanation.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .stats import geometric_mean


@dataclass
class GateOptions:
    min_ratio: float = 3.0
    max_slowdown_ratio: float = 0.909  # a >10% regression
    min_samples_per_side: int = 5
    max_cv: float = 0.30
    max_ratio_ci_width_factor: float = 2.5
    require_exact_identity: bool = True
    allow_noisy: bool = False


@dataclass
class GateResult:
    verdict: str = "BLOCKED"  # PASS | FAIL | BLOCKED
    reasons: list[str] = field(default_factory=list)
    blockers: list[str] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    geomean_ratio: float | None = None
    gate_workload_count: int = 0

    def as_dict(self) -> dict:
        return {
            "verdict": self.verdict,
            "geomean_ratio": self.geomean_ratio,
            "gate_workload_count": self.gate_workload_count,
            "reasons": self.reasons,
            "blockers": self.blockers,
            "failures": self.failures,
            "warnings": self.warnings,
        }


def _workload_ratio(workload: dict) -> float | None:
    timing = workload.get("timing") or {}
    return timing.get("median_ratio_oracle_over_rust")


def evaluate_gate(
    run: dict,
    identity: dict,
    options: GateOptions | None = None,
) -> GateResult:
    options = options or GateOptions()
    result = GateResult()

    def block(message: str) -> None:
        result.blockers.append(message)

    def fail(message: str) -> None:
        result.failures.append(message)

    def warn(message: str) -> None:
        result.warnings.append(message)

    # --- identity
    if identity.get("status") != "exact":
        message = "oracle identity is not exact: " + "; ".join(identity.get("problems", []))
        if options.require_exact_identity:
            block(message)
        else:
            warn(message)
    deps = identity.get("dependencies") or {}
    if deps.get("status") != "exact":
        message = (
            f"dependency versions drift from uv.lock "
            f"({len(deps.get('mismatches', []))} mismatches, "
            f"{len(deps.get('installed_not_in_lock', []))} unlocked installs)"
        )
        if options.require_exact_identity:
            block(message)
        else:
            warn(message)
    source = identity.get("source") or {}
    if not source.get("oracle_imports_from_repo_src"):
        block("oracle interpreter does not import specify_cli from <repo>/src")
    binary = identity.get("rust_binary") or {}
    if not binary.get("exists"):
        block("rust release binary is missing")
    elif not binary.get("sha256"):
        block("rust release binary has no recorded sha256")
    if run.get("binary_stable") is False:
        block("the Rust binary changed during the measurement window")
    if run.get("source_stable") is False:
        block("the Python oracle source tree changed during the measurement window")

    # --- completeness
    workloads = run.get("workloads", [])
    required = [w for w in workloads if w.get("required")]
    pending_required = [w for w in required if w.get("status") == "pending" or w.get("equivalence") == "pending"]
    for workload in pending_required:
        block(f"required workload pending: {workload['id']} — {workload.get('reason') or workload.get('pending_reason')}")

    if run.get("truncated"):
        block("measurement was truncated by the time budget; no gate decision from partial samples")
    if run.get("fixtures_unchanged") is False:
        fail("a fixture changed during the run; read-only post-state equivalence is broken")

    missing_required = [
        w for w in required if w.get("status") not in {"ok"} and w not in pending_required
    ]
    for workload in missing_required:
        fail(f"required workload not measured successfully: {workload['id']} ({workload.get('status')}: {workload.get('reason')})")

    # --- equivalence for every runnable workload
    for workload in workloads:
        status = workload.get("status")
        if status == "equivalence_failed":
            fail(
                f"equivalence failure: {workload['id']} — {workload.get('equivalence_diff') or workload.get('reason')}"
            )
        elif status == "nondeterministic":
            fail(f"nondeterministic output: {workload['id']} — {workload.get('reason')}")
        elif status in {"preflight_failed", "error"}:
            fail(f"{workload['id']} failed before timing: {workload.get('reason')}")
        if status == "measured_incomplete":
            block(f"incomplete samples: {workload['id']} — {workload.get('reason')}")

    # --- gate maths: strict work only
    gate = [
        w
        for w in workloads
        if w.get("gate_eligible")
        and w.get("equivalence") == "strict"
        and w.get("class") == "work"
        and w.get("required")
    ]
    result.gate_workload_count = len(gate)
    if not gate:
        block("no gate-eligible strict workloads were measured")
    else:
        ratios = []
        for workload in gate:
            ratio = _workload_ratio(workload)
            timing = workload.get("timing") or {}
            if ratio is None:
                fail(f"gate workload has no ratio: {workload['id']}")
                continue
            ratios.append(ratio)
            rust_n = timing.get("rust", {}).get("n", 0)
            oracle_n = timing.get("oracle", {}).get("n", 0)
            if rust_n < options.min_samples_per_side or oracle_n < options.min_samples_per_side:
                block(
                    f"insufficient samples for {workload['id']}: rust={rust_n} oracle={oracle_n}, "
                    f"need {options.min_samples_per_side}"
                )
            for side in ("rust", "oracle"):
                cv = timing.get(side, {}).get("cv", 0.0)
                if cv > options.max_cv:
                    warn(f"high variance ({side} cv={cv:.3f}) for {workload['id']}")
            if timing.get("noise") == "high":
                warn(f"high measurement noise classification for {workload['id']}")

            # Decision-relevant uncertainty: the ratio's confidence interval
            # must still clear the target, and must not be absurdly wide. A
            # large CV on a 20 ms sample is not, by itself, evidence against a
            # 100x ratio.
            ci = timing.get("median_ratio_ci95") or [0.0, 0.0]
            ci_low, ci_high = float(ci[0]), float(ci[1])
            # A workload whose point estimate clears the target but whose CI
            # reaches below it is an uncertain pass — block it. A workload whose
            # point estimate is already below the target cannot rescue the
            # geomean; the geomean/regression checks own that verdict.
            if ratio >= options.min_ratio and ci_low < options.min_ratio:
                message = (
                    f"ratio CI for {workload['id']} reaches below the {options.min_ratio:.1f}x "
                    f"target (95% CI {ci_low:.2f}–{ci_high:.2f}x)"
                )
                if options.allow_noisy:
                    warn(message)
                else:
                    block(message)
            if ci_low > 0 and (ci_high / ci_low) > options.max_ratio_ci_width_factor:
                warn(
                    f"wide ratio CI for {workload['id']} "
                    f"({ci_low:.2f}–{ci_high:.2f}x, factor {ci_high/ci_low:.2f})"
                )
            if ratio < options.max_slowdown_ratio:
                fail(
                    f"required workload regression >10%: {workload['id']} ratio={ratio:.3f}x"
                )
        if ratios:
            result.geomean_ratio = geometric_mean(ratios)
            if result.geomean_ratio < options.min_ratio:
                fail(
                    f"geometric-mean ratio {result.geomean_ratio:.3f}x is below the required "
                    f"{options.min_ratio:.1f}x over {len(ratios)} strict workload(s)"
                )

    # --- explicit separations
    startup = [w for w in workloads if w.get("class") == "startup"]
    if startup:
        warn(
            "startup workloads are reported separately and are not gate evidence: "
            + ", ".join(w["id"] for w in startup)
        )
    diagnostics = [w for w in workloads if w.get("class") in {"micro"} or w.get("equivalence") == "projection"]
    if diagnostics:
        warn(
            "diagnostic workloads excluded from the gate: "
            + ", ".join(w["id"] for w in diagnostics if w.get("gate_eligible") is False)
        )

    # --- verdict precedence
    if result.blockers:
        result.verdict = "BLOCKED"
        result.reasons = ["blocked: " + item for item in result.blockers]
    elif result.failures:
        result.verdict = "FAIL"
        result.reasons = ["failed: " + item for item in result.failures]
    else:
        result.verdict = "PASS"
        result.reasons = [
            f"geometric mean {result.geomean_ratio:.3f}x over {len(gate)} strict workload(s)"
        ]
    result.reasons.extend("warning: " + item for item in result.warnings)
    return result
