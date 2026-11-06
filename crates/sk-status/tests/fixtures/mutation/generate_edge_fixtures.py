"""Generate numeric/UUID edge contracts using the verified, pinned oracle only."""

from __future__ import annotations

import hashlib
import json
import math
import random
import struct
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[5]
BASELINE = "9aed3a8012cb5eb22c0128b14d525039f95fea7a"
subprocess.run(
    [sys.executable, str(ROOT / "scripts/rust/verify_oracle.py"), "--root", str(ROOT)],
    check=True,
    capture_output=True,
)
sys.path.insert(0, str(ROOT / "src"))
from specify_cli.status import migrate_lifecycle_envelope as migration  # noqa: E402

sources = {}
for relative in [
    "src/specify_cli/status/migrate_lifecycle_envelope.py",
    "src/specify_cli/identity/project.py",
]:
    actual = (ROOT / relative).read_bytes()
    pinned = subprocess.check_output(["git", "show", f"{BASELINE}:{relative}"], cwd=ROOT)
    if actual != pinned:
        raise SystemExit(f"oracle source drift: {relative}")
    # Source-file integrity, not a charter document freshness hash.
    sources[relative] = hashlib.sha256(actual).hexdigest()  # noqa: TID251

rng = random.Random(442)  # noqa: S311 - reproducible IEEE test vectors, not secrets
values = [0.0, -0.0, 810553441865041.25, 1e-4, 1e-5, 1e15, 1e16, 5e-324]
while len(values) < 2056:
    value = struct.unpack(">d", rng.getrandbits(64).to_bytes(8, "big"))[0]
    if math.isfinite(value):
        values.append(value)
floats = [
    {
        "bits": struct.pack(">d", value).hex(),
        "expected": json.dumps(value, allow_nan=False),
    }
    for value in values
]

uuid = "12345678-1234-5678-9abc-def012345678"
cases = []
for raw in [
    uuid,
    uuid.upper(),
    uuid.replace("-", ""),
    "{" + uuid + "}",
    "{{" + uuid + "}}",
    "urn:uuid:" + uuid,
    11111111111111111111111111111111,
    True,
    42,
    {"bad": "uuid"},
]:
    row = {
        "event_id": "01K9M0R3WJ8V6Q1Y4T7B2N5C0A",
        "event_type": "ProjectInitialized",
        "schema_version": "5.0.0",
        "aggregate_type": "project",
        "project_uuid": raw,
        "payload": {"metadata": {"float": 810553441865041.25}},
    }
    case = {"input": row}
    try:
        with patch.object(migration, "_generate_node_id", return_value="abcdef123456"):
            expected, action = migration._migrate_row(row)
        case.update(expected=expected, action=action)
    except (ValueError, TypeError, AttributeError) as error:
        case["error_type"] = type(error).__name__
    cases.append(case)

output = {
    "baseline": BASELINE,
    "python": sys.version.split()[0],
    "source_sha256": sources,
    "floats": floats,
    "uuid_migrations": cases,
}
Path(__file__).with_name("edge-cases.json").write_text(json.dumps(output, indent=2, sort_keys=True, ensure_ascii=True) + "\n", encoding="utf-8")
