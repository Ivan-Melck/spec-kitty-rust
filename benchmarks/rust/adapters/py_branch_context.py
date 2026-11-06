#!/usr/bin/env python3
"""Python-reference oracle for ``agent mission branch-context --json``.

Calls the pinned upstream command implementation. The pair is a projection:
wall-clock fields are declared nondeterministic, and upstream emits
``spec_kitty_version`` while the native route does not. Every branch-contract
field (current/target/base/primary branches, match verdicts, strategy and
reason text) is compared.
"""

from __future__ import annotations

import argparse
import json
import os
import sys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--target-branch", default=None)
    args = parser.parse_args()

    from specify_cli.cli.commands.agent.mission_branch_context import branch_context

    os.chdir(args.repo)
    branch_context(json_output=True, target_branch=args.target_branch)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1) from None
