#!/usr/bin/env python3
"""Python-reference oracle for the pure work-package lane FSM.

Calls the single upstream authority for lane edges, guards, and force override:
``specify_cli.status.wp_state.wp_state_for(from).check_transition(target, ctx)``.
No event is written and no status snapshot is read for the common guard inputs.
"""

from __future__ import annotations

import argparse
import json
import sys


def _tri(value: str | None) -> bool | None:
    if value is None:
        return None
    return value.lower() in {"1", "true", "yes", "on"}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("from_lane")
    parser.add_argument("to_lane")
    parser.add_argument("--actor", default=None)
    parser.add_argument("--reason", default=None)
    parser.add_argument("--review-ref", default=None)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--workspace-context", default=None)
    parser.add_argument("--subtasks-complete", default=None)
    parser.add_argument("--implementation-evidence-present", default=None)
    parser.add_argument("--dependency-ready", default=None)
    parser.add_argument("--current-actor", default=None)
    args = parser.parse_args()

    from specify_cli.status.models import GuardContext, Lane
    from specify_cli.status.wp_state import wp_state_for

    ctx = GuardContext(
        actor=args.actor,
        workspace_context=args.workspace_context,
        subtasks_complete=_tri(args.subtasks_complete),
        implementation_evidence_present=_tri(args.implementation_evidence_present),
        reason=args.reason,
        review_ref=args.review_ref,
        evidence=None,
        force=args.force,
        review_result=None,
        current_actor=args.current_actor,
        dependency_ready=_tri(args.dependency_ready),
    )
    source = Lane(args.from_lane.replace("doing", "in_progress"))
    target = Lane(args.to_lane.replace("doing", "in_progress"))
    admitted, refusal = wp_state_for(source).check_transition(target, ctx)
    payload = {
        "from": source.value,
        "to": target.value,
        "input_from": args.from_lane,
        "input_to": args.to_lane,
        "admitted": admitted,
        "force": bool(args.force),
        "refusal": refusal,
    }
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return 0 if admitted else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1)
