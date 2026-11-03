"""Small, dependency-free statistics helpers.

The harness deliberately avoids numpy/scipy so it can run under the pinned
oracle interpreter without changing that environment. Bootstrap intervals use a
seeded ``random.Random`` so a report is reproducible from its seed.
"""

from __future__ import annotations

import math
import random
import statistics
from typing import Iterable, Sequence


def median(values: Sequence[float]) -> float:
    return statistics.median(values)


def mean(values: Sequence[float]) -> float:
    return statistics.fmean(values)


def stdev(values: Sequence[float]) -> float:
    return statistics.stdev(values) if len(values) > 1 else 0.0


def cv(values: Sequence[float]) -> float:
    """Coefficient of variation (stdev / mean); 0 when mean is 0 or n < 2."""
    if len(values) < 2:
        return 0.0
    m = mean(values)
    if m == 0:
        return 0.0
    return stdev(values) / m


def median_absolute_deviation(values: Sequence[float]) -> float:
    if not values:
        return 0.0
    m = median(values)
    return statistics.median([abs(v - m) for v in values])


def modified_zscores(values: Sequence[float]) -> list[float]:
    """Robust z-scores (Iglewicz-Hoaglin). 0 when MAD is 0."""
    if not values:
        return []
    m = median(values)
    mad = median_absolute_deviation(values)
    if mad == 0:
        return [0.0 for _ in values]
    return [0.6745 * (v - m) / mad for v in values]


def outlier_indices(values: Sequence[float], threshold: float = 3.5) -> list[int]:
    zs = modified_zscores(values)
    return [i for i, z in enumerate(zs) if abs(z) > threshold]


def geometric_mean(values: Iterable[float]) -> float:
    items = [v for v in values]
    if not items:
        raise ValueError("geometric mean of empty sequence")
    if any(v <= 0 for v in items):
        raise ValueError("geometric mean requires positive values")
    return math.exp(math.fsum(math.log(v) for v in items) / len(items))


def bootstrap_median_ci(
    values: Sequence[float],
    *,
    iterations: int = 2000,
    confidence: float = 0.95,
    rng: random.Random | None = None,
) -> tuple[float, float]:
    """Percentile bootstrap confidence interval for the median."""
    if not values:
        raise ValueError("bootstrap of empty sequence")
    if len(values) == 1 or iterations <= 0:
        m = median(values)
        return m, m
    rng = rng or random.Random(0)
    n = len(values)
    medians = []
    for _ in range(iterations):
        sample = [values[rng.randrange(n)] for _ in range(n)]
        medians.append(median(sample))
    medians.sort()
    alpha = (1.0 - confidence) / 2.0
    lo = medians[max(0, int(math.floor(alpha * iterations)))]
    hi = medians[min(iterations - 1, int(math.ceil((1.0 - alpha) * iterations)) - 1)]
    return lo, hi


def bootstrap_ratio_ci(
    numerator: Sequence[float],
    denominator: Sequence[float],
    *,
    iterations: int = 2000,
    confidence: float = 0.95,
    rng: random.Random | None = None,
) -> tuple[float, float]:
    """Percentile bootstrap CI for median(numerator)/median(denominator).

    ``numerator`` is the reference (Python) sample and ``denominator`` the
    candidate (Rust) sample. A ratio > 1 means the candidate is faster.
    """
    if not numerator or not denominator:
        raise ValueError("bootstrap ratio needs two non-empty samples")
    rng = rng or random.Random(0)
    n_num, n_den = len(numerator), len(denominator)
    ratios = []
    for _ in range(iterations):
        num_sample = [numerator[rng.randrange(n_num)] for _ in range(n_num)]
        den_sample = [denominator[rng.randrange(n_den)] for _ in range(n_den)]
        den_median = median(den_sample)
        if den_median == 0:
            continue
        ratios.append(median(num_sample) / den_median)
    if not ratios:
        raise ValueError("bootstrap ratio produced no finite resamples")
    ratios.sort()
    alpha = (1.0 - confidence) / 2.0
    lo = ratios[max(0, int(math.floor(alpha * len(ratios))))]
    hi = ratios[min(len(ratios) - 1, int(math.ceil((1.0 - alpha) * len(ratios))) - 1)]
    return lo, hi


def summarize(values: Sequence[float]) -> dict:
    """Raw-sample summary used in every workload report."""
    if not values:
        return {"n": 0}
    return {
        "n": len(values),
        "median_s": median(values),
        "mean_s": mean(values),
        "min_s": min(values),
        "max_s": max(values),
        "stdev_s": stdev(values),
        "cv": cv(values),
        "mad_s": median_absolute_deviation(values),
        "outlier_indices": outlier_indices(values),
        "median_ci95_s": list(bootstrap_median_ci(values)),
    }


def noise_level(rust_cv: float, oracle_cv: float, ci_low: float, ci_high: float) -> str:
    """Classify measurement noise for a paired workload.

    ``low`` requires both samples reasonably stable and a ratio CI that does
    not straddle 1.0. This is descriptive, not a statistical significance test.
    """
    worst_cv = max(rust_cv, oracle_cv)
    width = (ci_high / ci_low) if ci_low > 0 else float("inf")
    if worst_cv <= 0.10 and width <= 1.25:
        return "low"
    if worst_cv <= 0.30 and width <= 2.00:
        return "moderate"
    return "high"


# ---------------------------------------------------------------------------
# Paired (interleaved) statistics
#
# Each repetition runs the Python and Rust side adjacent in the same pair, in
# randomized order. A per-pair ratio is therefore a much better estimator than
# a difference of independent medians: shared machine noise cancels instead of
# inflating the interval. The point estimate used for the checker-facing
# speedup stays median(python)/median(rust) (that is what the completion
# checker recomputes), while uncertainty is derived from the paired ratios.
# ---------------------------------------------------------------------------


def paired_ratios(numerator: Sequence[float], denominator: Sequence[float]) -> list[float]:
    """Ratio per matched pair; requires equal, positive, pair-aligned samples."""
    if len(numerator) != len(denominator):
        raise ValueError("paired samples must have equal length")
    ratios = []
    for num, den in zip(numerator, denominator):
        if num <= 0 or den <= 0:
            raise ValueError("paired ratios require positive latencies")
        ratios.append(num / den)
    return ratios


def bootstrap_median_ci_of(values: Sequence[float], *, iterations: int = 2000,
                           confidence: float = 0.95, seed: int = 0) -> tuple[float, float]:
    """Percentile bootstrap CI for the median of ``values`` (seeded)."""
    if not values:
        raise ValueError("bootstrap of empty sequence")
    rng = random.Random(seed)
    n = len(values)
    medians = [median([values[rng.randrange(n)] for _ in range(n)]) for _ in range(max(1, iterations))]
    medians.sort()
    alpha = (1.0 - confidence) / 2.0
    lo = medians[max(0, int(math.floor(alpha * len(medians))))]
    hi = medians[min(len(medians) - 1, int(math.ceil((1.0 - alpha) * len(medians))) - 1)]
    return lo, hi


def paired_ratio_ci(numerator: Sequence[float], denominator: Sequence[float], *,
                    iterations: int = 2000, confidence: float = 0.95, seed: int = 0) -> dict:
    """Median paired ratio with a seeded paired bootstrap CI."""
    ratios = paired_ratios(numerator, denominator)
    point = median(ratios)
    lo, hi = bootstrap_median_ci_of(ratios, iterations=iterations, confidence=confidence, seed=seed)
    return {
        "pairs": len(ratios),
        "point": point,
        "ci95": [lo, hi],
        "pair_ratios": ratios,
        "seed": seed,
        "iterations": iterations,
    }


def geomean_ci(
    per_workload: dict[str, Sequence[float]],
    *,
    iterations: int = 2000,
    confidence: float = 0.95,
    seed: int = 0,
) -> dict:
    """Geometric mean over workloads with an uncertainty interval.

    Every bootstrap iteration resamples the *pairs* of every workload (so
    per-workload sampling error is represented), recomputes each workload's
    median paired ratio, and takes the geometric mean across workloads. This
    answers the question the gate actually asks: how uncertain is the aggregate
    floor, not how noisy one machine happened to be.
    """
    if not per_workload:
        raise ValueError("geomean CI needs at least one workload")
    point_ratios = {wid: median(list(values)) for wid, values in per_workload.items()}
    point = geometric_mean(point_ratios.values())
    rng = random.Random(seed)
    draws: list[float] = []
    items = [(wid, list(values)) for wid, values in per_workload.items()]
    for _ in range(max(1, iterations)):
        sampled = []
        for _wid, values in items:
            n = len(values)
            sampled.append(median([values[rng.randrange(n)] for _ in range(n)]))
        draws.append(geometric_mean(sampled))
    draws.sort()
    alpha = (1.0 - confidence) / 2.0
    lo = draws[max(0, int(math.floor(alpha * len(draws))))]
    hi = draws[min(len(draws) - 1, int(math.ceil((1.0 - alpha) * len(draws))) - 1)]
    return {
        "point": point,
        "ci95": [lo, hi],
        "workloads": len(items),
        "iterations": iterations,
        "seed": seed,
    }


def regression_verdict(
    ratio: float,
    ci_low: float,
    ci_high: float,
    max_regression_fraction: float,
) -> tuple[str, str]:
    """Classify a slow-down with uncertainty.

    ``ok``            the point estimate is within the allowed regression.
    ``material``      the whole interval is below the allowed floor: the
                      candidate is confidently more than ``max_regression``
                      slower, which fails the gate.
    ``uncertain``     the point estimate regresses but the interval still
                      reaches the allowed region: not evidence of a pass, so
                      the gate blocks instead of certifying.
    """
    floor = 1.0 - max_regression_fraction
    if ratio >= floor:
        return "ok", ""
    if ci_high < floor:
        return "material", (
            f"speedup {ratio:.4f} with 95% CI {ci_low:.4f}-{ci_high:.4f} is confidently "
            f"more than {max_regression_fraction:.0%} slower than Python"
        )
    if ci_high < 1.0:
        return "uncertain", (
            f"speedup {ratio:.4f} with 95% CI {ci_low:.4f}-{ci_high:.4f} is confidently "
            f"slower than Python, but the interval does not exclude the "
            f"{floor:.3f} floor"
        )
    return "uncertain", (
        f"speedup {ratio:.4f} with 95% CI {ci_low:.4f}-{ci_high:.4f} regresses at the "
        f"point estimate; the interval still reaches the allowed region"
    )
