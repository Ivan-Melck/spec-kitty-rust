#!/usr/bin/env python3
"""Generate the native status-writer CONTRACT vectors from the pinned oracle.

Run once from the repository root with the pinned oracle interpreter:

    PYTHONPATH=src .venv-rust-oracle/bin/python \
        crates/sk-status/tests/fixtures/mutation/generate_contract_vectors.py

Unlike ``generate_fixtures.py`` (byte-level differential fixtures), this file
records **semantic contract vectors** for the surfaces the writer validates:

* each canonical lifecycle payload, produced by the pinned producer's own
  convenience emitter (never by hand), so the native validator is compared with
  the bytes the baseline actually persists;
* payloads the pinned pydantic models refuse (with the field that refused), so
  the native "no arbitrary JSON" seam is compared against real refusals;
* ``WPInnerStateDelta`` coercion/normalisation vectors
  (``from_dict`` -> ``to_dict`` + ``is_empty``);
* the ``planned -> claimed`` claim ``policy_metadata`` shape;
* ``derive_mission_lifecycle`` over a seeded mission.

The source baseline is ``9aed3a8012cb5eb22c0128b14d525039f95fea7a`` and
``spec-kitty-events`` is pinned by ``uv.lock`` (11.3.0).
"""

from __future__ import annotations

import importlib.metadata
import json
import shutil
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[5]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

FIXTURES = Path(__file__).resolve().parent
BASELINE = "9aed3a8012cb5eb22c0128b14d525039f95fea7a"
MISSION_SLUG = "034-mutation-fixture"
MISSION_ID = "01K9M0R3W00000000000000000"
PROJECT_UUID = "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d"
AT = "2026-11-02T09:00:00Z"

from specify_cli.status import lifecycle_events, store  # noqa: E402
from specify_cli.status import lifecycle as lifecycle_module  # noqa: E402
from specify_cli.status import emit as status_emit  # noqa: E402
from specify_cli.status import models, reducer  # noqa: E402

ULIDS = [
    "01K9M0R3WJ8V6Q1Y4T7B2N5C0A",
    "01K9M0R3WK9W7R2Z5V8C3P6D1B",
    "01K9M0R3WMADXS8SA6W9D4Q7E2",
    "01K9M0R3WNBEYT9TB7XAE5R8F3",
    "01K9M0R3WPCFZVAWC8YBF6S9GX",
    "01K9M0R3WQDGZVBWD9ZCG7T0H5",
    "01K9M0R3WREFZVCXE0ADH8U1J6",
    "01K9M0R3WSFGZVDEY1BEJ9V2K7",
    "01K9M0R3WTGHZVEFZ2CFKA3L8",
    "01K9M0R3WUHIZVFG03DGHB4M9X",
]


def dump_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def mission(root: Path) -> Path:
    feature = root / "kitty-specs" / MISSION_SLUG
    write(
        feature / "meta.json",
        json.dumps(
            {
                "mission_slug": MISSION_SLUG,
                "mission_id": MISSION_ID,
                "mission_number": None,
                "mission_type": "software-dev",
                "target_branch": "main",
                "created_at": AT,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
    )
    write(
        feature / "tasks" / "WP01.md",
        f"""---
work_package_id: WP01
title: Contract vectors work package
dependencies: []
subtasks:
- T001
- T002
---

# WP01
""",
    )
    return feature


def pin_ids() -> None:
    """Pin the minted ULIDs so the fixture is byte-stable across runs."""
    ids = iter(ULIDS * 10)
    status_emit._generate_ulid = lambda: next(ids)  # noqa: SLF001
    lifecycle_events._generate_event_id = lambda: next(ids)  # noqa: SLF001


def last_lifecycle_payload(log_path: Path) -> dict:
    events = lifecycle_events.read_lifecycle_events(log_path)
    return events[-1]["payload"]


def lifecycle_vectors(work: Path) -> tuple[dict, list[dict]]:
    pin_ids()
    root = work / "lifecycle"
    feature = mission(root)
    vectors: dict[str, dict] = {}

    project_log = lifecycle_events.project_event_log_path(root)
    lifecycle_events.emit_project_initialized(
        root,
        project_uuid=PROJECT_UUID,
        project_slug="contract-vectors",
        actor="cli",
        runtime_version="4.1.0",
        initialized_at=AT,
    )
    vectors["ProjectInitialized"] = last_lifecycle_payload(project_log)

    lifecycle_events.emit_mission_created_local(
        feature,
        mission_slug=MISSION_SLUG,
        mission_id=MISSION_ID,
        mission_number=None,
        mission_type="software-dev",
        target_branch="main",
        wp_count=1,
        created_at=AT,
        actor="cli",
        fanout=False,
        repo_root=root,
    )
    mission_log = lifecycle_events.mission_event_log_path(feature)
    vectors["MissionCreated"] = last_lifecycle_payload(mission_log)

    for event_type, kwargs in [
        (lifecycle_events.SPECIFY_STARTED, {}),
        (
            lifecycle_events.SPECIFY_COMPLETED,
            {"artifact_path": "kitty-specs/034-mutation-fixture/spec.md", "summary": "Spec ready"},
        ),
        (lifecycle_events.PLAN_STARTED, {}),
        (
            lifecycle_events.PLAN_COMPLETED,
            {"artifact_path": "kitty-specs/034-mutation-fixture/plan.md", "summary": "Plan ready"},
        ),
        (lifecycle_events.TASKS_STARTED, {}),
        (
            lifecycle_events.TASKS_COMPLETED,
            {
                "artifact_path": "kitty-specs/034-mutation-fixture/tasks.md",
                "summary": "Tasks ready",
                "wp_count": 1,
            },
        ),
    ]:
        lifecycle_events.emit_artifact_phase_local(
            feature,
            event_type=event_type,
            mission_slug=MISSION_SLUG,
            mission_number=None,
            actor="cli",
            at=AT,
            repo_root=root,
            **kwargs,
        )
        vectors[event_type] = last_lifecycle_payload(mission_log)

    lifecycle_events.emit_wp_created_local(
        feature,
        mission_slug=MISSION_SLUG,
        wp_id="WP01",
        wp_title="Contract vectors work package",
        wp_path="kitty-specs/034-mutation-fixture/tasks/WP01.md",
        depends_on=["WP00"],
        actor="cli",
        created_at=AT,
        repo_root=root,
    )
    vectors["WPCreated"] = last_lifecycle_payload(mission_log)

    lifecycle_events.emit_reviewer_self_approval(
        feature,
        mission_slug=MISSION_SLUG,
        wp_id="WP01",
        implementing_actor="claude",
        intended_reviewer="alice",
        failure_reason="reviewer unavailable",
        fallback_approved=True,
        at=AT,
    )
    vectors["ReviewerSelfApproval"] = last_lifecycle_payload(mission_log)

    # A completed mission is the precondition for the two post-mission facts.
    rows = [
        {
            "event_id": ULIDS[0],
            "mission_slug": MISSION_SLUG,
            "mission_id": MISSION_ID,
            "wp_id": "WP01",
            "from_lane": "genesis",
            "to_lane": "planned",
            "at": AT,
            "actor": "cli",
            "force": False,
            "execution_mode": "worktree",
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
        {
            "event_id": ULIDS[1],
            "mission_slug": MISSION_SLUG,
            "mission_id": MISSION_ID,
            "wp_id": "WP01",
            "from_lane": "planned",
            "to_lane": "done",
            "at": AT,
            "actor": "cli",
            "force": True,
            "execution_mode": "worktree",
            "reason": "fixture completion",
            "review_ref": None,
            "evidence": {
                "review": {"reviewer": "alice", "verdict": "approved", "reference": "PR#1"}
            },
            "policy_metadata": None,
        },
    ]
    write(
        feature / "status.events.jsonl",
        "".join(store.serialize_event_line(row) + "\n" for row in rows),
    )
    reducer.materialize(feature)

    # A follow-up is a post-mission fact recorded while the mission is still
    # completed; a re-open makes the mission actionable again and would block it.
    lifecycle_events.emit_follow_up_recorded(
        feature,
        mission_id=MISSION_ID,
        mission_slug=MISSION_SLUG,
        follow_up_type="commit",
        commit_sha="0123456789abcdef0123456789abcdef01234567",
        recorded_by="alice",
        recorded_at=AT,
    )
    vectors["FollowUpRecorded"] = last_lifecycle_payload(mission_log)

    lifecycle_events.emit_mission_reopened(
        feature,
        mission_id=MISSION_ID,
        mission_slug=MISSION_SLUG,
        reason="operator reopened",
        reopened_by="alice",
        reopened_at=AT,
    )
    vectors["MissionReopened"] = last_lifecycle_payload(mission_log)

    # Refusal vectors, taken from the pinned pydantic models themselves.
    from spec_kitty_events import lifecycle as events_lifecycle
    from spec_kitty_events import project_lifecycle

    refusals: list[dict] = []

    def refusal(model, event_type: str, payload: dict) -> None:
        try:
            model(**payload)
        except Exception as exc:  # noqa: BLE001 - the refusal itself is the vector
            refusals.append(
                {
                    "event_type": event_type,
                    "payload": payload,
                    "oracle_error": type(exc).__name__,
                }
            )
        else:
            raise SystemExit(f"oracle accepted a payload expected to be refused: {event_type}")

    refusal(
        project_lifecycle.ProjectInitializedPayload,
        "ProjectInitialized",
        {"project_uuid": "u", "actor": "a", "hostname": "builder"},
    )
    refusal(
        project_lifecycle.ProjectInitializedPayload,
        "ProjectInitialized",
        {"project_uuid": "u", "actor": ""},
    )
    refusal(
        project_lifecycle.WPCreatedPayload,
        "WPCreated",
        {
            "mission_slug": "m",
            "wp_id": "WP01",
            "wp_title": "t",
            "actor": "a",
            "depends_on": [1],
        },
    )
    refusal(
        project_lifecycle.TasksCompletedPayload,
        "TasksCompleted",
        {
            "mission_slug": "m",
            "actor": "a",
            "artifact_path": "tasks.md",
            "wp_count": -1,
        },
    )
    refusal(
        events_lifecycle.MissionCreatedPayload,
        "MissionCreated",
        {
            "mission_slug": "m",
            "mission_number": None,
            "mission_type": "software-dev",
            "target_branch": "main",
            "wp_count": 1,
            "friendly_name": "M",
            "purpose_tldr": "M",
            "purpose_context": "C",
            "unexpected": True,
        },
    )

    derivation = lifecycle_module.derive_mission_lifecycle(
        feature,
        now=datetime(2026, 11, 2, 12, 0, 0, tzinfo=timezone.utc),
    ).to_dict()
    envelopes = [
        event
        for event in lifecycle_events.read_lifecycle_events(mission_log)
        if event.get("event_type") in {"MissionReopened", "FollowUpRecorded"}
    ]

    return (
        {
            "vectors": vectors,
            "refusals": refusals,
            "derivation": derivation,
            "derivation_now": "2026-11-02T12:00:00+00:00",
            "derivation_inputs": {
                "status_rows": rows,
                "lifecycle_envelopes": envelopes,
            },
        },
        [],
    )


def inner_state_vectors() -> list[dict]:
    cases = [
        {"shell_pid": "4242", "agent": "claude", "shell_pid_created_at": "2026-11-02T09:00:00Z"},
        {"shell_pid": 4242.9, "agent": "", "assignee": "  "},
        {"subtasks": {"T001": "not_applicable", "T002": "done", "T003": "planned"}},
        {"tracker_refs": [], "tracker_refs_replace": ["#1"], "note": ""},
        {
            "review": {
                "at": "2026-11-02T09:00:00Z",
                "actor": "alice",
                "wp_id": "WP01",
                "reason": "arbiter",
            }
        },
        {"review": {"at": "", "actor": "", "wp_id": "", "reason": ""}},
        {"release_runtime_claim": True},
        {"release_runtime_claim": 0},
        {"shell_pid": True, "role": "reviewer", "agent_profile_version": "2"},
    ]
    vectors = []
    for case in cases:
        delta = models.WPInnerStateDelta.from_dict(case)
        vectors.append(
            {
                "input": case,
                "canonical": delta.to_dict(),
                "is_empty": delta.is_empty(),
            }
        )
    return vectors


def claim_metadata_vector() -> dict:
    return status_emit.build_claim_policy_metadata(
        shell_pid=4242,
        shell_pid_created_at="",
        agent="claude",
    )


def main() -> None:
    work = Path(tempfile.mkdtemp(prefix="sk-contract-vectors-"))
    try:
        lifecycle, _ = lifecycle_vectors(work)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    fixture = {
        "baseline": BASELINE,
        "oracle": {
            "python": sys.version.split()[0],
            "spec_kitty_events": importlib.metadata.version("spec-kitty-events"),
            "generation_command": "PYTHONPATH=src .venv-rust-oracle/bin/python crates/sk-status/tests/fixtures/mutation/generate_contract_vectors.py",
        },
        "lifecycle": lifecycle["vectors"],
        "lifecycle_refusals": lifecycle["refusals"],
        "inner_state": inner_state_vectors(),
        "claim_policy_metadata": claim_metadata_vector(),
        "lifecycle_derivation": lifecycle["derivation"],
        "lifecycle_derivation_now": lifecycle["derivation_now"],
        "lifecycle_derivation_inputs": lifecycle["derivation_inputs"],
    }
    dump_json(FIXTURES / "contract_vectors.json", fixture)
    print(f"wrote {FIXTURES / 'contract_vectors.json'}")


if __name__ == "__main__":
    main()
