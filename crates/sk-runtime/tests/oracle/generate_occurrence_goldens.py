#!/usr/bin/env python3
"""Regenerate the occurrence-gate golden from the pinned oracle.

Development tool only; never invoked by the Rust build or its tests.

    PYTHONPATH=src SPEC_KITTY_PACKS_ROOT=$PWD/packs \
      .venv-rust-oracle/bin/python \
      crates/sk-runtime/tests/oracle/generate_occurrence_goldens.py \
      > crates/sk-runtime/tests/golden/occurrence_oracle.json
"""

from __future__ import annotations

import json
import sys

from specify_cli.bulk_edit.occurrence_map import (
    OccurrenceMap,
    check_admissibility,
    validate_occurrence_map,
)


def as_map(raw: dict) -> OccurrenceMap:
    target = raw.get("target", {}) if isinstance(raw.get("target"), dict) else {}
    categories = raw.get("categories", {}) if isinstance(raw.get("categories"), dict) else {}
    return OccurrenceMap(
        target_term=target.get("term", ""),
        target_replacement=target.get("replacement"),
        target_operation=target.get("operation", ""),
        categories=categories,
        exceptions=raw.get("exceptions", []) or [],
        status=raw.get("status"),
        raw=raw,
    )


def validate(raw: dict):
    result = validate_occurrence_map(as_map(raw))
    return {"errors": result.errors, "warnings": result.warnings}


def admissible(raw: dict):
    result = check_admissibility(as_map(raw))
    return {"errors": result.errors, "warnings": result.warnings}


BASE = {
    "target": {"term": "Widget", "operation": "rename", "replacement": "Gadget"},
    "categories": {
        name: {"action": "rename"}
        for name in [
            "code_symbols",
            "import_paths",
            "filesystem_paths",
            "serialized_keys",
            "cli_commands",
            "user_facing_strings",
            "tests_fixtures",
            "logs_telemetry",
        ]
    },
}

CASES = {
    "valid": BASE,
    "missing_target": {"categories": BASE["categories"]},
    "target_not_mapping": {"target": "Widget", "categories": BASE["categories"]},
    "missing_term": {"target": {"operation": "rename"}, "categories": BASE["categories"]},
    "empty_term": {"target": {"term": "  "}, "categories": BASE["categories"]},
    "bad_operation": {
        "target": {"term": "Widget", "operation": "destroy"},
        "categories": BASE["categories"],
    },
    "missing_categories": {"target": {"term": "Widget"}},
    "empty_categories": {"target": {"term": "Widget"}, "categories": {}},
    "category_not_mapping": {
        "target": {"term": "Widget"},
        "categories": {"code_symbols": "rename"},
    },
    "category_missing_action": {
        "target": {"term": "Widget"},
        "categories": {"code_symbols": {}},
    },
    "category_bad_action": {
        "target": {"term": "Widget"},
        "categories": {"code_symbols": {"action": "delete"}},
    },
    "unknown_key": {**BASE, "surprise": True},
    "bad_exceptions": {**BASE, "exceptions": "nope"},
    "exception_entry": {
        **BASE,
        "exceptions": [{"path": "", "action": "delete", "field_path": "   "}],
    },
    "bad_moves": {**BASE, "moves": {}},
    "move_entry": {**BASE, "moves": [{"from": [], "to": ""}]},
    "bad_structural": {**BASE, "structural_targets": {}},
    "structural_entry": {
        **BASE,
        "structural_targets": [
            {"path": "src/**/*.py", "reason": ""},
            {"path": "src", "reason": "dir"},
            {"path": "src/foo/bar.py", "reason": "specific"},
        ],
    },
    "placeholder_term": {
        "target": {"term": "TODO"},
        "categories": {
            name: {"action": "do_not_change"}
            for name in [
                "code_symbols",
                "import_paths",
                "filesystem_paths",
            ]
        },
    },
    "too_few_categories": {
        "target": {"term": "Widget"},
        "categories": {"code_symbols": {"action": "rename"}},
    },
    "missing_standard_categories": {
        "target": {"term": "Widget"},
        "categories": {
            "code_symbols": {"action": "rename"},
            "import_paths": {"action": "rename"},
            "filesystem_paths": {"action": "rename"},
        },
    },
}


def main() -> int:
    payload = {
        "provenance": {
            "source": "spec-kitty 9aed3a8012cb5eb22c0128b14d525039f95fea7a",
            "python": "3.11.15",
            "generator": "crates/sk-runtime/tests/oracle/generate_occurrence_goldens.py",
        },
        "cases": {
            name: {
                "map": raw,
                "validation": validate(raw),
                "admissibility": admissible(raw),
            }
            for name, raw in CASES.items()
        },
    }
    json.dump(payload, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
