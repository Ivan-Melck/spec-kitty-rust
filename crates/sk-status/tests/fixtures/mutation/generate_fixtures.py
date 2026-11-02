#!/usr/bin/env python3
"""Generate the native status-writer differential fixtures.

Run once from the repository root with the pinned oracle interpreter:

    PYTHONPATH=src .venv-rust-oracle/bin/python \
        crates/sk-status/tests/fixtures/mutation/generate_fixtures.py

``spec-kitty-events`` is pinned to 11.3.0 by ``uv.lock``; the source baseline is
``9aed3a8012cb5eb22c0128b14d525039f95fea7a``. Every expected artifact is produced
by the **pinned Python implementation**, never by the Rust code under test.
Identity and wall-clock values are monkeypatched so the fixtures are
deterministic; nothing else about the baseline is changed.

The generated ``provenance.json`` records the interpreter, package versions and
the exact generation command.
"""

from __future__ import annotations

import importlib.metadata
import json
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[5]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

FIXTURES = Path(__file__).resolve().parent
BASELINE = "9aed3a8012cb5eb22c0128b14d525039f95fea7a"

from specify_cli.status import emit as status_emit  # noqa: E402
from specify_cli.status import lifecycle_events, reducer, store  # noqa: E402
from specify_cli.status import migrate_lifecycle_envelope as migration  # noqa: E402

ULIDS = [
    "01K9M0R3WJ8V6Q1Y4T7B2N5C0A",
    "01K9M0R3WK9W7R2Z5V8C3P6D1B",
    "01K9M0R3WMADXS8SA6W9D4Q7E2",
    "01K9M0R3WNBEYT9TB7XAE5R8F3",
    "01K9M0R3WPCFZVAWC8YBF6S9GX",
    "01K9M0R3WQDGZVBWD9ZCG7T0H5",
]

TIMES = [
    "2026-11-02T09:00:00+00:00",
    "2026-11-02T09:00:01+00:00",
    "2026-11-02T09:00:02+00:00",
    "2026-11-02T09:00:03+00:00",
    "2026-11-02T09:00:04+00:00",
    "2026-11-02T09:00:05+00:00",
]

MISSION_SLUG = "034-mutation-fixture"
MISSION_ID = "01K9M0R3W00000000000000000"
PROJECT_UUID = "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d"

WP_FRONTMATTER = """---
work_package_id: WP01
title: Mutation fixture work package
dependencies: []
subtasks: []
---

# WP01

Fixture body.
"""


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def dump_rows(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(store.serialize_event_line(row) + "\n" for row in rows), encoding="utf-8")


def dump_json(path: Path, value: object) -> None:
    write(path, json.dumps(value, indent=2, sort_keys=True) + "\n")


def fixture_meta() -> dict:
    return {
        "mission_slug": MISSION_SLUG,
        "mission_id": MISSION_ID,
        "mission_number": None,
        "mission_type": "software-dev",
        "target_branch": "main",
    }


def status_append_fixture() -> None:
    target = FIXTURES / "status_append"
    if target.exists():
        shutil.rmtree(target)
    input_root = target / "input"
    input_dir = input_root / "kitty-specs" / MISSION_SLUG
    write(input_dir / "meta.json", json.dumps(fixture_meta(), indent=2, sort_keys=True) + "\n")
    write(input_dir / "tasks" / "WP01.md", WP_FRONTMATTER)

    existing = [
        {
            "event_id": ULIDS[0],
            "mission_slug": MISSION_SLUG,
            "mission_id": MISSION_ID,
            "wp_id": "WP01",
            "from_lane": "genesis",
            "to_lane": "planned",
            "at": TIMES[0],
            "actor": "system",
            "force": False,
            "execution_mode": "worktree",
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
        {
            "event_id": ULIDS[1],
            "kind": "annotation",
            "wp_id": "WP01",
            "at": TIMES[1],
            "actor": "agent-1",
            "delta": {"subtasks": {"T001": "done"}},
        },
        {
            "event_id": ULIDS[2],
            "event_type": "WPCreated",
            "aggregate_id": "WP01",
            "aggregate_type": "WorkPackage",
            "schema_version": "5.0.0",
            "timestamp": TIMES[1],
            "payload": {"mission_slug": MISSION_SLUG, "wp_id": "WP01", "wp_title": "Fixture"},
            "project_uuid": PROJECT_UUID,
            "project_slug": "mutation-fixture",
        },
        {
            "event_id": ULIDS[3],
            "mission_slug": MISSION_SLUG,
            "mission_id": MISSION_ID,
            "wp_id": "WP01",
            "from_lane": "planned",
            "to_lane": "claimed",
            "at": TIMES[2],
            "actor": {"role": "implementer", "profile": "python-pedro", "tool": "claude", "model": "opus"},
            "force": False,
            "execution_mode": "worktree",
            "reason": "caf\u00e9 \u2615 \u00fcmlaut",
            "review_ref": None,
            "evidence": None,
            "policy_metadata": {"shell_pid": 4242, "agent": "claude"},
            "hostname": "should-be-stripped",
            "developer_email": "nobody@example.invalid",
            "session_started_at": TIMES[0],
            "session_ended_at": TIMES[2],
        },
    ]
    log_path = input_dir / store.EVENTS_FILENAME
    dump_rows(log_path, existing)
    # Strip the final newline so both writers must repair it without losing bytes.
    log_path.write_text(log_path.read_text(encoding="utf-8").rstrip("\n"), encoding="utf-8")

    appended = [
        {
            "event_id": ULIDS[4],
            "mission_slug": MISSION_SLUG,
            "mission_id": MISSION_ID,
            "wp_id": "WP01",
            "from_lane": "claimed",
            "to_lane": "in_progress",
            "at": TIMES[3],
            "actor": "claude",
            "force": False,
            "execution_mode": "worktree",
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
        {
            "event_id": ULIDS[5],
            "mission_slug": MISSION_SLUG,
            "mission_id": MISSION_ID,
            "wp_id": "WP02",
            "from_lane": "planned",
            "to_lane": "canceled",
            "at": TIMES[4],
            "actor": "operator",
            "force": True,
            "execution_mode": "worktree",
            "reason": "operator canceled",
            "reason_source": "operator",
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
    ]
    dump_json(target / "rows_to_append.json", appended)

    with tempfile.TemporaryDirectory() as raw:
        shutil.copytree(input_root, raw, dirs_exist_ok=True)
        work = Path(raw) / "kitty-specs" / MISSION_SLUG
        store.append_raw_rows_atomic(work / store.EVENTS_FILENAME, appended)
        reducer.materialize(work)
        write(target / "expected" / store.EVENTS_FILENAME, (work / store.EVENTS_FILENAME).read_text(encoding="utf-8"))
        write(target / "expected" / reducer.SNAPSHOT_FILENAME, (work / reducer.SNAPSHOT_FILENAME).read_text(encoding="utf-8"))


def emit_sequence_fixture() -> None:
    target = FIXTURES / "emit_sequence"
    if target.exists():
        shutil.rmtree(target)
    input_root = target / "input"
    input_dir = input_root / "kitty-specs" / MISSION_SLUG
    write(input_dir / "meta.json", json.dumps(fixture_meta(), indent=2, sort_keys=True) + "\n")
    write(input_dir / "tasks" / "WP01.md", WP_FRONTMATTER)

    requests = [
        {
            "event_id": ULIDS[0],
            "at": TIMES[0],
            "wp_id": "WP01",
            "to_lane": "planned",
            "actor": "claude",
            "force": False,
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
        {
            "event_id": ULIDS[1],
            "at": TIMES[1],
            "wp_id": "WP01",
            "to_lane": "claimed",
            "actor": "claude",
            "force": False,
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
        {
            "event_id": ULIDS[2],
            "at": TIMES[2],
            "wp_id": "WP01",
            "to_lane": "in_progress",
            "actor": "claude",
            "force": False,
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
        {
            "event_id": ULIDS[3],
            "at": TIMES[3],
            "wp_id": "WP01",
            "to_lane": "for_review",
            "actor": "claude",
            "force": False,
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
    ]

    ids = iter(ULIDS)
    times = iter(TIMES)
    original_ulid = status_emit._generate_ulid
    original_now = status_emit.now_utc_iso
    status_emit._generate_ulid = lambda: next(ids)
    status_emit.now_utc_iso = lambda: next(times)
    try:
        with tempfile.TemporaryDirectory() as raw:
            shutil.copytree(input_root, raw, dirs_exist_ok=True)
            repo_root = Path(raw)
            work = repo_root / "kitty-specs" / MISSION_SLUG
            for request in requests:
                status_emit.emit_status_transition(
                    feature_dir=work,
                    mission_slug=MISSION_SLUG,
                    wp_id=request["wp_id"],
                    to_lane=request["to_lane"],
                    actor=request["actor"],
                    repo_root=repo_root,
                )
            dump_json(target / "requests.json", requests)
            write(target / "expected" / store.EVENTS_FILENAME, (work / store.EVENTS_FILENAME).read_text(encoding="utf-8"))
            write(target / "expected" / reducer.SNAPSHOT_FILENAME, (work / reducer.SNAPSHOT_FILENAME).read_text(encoding="utf-8"))
    finally:
        status_emit._generate_ulid = original_ulid
        status_emit.now_utc_iso = original_now


def lifecycle_project_fixture() -> None:
    target = FIXTURES / "lifecycle_project"
    if target.exists():
        shutil.rmtree(target)
    original_id = lifecycle_events._generate_event_id
    original_now = lifecycle_events.now_utc_iso
    lifecycle_events._generate_event_id = lambda: ULIDS[0]
    lifecycle_events.now_utc_iso = lambda: TIMES[0]
    try:
        with tempfile.TemporaryDirectory() as raw:
            repo_root = Path(raw)
            envelope = lifecycle_events.emit_project_initialized(
                repo_root,
                project_uuid=PROJECT_UUID,
                project_slug="mutation-fixture",
                actor="cli",
                runtime_version="4.1.0",
                initialized_at=TIMES[0],
            )
            log_path = lifecycle_events.project_event_log_path(repo_root)
            text = log_path.read_text(encoding="utf-8")
            # A second call must be a no-op (dedupe on project_uuid).
            again = lifecycle_events.emit_project_initialized(
                repo_root,
                project_uuid=PROJECT_UUID,
                project_slug="mutation-fixture",
                actor="cli",
                runtime_version="4.1.0",
                initialized_at=TIMES[0],
            )
            if again is not None or log_path.read_text(encoding="utf-8") != text:
                raise SystemExit("ProjectInitialized dedupe fixture is not idempotent")
            dump_json(
                target / "request.json",
                {
                    "event_id": ULIDS[0],
                    "timestamp": TIMES[0],
                    "log_path": lifecycle_events.PROJECT_EVENTS_FILENAME,
                    "event_type": "ProjectInitialized",
                    "payload": envelope["payload"],
                    "aggregate_id": envelope["aggregate_id"],
                    "aggregate_type": envelope["aggregate_type"],
                    "project_uuid": envelope["project_uuid"],
                    "project_slug": envelope["project_slug"],
                    "dedup_keys": {"project_uuid": PROJECT_UUID},
                },
            )
            write(target / "expected" / lifecycle_events.PROJECT_EVENTS_FILENAME, text)
    finally:
        lifecycle_events._generate_event_id = original_id
        lifecycle_events.now_utc_iso = original_now


def lifecycle_mission_fixture() -> None:
    target = FIXTURES / "lifecycle_mission"
    if target.exists():
        shutil.rmtree(target)
    from spec_kitty_events.project_lifecycle import WPCreatedPayload

    payload = WPCreatedPayload(
        mission_slug=MISSION_SLUG,
        mission_number=None,
        wp_id="WP01",
        wp_title="Mutation fixture work package",
        wp_path="kitty-specs/034-mutation-fixture/tasks/WP01.md",
        depends_on=[],
        actor="cli",
        created_at=TIMES[0],
    ).model_dump(mode="json", exclude_none=False)
    original_id = lifecycle_events._generate_event_id
    original_now = lifecycle_events.now_utc_iso
    lifecycle_events._generate_event_id = lambda: ULIDS[3]
    lifecycle_events.now_utc_iso = lambda: TIMES[3]
    try:
        with tempfile.TemporaryDirectory() as raw:
            feature_dir = Path(raw) / "kitty-specs" / MISSION_SLUG
            feature_dir.mkdir(parents=True)
            log_path = lifecycle_events.mission_event_log_path(feature_dir)
            envelope = lifecycle_events.persist_lifecycle_event_local(
                log_path,
                lifecycle_events.WP_CREATED,
                payload,
                aggregate_id="WP01",
                aggregate_type="WorkPackage",
                dedup_keys={"mission_slug": MISSION_SLUG, "wp_id": "WP01"},
                mission_slug=MISSION_SLUG,
            )
            text = log_path.read_text(encoding="utf-8")
            dump_json(
                target / "request.json",
                {
                    "event_id": ULIDS[3],
                    "timestamp": TIMES[3],
                    "log_path": lifecycle_events.MISSION_EVENTS_FILENAME,
                    "event_type": envelope["event_type"],
                    "payload": envelope["payload"],
                    "aggregate_id": envelope["aggregate_id"],
                    "aggregate_type": envelope["aggregate_type"],
                    "project_uuid": envelope["project_uuid"],
                    "project_slug": envelope["project_slug"],
                    "dedup_keys": {"mission_slug": MISSION_SLUG, "wp_id": "WP01"},
                },
            )
            write(target / "expected" / lifecycle_events.MISSION_EVENTS_FILENAME, text)
    finally:
        lifecycle_events._generate_event_id = original_id
        lifecycle_events.now_utc_iso = original_now


def migration_fixture() -> None:
    target = FIXTURES / "migration"
    if target.exists():
        shutil.rmtree(target)
    input_dir = target / "input"
    rows = [
        {
            "event_id": ULIDS[0],
            "event_type": "MissionCreated",
            "aggregate_id": MISSION_ID,
            "aggregate_type": "Mission",
            "schema_version": "5.0.0",
            "timestamp": TIMES[0],
            "payload": {"mission_slug": MISSION_SLUG, "mission_id": MISSION_ID, "wp_count": 2},
            "project_uuid": PROJECT_UUID,
            "project_slug": "mutation-fixture",
        },
        {
            "event_id": ULIDS[1],
            "event_type": "SpecifyCompleted",
            "aggregate_id": MISSION_SLUG,
            "aggregate_type": "Mission",
            "schema_version": "5.0.0",
            "timestamp": TIMES[1],
            "payload": {"mission_slug": MISSION_SLUG, "artifact_path": "spec.md"},
            "project_uuid": PROJECT_UUID,
            "project_slug": "mutation-fixture",
        },
        {
            "event_id": ULIDS[2],
            "event_type": "ReviewerSelfApproval",
            "aggregate_id": "WP01",
            "aggregate_type": "WorkPackage",
            "schema_version": "5.0.0",
            "timestamp": TIMES[2],
            "payload": {"mission_slug": MISSION_SLUG, "wp_id": "WP01"},
            "project_uuid": PROJECT_UUID,
            "project_slug": "mutation-fixture",
        },
        {
            "event_id": ULIDS[3],
            "event_type": "TasksStarted",
            "aggregate_id": MISSION_SLUG,
            "aggregate_type": "Mission",
            "schema_version": "5.0.0",
            "timestamp": TIMES[3],
            "payload": {"mission_slug": MISSION_SLUG},
            "project_uuid": None,
            "project_slug": None,
        },
        {
            "event_id": ULIDS[4],
            "event_type": "MissionCreated",
            "aggregate_id": MISSION_ID,
            "schema_version": "3.0.0",
            "timestamp": TIMES[0],
            "payload": {"mission_slug": MISSION_SLUG, "wp_count": 2},
            "project_uuid": PROJECT_UUID,
            "project_slug": "mutation-fixture",
            "causation_id": None,
            "correlation_id": ULIDS[4],
            "data_tier": 0,
            "node_id": "stable-node1",
            "lamport_clock": 0,
            "build_id": "00000000-0000-5000-8000-000000000000",
        },
        {
            "event_id": ULIDS[5],
            "mission_slug": MISSION_SLUG,
            "wp_id": "WP01",
            "from_lane": "planned",
            "to_lane": "claimed",
            "at": TIMES[4],
            "actor": "claude",
            "force": False,
            "execution_mode": "worktree",
            "reason": None,
            "review_ref": None,
            "evidence": None,
            "policy_metadata": None,
        },
        {
            "event_id": "01K9M0R3WR0Z0W1X2Y3Z4A5B6C",
            "kind": "annotation",
            "wp_id": "WP01",
            "at": TIMES[5],
            "actor": "agent-1",
            "delta": {"subtasks": {"T001": "done"}},
        },
        {
            "event_id": "01K9M0R3WS1A1B2C3D4E5F6G7H",
            "event_type": "DecisionPointOpened",
            "aggregate_id": MISSION_ID,
            "payload": {"decision_id": "D-1"},
        },
        {
            "event_id": "01K9M0R3WT2B2C3D4E5F6G7H8J",
            "event_name": "retrospective.captured",
            "payload": {"status": "captured"},
        },
    ]
    dump_rows(input_dir / store.EVENTS_FILENAME, rows)

    original_node = migration._generate_node_id
    migration._generate_node_id = lambda: "abcdef123456"
    try:
        with tempfile.TemporaryDirectory() as raw:
            work = Path(raw) / "mission"
            shutil.copytree(input_dir, work)
            log_path = work / store.EVENTS_FILENAME
            manifest = migration.migrate_lifecycle_envelope(log_path)
            if manifest.refused_reason:
                raise SystemExit(f"migration fixture refused: {manifest.refused_reason}")
            write(target / "expected" / store.EVENTS_FILENAME, log_path.read_text(encoding="utf-8"))
            backup = log_path.with_name(log_path.name + ".pre-migration.bak")
            write(
                target / "expected" / (store.EVENTS_FILENAME + ".pre-migration.bak"),
                backup.read_text(encoding="utf-8"),
            )
            dump_json(
                target / "expected" / "manifest.json",
                {
                    "total_rows": manifest.total_rows,
                    "migrated_count": manifest.migrated_count,
                    "unchanged_count": manifest.unchanged_count,
                    "skipped_count": manifest.skipped_count,
                    "rows": [
                        {
                            "event_id": row.event_id,
                            "event_type": row.event_type,
                            "action": row.action,
                            "pre_hash": row.pre_hash,
                            "post_hash": row.post_hash,
                        }
                        for row in manifest.rows
                    ],
                },
            )
    finally:
        migration._generate_node_id = original_node


def provenance() -> None:
    dump_json(
        FIXTURES / "provenance.json",
        {
            "baseline_commit": BASELINE,
            "generated_by": "crates/sk-status/tests/fixtures/mutation/generate_fixtures.py",
            "command": "PYTHONPATH=src .venv-rust-oracle/bin/python crates/sk-status/tests/fixtures/mutation/generate_fixtures.py",
            "python": sys.version.split()[0],
            "spec_kitty_events": importlib.metadata.version("spec-kitty-events"),
            "records": [
                "status_append: store.append_raw_rows_atomic + reducer.materialize",
                "emit_sequence: emit.emit_status_transition (ULID/time monkeypatched)",
                "lifecycle_project: lifecycle_events.emit_project_initialized (id/time monkeypatched)",
                "lifecycle_mission: lifecycle_events.persist_lifecycle_event_local (id/time monkeypatched)",
                "migration: migrate_lifecycle_envelope.migrate_lifecycle_envelope (node id monkeypatched)",
            ],
            "note": "Expected artifacts are produced only by the pinned Python implementation; the Rust writer is compared against them byte-for-byte.",
        },
    )


def main() -> None:
    status_append_fixture()
    emit_sequence_fixture()
    lifecycle_project_fixture()
    lifecycle_mission_fixture()
    migration_fixture()
    provenance()
    print(f"fixtures written under {FIXTURES}")


if __name__ == "__main__":
    main()
