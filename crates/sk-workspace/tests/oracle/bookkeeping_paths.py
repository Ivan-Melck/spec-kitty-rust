"""Read-only Source receipts for exact bookkeeping classification (not a writer)."""

import json
import os
import sys
from pathlib import Path

root = Path(os.environ["SK_BOOKKEEPING_ROOT"])
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

# Import the product only after checking the actual execution environment.
from specify_cli.coordination.coherence import is_self_bookkeeping_churn  # noqa: E402
from specify_cli.status import is_dossier_snapshot  # noqa: E402

positive = [
    ".kittify/meta.json",
    "kitty-specs/m/meta.json",
    ".kittify/encoding-provenance/global.jsonl",
    ".kittify/mission-state-audit",
    ".kittify/mission-state-audit/quarantine/x",
    ".kittify/derived/m/status.md",
    ".kittify/dossiers/m/snapshot-latest.json",
    ".kittify/dossiers/a/deep/snapshot-latest.json",
    ".kittify/dossiers//snapshot-latest.json",
    "kitty-specs/m/decisions/index.json.lock",
    "kitty-ops/" + "0" * 26 + ".jsonl",
    "kitty-ops/" + "Z" * 26 + ".jsonl",
]
negative = [
    "src/meta.json",
    "kitty-specs/m/research/meta.json",
    "kitty-specs//meta.json",
    ".kittify/derived/status.md",
    ".kittify/derived/m/nested/view.md",
    ".kittify/mission-state-audit-notes",
    "kitty-specs/m/nested/decisions/index.json.lock",
    ".kittify/dossiers/m/snapshot-latest.json.old",
    "decisions/index.json.lock",
    "kitty-ops/" + "I" * 26 + ".jsonl",
    "kitty-ops/" + "z" * 26 + ".jsonl",
    "kitty-ops/" + "0" * 25 + ".jsonl",
    "kitty-ops/" + "0" * 27 + ".jsonl",
]
assert all(is_self_bookkeeping_churn(path) for path in positive)
assert not any(is_self_bookkeeping_churn(path) for path in negative)
# Preserve Source's actual regex-$ and fnmatch wildcard domains, not cleaner rules.
assert is_self_bookkeeping_churn("kitty-specs/m/meta.json\n")
assert not is_self_bookkeeping_churn("kitty-specs/m/meta.json\n\n")
assert is_self_bookkeeping_churn("prefix.kittify/encoding-provenance/global.jsonl")
assert is_dossier_snapshot(".kittify/dossiers//snapshot-latest.json")
assert is_dossier_snapshot(".kittify/dossiers/a/b/snapshot-latest.json")
assert not is_dossier_snapshot(".kittify/dossiers/a/snapshot-latest.json\n")

paths = []
for prefix in ["", "sub/", "./", "/absolute/", "../", "sub\n/", "sub\\"]:
    for path in positive + negative:
        for suffix in ["", "/", "///", "\n", "\n/", "\n\n", "\r\n", " ", "/child"]:
            paths.append(prefix + path + suffix)
for point in [0, *range(0xD800, 0xE000), 0xE000, 0xFDD0, 0xFFFE, 0x10FFFF]:
    char = chr(point)
    paths.extend(
        [
            "kitty-specs/" + char + "/meta.json",
            ".kittify/derived/" + char + "/view",
            ".kittify/dossiers/" + char + "/snapshot-latest.json",
            "kitty-ops/" + char * 26 + ".jsonl",
        ]
    )
rows = [{"points": list(map(ord, path)), "bookkeeping": is_self_bookkeeping_churn(path), "dossier": is_dossier_snapshot(path)} for path in paths]
raw_rows = []
for byte in range(256):
    for before, after in [(b"kitty-specs/", b"/meta.json"), (b".kittify/dossiers/", b"/snapshot-latest.json"), (b"kitty-ops/", b".jsonl")]:
        raw = before + bytes([byte]) + after
        decoded = os.fsdecode(raw)
        raw_rows.append({"bytes": list(raw), "bookkeeping": is_self_bookkeeping_churn(decoded), "dossier": is_dossier_snapshot(decoded)})
print(json.dumps({"rows": rows, "raw_rows": raw_rows}, separators=(",", ":")))
