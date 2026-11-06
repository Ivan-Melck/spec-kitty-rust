# QUARANTINED: dev-only fixture generator.
#
# This file is NOT a production code path and is not run by `cargo test`. It
# regenerates `crates/sk-run-store/tests/fixtures/next_scenario.json` from the
# pinned upstream Python runtime so the native run store can be differentially
# tested against the reference implementation.
#
# Usage (from the repository root):
#
#   PYTHONPATH=src SPEC_KITTY_PACKS_ROOT=$PWD/packs \
#     .venv-rust-oracle/bin/python \
#     crates/sk-run-store/tests/oracle/generate_next_fixtures.py
#
# The scenario is a `requires_inputs` mission: the first advance issues a
# `DecisionInputRequested` for `input:owner` (no step issued yet), and the
# human answer both records the answer and advances to the step. That exercises
# the cursor shape, the journal shape and the two timestamp formats in one
# transcript.

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TEMPLATE = """\
mission:
  key: demo-io
  name: Demo IO
  version: "1.0"
steps:
  - id: specify
    title: Specify
    description: Write the specification
    prompt: "Write the specification for this mission."
    requires_inputs: [owner]
  - id: plan
    title: Plan
    depends_on: [specify]
    prompt: "Write the plan."
"""

META = {
    "mission_id": "01M6GC7EV7RY46WMHMXYWS6XZM",
    "mid8": "01M6GC7E",
    "mission_slug": "demo-io-01M6GC7E",
    "slug": "demo-io-01M6GC7E",
    "mission_number": None,
    "mission_type": "demo-io",
    "friendly_name": "demo io",
    "target_branch": "main",
    "topology": "lanes",
}

ISO = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})")


def run(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    home = cwd / "home"
    home.mkdir(exist_ok=True)
    env = dict(os.environ, HOME=str(home))
    return subprocess.run(
        [sys.executable, "-m", "specify_cli", *args],
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )


def normalize(value: object, *, run_id: str, template_path: str) -> object:
    if isinstance(value, dict):
        out = {}
        for key, item in value.items():
            if key == "run_id":
                out[key] = "<RUN_ID>"
            elif key == "template_path":
                out[key] = "<TEMPLATE_PATH>"
            elif key in {"timestamp", "answered_at", "requested_at"}:
                out[key] = "<TIMESTAMP>"
            else:
                out[key] = normalize(item, run_id=run_id, template_path=template_path)
        return out
    if isinstance(value, list):
        return [normalize(item, run_id=run_id, template_path=template_path) for item in value]
    if isinstance(value, str):
        text = value.replace(run_id, "<RUN_ID>").replace(template_path, "<TEMPLATE_PATH>")
        return ISO.sub("<TIMESTAMP>", text)
    return value


def snapshot(root: Path) -> tuple[dict, list[dict]]:
    runs = root / ".kittify" / "runtime" / "runs"
    run_dir = next(path for path in runs.iterdir() if path.is_dir())
    state = json.loads((run_dir / "state.json").read_text())
    events = [
        json.loads(line)
        for line in (run_dir / "run.events.jsonl").read_text().splitlines()
        if line.strip()
    ]
    return state, events


def main() -> int:
    root = Path(tempfile.mkdtemp(prefix="sk-run-store-oracle-"))
    try:
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=root, check=True)
        run(root, "init", "--ai", "codex", "--non-interactive")
        mission_dir = root / "kitty-specs" / "demo-io-01M6GC7E"
        (mission_dir / "tasks").mkdir(parents=True, exist_ok=True)
        (root / ".kittify" / "missions" / "demo-io").mkdir(parents=True, exist_ok=True)
        (root / ".kittify" / "missions" / "demo-io" / "mission-runtime.yaml").write_text(TEMPLATE)
        (mission_dir / "meta.json").write_text(json.dumps(META, indent=2, sort_keys=True) + "\n")
        (mission_dir / "spec.md").write_text("# Spec\n")
        subprocess.run(["git", "add", "-A"], cwd=root, check=True)
        subprocess.run(
            ["git", "-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qm", "fixture"],
            cwd=root,
            check=True,
        )

        steps = []
        first = run(root, "next", "--agent", "claude", "--result", "success", "--json")
        state, events = snapshot(root)
        run_id = state["run_id"]
        template_path = state["template_path"]
        steps.append(
            {
                "invocation": ["next", "--agent", "claude", "--result", "success", "--json"],
                "decision": json.loads(first.stdout),
                "state": state,
                "events": events,
            }
        )
        second = run(
            root,
            "next",
            "--agent",
            "claude",
            "--answer",
            "alice",
            "--result",
            "success",
            "--json",
        )
        state, events = snapshot(root)
        steps.append(
            {
                "invocation": [
                    "next",
                    "--agent",
                    "claude",
                    "--answer",
                    "alice",
                    "--result",
                    "success",
                    "--json",
                ],
                "decision": json.loads(second.stdout),
                "state": state,
                "events": events,
            }
        )

        for step in steps:
            step["decision"] = normalize(step["decision"], run_id=run_id, template_path=template_path)
            step["state"] = normalize(step["state"], run_id=run_id, template_path=template_path)
            step["events"] = normalize(step["events"], run_id=run_id, template_path=template_path)

        payload = {
            "provenance": {
                "generator": "crates/sk-run-store/tests/oracle/generate_next_fixtures.py",
                "source_baseline": "9aed3a8012cb5eb22c0128b14d525039f95fea7a",
                "spec_kitty_version": "4.0.0rc8",
                "python": f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}",
                "normalized": ["run_id", "template_path", "timestamp", "answered_at", "requested_at"],
            },
            "template": TEMPLATE,
            "meta": META,
            "steps": steps,
        }
        target = Path(__file__).resolve().parents[1] / "fixtures" / "next_scenario.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        print(f"wrote {target}")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
