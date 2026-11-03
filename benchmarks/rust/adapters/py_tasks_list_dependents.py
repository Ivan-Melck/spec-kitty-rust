#!/usr/bin/env python3
"""Python-reference oracle for ``agent tasks list-dependents --json``.

Calls the pinned upstream command implementation. The comparison is a
projection: upstream's ``depends_on`` field comes from a scalar frontmatter
reader that returns raw list text (``"[]"`` / ``"- WP01"``), while native returns
a parsed array. Only ``wp_id`` and the dependency-graph ``dependents`` are
shared upstream-defined facts; the raw scalar is excluded and reported by the
workload notes instead of being silently normalised in a strict pair.
"""

from __future__ import annotations

import argparse
import json
import os
import sys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--mission", required=True)
    parser.add_argument("--wp-id", required=True)
    args = parser.parse_args()

    from specify_cli.cli.commands.agent.tasks import list_dependents

    os.chdir(args.repo)
    list_dependents(wp_id=args.wp_id, mission=args.mission, json_output=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1) from None
