#!/usr/bin/env python3
"""Python-reference oracle for Mission listing (projection).

Upstream has no command that emits the Rust inventory's full metadata document.
The one upstream population/identity definition is
``list_missions_for_selection``; this adapter projects it to the fields both
sides can be held to exactly: slug, mid8, friendly name.

This is a *projection* pair: it is reported for diagnosis and is excluded from
the 3x acceptance gate until a full upstream inventory contract exists.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    args = parser.parse_args()

    from specify_cli.context.mission_resolver import list_missions_for_selection

    listings = list_missions_for_selection(Path(args.repo))
    rows = [
        {"slug": item.mission_slug, "mid8": item.mid8, "friendly_name": item.friendly_name}
        for item in listings
    ]
    print(json.dumps({"count": len(rows), "missions": rows}, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1)
