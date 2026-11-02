#!/usr/bin/env python3
"""Python-reference oracle for glossary term resolution.

Calls the pinned upstream glossary primitives:

* ``glossary.scope.load_seed_file`` (seed parsing/validation),
* ``glossary.resolution.resolve_term`` (scope-order lookup),
* ``glossary.conflict.classify_conflict`` / ``score_severity`` /
  ``create_conflict`` (ambiguity/unknown classification).

The harness maps the upstream objects into the Rust lookup document's shape.
``result.status`` is a Rust-side aggregation of upstream match facts (count of
active senses); the harness marks it as a derived field, and every other field
is upstream-defined.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

SCOPE_ORDER = ("mission_local", "team_domain", "audience_domain", "spec_kitty_core")


def _sense_dict(sense) -> dict:
    status = getattr(sense.status, "value", sense.status)
    return {
        "surface": sense.surface.surface_text,
        "scope": sense.scope,
        "definition": sense.definition,
        "confidence": sense.confidence,
        "status": status,
        "see_also": [],
        "synonyms_to_avoid": [],
        "introduced_in_mission": None,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--term", required=True)
    parser.add_argument("--scope", action="append", default=[])
    parser.add_argument("--confidence", type=float, default=1.0)
    parser.add_argument("--critical-step", action="store_true")
    args = parser.parse_args()

    from glossary.conflict import classify_conflict, create_conflict, score_severity
    from glossary.extraction import ExtractedTerm
    from glossary.models import SenseStatus
    from glossary.resolution import resolve_term
    from glossary.scope import GlossaryScope, load_seed_file
    from glossary.store import GlossaryStore

    repo = Path(args.repo)
    scopes = [GlossaryScope(s) for s in (args.scope or SCOPE_ORDER)]

    store = GlossaryStore(repo / ".kittify" / "events" / "glossary" / "_bench.events.jsonl")
    seeds = []
    for scope in scopes:
        path = repo / ".kittify" / "glossaries" / f"{scope.value}.yaml"
        senses = load_seed_file(scope, repo) if path.exists() else []
        for sense in senses:
            store.add_sense(sense)
        seeds.append(
            {
                "scope": scope.value,
                "path": str(path),
                "source": "default",
                "present": path.exists(),
                "terms": len(senses),
            }
        )

    normalized = args.term.lower().strip()
    matches = resolve_term(normalized, scopes, store)
    active = [s for s in matches if s.status == SenseStatus.ACTIVE]
    if not matches:
        status = "unknown"
    elif len(active) > 1:
        status = "ambiguous"
    elif len(active) == 1:
        status = "resolved"
    else:
        status = "no_active_sense"

    term = ExtractedTerm(surface=normalized, source="benchmark", confidence=args.confidence)
    conflict_type = classify_conflict(term, matches, is_critical_step=args.critical_step)
    conflict = None
    if conflict_type is not None:
        severity = score_severity(conflict_type, args.confidence, args.critical_step)
        upstream = create_conflict(term, conflict_type, severity, matches)
        conflict = {
            "term": upstream.term.surface_text,
            "conflict_type": getattr(upstream.conflict_type, "value", upstream.conflict_type),
            "severity": getattr(upstream.severity, "value", upstream.severity),
            "confidence": upstream.confidence,
            "candidate_senses": [_sense_dict(s) for s in matches],
            "context": upstream.context,
        }

    payload = {
        "requested_term": args.term,
        "term": normalized,
        "scopes": [s.value for s in scopes],
        "seeds": seeds,
        "confidence": args.confidence,
        "critical_step": bool(args.critical_step),
        "result": {
            "surface": normalized,
            "status": status,
            "conflict": conflict,
            "matches": [_sense_dict(s) for s in matches],
        },
    }
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1)
