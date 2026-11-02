"""Generate committed oracle fixtures from the pinned Python reference.

This is a development-time generator, never part of the Rust runtime. It reads
the Python reference under ``src/`` (the pinned upstream snapshot) and writes
deterministic fixtures under ``tests/fixtures/oracle/``. Every output is derived
from a temporary directory; the real repository ``kitty-specs/`` and the user's
home are never touched.

Run from the repository root:

    /home/exedev/.local/share/uv/tools/spec-kitty-cli/bin/python \
        crates/sk-mission/tests/oracle/generate_oracle_fixtures.py

The provenance record names the exact interpreter, upstream commit, and source
modules used.
"""

from __future__ import annotations

import json
import platform
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(REPO_ROOT / "src"))

from mission_runtime import MissionTopology  # noqa: E402
from specify_cli.core.dependency_parser import (  # noqa: E402
    parse_dependencies_from_tasks_md,
)
from specify_cli.core.mission_creation_identity import _Purpose  # noqa: E402
from specify_cli.core.mission_payload import (  # noqa: E402
    default_mission_display_name,
    default_mission_purpose_context,
)
from specify_cli.core.mission_creation_meta import _build_create_meta  # noqa: E402
from specify_cli.core.mission_creation_scaffold import (  # noqa: E402
    render_tasks_readme_content,
)
from specify_cli.mission_metadata import write_meta  # noqa: E402

FIXTURES = Path(__file__).resolve().parents[1] / "fixtures" / "oracle"
MISSION_ID = "01KZ6G6H4R8Y9J2N4Q7V3B6M1F"
MID8 = MISSION_ID[:8]
CREATED_AT = "2026-11-02T10:00:00Z"
SLUG = f"demo-mission-{MID8}"


def upstream_commit() -> str:
    return subprocess.run(
        ["git", "-C", str(REPO_ROOT), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def write_text(name: str, text: str) -> None:
    path = FIXTURES / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="")
    print(f"wrote {path.relative_to(FIXTURES)} ({len(text.encode())} bytes)")


def write_json(name: str, value: object) -> None:
    write_text(name, json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def build_meta(
    *,
    topology: MissionTopology,
    mission_type: str = "software-dev",
    friendly_name: str = "Demo Mission",
    tldr: str = "Demo Mission",
    context: str = "A demonstration mission.",
    pr_bound: bool = False,
    retain_branches: bool = False,
    retain_worktrees: bool = False,
    commit_to_target: bool = False,
    existing: dict[str, object] | None = None,
) -> str:
    with tempfile.TemporaryDirectory() as temp:
        feature_dir = Path(temp) / "kitty-specs" / SLUG
        feature_dir.mkdir(parents=True)
        if existing is not None:
            (feature_dir / "meta.json").write_text(
                json.dumps(existing, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
        built = _build_create_meta(
            feature_dir=feature_dir,
            mission_id=MISSION_ID,
            mid8=MID8,
            mission_slug_formatted=SLUG,
            normalized_friendly_name=friendly_name,
            purpose=_Purpose(tldr=tldr, context=context),
            mission=mission_type,
            planning_branch="main",
            pr_bound=pr_bound,
            retain_branches=retain_branches,
            retain_worktrees=retain_worktrees,
            commit_to_target=commit_to_target,
            resolved_root=Path(temp),
            topology=topology,
            force_recreate_coordination_branch=False,
            created_at=CREATED_AT,
        )
        write_meta(feature_dir, built.meta)
        return (feature_dir / "meta.json").read_text(encoding="utf-8")


DEPENDENCY_CASES = [
    {
        "name": "three_supported_formats",
        "tasks_md": (
            "## WP01 - Foundation\n\nSubtasks: T001, T002\n\n"
            "## WP02 - API\n\nDepends on WP01\n\n"
            "## WP03 - UI\n\n**Dependencies**: WP01, WP02\n"
        ),
    },
    {
        "name": "bullet_list_and_duplicate_order",
        "tasks_md": (
            "## Work Package 1 — Foundation\n\n"
            "Deliver T001.\n\n"
            "### Dependencies\n"
            "- WP02 (contract)\n"
            "- WP01\n"
            "- WP02\n"
        ),
    },
    {
        "name": "final_section_stops_at_non_wp_heading",
        "tasks_md": (
            "## WP01 - Foundation\nDepends on WP02\n\n"
            "## WP02 - API\nDepends on WP01\n\n"
            "## Notes\n\nDepends on WP03 (prose, not part of WP02)\n"
        ),
    },
    {
        "name": "none_and_empty_declarations",
        "tasks_md": (
            "## WP01 - Foundation\n**Dependencies**: None\n\n"
            "## WP02 - API\n**Dependencies**: []\n\n"
            "## WP03 - UI\n**Dependencies**: No dependencies\n"
        ),
    },
    {
        "name": "pipe_table_dependency_cell",
        "tasks_md": (
            "## WP01 - Foundation\n\n| Field | Value |\n|---|---|\n| **Dependencies**: WP02 | note |\n\n"
            "## WP02 - API\n\n| **Dependencies**: WP01 |\n"
        ),
    },
    {
        "name": "pipe_table_dependency_later_cell",
        "tasks_md": "## WP01 - Foundation\n\n| Field | Value |\n|---|---|\n| Task | **Dependencies**: WP02 |\n",
    },
    {
        "name": "table_row_and_heading_owner",
        "tasks_md": (
            "## WP01 - Foundation\n\n| Task | Owner |\n|---|---|\n| T001 | WP01 |\n\n"
            "## WP02 - API\n\n- [ ] T002 deliver API\n"
        ),
    },
]


def main() -> None:
    FIXTURES.mkdir(parents=True, exist_ok=True)

    write_text("create/meta_single_branch.json", build_meta(topology=MissionTopology.SINGLE_BRANCH))
    default_display = default_mission_display_name("demo-mission")
    write_text(
        "create/meta_defaults.json",
        build_meta(
            topology=MissionTopology.SINGLE_BRANCH,
            friendly_name=default_display,
            tldr=default_display,
            context=default_mission_purpose_context(default_display, "main"),
        ),
    )
    write_text(
        "create/meta_lanes_with_flags.json",
        build_meta(
            topology=MissionTopology.LANES,
            mission_type="research",
            tldr="Research Mission",
            context="Research context.",
            pr_bound=True,
            retain_branches=True,
            retain_worktrees=True,
        ),
    )
    write_text(
        "create/meta_repair_preserves_existing.json",
        build_meta(
            topology=MissionTopology.SINGLE_BRANCH,
            existing={
                "mission_id": MISSION_ID,
                "mid8": MID8,
                "mission_number": 7,
                "slug": "legacy-demo",
                "mission_slug": "legacy-demo",
                "friendly_name": "Operator Title",
                "mission_type": "software-dev",
                "target_branch": "release/2",
                "created_at": "2025-01-01T00:00:00Z",
                "custom_extension": {"keep": ["verbatim", 42]},
                "review_playbook": {"notes": "preserve me"},
            },
        ),
    )
    write_text("scaffold/tasks-readme-main.md", render_tasks_readme_content("main"))
    write_text("scaffold/tasks-readme-release.md", render_tasks_readme_content("release/2"))

    deps = []
    for case in DEPENDENCY_CASES:
        deps.append(
            {
                "name": case["name"],
                "tasks_md": case["tasks_md"],
                "expected": parse_dependencies_from_tasks_md(case["tasks_md"]),
            }
        )
    write_json("tasks/dependency_cases.json", deps)

    provenance = {
        "generated_by": "crates/sk-mission/tests/oracle/generate_oracle_fixtures.py",
        "python_executable": sys.executable,
        "python_version": platform.python_version(),
        "documented_baseline_commit": "9aed3a8012cb5eb22c0128b14d525039f95fea7a",
        "oracle_source_commit": upstream_commit(),
        "source_paths": [
            "src/specify_cli/core/mission_creation_meta.py",
            "src/specify_cli/core/mission_creation_identity.py",
            "src/specify_cli/core/mission_creation_scaffold.py",
            "src/specify_cli/core/dependency_parser.py",
            "src/specify_cli/mission_metadata.py",
        ],
        "creation_inputs": {
            "mission_id": MISSION_ID,
            "mid8": MID8,
            "directory_name": SLUG,
            "friendly_name": "Demo Mission",
            "created_at": CREATED_AT,
            "target_branch": "main",
        },
        "notes": (
            "The documented baseline is 9aed3a8012cb5eb22c0128b14d525039f95fea7a; "
            "the oracle source files are byte-identical at the recorded oracle_source_commit. "
            "meta.json fixtures were produced by _build_create_meta + write_meta "
            "(the canonical serialization: sorted keys, two-space indent, trailing "
            "newline). No coordination-branch mint is recorded in these fixtures; "
            "the coordination_branch field is supplied by the workspace owner."
        ),
    }
    write_json("provenance.json", provenance)


if __name__ == "__main__":
    main()
