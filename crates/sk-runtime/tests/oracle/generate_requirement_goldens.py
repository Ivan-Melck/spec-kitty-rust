#!/usr/bin/env python3
"""Regenerate the requirement-mapping golden from the pinned oracle.

Development tool only. It is never invoked by the Rust build or tests.

    PYTHONPATH=src SPEC_KITTY_PACKS_ROOT=$PWD/packs \
      .venv-rust-oracle/bin/python \
      crates/sk-runtime/tests/oracle/generate_requirement_goldens.py \
      > crates/sk-runtime/tests/golden/requirements_oracle.json

Every value in the output is produced by the reference implementation
(`specify_cli.requirement_mapping` and
`runtime.next.runtime_bridge_cores`), so the Rust port is checked against
verified oracle evidence rather than a structural expectation.
"""

from __future__ import annotations

import json
import sys

from runtime.next import runtime_bridge_cores as cores
from specify_cli.requirement_mapping import (
    find_bare_prose_requirement_ids,
    grammar,
    parse_requirement_ids_from_spec_md,
)


def find_all(text: str, *, spec_scan: bool, mission_type: str = "software-dev"):
    ids = grammar.find_all(text, spec_scan=spec_scan, mission_type=mission_type)
    return [
        {
            "kind": item.kind,
            "digits": item.digits,
            "suffix": item.suffix,
            "mission": item.mission,
            "canonical": item.canonical,
            "rendered": str(item),
        }
        for item in ids
    ]


def classify(raw: str, declared: list[str], mission_type: str = "software-dev"):
    verdict = grammar.classify(raw, set(declared), mission_type=mission_type)
    if isinstance(verdict, grammar.Accepted):
        return {"accepted": verdict.requirement_id.canonical}
    return {"rejected": {"raw": verdict.raw, "reason": verdict.reason}}


def mapping(facts: dict):
    return cores._evaluate_requirement_mapping(
        cores.RequirementMappingFacts(
            spec_requirement_ids=frozenset(facts["spec"]),
            functional_requirement_ids=frozenset(facts["functional"]),
            wp_ids=tuple(facts["wp_ids"]),
            wp_requirement_refs={k: tuple(v) for k, v in facts["refs"].items()},
            feature_dir_name=facts["feature_dir_name"],
            grammar=grammar,
            mission_type=facts.get("mission_type", "software-dev"),
        )
    )


SPECS = {
    "table_and_bullets": """# Spec

| ID | Requirement |
| --- | --- |
| **FR-001** | first |
| ~~FR-002~~ | retired |
| SC-001 | success |

- FR-003: third
- **NFR-001**: non functional
1. C-001 constraint

**FR-004 — bold lead.** prose

Prose citation of FR-021 is not declared.
Compound FR-008-mandated is not declared.
""",
    "headings_and_comments": """# Spec

### FR-001: first
#### NFR-002
<!--
| FR-099 | commented out |
-->
## FR-001a suffixed
""",
    "multi_id_line": """# Spec

- **FR-001 / FR-002**: both hold.
| FR-003 | third |
""",
    "quoting": """# Spec

See FR-021's default-pack materialization; do not count it.
| FR-001 | real |
""",
    "only_prose": """# Spec

FR-001 must hold. FR-002 too.
""",
    "foreign_and_unknown": """# Spec

| FR-001 | real |
""",
}

BARE_PROSE = {
    "mixed_section": """# Spec

## Functional Requirements

| ID | Requirement |
| --- | --- |
| NFR-001 | declared |

FR-001 must hold. FR-002 too.
""",
    "declared_everywhere": """# Spec

## Functional Requirements

| FR-001 | declared |

FR-001 is cited again here.
""",
    "compound_and_suffix": """# Spec

## Functional Requirements

FR-008-mandated is not an id. FR-001a is suffixed. SC-001 is a success criterion.
""",
    "no_requirement_section": """# Spec

## Overview

FR-001 is not scanned without a requirement heading.
""",
}

TASKS_MD = """# Tasks

## WP01 - first

Requirement refs:
- FR-001
- `NFR-001`

## WP02 - second

Requirement refs: FR-002, bogus

## Work Package WP03 - third

### Requirements

- C-001
"""

FACTS = [
    {
        "name": "complete",
        "spec": ["FR-001", "FR-002"],
        "functional": ["FR-001", "FR-002"],
        "wp_ids": ["WP01", "WP02"],
        "refs": {"WP01": ["FR-001"], "WP02": ["FR-002"]},
        "feature_dir_name": "demo-01M6GC7E",
    },
    {
        "name": "missing_refs",
        "spec": ["FR-001", "FR-002"],
        "functional": ["FR-001", "FR-002"],
        "wp_ids": ["WP01", "WP02"],
        "refs": {"WP01": ["FR-001"]},
        "feature_dir_name": "demo-01M6GC7E",
    },
    {
        "name": "unmapped_functional",
        "spec": ["FR-001", "FR-002", "SC-001"],
        "functional": ["FR-001", "FR-002"],
        "wp_ids": ["WP01", "WP02"],
        "refs": {"WP01": ["FR-001", "SC-001"], "WP02": ["FR-001"]},
        "feature_dir_name": "demo-01M6GC7E",
    },
    {
        "name": "rejected_refs",
        "spec": ["FR-001"],
        "functional": ["FR-001"],
        "wp_ids": ["WP01", "WP02"],
        "refs": {
            "WP01": ["FR-001", "bogus", "FR-999"],
            "WP02": ["other#FR-001"],
        },
        "feature_dir_name": "demo-01M6GC7E",
    },
    {
        "name": "research_kinds",
        "spec": ["FR-001"],
        "functional": ["FR-001"],
        "wp_ids": ["WP01"],
        "refs": {"WP01": ["DR-001"]},
        "feature_dir_name": "research-01M6GC7E",
        "mission_type": "research",
    },
]

CASES = [
    ("FR-001", ["FR-001"]),
    ("fr-001", ["FR-001"]),
    ("FR-001a", ["FR-001a"]),
    ("FR-001A", ["FR-001a"]),
    ("FR-001-mandated", ["FR-001"]),
    ("FR-001.3", ["FR-001"]),
    ("other#FR-001", ["FR-001"]),
    ("other-01M6GC7E#FR-001", ["FR-001"]),
    ("Other#FR-001", ["FR-001"]),
    ("FR-999", ["FR-001"]),
    ("SC-001", ["SC-001"]),
    ("DR-001", ["DR-001"]),
    ("bogus", ["FR-001"]),
]

PROSE = [
    "FR-001 and FR-002",
    "FR-008-mandated is not an id",
    "FR-002.3 is not an id",
    "see FR-001. next",
    "Other-Mission#FR-001 leaked qualifier",
    "x.y#FR-001 truncated qualifier",
    "other#FR-001 foreign",
    "other-01M6GC7E#FR-001 foreign mid8",
    "FR-001aNFR-002 adjacent",
    "FR-001 FR-001",
]


def main() -> int:
    payload = {
        "provenance": {
            "source": "spec-kitty 9aed3a8012cb5eb22c0128b14d525039f95fea7a",
            "python": "3.11.15",
            "generator": "crates/sk-runtime/tests/oracle/generate_requirement_goldens.py",
        },
        "specs": {},
        "prose": {text: find_all(text, spec_scan=False) for text in PROSE},
        "prose_spec_scan": {text: find_all(text, spec_scan=True) for text in PROSE},
        "classify": [
            {"raw": raw, "declared": declared, "verdict": classify(raw, declared)}
            for raw, declared in CASES
        ],
        "mapping": [{"facts": facts, "failures": mapping(facts)} for facts in FACTS],
        "bare_prose": {
            name: {
                "content": text,
                "candidates": [
                    {"section_heading": candidate.section_heading, "ids": candidate.ids}
                    for candidate in find_bare_prose_requirement_ids(text)
                ],
            }
            for name, text in BARE_PROSE.items()
        },
        "bare_prose_failures": {
            name: cores._evaluate_bare_prose_requirements(
                cores.BareProseRequirementFacts(
                    flagged={
                        candidate.section_heading: tuple(candidate.ids)
                        for candidate in find_bare_prose_requirement_ids(text)
                    },
                    classification_error=None,
                )
            )
            for name, text in BARE_PROSE.items()
        },
        "tasks_md_refs": {
            "software-dev": cores._parse_requirement_refs_from_tasks_md(
                TASKS_MD, grammar=grammar, mission_type="software-dev"
            )
        },
    }
    for name, text in SPECS.items():
        payload["specs"][name] = {
            "content": text,
            "parsed": parse_requirement_ids_from_spec_md(text, mission_type="software-dev"),
        }
    # The declared set itself, per spec.
    from specify_cli.requirement_mapping import _declared_ids

    for name, text in SPECS.items():
        payload["specs"][name]["declared"] = sorted(_declared_ids(text))
    json.dump(payload, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
