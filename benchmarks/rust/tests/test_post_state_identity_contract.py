"""The written status document must describe the same Mission as stdout."""

import hashlib
import json
from pathlib import Path

import pytest

from spkbench import expectations


def test_status_post_state_pins_the_output_identity_and_schema():
    root = Path(__file__).resolve().parents[3]
    registry = json.loads(
        (root / "docs/rust/inventory/performance-registry.json").read_text()
    )
    entry = next(row for row in registry["workloads"] if row["id"] == "status.replay")
    expectations = entry["expectations"]
    output = expectations["payload"]["equals"]
    written = expectations["post_state"]["json_payload"]["equals"]
    for field in ("mission_slug", "mission_number", "mission_type", "schema_version"):
        assert written.get(field) == output[field], (
            f"written status must bind {field} to the same contract as stdout"
        )


@pytest.mark.parametrize(
    ("field", "foreign"),
    [
        ("mission_slug", "999-another-mission"),
        ("mission_number", "999"),
        ("mission_type", "research"),
        ("schema_version", 1),
    ],
)
def test_stored_status_rejects_foreign_identity_despite_matching_counts(field, foreign):
    root = Path(__file__).resolve().parents[3]
    registry = json.loads(
        (root / "docs/rust/inventory/performance-registry.json").read_text()
    )
    entry = next(row for row in registry["workloads"] if row["id"] == "status.replay")
    schema = entry["expectations"]["post_state"]
    slug = entry["params"]["mission_slug"]
    output_path = f"kitty-specs/{slug}/status.json"
    log_path = f"kitty-specs/{slug}/status.events.jsonl"
    payload = {
        "mission_slug": slug,
        "mission_number": "2",
        "mission_type": "software-dev",
        "schema_version": 2,
        "materialized_at": "2026-11-02T00:00:00Z",
        "last_event_id": "01K9M0R3W00000000000000000",
        "event_count": 2800,
        "work_packages": {f"WP{i:03}": {"lane": "done"} for i in range(200)},
        "summary": {"done": 200},
    }

    def validate(document):
        data = json.dumps(document).encode()
        digest = hashlib.sha256(data).hexdigest()
        before = {log_path: hashlib.sha256(b"unchanged log\n").hexdigest()}
        after = {**before, output_path: digest}
        delta = {
            "added": [output_path], "changed": [], "removed": [],
            "added_sha256": {output_path: digest},
            "changed_sha256": {}, "removed_sha256": {},
        }
        return expectations.validate_post_state(
            before=before, after=after, delta=delta, schema=schema,
            params=entry["params"], files={output_path: data},
        )

    assert validate(payload) == []
    payload[field] = foreign
    problems = validate(payload)
    assert any(f"payload {field} is" in problem for problem in problems), problems
