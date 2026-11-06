#!/usr/bin/env python3
"""Python-reference oracle for ``agent status materialize --json``.

Calls the pinned upstream command implementation in the repository working
directory. It writes the canonical ``status.json`` exactly as the CLI does, so
the mutation executor compares both the emitted JSON document and the resulting
file bytes against a fresh clone of the same pristine fixture.

The status lock artifact is implementation-private and is recorded separately
(upstream ``.git/spec-kitty-locks/<slug>.status.lock`` or
``.kittify/spec-kitty-locks/<slug>.status.lock``) rather than treated as equal.
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
    args = parser.parse_args()

    from specify_cli.cli.commands.agent.status import materialize

    os.chdir(args.repo)
    materialize(mission=args.mission, json_output=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1) from None
