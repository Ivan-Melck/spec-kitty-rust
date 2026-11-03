"""Make the benchmark harness importable without installing it.

These tests cover the development-only adapters, fixture recipes and workload
registration under ``benchmarks/rust``. They never run a timing campaign.
"""

from __future__ import annotations

import sys
from pathlib import Path

BENCH_ROOT = Path(__file__).resolve().parents[1]
if str(BENCH_ROOT) not in sys.path:
    sys.path.insert(0, str(BENCH_ROOT))
