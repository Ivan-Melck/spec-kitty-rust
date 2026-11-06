#!/usr/bin/env python3
"""Regenerate src/migration_catalog.json from the pinned Python reference tree.

Development only: the Rust crate never runs Python. This script is the honest
provenance of the embedded catalog (declared ids, order, partition, git
requirement, supersession note, module name and content hash) taken from
`spec-kitty/spec-kitty` at the snapshot recorded in docs/rust/LOOP.md.

Usage, from the repository root, with an isolated environment:

    UV_PROJECT_ENVIRONMENT=/tmp/sk-oracle-venv uv run --offline \
        python crates/sk-install/tools/generate_migration_catalog.py \
        > crates/sk-install/src/migration_catalog.json

The output is a JSON array in canonical execution order. Review the diff before
committing: a changed catalog means the pinned reference changed.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "src"))

from specify_cli.upgrade.ledger import compute_content_hash  # noqa: E402
from specify_cli.upgrade.migrations import auto_discover_migrations  # noqa: E402
from specify_cli.upgrade.registry import MigrationRegistry  # noqa: E402

auto_discover_migrations()
rows = []
for migration in MigrationRegistry.get_all():
    rows.append(
        {
            "id": migration.migration_id,
            "identity": getattr(migration, "identity", None) or migration.migration_id,
            "module": type(migration).__module__.split(".")[-1],
            "target_version": str(migration.target_version),
            "clone_local": bool(getattr(migration, "clone_local_effect", False)),
            "requires_git_repository": bool(
                getattr(migration, "requires_git_repository", False)
            ),
            "superseded_by": getattr(migration, "superseded_by", "") or "",
            "content_hash": compute_content_hash(migration),
        }
    )
print(json.dumps(rows, indent=1, sort_keys=True))
