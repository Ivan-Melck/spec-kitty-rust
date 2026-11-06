"""Actual pinned-Source receipts for both Source YAML loader dialects.

Reads `<root>/cases/index.json` (`[{"label", "dialect", "dir"}]`), loads each
fixture exactly as Source does for that dialect, and prints one JSON row per case:

* `dialect` — `pyyaml` (`yaml.safe_load` on a text stream) or `ruamel`
  (`ruamel.yaml.YAML(typ="safe").load(path)`);
* `ok` — did the loader return a value;
* `document` — the decoded Python value, transported exactly (code points for
  strings, so a lone surrogate survives);
* `error_class` / `error` — the exception class name and message when it failed.

`PYTHONHASHSEED` is pinned by the caller on both sides, so a `!!set`'s iteration
order is one observable answer.
"""

import json
import os
import sys
from datetime import UTC, date, datetime
import warnings
from pathlib import Path

import yaml
import ruamel.yaml

warnings.simplefilter("ignore")

root = Path(sys.argv[1])
# Assertions belong to THIS actual Source leg, not just a prior probe or the
# parent command builder. Historical L4/L5 receipts lack these root assertions.
assert Path.cwd() == root
assert sys.version_info[:3] == (3, 11, 15)
for key, name in [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
]:
    assert os.environ[key] == str(root / name), (key, os.environ.get(key))
repo = Path(os.environ["PYTHONPATH"]).parent
assert repo.is_absolute() and Path(os.environ["PYTHONPATH"]) == repo / "src"
assert os.environ["SPEC_KITTY_PACKS_ROOT"] == str(repo / "packs")
assert os.environ["PYTHONHASHSEED"] == "0"
assert sys.hash_info.algorithm == "siphash13"
# These are actual Source hash facts, asserted before the Native process runs.
assert hash(date(2026, 11, 5)) == 7311123894698728974
assert hash(datetime(2026, 11, 5)) == 7281516447428217606
assert hash(datetime(2026, 11, 5, tzinfo=UTC)) == -2210446007498203512
assert date(1970, 1, 1).toordinal() == 719163


def encode(value, seen=None):
    if seen is None:
        seen = set()
    if id(value) in seen:
        # A recursive alias: Source really builds a cycle. The receipt records
        # that shape instead of recursing forever (only the declared open case
        # `recursive-alias` hits this, and it never compares documents).
        return {"t": "cycle"}
    if value is None:
        return {"t": "none"}
    if isinstance(value, bool):
        return {"t": "bool", "v": value}
    if isinstance(value, int):
        return {"t": "int", "v": str(value)}
    if isinstance(value, float):
        return {"t": "float", "v": repr(value)}
    if isinstance(value, str):
        return {"t": "str", "v": [ord(character) for character in value]}
    if isinstance(value, (bytes, bytearray)):
        return {"t": "bytes", "v": list(value)}
    seen.add(id(value))
    try:
        return _encode_container(value, seen)
    finally:
        seen.discard(id(value))


def _encode_container(value, seen):
    if isinstance(value, set):
        return {"t": "set", "v": [encode(item, seen) for item in value]}
    if isinstance(value, tuple):
        return {"t": "tuple", "v": [encode(item, seen) for item in value]}
    if isinstance(value, list):
        # `!!pairs` and PyYAML's `!!omap` construct a Python list of 2-tuples,
        # which is the value domain's `Pairs` kind; a plain list is a `seq`.
        if value and all(isinstance(item, tuple) and len(item) == 2 for item in value):
            return {"t": "pairs", "v": [[encode(key, seen), encode(item, seen)] for key, item in value]}
        return {"t": "seq", "v": [encode(item, seen) for item in value]}
    if hasattr(value, "keys"):
        try:
            items = list(value.items())
        except AttributeError:
            items = list(value)
        encoded = {"t": "map", "v": [[encode(key, seen), encode(item, seen)] for key, item in items]}
        encoded["cls"] = type(value).__name__
        return encoded
    if hasattr(value, "hour"):
        return {"t": "datetime", "v": str(value)}
    if hasattr(value, "year"):
        return {"t": "date", "v": str(value)}
    raise TypeError(f"unencodable {value!r}")


def source_class(error):
    """The class name the reference's callers branch on (skip vs abort)."""
    return type(error).__name__


index = json.loads((root / "cases" / "index.json").read_text())
rows = []
for case in index:
    label = case["label"]
    dialect = case["dialect"]
    path = root / case["dir"] / "case.yaml"
    row = {"label": label, "dialect": dialect}
    try:
        if dialect == "pyyaml":
            text = path.read_text(encoding="utf-8")
            value = yaml.safe_load(text)
        else:
            value = ruamel.yaml.YAML(typ="safe").load(path)
    except BaseException as error:  # noqa: BLE001 - the receipt is the class
        row["ok"] = False
        row["error_class"] = source_class(error)
        row["error"] = [ord(character) for character in str(error).split("\n")[0]]
    else:
        row["ok"] = True
        row["document"] = encode(value)
    # Assert intended new Source causes outside the product exception handler,
    # so a harness premise failure cannot masquerade as a product refusal.
    if label.startswith(("parent-date-set", "parent-naive-set", "parent-aware-set")):
        assert row["ok"] and isinstance(value["v"], set)
        assert len(value["v"]) == (4 if label.startswith("parent-date-set") else 3)
    elif label == "parent-pairs-as-key":
        assert not row["ok"] and row["error_class"] == "ConstructorError"
    elif label == "parent-pairs-as-key-12":
        assert row["ok"] and list(value) == [(("a", "b"),)]
    elif label == "parent-pairs-set-key-12":
        assert row["ok"] and value == {(("a", "b"),), "x"}
    elif label == "parent-pairs-unhashable-value-12":
        assert not row["ok"] and row["error_class"] == "TypeError"
        assert row["error"] == list(map(ord, "unhashable type: 'list'"))
    rows.append(row)

json.dump({"rows": rows}, sys.stdout, ensure_ascii=True)
