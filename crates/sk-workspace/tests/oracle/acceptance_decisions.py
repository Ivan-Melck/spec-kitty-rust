"""Source clarification lookup/read-phase receipts; not full accept approval."""

import json
import os
import pathlib
import stat
import sys
import threading
from pathlib import Path

root = Path(os.environ["SK_ACCEPT_DECISION_ROOT"])
repo = Path(os.environ["PYTHONPATH"]).parent
assert Path.cwd() == root and sys.version_info[:3] == (3, 11, 15)
for key, name in [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
]:
    assert Path(os.environ[key]) == root / name
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
from ulid import ULID  # noqa: E402
from specify_cli.acceptance import _check_needs_clarification, _iter_clarification_decision_ids, _read_text_strict  # noqa: E402
from specify_cli.decisions.store import load_index  # noqa: E402

slug = "001-decision-probe"
mission_id = str(ULID())
marker = "[NEEDS CLARIFICATION: choose] <!-- decision_id: DM -->\n"


def project(name):
    base = root / "cases" / name
    feature = base / "kitty-specs" / slug
    (feature / "tasks").mkdir(parents=True)
    (feature / "meta.json").write_text(
        json.dumps({"mission_id": mission_id, "mission_slug": slug, "mission_type": "software-dev", "topology": "lanes", "target_branch": "main"})
    )
    return base, feature


def entry(decision_id, status):
    return {
        "decision_id": decision_id,
        "origin_flow": "specify",
        "step_id": "specify",
        "input_key": "q",
        "question": "Choose?",
        "status": status,
        "created_at": "2026-01-01T00:00:00Z",
        "mission_id": mission_id,
        "mission_slug": slug,
    }


def payload(statuses):
    return json.dumps({"mission_id": mission_id, "entries": [entry(key, status) for key, status in statuses]}).encode()


resolved = payload([("DM", "resolved"), ("\ud800", "open"), ("\udfff", "canceled"), ("\x00", "deferred"), ("\ue000", "resolved"), ("\ufdd0", "open")])
open_payload = payload([("DM", "open")])
rows = []


def case(name, text, index_bytes, *, expected=None, error=None, plan=None, directory=False, unreachable=False):
    base, feature = project(name)
    spec = feature / "spec.md"
    spec.write_bytes(text if isinstance(text, bytes) else text.encode())
    if plan is not None:
        (feature / "plan.md").write_bytes(plan)
    if index_bytes is not None or directory or unreachable:
        (feature / "decisions").mkdir()
        if directory:
            (feature / "decisions/index.json").mkdir()
        else:
            (feature / "decisions/index.json").write_bytes(index_bytes or b"bad")
    if unreachable:
        (feature / "decisions").chmod(0)
    caught = None
    try:
        ids = list(_iter_clarification_decision_ids(_read_text_strict(spec)))
        files = [spec] + ([feature / "plan.md"] if plan is not None else [])
        result = _check_needs_clarification(files)
        keys = []
        if ids:
            keys = sorted(e.decision_id for e in load_index(feature).entries)
    except Exception as exc:
        caught = exc
    if caught is not None:
        assert error is not None and type(caught).__name__ == error, (name, type(caught).__name__, str(caught), error)
        rows.append(
            {
                "name": name,
                "project": str(base),
                "error": type(caught).__name__,
                "message": str(caught),
                "compare_text": error == "ArtifactEncodingError" or (error == "DecisionIndexReadError" and not directory),
            }
        )
    else:
        assert error is None and bool(result) == expected, (name, result, error)
        rows.append(
            {
                "name": name,
                "project": str(base),
                "blocked": result,
                "ids": [list(map(ord, key)) for key in ids],
                "keys": [list(map(ord, key)) for key in sorted(set(keys))],
            }
        )


for status in ["resolved", "canceled", "open", "deferred"]:
    case(status, marker, payload([("DM", status)]), expected=status in ["open", "deferred"])
case("absent", marker, None, expected=True)
case("unknown", marker, payload([]), expected=True)
case("duplicate-last-resolved", marker, payload([("DM", "open"), ("DM", "resolved")]), expected=False)
case("duplicate-last-open", marker, payload([("DM", "resolved"), ("DM", "open")]), expected=True)
case("corrupt-linked", marker, b"not json", error="DecisionIndexReadError")
case("corrupt-prose", "[NEEDS CLARIFICATION: prose]\n", b"not json", expected=False)
case("corrupt-unlinked", "no linked markers\n", b"not json", expected=False)
case("invalid-artifact", b"\xef\xbb\xbfabc\xff", resolved, error="ArtifactEncodingError")
case("index-directory", marker, None, directory=True, error="DecisionIndexReadError")
case("probe-unreachable", marker, b"bad", unreachable=True, error="PermissionError")
case("unused-unreachable", "no marker", b"bad", unreachable=True, expected=False)
case("index-error-before-later-artifact", marker, b"bad", plan=b"\xff", error="DecisionIndexReadError")
for number, boundary in enumerate(["\n", "\r", "\r\n", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"]):
    case("line-" + str(number), "[NEEDS CLARIFICATION: q]" + boundary + "<!-- decision_id: DM -->", b"bad", expected=False)
for point in range(0x110000):
    if chr(point).isspace():
        char = chr(point)
        text = "[NEEDS CLARIFICATION: q] <!-- decision_id:" + char + "DM" + char + "note -->"
        case("space-" + str(point), text, resolved, expected=False)
case("bom", b"\xef\xbb\xbf" + marker.encode(), resolved, expected=False)
case("exact-keys", marker, resolved, expected=False)
case("nul-marker", "[NEEDS CLARIFICATION: q] <!-- decision_id: DM\x00X -->", payload([("DM\x00X", "resolved")]), expected=False)

# No sleeps, polling, or timing assumptions. Opening the plan FIFO is the
# handshake proving the first index read closed before the second payload.
base, feature = project("stream")
(feature / "spec.md").write_text(marker)
(feature / "decisions").mkdir()
index = feature / "decisions/index.json"
plan = feature / "plan.md"
os.mkfifo(index)
os.mkfifo(plan)
node_before = [(p.stat().st_ino, p.stat().st_mode) for p in [index, plan]]


def feed():
    for path, data in [(index, open_payload), (plan, marker.encode()), (index, payload([("DM", "resolved")]))]:
        with path.open("wb") as stream:
            stream.write(data)


worker = threading.Thread(target=feed, daemon=True)
worker.start()
stream_result = _check_needs_clarification([feature / "spec.md", plan])
worker.join()
assert stream_result == [str(feature / "spec.md")], stream_result
assert node_before == [(p.stat().st_ino, p.stat().st_mode) for p in [index, plan]]
assert all(stat.S_ISFIFO(p.stat().st_mode) for p in [index, plan])
print(
    json.dumps(
        {
            "rows": rows,
            "stream": {
                "project": str(base),
                "blocked": stream_result,
                "payload_open": list(open_payload),
                "payload_resolved": list(payload([("DM", "resolved")])),
                "plan": list(marker.encode()),
            },
            "ignored_errnos": [n for n in range(256) if pathlib._ignore_error(OSError(n, "probe"))],
        },
        separators=(",", ":"),
    )
)
