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
