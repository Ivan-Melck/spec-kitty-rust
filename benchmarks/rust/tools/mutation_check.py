#!/usr/bin/env python3
"""Prove equivalence for mutating benchmark workloads without timing them.

Usage::

    PYTHONPATH=src .venv-rust-oracle/bin/python \
      benchmarks/rust/tools/mutation_check.py \
      --rust-binary target/release/spec-kitty \
      --out /tmp/spkbench-mutation

Each side runs in its own fresh clone of the pristine fixture. The tool compares
the normalized result document, the post-state delta, and the pristine fixture
afterwards. It never measures a ratio: the frozen runner still needs the
additive hook described in ``benchmarks/rust/ADAPTER-REPORT.md`` before a
mutation workload may be timed.

Exit codes: 0 all selected workloads equivalent, 1 a workload failed, 2 setup
error (missing binary/interpreter/fixture).
"""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST_BENCH = HERE.parent
REPO_ROOT = RUST_BENCH.parents[1]
sys.path.insert(0, str(RUST_BENCH))

from spkbench.execute import Executor  # noqa: E402
from spkbench.fixtures import FixtureBuilder, verify_manifest  # noqa: E402
from spkbench.mutation import mutation_summary, run_mutation_pair  # noqa: E402
from spkbench.workloads import build_workloads  # noqa: E402


def _default_oracle() -> Path | None:
    env = os.environ.get("SPKBENCH_ORACLE_PYTHON")
    if env:
        return Path(env)
    candidate = REPO_ROOT / ".venv-rust-oracle" / "bin" / "python"
    return candidate if candidate.exists() else None


def _sha256(path: Path) -> str | None:
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None


def _git_head(repo: Path) -> str | None:
    import subprocess

    try:
        return subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "HEAD"],
            capture_output=True,
            check=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust-binary", default=str(REPO_ROOT / "target" / "release" / "spec-kitty"))
    parser.add_argument("--oracle-python", default=None)
    parser.add_argument("--repo", default=str(REPO_ROOT))
    parser.add_argument("--packs-root", default=None)
    parser.add_argument("--out", default=None)
    parser.add_argument("--seed", type=int, default=20261102)
    parser.add_argument("--invocations", type=int, default=2)
    parser.add_argument("--workload", action="append", default=[], help="fnmatch pattern; repeatable")
    parser.add_argument("--list", action="store_true", help="list mutation workloads and exit")
    parser.add_argument("--keep-clones", action="store_true")
    args = parser.parse_args()

    binary = Path(args.rust_binary).resolve()
    oracle = Path(args.oracle_python).resolve() if args.oracle_python else _default_oracle()
    repo = Path(args.repo).resolve()
    packs_root = Path(args.packs_root).resolve() if args.packs_root else repo / "packs"

    definitions = build_workloads(
        builder=FixtureBuilder(Path("/tmp/unused-spkbench-fixtures"), seed=args.seed, dry_run=True),
        rust_binary=binary,
        oracle_python=oracle or Path("/usr/bin/python3"),
        adapters_dir=RUST_BENCH / "adapters",
        include_micro=False,
    )
    mutations = [item for item in definitions if item.mutation is not None]
    if args.workload:
        mutations = [
            item
            for item in mutations
            if any(fnmatch.fnmatch(item.id, pattern) for pattern in args.workload)
        ]
        missing = [
            pattern
            for pattern in args.workload
            if not any(fnmatch.fnmatch(item.id, pattern) for item in definitions if item.mutation)
        ]
        if missing:
            raise SystemExit(f"unknown mutation workload pattern(s): {', '.join(missing)}")

    if args.list:
        print(json.dumps([mutation_summary(item) for item in mutations], indent=2))
        return 0
    if not mutations:
        print("no mutation workloads selected", file=sys.stderr)
        return 2
    if not binary.exists():
        print(f"rust binary not found: {binary}", file=sys.stderr)
        return 2
    if oracle is None or not oracle.exists():
        print("lock-exact oracle interpreter not found (use --oracle-python)", file=sys.stderr)
        return 2

    os.environ["PYTHONPATH"] = str(repo / "src")
    if packs_root.exists():
        os.environ["SPEC_KITTY_PACKS_ROOT"] = str(packs_root)
    os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")

    out_dir = Path(args.out).resolve() if args.out else Path(tempfile.mkdtemp(prefix="spkbench-mutation-"))
    out_dir.mkdir(parents=True, exist_ok=True)
    fixtures_dir = out_dir / "fixtures"
    executor = Executor(run_dir=out_dir, cwd=out_dir)

    builder = FixtureBuilder(fixtures_dir, seed=args.seed)
    fixture_records: dict[str, dict] = {}
    results: list[dict] = []
    ok = True
    for workload in mutations:
        if workload.fixture is None:
            print(f"{workload.id}: mutation workload has no fixture", file=sys.stderr)
            ok = False
            continue
        fixture = builder.get(workload.fixture)
        fixture_records[workload.fixture] = fixture.as_dict()
        result = run_mutation_pair(
            workload=workload,
            fixture=fixture.path,
            executor=executor,
            invocations=args.invocations,
            keep_clones=args.keep_clones,
        )
        integrity = verify_manifest(fixture)
        payload = result.as_dict()
        payload["fixture_integrity"] = integrity
        payload["ignored_artifacts_differ"] = _ignored_artifacts_differ(payload)
        results.append(payload)
        ok = ok and result.equivalent and integrity["unchanged"]
        print(
            f"{workload.id}: equivalent={result.equivalent} "
            f"post_state_equal={result.post_state_equal} "
            f"master_unchanged={result.master_unchanged} "
            f"ignored_artifacts_differ={payload['ignored_artifacts_differ']}",
            file=sys.stderr,
        )
        for problem in result.problems:
            print(f"  - {problem}", file=sys.stderr)
        if result.equivalence_diff:
            print(f"  - diff: {result.equivalence_diff}", file=sys.stderr)

    report = {
        "schema": "spkbench.mutation-check.v1",
        "repo": str(repo),
        "git_head": _git_head(repo),
        "rust_binary": str(binary),
        "rust_binary_sha256": _sha256(binary),
        "oracle_python": str(oracle),
        "packs_root": str(packs_root) if packs_root.exists() else None,
        "seed": args.seed,
        "invocations": args.invocations,
        "ok": ok,
        "note": (
            "Equivalence/post-state proof only. No timing is recorded: mutation workloads stay "
            "pending until the runner clone hook lands (see ADAPTER-REPORT.md)."
        ),
        "fixtures": fixture_records,
        "results": results,
    }
    report_path = out_dir / "mutation-report.json"
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2, sort_keys=True))
    print(f"report: {report_path}", file=sys.stderr)
    return 0 if ok else 1


def _ignored_artifacts_differ(payload: dict) -> bool:
    artifacts: dict[str, list[dict]] = {}
    for observation in payload.get("observations", []):
        if observation.get("invocation") != 0:
            continue
        artifacts.setdefault(observation["side"], []).append(observation.get("ignored_artifacts") or {})
    if set(artifacts) != {"rust", "oracle"}:
        return False
    return artifacts["rust"] != artifacts["oracle"]


if __name__ == "__main__":
    raise SystemExit(main())
