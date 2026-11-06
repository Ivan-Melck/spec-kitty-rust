"""Actual pinned-Source receipts for the graph-backed Mission definition loader.

This child runs FIRST, against the real PyYAML `safe_load` + `specify_cli.Mission`,
in an asserted temporary CWD/HOME/all-seven-XDG/TMPDIR sandbox. For every case it
asserts the **intended cause** before any Native leg exists, then records Source's
own verdict and, for the cyclic cases, the exact `str()`/`repr()` Source prints.

The consumed facts:
  * a cycle in an IGNORED compatibility field (`states`, `transitions`) is
    accepted by Source's model;
  * a cycle in a REQUIRED field (`name`) is refused in the MODEL phase with
    "Input should be a valid string";
  * a shared alias is one object (`is`), not a copy.
"""

import json
import os
import sys
import warnings
from pathlib import Path

root = Path(sys.argv[1])
repo = Path(sys.argv[2])

# The asserted sandbox: every root must be this temp tree, never the operator's.
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo_src = os.environ["PYTHONPATH"]
assert repo.is_absolute() and Path(repo_src) == repo / "src", repo_src
packs = os.environ["SPEC_KITTY_PACKS_ROOT"]
assert Path(packs) == repo / "packs", packs
roots = {
    "HOME": "home",
    "XDG_CONFIG_HOME": "config",
    "XDG_CACHE_HOME": "cache",
    "XDG_DATA_HOME": "data",
    "XDG_STATE_HOME": "state",
    "XDG_RUNTIME_DIR": "runtime",
    "XDG_CONFIG_DIRS": "config-dirs",
    "XDG_DATA_DIRS": "data-dirs",
    "TMPDIR": "tmp",
    "TMP": "tmp",
    "TEMP": "tmp",
}
for key, name in roots.items():
    assert os.environ.get(key) == str(root / name), (key, os.environ.get(key))
assert Path.cwd() == root, Path.cwd()

import yaml  # noqa: E402 - assert actual roots before product/dependency import
from specify_cli.mission import Mission  # noqa: E402

env_receipt = {
    "cpython": list(sys.version_info[:3]),
    "implementation": sys.implementation.name,
    "cwd": str(Path.cwd()),
    "home": os.environ["HOME"],
    "xdg": {
        key: os.environ[key]
        for key in (
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
            "XDG_CONFIG_DIRS",
            "XDG_DATA_DIRS",
            "TMPDIR",
            "TMP",
            "TEMP",
        )
    },
    "pythonpath": os.environ["PYTHONPATH"],
    "packs": os.environ["SPEC_KITTY_PACKS_ROOT"],
}


def units(text):
    """Transport a str as exact code points (JSON cannot carry a lone surrogate)."""
    return {"u": [ord(character) for character in text]}


rows = []
index = json.loads((root / "cases" / "index.json").read_text(encoding="utf-8"))
for case in index:
    label = case["label"]
    kind = case["kind"]
    directory = root / case["dir"]
    text = (directory / "mission.yaml").read_text(encoding="utf-8")
    row = {"label": label, "kind": kind}

    raw = yaml.safe_load(text)
    row["parsed_ok"] = True

    # Assert the INTENDED CAUSE right here, in the Source process, before any
    # Native comparison can be written.
    observed = {}
    if kind == "ignored_list_cycle":
        value = raw["states"]
        assert isinstance(value, list), type(value)
        assert value[0] is value, "the ignored field must be a real self-cycle"
        assert str(value) == "[[...]]", str(value)
        assert repr(value) == "[[...]]", repr(value)
        observed = {"type": "list", "str": units(str(value)), "self_cycle": True}
    elif kind == "ignored_map_cycle":
        value = raw["transitions"]
        assert isinstance(value, dict), type(value)
        assert value["k"] is value, "the ignored field must be a real self-cycle"
        assert str(value) == "{'k': {...}}", str(value)
        observed = {"type": "dict", "str": units(str(value)), "self_cycle": True}
    elif kind == "required_name_cycle":
        value = raw["name"]
        assert isinstance(value, list), type(value)
        assert value[0] is value, "the required field must be a real self-cycle"
        assert str(value) == "[[...]]", str(value)
        observed = {"type": "list", "str": units(str(value)), "self_cycle": True}
    elif kind == "alias_shared_string":
        assert raw["name"] is raw["description"], "an alias must be one object"
        assert raw["name"] == "N", raw["name"]
        observed = {"type": "str", "same_object": True, "value": raw["name"]}
    elif kind == "ignored_cycle_missing_required":
        value = raw["states"]
        assert value[0] is value
        assert "artifacts" not in raw
        observed = {"type": "list", "str": units(str(value)), "self_cycle": True}
    else:
        raise AssertionError("unknown case kind " + kind)
    row["observed"] = observed

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        try:
            Mission(directory)
        except Exception as error:  # noqa: BLE001 - exact Source verdict transport
            row["ok"] = False
            row["error_type"] = type(error).__name__
            row["error"] = units(str(error))
        else:
            row["ok"] = True
        row["warnings"] = [units(str(item.message)) for item in caught]
    rows.append(row)

json.dump({"env": env_receipt, "rows": rows}, sys.stdout, ensure_ascii=True)
