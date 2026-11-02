#!/usr/bin/env python3
"""Oracle import-floor control.

Imports exactly the upstream modules a paired adapter needs, then exits. The
difference between an adapter's total time and this floor is the measured
*work*; the floor itself is the interpreter/import cost of the Python reference
protocol and is reported, never hidden.
"""

from __future__ import annotations

import argparse
import importlib


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--module", action="append", default=[])
    args = parser.parse_args()
    for name in args.module:
        importlib.import_module(name)
    print("{}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
