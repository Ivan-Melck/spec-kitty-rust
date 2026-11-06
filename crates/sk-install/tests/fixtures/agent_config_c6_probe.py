"""C6 Source-first: the `AgentConfig.auto_commit` matrix (pinned `core/agent_config`)."""
import json, os, sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
assert str(root).startswith("/tmp/sk-c6-oracle."), root
for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[key]).parent == root, (key, os.environ[key])
assert Path.cwd() == root / "work", Path.cwd()

from specify_cli.core.agent_config import load_agent_config, get_auto_commit_default  # noqa: E402

CASES = [
    ("no_file", None),
    ("agents_false", "agents:\n  available:\n    - claude\n  auto_commit: false\n"),
    ("agents_absent_setting_top_false", "agents:\n  available:\n    - claude\nauto_commit: false\n"),
    ("agents_null_setting_top_false", "agents:\n  available:\n    - claude\n  auto_commit: null\nauto_commit: false\n"),
    ("agents_null_setting_top_true", "agents:\n  available:\n    - claude\n  auto_commit: null\nauto_commit: true\n"),
    ("agents_string_false", 'agents:\n  available:\n    - claude\n  auto_commit: "false"\n'),
    ("agents_int_one", "agents:\n  available:\n    - claude\n  auto_commit: 1\n"),
    ("agents_int_zero", "agents:\n  available:\n    - claude\n  auto_commit: 0\n"),
    ("agents_null_tools_false", "agents: null\ntools:\n  available:\n    - claude\n  auto_commit: false\n"),
    ("agents_empty_tools_false", "agents: {}\ntools:\n  available:\n    - claude\n  auto_commit: false\n"),
    ("agents_setting_no_available_tools", "agents:\n  auto_commit: false\ntools:\n  available:\n    - claude\n  auto_commit: true\n"),
    ("tools_scalar_available", "tools:\n  available: claude\n  auto_commit: false\n"),
    ("top_only_false", "auto_commit: false\n"),
    ("agents_available_null", "agents:\n  available: null\n"),
    ("agents_sequence", "agents: []\n"),
    ("top_scalar", "just-a-string\n"),
    ("empty_file", "{}\n"),
    ("unknown_agent", "agents:\n  available:\n    - not-an-agent\n"),
    ("agents_wins_over_tools", "agents:\n  available:\n    - claude\n  auto_commit: false\ntools:\n  available:\n    - claude\n  auto_commit: true\n"),
    ("agents_empty_list", "agents:\n  available: []\n"),
    ("agents_available_int", "agents:\n  available: 3\n"),
    ("legacy_tools_true", "tools:\n  available:\n    - claude\n  auto_commit: true\n"),
]

out = {}
for name, text in CASES:
    proj = root / "cfg" / name
    (proj / ".kittify").mkdir(parents=True)
    if text is not None:
        (proj / ".kittify/config.yaml").write_text(text, encoding="utf-8")
    try:
        config = load_agent_config(proj)
        out[name] = {
            "outcome": "ok",
            "available": config.available,
            "auto_commit": config.auto_commit,
            "explicit_agent_list": config.explicit_agent_list,
            "default_auto_commit": bool(get_auto_commit_default(proj)),
        }
    except Exception as exc:  # noqa: BLE001 - the refusal is contract data
        out[name] = {"outcome": "error", "error_type": type(exc).__name__, "error": str(exc)}

for name in ("agents_null_tools_false", "agents_available_null", "agents_string_false"):
    assert out[name]["outcome"] == ("error" if name == "agents_available_null" else "ok"), out[name]
assert out["agents_string_false"]["auto_commit"] is True, out["agents_string_false"]
assert out["agents_null_setting_top_true"]["auto_commit"] is True, out["agents_null_setting_top_true"]
assert out["agents_null_setting_top_false"]["auto_commit"] is False, out["agents_null_setting_top_false"]
assert out["agents_null_tools_false"]["available"] == ["claude"], out["agents_null_tools_false"]
assert out["top_only_false"]["auto_commit"] is False, out["top_only_false"]
assert out["no_file"]["auto_commit"] is True and out["no_file"]["explicit_agent_list"] is False
assert out["agents_wins_over_tools"]["auto_commit"] is False
assert out["tools_scalar_available"]["available"] == ["claude"]

record = {"assertions": "all passed", "probe": "c6-agent-config", "python": sys.version.split()[0], "cases": out}
path = root / "agent-config-record-c6.json"
path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print("record=", path)
print(json.dumps(record, indent=2, sort_keys=True))
