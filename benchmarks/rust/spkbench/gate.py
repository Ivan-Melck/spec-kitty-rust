"""Frozen gate policy.

The first harness exposed ``--min-ratio`` / ``--min-samples`` /
``--allow-noise`` / ``--allow-identity-drift`` and judged a self-declared JSON
report. Both are gone: thresholds now live in the frozen registry and every
decision is recomputed from raw paired samples, live identity and hash-bound
artifacts by :mod:`spkbench.certify`.

The old ``GateOptions`` relaxation surface is intentionally not re-exported. A
caller that tries to lower a floor gets an ``AttributeError`` rather than a
silently weaker certification.
"""

from __future__ import annotations

from .certify import evaluate_bundle  # noqa: F401 - the one gate implementation
from .registry import PerformanceRegistry  # noqa: F401


def frozen_policy(registry: PerformanceRegistry) -> dict:
    """The decision-relevant floors, for display only."""
    policy = registry.policy
    return {
        "minimum_speedup": policy.get("minimum_speedup"),
        "minimum_samples_per_side": policy.get("minimum_samples_per_side"),
        "minimum_strict_workloads": policy.get("minimum_strict_workloads"),
        "max_per_workload_regression_fraction": policy.get("max_per_workload_regression_fraction"),
        "confidence_level": policy.get("confidence_level"),
        "min_work_excess_ratio": policy.get("min_work_excess_ratio"),
        "max_load_1m": policy.get("max_load_1m"),
    }
