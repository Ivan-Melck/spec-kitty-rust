#!/usr/bin/env python3
"""Python-reference oracle for work-package dependency edges (projection).

Uses ``specify_cli.core.dependency_graph.build_dependency_graph`` (the upstream
``tasks/WP*.md`` frontmatter reader) and projects it to the fields the Rust
``mission graph`` document exposes identically: each WP's declared dependencies
and its direct dependents.

Topological order is deliberately not compared: the two implementations use
different deterministic tie-breaks and upstream has no single published order.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--slug", required=True)
    args = parser.parse_args()

    from specify_cli.core.dependency_graph import build_dependency_graph, get_dependents

    feature_dir = Path(args.repo) / "kitty-specs" / args.slug
    graph = build_dependency_graph(feature_dir)
    packages = {
        wp_id: {
            "dependencies": sorted(dependencies),
            "dependents": sorted(get_dependents(wp_id, graph)),
        }
        for wp_id, dependencies in sorted(graph.items())
    }
    print(
        json.dumps(
            {"slug": args.slug, "count": len(packages), "packages": packages},
            sort_keys=True,
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - oracle adapter boundary
        print(json.dumps({"oracle_error": type(exc).__name__, "message": str(exc)}), file=sys.stderr)
        raise SystemExit(1)
