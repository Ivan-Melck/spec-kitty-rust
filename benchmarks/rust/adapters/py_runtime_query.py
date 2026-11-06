#!/usr/bin/env python3
"""Python-reference oracle for the pure mission-runtime planner.

This is development tooling, not a production fallback. It calls the pinned
upstream implementation (``runtime.next._internal_runtime.planner.plan_next``)
with the same frozen template and run snapshot the Rust command receives, then
prints the upstream decision document.

Exit codes mirror the Rust adapter's contract: 0 for a step/terminal decision,
3 for a blocked decision, 1 for an input/validation refusal.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--template", required=True)
    parser.add_argument("--state", required=True)
    parser.add_argument("--live-template", default=None)
    args = parser.parse_args()

    from runtime.next._internal_runtime.planner import plan_next
    from runtime.next._internal_runtime.schema import MissionRunSnapshot, load_mission_template_file

    template = load_mission_template_file(Path(args.template))
    snapshot = MissionRunSnapshot.model_validate_json(Path(args.state).read_text(encoding="utf-8"))
    live = Path(args.live_template) if args.live_template else None

    decision = plan_next(
        snapshot,
        template,
        snapshot.policy_snapshot,
        actor_context=None,
        live_template_path=live,
    )
    payload = decision.model_dump(mode="json")
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return 3 if payload.get("kind") == "blocked" else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1)
