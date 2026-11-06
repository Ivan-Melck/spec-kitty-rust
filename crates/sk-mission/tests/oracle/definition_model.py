"""Actual pinned-Source receipts for Mission definition validation.

Reads `<root>/cases/index.json` (a list of {"label", "dir"}) written by the Rust
test, loads each `mission.yaml` exactly as Source does, and prints one JSON row
per case: the PyYAML-decoded document (a declared transport encoding, so the
Native model is proven against the same decoded *Python* values), plus Source's
own accept/reject verdict, rendered message, warnings, and config dump.
"""
import json, os, sys, warnings
from pathlib import Path

import yaml
from specify_cli.mission import Mission

root = Path(sys.argv[1])


def encode(value):
    if value is None:
        return {"t": "none"}
    if isinstance(value, bool):
        return {"t": "bool", "v": value}
    if isinstance(value, int):
        return {"t": "int", "v": str(value)}
    if isinstance(value, float):
        return {"t": "float", "v": value}
    if isinstance(value, str):
        return {"t": "str", "v": [ord(character) for character in value]}
    if isinstance(value, (bytes, bytearray)):
        return {"t": "bytes", "v": list(value)}
    if isinstance(value, set):
        return {"t": "set", "v": [encode(item) for item in sorted(value, key=repr)]}
    if isinstance(value, (list, tuple)):
        return {"t": "seq", "v": [encode(item) for item in value]}
    if isinstance(value, dict):
        return {"t": "map", "v": [[encode(key), encode(item)] for key, item in value.items()]}
    return {"t": "date" if hasattr(value, "year") and not hasattr(value, "hour") else "datetime",
            "v": str(value)}


def units(value):
    """Transport every authored string as code points (JSON cannot carry a lone
    surrogate, and a lossy encoding would defeat the exactness this proves)."""
    if isinstance(value, str):
        return {"u": [ord(character) for character in value]}
    if isinstance(value, dict):
        return {key: units(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [units(item) for item in value]
    return value


def dump_config(config):
    payload = config.model_dump()
    return {
        "name": payload["name"],
        "description": payload["description"],
        "version": payload["version"],
        "domain": payload["domain"],
        "workflow": {"phases": [{"name": p["name"], "description": p["description"]}
                               for p in payload["workflow"]["phases"]]},
        "artifacts": {"required": list(payload["artifacts"]["required"]),
                      "optional": list(payload["artifacts"]["optional"])},
        "paths": [[key, value] for key, value in payload["paths"].items()],
        "validation": {"checks": list(payload["validation"]["checks"]),
                       "custom_validators": payload["validation"]["custom_validators"]},
        "mcp_tools": None if payload["mcp_tools"] is None else {
            "required": list(payload["mcp_tools"]["required"]),
            "recommended": list(payload["mcp_tools"]["recommended"]),
            "optional": list(payload["mcp_tools"]["optional"]),
        },
        "agent_context": payload["agent_context"],
        "task_metadata": None if payload["task_metadata"] is None else {
            "required": list(payload["task_metadata"]["required"]),
            "optional": list(payload["task_metadata"]["optional"]),
        },
        "commands": None if payload["commands"] is None else [
            [key, {"prompt": value["prompt"]}] for key, value in payload["commands"].items()],
        "task_types": None if payload["task_types"] is None else [
            [key, {"agent_role": value["agent_role"], "description": value["description"]}]
            for key, value in payload["task_types"].items()],
    }


rows = []
index = json.loads((root / "cases" / "index.json").read_text())
for case in index:
    label = case["label"]
    directory = root / case["dir"]
    text = (directory / "mission.yaml").read_text(encoding="utf-8")
    row = {"label": label}
    try:
        raw = yaml.safe_load(text)
        row["parsed_ok"] = True
        row["document"] = encode(raw)
    except yaml.YAMLError as error:
        row["parsed_ok"] = False
        row["parse_error"] = units(str(error))
        rows.append(row)
        continue
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        try:
            mission = Mission(directory)
        except Exception as error:  # noqa: BLE001 - exact Source verdict transport
            row["ok"] = False
            row["error_type"] = units(type(error).__name__)
            row["error"] = units(str(error))
        else:
            row["ok"] = True
            row["config"] = units(dump_config(mission.config))
        row["warnings"] = units([str(item.message) for item in caught])
    rows.append(row)

json.dump({"rows": rows}, sys.stdout, ensure_ascii=True)
