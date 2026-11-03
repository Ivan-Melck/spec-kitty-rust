#!/usr/bin/env python3
"""Python-reference oracle for ``agent tasks list-tasks --json``.

Calls the pinned upstream command implementation
(``specify_cli.cli.commands.agent.tasks.list_tasks``) rather than
reimplementing its mission resolution, canonical-lane reduction or sort order.
The adapter only supplies the explicit mission handle and JSON mode, then
changes into the repository so upstream's project-root discovery is the same
one the CLI uses.

This is a *strict* pair with the native compatibility route: both emit
``{"tasks": [...], "count": n}`` and every compared field is upstream-defined.
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
    parser.add_argument("--lane", default=None)
    args = parser.parse_args()

    from specify_cli.cli.commands.agent.tasks import list_tasks

    os.chdir(args.repo)
    list_tasks(lane=args.lane, mission=args.mission, json_output=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1) from None
