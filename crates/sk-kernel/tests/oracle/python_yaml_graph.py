"""Source child for the canonical YAML object-graph oracle.

Not a runtime adapter: it runs under the pinned CPython 3.11.15 pair
(`PyYAML` 6.0.3 `safe_load`, `ruamel.yaml` 0.19.1 safe loader) inside the
asserted temporary sandbox the parent created, and prints one JSON receipt per
case. The parent asserts the intended *cause* on these receipts **before** the
Native leg runs, so a Native success alone never stands as Source evidence.

Each receipt records the preconditions (interpreter, CWD, HOME, all seven XDG
roots, PYTHONPATH, packs root, hash seed) as `env`, so a receipt cannot be
detached from the environment that produced it.
"""

import json
import math
import os
import sys
from pathlib import Path

# The exact sandbox layout the parent creates (one directory per variable).
ROOT_DIRS = {
    "HOME": "home",
    "XDG_CONFIG_HOME": "xdg_config_home",
    "XDG_CACHE_HOME": "xdg_cache_home",
    "XDG_DATA_HOME": "xdg_data_home",
    "XDG_STATE_HOME": "xdg_state_home",
    "XDG_RUNTIME_DIR": "xdg_runtime_dir",
    "XDG_CONFIG_DIRS": "xdg_config_dirs",
    "XDG_DATA_DIRS": "xdg_data_dirs",
    "TMPDIR": "tmp",
    "TMP": "tmp",
    "TEMP": "tmp",
}
ROOTS = list(ROOT_DIRS)


def env_receipt():
    root = Path(os.environ["PROBE_ROOT"])
    repo = Path(os.environ["PROBE_REPO"])
    assert sys.version_info[:3] == (3, 11, 15), sys.version_info
    assert Path.cwd() == root, Path.cwd()
    for key, name in ROOT_DIRS.items():
        assert os.environ.get(key) == str(root / name), key
    assert os.environ["PYTHONPATH"] == str(repo / "src")
    assert os.environ["SPEC_KITTY_PACKS_ROOT"] == str(repo / "packs")
    assert os.environ["PYTHONHASHSEED"] == "0"
    return {
        "version": list(sys.version_info[:3]),
        "cwd": str(Path.cwd()),
        "home": os.environ["HOME"],
        "xdg": {key: os.environ.get(key) for key in ROOTS if key.startswith("XDG_")},
        "tmp": {key: os.environ.get(key) for key in ("TMPDIR", "TMP", "TEMP")},
        "pythonpath": os.environ["PYTHONPATH"],
        "packs": os.environ["SPEC_KITTY_PACKS_ROOT"],
        "seed": os.environ["PYTHONHASHSEED"],
    }


def load(dialect, text):
    if dialect == "pyyaml":
        import yaml

        return yaml.safe_load(text)
    from ruamel.yaml import YAML

    return YAML(typ="safe").load(text)


def describe(value):
    """Python's own `str()`/`repr()` of the loaded object."""
    return repr(value)


def receipt(case, env):
    label, dialect, text = case
    row = {"label": label, "dialect": dialect, "env": env}
    try:
        value = load(dialect, text)
    except BaseException as error:  # noqa: BLE001 - the class is the observation
        row["outcome"] = "error"
        row["class"] = type(error).__name__
        row["message"] = str(error)
        return row
    row["outcome"] = "ok"
    row["repr"] = describe(value)
    row["len"] = len(value) if hasattr(value, "__len__") else None
    if label.startswith("cycle"):
        anchored = value["a"]
        if isinstance(anchored, dict):
            row["inner_is_self"] = anchored["k"] is anchored
        else:
            row["inner_is_self"] = anchored[0] is anchored
    if label.startswith("alias-shared"):
        row["alias_is_same"] = value["a"] is value["b"]
    if label.startswith("nan") or label.startswith("mixed-nan"):
        keys = list(value)
        row["key_count"] = len(keys)
        row["first_is_second"] = keys[0] is keys[1] if len(keys) > 1 else None
        row["hashes"] = [hash(key) for key in keys]
    if label.startswith("hash-facts"):
        from datetime import date, datetime, timezone

        import yaml

        row["hash_none"] = hash(None)
        row["hash_loader_nan"] = hash(yaml.SafeLoader.nan_value)
        row["none_is_nan"] = None is yaml.SafeLoader.nan_value
        row["hash_date"] = hash(date(2026, 11, 5))
        row["hash_datetime"] = hash(datetime(2026, 11, 5))
        row["hash_aware"] = hash(datetime(2026, 11, 5, tzinfo=timezone.utc))
        row["hash_str_a"] = hash("a")
        row["hash_bytes_a"] = hash(b"a")
        row["hash_int_one"] = hash(1)
        row["hash_float_one"] = hash(1.0)
        row["hash_tuple_a"] = hash(("a",))
        row["math_nan_is_math_nan"] = math.nan is math.nan
    return row


def main():
    env = env_receipt()
    cases = json.loads(Path(os.environ["PROBE_CASES"]).read_text())
    rows = [receipt((case["label"], case["dialect"], case["text"]), env) for case in cases]
    Path(os.environ["PROBE_OUT"]).write_text(json.dumps({"rows": rows}, ensure_ascii=False))
    print(json.dumps({"rows": len(rows), "version": env["version"]}))


if __name__ == "__main__":
    main()
