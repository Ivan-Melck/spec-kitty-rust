"""Shared fixtures for the benchmark test suite.

Required measurement prerequisites are **not skipped**. A suite that reports
green while the lock-exact oracle or the release binaries are missing protects
nothing, so an autouse session fixture fails the run instead.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

BENCH_ROOT = Path(__file__).resolve().parents[1]
REPO = BENCH_ROOT.parents[1]
if str(BENCH_ROOT) not in sys.path:
    sys.path.insert(0, str(BENCH_ROOT))

ORACLE = Path(os.environ.get("SPKBENCH_ORACLE_PYTHON", REPO / ".venv-rust-oracle" / "bin" / "python"))
BINARIES = {
    "spec-kitty": REPO / "target" / "release" / "spec-kitty",
    "spec-kitty-rust": REPO / "target" / "release" / "spec-kitty-rust",
}


@pytest.fixture(scope="session", autouse=True)
def require_measurement_environment() -> None:
    """Fail (never skip) when the oracle or release binaries are absent."""
    problems = []
    if not ORACLE.exists():
        problems.append(
            f"oracle interpreter missing: {ORACLE} "
            "(provision with `uv sync --frozen --no-dev` into .venv-rust-oracle)"
        )

    for name, path in BINARIES.items():
        if not path.exists():
            problems.append(f"release binary missing: {path} (run `bench.py build`)")
    if problems:
        pytest.fail(
            "required benchmark environment is absent; tests must fail rather than skip:\n  "
            + "\n  ".join(problems),
            pytrace=False,
        )
