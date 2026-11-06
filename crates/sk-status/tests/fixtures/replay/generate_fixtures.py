#!/usr/bin/env python3
"""Generate the committed sk-status replay golden corpus from the pinned oracle.

The oracle is the *installed* ``spec_kitty_events==11.3.0`` diary reducer
(``uv.lock`` pin) driven through the baseline CLI projection
``specify_cli.status.reducer.materialize_snapshot``.  It is a development-time
generator only: no Rust test or production path imports Python.

Run from the repository root with the pinned interpreter::

    SPEC_KITTY_PACKS_ROOT="$PWD/packs" PYTHONPATH=src \
    .venv-rust-oracle/bin/python \
    crates/sk-status/tests/fixtures/replay/generate_fixtures.py

The script refuses to run when the installed package version differs from the
version pinned in ``uv.lock``.  It writes ``provenance.json`` with the oracle
identity, source revision, and per-fixture input/output digests.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parents[4]
FIXTURE_ROOT = HERE

# Bound the committed corpus so review stays cheap.
MAX_TOTAL_EVENTS = 500
MAX_TOTAL_INPUT_BYTES = 400_000


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _read_pin() -> str:
    text = (REPO_ROOT / "uv.lock").read_text(encoding="utf-8")
    match = re.search(r'name = "spec-kitty-events"\nversion = "([^"]+)"', text)
    if match is None:
        raise SystemExit("could not find spec-kitty-events pin in uv.lock")
    return match.group(1)


def _require_oracle() -> tuple[str, str]:
    try:
        import spec_kitty_events  # noqa: PLC0415
        from specify_cli.status.reducer import materialize_snapshot, materialize_to_json  # noqa: PLC0415
    except Exception as exc:  # pragma: no cover - operator setup failure
        raise SystemExit(
            "oracle import failed; set PYTHONPATH=src and SPEC_KITTY_PACKS_ROOT to the "
            f"pinned built-in pack cache. Original error: {exc}"
        ) from exc
    pin = _read_pin()
    if spec_kitty_events.__version__ != pin:
        raise SystemExit(
            f"installed spec_kitty_events {spec_kitty_events.__version__} != uv.lock pin {pin}"
        )
    return pin, str(Path(sys.executable))


def _git_revision() -> str:
    return subprocess.run(
        ["git", "-C", str(REPO_ROOT), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


class FixtureBuilder:
    def __init__(self, slug: str, *, meta: dict[str, Any] | None = None) -> None:
        self.slug = slug
        self.rows: list[dict[str, Any]] = []
        self.meta = meta if meta is not None else {
            "slug": slug,
            "mission_slug": slug,
            "mission_number": 42,
            "mission_type": "software-dev",
        }
        self.existing_status: dict[str, Any] | None = None
        self._next = 0

    def ulid(self) -> str:
        self._next += 1
        return f"01J{self._next:023d}"

    def ev(
        self,
        wp: str,
        frm: str,
        to: str,
        at: str,
        *,
        actor: Any = "agent-1",
        force: bool = False,
        **extra: Any,
    ) -> None:
        row: dict[str, Any] = {
            "event_id": self.ulid(),
            "mission_slug": self.slug,
            "wp_id": wp,
            "from_lane": frm,
            "to_lane": to,
            "at": at,
            "actor": actor,
            "force": force,
            "execution_mode": "worktree",
        }
        row.update(extra)
        self.rows.append(row)

    def duplicate_last(self, **changes: Any) -> None:
        row = dict(self.rows[-1])
        row.update(changes)
        self.rows.append(row)

    def ann(
        self,
        wp: str,
        at: str,
        delta: dict[str, Any],
        *,
        actor: Any = "agent-1",
        event_id: str | None = None,
        **extra: Any,
    ) -> None:
        row: dict[str, Any] = {
            "event_id": event_id or self.ulid(),
            "kind": "annotation",
            "wp_id": wp,
            "at": at,
            "actor": actor,
            "delta": delta,
        }
        row.update(extra)
        self.rows.append(row)

    def non_lane(self, row: dict[str, Any]) -> None:
        self.rows.append(row)


def _write_case(name: str, rows: list[dict[str, Any]], *, meta: dict[str, Any] | None, status: Any, materialize_snapshot, materialize_to_json, raw_status: bytes | None = None) -> dict[str, Any]:
    case_dir = FIXTURE_ROOT / "cases" / name
    if case_dir.exists():
        shutil.rmtree(case_dir)
    case_dir.mkdir(parents=True)
    events_bytes = b"".join(
        (json.dumps(row, ensure_ascii=False, separators=(",", ":"), sort_keys=True) + "\n").encode("utf-8")
        for row in rows
    )
    (case_dir / "status.events.jsonl").write_bytes(events_bytes)

    with tempfile.TemporaryDirectory(prefix="sk-status-oracle-") as tmp:
        first_slug = rows[0].get("mission_slug") if rows else name
        work_name = (
            first_slug
            if isinstance(first_slug, str)
            and first_slug
            and "/" not in first_slug
            and ".." not in first_slug
            else name
        )
        work = Path(tmp) / work_name
        work.mkdir()
        (work / "status.events.jsonl").write_bytes(events_bytes)
        if meta is not None:
            meta_bytes = (json.dumps(meta, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode("utf-8")
            (work / "meta.json").write_bytes(meta_bytes)
            (case_dir / "meta.json").write_bytes(meta_bytes)
        status_bytes = raw_status
        if status is not None:
            status_bytes = (json.dumps(status, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode("utf-8")
        if status_bytes is not None:
            (work / "status.json").write_bytes(status_bytes)
            (case_dir / "status.json").write_bytes(status_bytes)

        snapshot = materialize_snapshot(work)
        expected = materialize_to_json(snapshot).encode("utf-8")
        (case_dir / "expected.json").write_bytes(expected)
        # Replay is read-only: the oracle must not mutate its inputs.
        assert (work / "status.events.jsonl").read_bytes() == events_bytes
        if status_bytes is not None:
            assert (work / "status.json").read_bytes() == status_bytes

    return {
        "kind": "synthetic",
        "name": name,
        "events": len(rows),
        "input_bytes": len(events_bytes),
        "schema_version": snapshot.schema_version,
        "event_count": snapshot.event_count,
        "expected_sha256": _sha256(expected),
        "events_sha256": _sha256(events_bytes),
    }


def _synthetic_cases(materialize_snapshot, materialize_to_json) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []

    def add(builder: FixtureBuilder, name: str, *, status: dict[str, Any] | None = None) -> None:
        entries.append(_write_case(name, builder.rows, meta=builder.meta, status=status, materialize_snapshot=materialize_snapshot, materialize_to_json=materialize_to_json))

    # 1. A realistic multi-WP stream with review evidence, a claim sidecar,
    # annotations and a non-lane Decision Moment row.
    b = FixtureBuilder("900-shuffled-basic")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="impl-alice", policy_metadata={"agent": "impl-alice", "shell_pid": 100, "shell_pid_created_at": "100.5"})
    b.ev("WP01", "claimed", "in_progress", "2026-01-01T00:00:02Z", actor="impl-alice")
    b.ann("WP01", "2026-01-01T00:00:03Z", {"subtasks": {"T001": "done"}, "note": "started", "role": "implementer", "agent_profile": "implementer-ivan", "model": "m1", "provider": "p1"})
    b.ev("WP01", "in_progress", "for_review", "2026-01-01T00:00:04Z", actor="impl-alice")
    b.ev("WP01", "for_review", "in_review", "2026-01-01T00:00:05Z", actor="reviewer-renata")
    b.ev("WP01", "in_review", "approved", "2026-01-01T00:00:06Z", actor="reviewer-renata", review_result={"reviewer": "reviewer-renata", "verdict": "approved", "reference": "review-cycle://900/WP01/1"}, evidence={"review": {"reviewer": "reviewer-renata", "verdict": "approved", "reference": "review-cycle://900/WP01/1"}})
    b.ev("WP01", "approved", "done", "2026-01-01T00:00:07Z", actor="merge", evidence={"review": {"reviewer": "reviewer-renata", "verdict": "approved", "reference": "review-cycle://900/WP01/1"}})
    b.ev("WP02", "planned", "claimed", "2026-01-01T00:00:01Z", actor="impl-bob")
    b.ann("WP02", "2026-01-01T00:00:02Z", {"agent": "impl-bob", "shell_pid": 200})
    b.non_lane({"event_type": "DecisionPointOpened", "payload": {"id": "D1"}, "mission_slug": b.slug})
    add(b, "shuffled-basic")

    # 2. Dedupe keeps the first real occurrence in file order.
    b = FixtureBuilder("900-dedupe-first-wins")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="first-agent")
    b.duplicate_last(actor="second-agent")
    add(b, "dedupe-first-wins", status={"schema_version": 2})

    # 3. Duplicate reconstruction seeds use the canonical-minimum payload, so
    # file order does not decide which claim attribution wins.
    b = FixtureBuilder("900-dedupe-seeds")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="migration:backfill_runtime_state", policy_metadata={"agent": "seed-zzz"})
    b.duplicate_last(policy_metadata={"agent": "seed-aaa"})
    add(b, "dedupe-seeds", status={"schema_version": 2})

    # 4. A stale approval that branched off in_review loses to the rollback on
    # causal continuity, even though it sorts later.
    b = FixtureBuilder("900-stale-approval")
    for i, (frm, to) in enumerate([("planned", "claimed"), ("claimed", "in_progress"), ("in_progress", "for_review"), ("for_review", "in_review")], 1):
        b.ev("WP01", frm, to, f"2026-01-01T00:00:0{i}Z", actor="agent")
    b.ev("WP01", "in_review", "planned", "2026-01-01T00:00:05Z", actor="reviewer", force=True, reason="reject", review_ref="feedback://900/WP01/r1")
    b.ev("WP01", "in_review", "approved", "2026-01-01T00:00:06Z", actor="reviewer", review_result={"reviewer": "reviewer", "verdict": "approved", "reference": "review-cycle://900/WP01/1"})
    add(b, "stale-approval", status={"schema_version": 2})

    # 5. A clock-ahead approval sorts after its own approval: the departure is
    # lifted (and the rework carried) so the final lane is the departure's.
    b = FixtureBuilder("900-approved-departure-clock-skew")
    for i, (frm, to) in enumerate([("planned", "claimed"), ("claimed", "in_progress"), ("in_progress", "for_review"), ("for_review", "in_review")], 1):
        b.ev("WP01", frm, to, f"2026-01-01T00:00:0{i}Z", actor="agent")
    b.ev("WP01", "in_review", "approved", "2026-01-01T00:00:10Z", actor="reviewer", review_result={"reviewer": "reviewer", "verdict": "approved", "reference": "review-cycle://900/WP01/1"})
    b.ev("WP01", "approved", "in_progress", "2026-01-01T00:00:05Z", actor="operator", review_ref="feedback://900/WP01/2")
    add(b, "approved-departure-clock-skew", status={"schema_version": 2})

    # 6. Same-timestamp rollback arbitration beats a forward approval.
    b = FixtureBuilder("900-rollback-same-timestamp")
    for i, (frm, to) in enumerate([("planned", "claimed"), ("claimed", "in_progress"), ("in_progress", "for_review"), ("for_review", "in_review")], 1):
        b.ev("WP01", frm, to, f"2026-01-01T00:00:0{i}Z", actor="agent")
    rollback = b.ulid()
    b.rows.append({"event_id": rollback, "mission_slug": b.slug, "wp_id": "WP01", "from_lane": "in_review", "to_lane": "planned", "at": "2026-01-01T00:00:10Z", "actor": "reviewer", "force": True, "execution_mode": "worktree", "reason": "reject", "review_ref": "feedback://900/WP01/r1"})
    b.ev("WP01", "in_review", "approved", "2026-01-01T00:00:10Z", actor="reviewer", review_result={"reviewer": "reviewer", "verdict": "approved", "reference": "review-cycle://900/WP01/1"})
    add(b, "rollback-same-timestamp", status={"schema_version": 2})

    # 7. A rollback reconstruction seed holds the lane against its bare sibling
    # approval, in either file order.
    b = FixtureBuilder("900-rollback-seed-bare-sibling")
    for i, (frm, to) in enumerate([("planned", "claimed"), ("claimed", "in_progress"), ("in_progress", "for_review"), ("for_review", "in_review")], 1):
        b.ev("WP01", frm, to, f"2026-01-01T00:00:0{i}Z", actor="agent")
    b.ev("WP01", "in_review", "approved", "2026-01-01T00:00:10Z", actor="reviewer", review_result={"reviewer": "reviewer", "verdict": "approved", "reference": "review-cycle://900/WP01/1"})
    b.ev("WP01", "in_review", "planned", "2026-01-01T00:00:10Z", actor="migration:backfill_runtime_state", review_ref="feedback://900/WP01/r1")
    add(b, "rollback-seed-bare-sibling", status={"schema_version": 2})

    # 8. A losing forward seed still folds claim attribution no real event wrote.
    b = FixtureBuilder("900-losing-seed-payload")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="impl")
    b.ev("WP01", "claimed", "in_progress", "2026-01-01T00:00:02Z", actor="impl")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:03Z", actor="migration:backfill_runtime_state", policy_metadata={"agent": "seed-agent", "shell_pid": 999, "shell_pid_created_at": "999.0"})
    add(b, "losing-seed-payload", status={"schema_version": 2})

    # 9. Annotations are a separate post-transition pass: earlier annotations
    # still touch the final state, claim release is explicit, and a
    # runtime-only WP stays uninitialized (excluded from the display summary).
    b = FixtureBuilder("900-annotation-runtime")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:05Z", actor="impl", policy_metadata={"agent": "claim-agent", "shell_pid": 111, "shell_pid_created_at": "111.0"})
    b.ann("WP01", "2026-01-01T00:00:01Z", {"agent": "ann-agent", "shell_pid": 222, "subtasks": {"T001": "done"}, "note": "first", "tracker_refs": ["GH-1", "GH-2"]})
    b.ann("WP01", "2026-01-01T00:00:02Z", {"subtasks": {"T002": "not_applicable"}, "note": "second", "tracker_refs": ["GH-2", "GH-3"], "review": {"at": "a", "actor": "r", "wp_id": "WP01", "reason": "override"}, "role": "reviewer", "agent_profile": "reviewer-renata", "agent_profile_version": "v1", "model": "m", "provider": "p"})
    b.ann("WP01", "2026-01-01T00:00:03Z", {"tracker_refs_replace": ["GH-9", "GH-9"], "review": {"at": "", "actor": "", "wp_id": "", "reason": ""}, "release_runtime_claim": True, "agent": "after-release", "shell_pid": 333})
    b.ann("WP01", "2026-01-01T00:00:04Z", {"agent": "", "assignee": "assignee-x"})
    b.ann("WP02", "2026-01-01T00:00:06Z", {"note": "runtime-only"})
    add(b, "annotation-runtime", status={"schema_version": 2})

    # 10. Cancellation provenance is projected from the raw stream: operator,
    # explicit reason_source, synthetic prefixes, blank reasons, and a rolled
    # back cancellation that must not carry the slots.
    b = FixtureBuilder("900-cancellation-provenance")
    b.ev("WP01", "planned", "canceled", "2026-01-01T00:00:01Z", actor="op", force=True, reason="Force move to canceled")
    b.ev("WP02", "planned", "canceled", "2026-01-01T00:00:02Z", actor="op", force=True, reason="operator said stop")
    b.ev("WP03", "planned", "canceled", "2026-01-01T00:00:03Z", actor="op", force=True, reason="whatever", reason_source="operator")
    b.ev("WP04", "planned", "canceled", "2026-01-01T00:00:04Z", actor="op", force=True, reason="")
    b.ev("WP05", "planned", "canceled", "2026-01-01T00:00:05Z", actor="op", force=True, reason="move-task: cancel")
    b.ev("WP06", "planned", "canceled", "2026-01-01T00:00:06Z", actor="op", force=True, reason="op")
    b.ev("WP06", "canceled", "planned", "2026-01-01T00:00:07Z", actor="op", force=True, reason="reinstate")
    add(b, "cancellation-provenance", status={"schema_version": 2})

    # 11. Retrospective rows are non-lane but still shape the derived snapshot.
    b = FixtureBuilder("900-retrospective")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="impl")
    b.non_lane({"event_name": "retrospective.requested", "event_id": b.ulid(), "at": "2026-01-01T00:00:02Z", "payload": {"mode": {"value": "autonomous", "source_signal": {"kind": "explicit_flag", "evidence": "--mode autonomous"}}}})
    b.non_lane({"event_name": "retrospective.proposal.generated", "event_id": b.ulid(), "at": "2026-01-01T00:00:03Z", "payload": {"id": "P1"}})
    b.non_lane({"event_name": "retrospective.proposal.generated", "event_id": b.ulid(), "at": "2026-01-01T00:00:04Z", "payload": {"id": "P2"}})
    b.non_lane({"event_name": "retrospective.proposal.applied", "event_id": b.ulid(), "at": "2026-01-01T00:00:05Z", "payload": {"id": "P1"}})
    b.non_lane({"event_name": "retrospective.completed", "event_id": b.ulid(), "at": "2026-01-01T00:00:06Z", "payload": {"record_path": "kitty-specs/900-retrospective/retrospective.md"}})
    b.non_lane({"event_type": "DecisionPointResolved", "payload": {"id": "D1"}})
    add(b, "retrospective")

    # 12/13. The same event stream replays at its own snapshot schema: legacy
    # (no marker, no implementer_of_record) versus v2 (marker + projection).
    for name, status in (
        ("legacy-schema", {"mission_slug": "900-legacy-schema", "materialized_at": "old", "event_count": 0, "last_event_id": None, "work_packages": {}, "summary": {}}),
        ("current-schema-v2", {"schema_version": 2}),
    ):
        b = FixtureBuilder("900-" + name)
        b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="impl-legacy", policy_metadata={"agent": "legacy-impl"})
        add(b, name, status=status)

    # 14. An empty log still derives identity and summary shape.
    b = FixtureBuilder("900-empty-stream", meta={"slug": "900-empty-stream", "mission_slug": "900-empty-stream", "mission_number": 7, "mission_type": "documentation"})
    add(b, "empty-stream")

    # 15. The untrusted event slug is sanitised at the snapshot seam.
    b = FixtureBuilder("../../evil", meta={"mission_slug": "../../evil", "mission_number": 9, "mission_type": "software-dev"})
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="impl")
    add(b, "unsafe-mission-slug")

    # Independent review regressions: presence survives corrupt/non-object JSON;
    # absent/typeless metadata must not manufacture software-dev identity.
    for name, raw_status in [("malformed-snapshot", b"not json"), ("array-snapshot", b"[]\n"), ("null-snapshot", b"null\n")]:
        b = FixtureBuilder("900-" + name)
        b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", policy_metadata={"agent": "impl-a"})
        entries.append(_write_case(name, b.rows, meta=b.meta, status=None, raw_status=raw_status, materialize_snapshot=materialize_snapshot, materialize_to_json=materialize_to_json))
    for name, meta in [("missing-meta", None), ("typeless-meta", {})]:
        b = FixtureBuilder("900-" + name)
        b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z")
        entries.append(_write_case(name, b.rows, meta=meta, status=None, materialize_snapshot=materialize_snapshot, materialize_to_json=materialize_to_json))
    b = FixtureBuilder("900-unicode-seeds")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="migration:backfill_runtime_state", policy_metadata={"agent": "é-agent"})
    b.duplicate_last(policy_metadata={"agent": "z-agent"})
    add(b, "unicode-seeds", status={"schema_version": 2})
    b = FixtureBuilder("900-floating-seeds")
    b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", actor="migration:backfill_runtime_state", policy_metadata={"aaa": 1e-5, "agent": "small"})
    b.duplicate_last(policy_metadata={"aaa": 0.0001, "agent": "large"})
    add(b, "floating-seeds", status={"schema_version": 2})
    for name, version in [("float-schema", 2.0), ("bool-schema", True), ("string-schema", "2"), ("future-schema", 3)]:
        b = FixtureBuilder("900-" + name)
        b.ev("WP01", "planned", "claimed", "2026-01-01T00:00:01Z", policy_metadata={"agent": "impl-a"})
        add(b, name, status={"schema_version": version})

    return entries


CORPUS = [
    "pre-and-post-mission-lifecycle-support-01KTNK1G",
    "013-fix-and-test-dashboard",
    "blocked-wp-unblock-path-01M29093",
    "native-stale-content-refusal-01M4S2CQ",
    "remove-charter-sync-noop-01M4RXKH",
    "ci-router-charter-dossier-routing-01M4SW1A",
    "friction-remediation-01M43DRV",
    "review-feedback-draft-retirement-01M66YR3",
    "053-orchestrator-api-json-contract-fidelity",
    "unblock-sync-identity-boundary-canary-01KRZJ07",
    "terminus-integrity-followups-01M393QR",
    "regression-slice-cleanup-01M42WCF",
]


def _archive_cases(materialize_snapshot, materialize_to_json) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for slug in CORPUS:
        source = REPO_ROOT / "kitty-specs" / slug
        if not source.is_dir():
            raise SystemExit(f"missing archive corpus mission: {source}")
        case_dir = FIXTURE_ROOT / "archives" / slug
        if case_dir.exists():
            shutil.rmtree(case_dir)
        case_dir.mkdir(parents=True)
        events = (source / "status.events.jsonl").read_bytes()
        meta_bytes = (source / "meta.json").read_bytes() if (source / "meta.json").exists() else None
        status_bytes = (source / "status.json").read_bytes() if (source / "status.json").exists() else None
        (case_dir / "status.events.jsonl").write_bytes(events)
        if meta_bytes is not None:
            (case_dir / "meta.json").write_bytes(meta_bytes)
        if status_bytes is not None:
            (case_dir / "status.json").write_bytes(status_bytes)

        with tempfile.TemporaryDirectory(prefix="sk-status-oracle-") as tmp:
            work = Path(tmp) / slug
            work.mkdir()
            (work / "status.events.jsonl").write_bytes(events)
            if meta_bytes is not None:
                (work / "meta.json").write_bytes(meta_bytes)
            if status_bytes is not None:
                (work / "status.json").write_bytes(status_bytes)
            snapshot = materialize_snapshot(work)
            expected = materialize_to_json(snapshot).encode("utf-8")
            (case_dir / "expected.json").write_bytes(expected)
            assert (work / "status.events.jsonl").read_bytes() == events
            if status_bytes is not None:
                assert (work / "status.json").read_bytes() == status_bytes

        entries.append({
            "kind": "archive",
            "name": slug,
            "events": len([line for line in events.splitlines() if line.strip()]),
            "input_bytes": len(events) + (len(meta_bytes) if meta_bytes else 0) + (len(status_bytes) if status_bytes else 0),
            "schema_version": snapshot.schema_version,
            "event_count": snapshot.event_count,
            "expected_sha256": _sha256(expected),
            "events_sha256": _sha256(events),
        })
    return entries


def main() -> int:
    pin, interpreter = _require_oracle()
    from specify_cli.status.reducer import materialize_snapshot, materialize_to_json  # noqa: PLC0415

    if (FIXTURE_ROOT / "cases").exists():
        shutil.rmtree(FIXTURE_ROOT / "cases")
    if (FIXTURE_ROOT / "archives").exists():
        shutil.rmtree(FIXTURE_ROOT / "archives")
    (FIXTURE_ROOT / "cases").mkdir()
    (FIXTURE_ROOT / "archives").mkdir()

    entries = _synthetic_cases(materialize_snapshot, materialize_to_json)
    entries += _archive_cases(materialize_snapshot, materialize_to_json)

    total_events = sum(entry["events"] for entry in entries)
    total_bytes = sum(entry["input_bytes"] for entry in entries)
    if total_events > MAX_TOTAL_EVENTS or total_bytes > MAX_TOTAL_INPUT_BYTES:
        raise SystemExit(f"fixture corpus exceeds bound: {total_events} events, {total_bytes} bytes")

    provenance = {
        "generator": "crates/sk-status/tests/fixtures/replay/generate_fixtures.py",
        "oracle": {
            "interpreter": interpreter,
            "spec_kitty_events_version": pin,
            "uv_lock_pin": pin,
            "entry_point": "specify_cli.status.reducer.materialize_snapshot",
            "external_reducer": "spec_kitty_events.diary.reduce_parsed",
        },
        "source_revision": _git_revision(),
        "bounds": {"max_total_events": MAX_TOTAL_EVENTS, "max_total_input_bytes": MAX_TOTAL_INPUT_BYTES},
        "totals": {"fixtures": len(entries), "events": total_events, "input_bytes": total_bytes},
        "fixtures": entries,
    }
    (FIXTURE_ROOT / "provenance.json").write_text(json.dumps(provenance, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {len(entries)} fixtures, {total_events} events, {total_bytes} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
