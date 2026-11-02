#!/usr/bin/env python3
"""Spec Kitty Rust-vs-Python benchmark harness.

Usage:
    python3 benchmarks/rust/bench.py list
    python3 benchmarks/rust/bench.py oracle-provision
    python3 benchmarks/rust/bench.py run --oracle-python /path/to/venv/bin/python
    python3 benchmarks/rust/bench.py gate benchmarks/rust/evidence/<run>/report.json
    python3 benchmarks/rust/bench.py selftest

The harness is development tooling. It never changes the installed Spec Kitty
package, real user state, or the repository's ``src`` tree.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

from spkbench import env as envmod  # noqa: E402
from spkbench import identity as identitymod  # noqa: E402
from spkbench.fixtures import FixtureBuilder, verify_manifest  # noqa: E402
from spkbench.gate import GateOptions, evaluate_gate  # noqa: E402
from spkbench.report import write_reports  # noqa: E402
from spkbench.runner import run_benchmark, summarise_console  # noqa: E402
from spkbench.execute import Executor  # noqa: E402
from spkbench.selftest import run_selftest  # noqa: E402
from spkbench.workloads import build_workloads  # noqa: E402

DEFAULT_BINARY = REPO_ROOT / "target" / "release" / "spec-kitty-rust"
DEFAULT_EVIDENCE = HERE / "evidence"


def _utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def _find_oracle_python(explicit: str | None) -> Path:
    candidates = []
    if explicit:
        candidates.append(Path(explicit))
    if os.environ.get("SPKBENCH_ORACLE_PYTHON"):
        candidates.append(Path(os.environ["SPKBENCH_ORACLE_PYTHON"]))
    candidates.extend(
        [
            Path("/tmp/spk-bench-oracle/venv/bin/python"),
            Path("/tmp/spk-bench-lockenv/.venv/bin/python"),
        ]
    )
    for candidate in candidates:
        if candidate.exists():
            # Do not resolve the symlink: a venv's interpreter is selected by
            # its path (pyvenv.cfg lives beside it). Resolving loses the venv.
            return candidate.absolute()
    raise SystemExit(
        "no lock-exact oracle interpreter found.\n"
        "Provision one with:\n"
        "  python3 benchmarks/rust/bench.py oracle-provision\n"
        "then pass --oracle-python <dir>/venv/bin/python (or set SPKBENCH_ORACLE_PYTHON)."
    )


def _find_packs_root(explicit: str | None) -> Path | None:
    if explicit:
        return Path(explicit).resolve()
    if os.environ.get("SPEC_KITTY_PACKS_ROOT"):
        return Path(os.environ["SPEC_KITTY_PACKS_ROOT"]).resolve()
    import glob

    patterns = [
        str(Path.home() / ".local/share/uv/tools/spec-kitty-cli/lib/python*/site-packages/packs"),
        str(Path.home() / ".local/share/uv/tools/spec-kitty-cli/Lib/site-packages/packs"),
    ]
    for pattern in patterns:
        matches = sorted(glob.glob(pattern))
        if matches:
            return Path(matches[-1]).resolve()
    return None


def _filter_workloads(workloads, patterns: list[str]):
    if not patterns:
        return workloads
    selected = []
    for workload in workloads:
        if any(fnmatch.fnmatch(workload.id, pattern) for pattern in patterns):
            selected.append(workload)
    missing = [
        pattern
        for pattern in patterns
        if not any(fnmatch.fnmatch(workload.id, pattern) for workload in workloads)
    ]
    if missing:
        raise SystemExit(f"unknown workload pattern(s): {', '.join(missing)}")
    return selected


def command_list(args: argparse.Namespace) -> int:
    workloads = build_workloads(
        builder=FixtureBuilder(Path("/tmp/unused-spkbench-fixtures"), seed=0, dry_run=True),
        rust_binary=DEFAULT_BINARY,
        oracle_python=Path("/usr/bin/python3"),
        adapters_dir=HERE / "adapters",
        include_micro=True,
    )
    rows = []
    for workload in workloads:
        rows.append(
            {
                "id": workload.id,
                "class": workload.klass,
                "equivalence": workload.equivalence,
                "required": workload.required,
                "fixture": workload.fixture,
                "gate_eligible": workload.counts_for_gate(),
                "pending_reason": workload.pending_reason,
            }
        )
    print(json.dumps(rows, indent=2))
    return 0


def command_oracle_provision(args: argparse.Namespace) -> int:
    target = Path(args.dir).resolve()
    venv = target / "venv"
    if target.exists() and args.force:
        shutil.rmtree(target)
    target.mkdir(parents=True, exist_ok=True)
    for name in ("pyproject.toml", "uv.lock"):
        shutil.copy2(REPO_ROOT / name, target / name)
    uv = shutil.which("uv")
    if not uv:
        raise SystemExit("uv is required to provision the lock-exact oracle environment")
    command = [
        uv,
        "sync",
        "--frozen",
        "--no-dev",
        "--no-install-project",
        "--project",
        str(target),
        "--python",
        "3.11",
    ]
    env = dict(os.environ)
    env.setdefault("UV_PROJECT_ENVIRONMENT", str(venv))
    print("+ " + " ".join(command))
    proc = subprocess.run(command, cwd=str(target), env=env, check=False)
    if proc.returncode != 0:
        raise SystemExit(f"uv sync failed with exit {proc.returncode}")
    python = venv / "bin" / "python"
    if not python.exists():
        python = venv / "Scripts" / "python.exe"
    deps = identitymod.dependency_identity(REPO_ROOT / "uv.lock", python)
    print(
        json.dumps(
            {
                "oracle_python": str(python),
                "dependency_identity": deps["status"],
                "mismatches": deps["mismatches"],
                "installed_not_in_lock": deps["installed_not_in_lock"],
            },
            indent=2,
        )
    )
    if deps["status"] != "exact":
        return 1
    return 0


def command_identity(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    binary = Path(args.rust_binary).resolve()
    oracle_python = _find_oracle_python(args.oracle_python)
    packs_root = _find_packs_root(args.packs_root)
    identity = identitymod.collect(
        repo=repo, binary=binary, oracle_python=oracle_python, packs_root=packs_root
    )
    identity["machine"] = envmod.capture_static(
        repo_root=repo, rust_binary=binary, oracle_python=oracle_python
    )
    print(json.dumps(identity, indent=2, sort_keys=True))
    return 0 if identity["status"] == "exact" else 1


def command_selftest(args: argparse.Namespace) -> int:
    report = run_selftest()
    print(json.dumps(report, indent=2))
    return 0 if report["ok"] else 1


def command_gate(args: argparse.Namespace) -> int:
    report = json.loads(Path(args.report).read_text(encoding="utf-8"))
    gate = evaluate_gate(
        report["run"],
        report["identity"],
        GateOptions(
            min_ratio=args.min_ratio,
            max_slowdown_ratio=1.0 / args.max_slowdown,
            min_samples_per_side=args.min_samples,
            max_cv=args.max_cv,
            require_exact_identity=not args.allow_identity_drift,
            allow_noisy=args.allow_noisy,
        ),
    )
    print(json.dumps(gate.as_dict(), indent=2))
    return 0 if gate.verdict == "PASS" else (1 if gate.verdict == "FAIL" else 2)


def command_run(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    binary = Path(args.rust_binary).resolve()
    if not binary.exists():
        raise SystemExit(f"rust binary not found: {binary}")
    oracle_python = _find_oracle_python(args.oracle_python)
    packs_root = _find_packs_root(args.packs_root)
    adapters_dir = HERE / "adapters"

    # Make the oracle source identity explicit and identical for every process.
    os.environ["PYTHONPATH"] = str(repo / "src")
    if packs_root:
        os.environ["SPEC_KITTY_PACKS_ROOT"] = str(packs_root)
    os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")

    identity = identitymod.collect(
        repo=repo, binary=binary, oracle_python=oracle_python, packs_root=packs_root
    )
    identity["machine"] = envmod.capture_static(
        repo_root=repo, rust_binary=binary, oracle_python=oracle_python
    )
    identity["harness"] = {
        "version": "1.0.0",
        "entrypoint": str(Path(__file__).resolve()),
        "argv": sys.argv,
    }

    out_dir = (
        Path(args.out).resolve()
        if args.out
        else (DEFAULT_EVIDENCE / f"{_utc_stamp()}-seed{args.seed}").resolve()
    )
    out_dir.mkdir(parents=True, exist_ok=True)
    fixtures_dir = out_dir / "fixtures"
    empty_cwd = out_dir / "empty-cwd"
    empty_cwd.mkdir(parents=True, exist_ok=True)

    workloads = build_workloads(
        builder=FixtureBuilder(fixtures_dir, seed=args.seed),
        rust_binary=binary,
        oracle_python=oracle_python,
        adapters_dir=adapters_dir,
        include_micro=not args.no_micro,
    )
    workloads = _filter_workloads(workloads, args.workload)

    # Build only fixtures actually referenced by the selected runnable work.
    builder = FixtureBuilder(fixtures_dir, seed=args.seed)
    fixture_paths: dict[str, Path] = {}
    fixture_records: dict[str, dict] = {}
    fixture_start = time.time()
    for name in sorted({w.fixture for w in workloads if w.fixture and w.runnable}):
        fixture = builder.get(name)
        fixture_paths[name] = fixture.path
        fixture_records[name] = fixture.as_dict()
    fixture_seconds = time.time() - fixture_start

    executor = Executor(run_dir=out_dir, cwd=empty_cwd, cpus=args.cpus)

    def progress(phase: str, done: int, total: int, detail: str) -> None:
        if args.quiet:
            return
        print(f"[{phase}] {done}/{total} {detail}", file=sys.stderr)

    run = run_benchmark(
        workloads=workloads,
        executor=executor,
        fixture_paths=fixture_paths,
        empty_cwd=empty_cwd,
        adapters_dir=adapters_dir,
        rust_binary=binary,
        oracle_python=oracle_python,
        reps=args.reps,
        warmups=args.warmups,
        seed=args.seed,
        max_seconds=args.max_seconds,
        floor_reps=args.floor_reps,
        progress=progress,
    )
    fixture_integrity = {name: verify_manifest(builder.get(name)) for name in fixture_paths}
    run["fixture_build_seconds"] = fixture_seconds
    run["fixture_integrity"] = fixture_integrity
    run["fixtures_unchanged"] = all(item["unchanged"] for item in fixture_integrity.values())
    run["cpus"] = args.cpus
    run["oracle_python"] = str(oracle_python)
    run["rust_binary"] = str(binary)
    run["binary_sha256_at_start"] = identity["rust_binary"].get("sha256")
    run["binary_sha256_at_end"] = identitymod.sha256_file(binary) if binary.exists() else None
    run["binary_stable"] = run["binary_sha256_at_start"] == run["binary_sha256_at_end"]
    src_end = identitymod.tree_manifest(
        repo / "src", suffixes=(".py", ".yaml", ".yml", ".json", ".toml")
    )
    run["src_digest_at_start"] = identity["source"]["src_tree"]["digest"]
    run["src_digest_at_end"] = src_end.get("digest")
    run["source_stable"] = run["src_digest_at_start"] == run["src_digest_at_end"]

    gate = evaluate_gate(
        run,
        identity,
        GateOptions(
            min_ratio=args.min_ratio,
            max_slowdown_ratio=1.0 / args.max_slowdown,
            min_samples_per_side=args.min_samples,
            max_cv=args.max_cv,
            require_exact_identity=not args.allow_identity_drift,
            allow_noisy=args.allow_noisy,
        ),
    )
    paths = write_reports(
        out_dir=out_dir,
        run=run,
        identity=identity,
        gate=gate.as_dict(),
        fixtures=fixture_records,
        packs_root=str(packs_root) if packs_root else None,
    )
    if not args.keep_fixtures:
        shutil.rmtree(fixtures_dir, ignore_errors=True)
        paths["fixtures_removed"] = "fixtures deleted; manifests are in report.json/fixtures.json"

    if not args.quiet:
        print("")
        print(summarise_console(run))
        print("")
        print(f"gate: {gate.verdict}")
        for reason in gate.reasons:
            print(f"  - {reason}")
        print("")
        for label, path in paths.items():
            print(f"{label}: {path}")
    bad_statuses = {"equivalence_failed", "nondeterministic", "error", "preflight_failed"}
    diagnostic_ok = not any(w.get("status") in bad_statuses for w in run["workloads"]) and not run["truncated"]
    run["diagnostic_ok"] = diagnostic_ok
    if args.json_stdout:
        print(
            json.dumps(
                {
                    "gate": gate.as_dict(),
                    "diagnostic_ok": diagnostic_ok,
                    "paths": {k: str(v) for k, v in paths.items()},
                }
            )
        )
    if args.diagnostic:
        if not args.quiet:
            print(f"diagnostic (subset) result: {'ok' if diagnostic_ok else 'FAILED'}")
        return 0 if diagnostic_ok else 1
    return 0 if gate.verdict == "PASS" else (1 if gate.verdict == "FAIL" else 2)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    subparsers = parser.add_subparsers(dest="command", required=True)

    list_parser = subparsers.add_parser("list", help="list workloads and their status")
    list_parser.set_defaults(func=command_list)

    provision = subparsers.add_parser(
        "oracle-provision", help="build a lock-exact Python oracle environment with uv"
    )
    provision.add_argument("--dir", default="/tmp/spk-bench-oracle")
    provision.add_argument("--force", action="store_true")
    provision.set_defaults(func=command_oracle_provision)

    identity = subparsers.add_parser("identity", help="capture and validate source/binary/dependency identity")
    identity.add_argument("--repo", default=str(REPO_ROOT))
    identity.add_argument("--rust-binary", default=str(DEFAULT_BINARY))
    identity.add_argument("--oracle-python", default=None)
    identity.add_argument("--packs-root", default=None)
    identity.set_defaults(func=command_identity)

    selftest = subparsers.add_parser("selftest", help="run harness negative tests")
    selftest.set_defaults(func=command_selftest)

    gate = subparsers.add_parser("gate", help="re-evaluate a saved report")
    gate.add_argument("report")
    gate.add_argument("--min-ratio", type=float, default=3.0)
    gate.add_argument("--max-slowdown", type=float, default=1.10, help="allowed regression factor")
    gate.add_argument("--min-samples", type=int, default=5)
    gate.add_argument("--max-cv", type=float, default=0.30)
    gate.add_argument("--allow-identity-drift", action="store_true")
    gate.add_argument("--allow-noisy", action="store_true")
    gate.set_defaults(func=command_gate)

    run = subparsers.add_parser("run", help="run the benchmark")
    run.add_argument("--repo", default=str(REPO_ROOT))
    run.add_argument("--rust-binary", default=str(DEFAULT_BINARY))
    run.add_argument("--oracle-python", default=None)
    run.add_argument("--packs-root", default=None)
    run.add_argument("--out", default=None)
    run.add_argument("--workload", action="append", default=[], help="workload id glob; repeatable")
    run.add_argument("--reps", type=int, default=5)
    run.add_argument("--warmups", type=int, default=1)
    run.add_argument("--seed", type=int, default=20261102)
    run.add_argument("--max-seconds", type=float, default=300.0)
    run.add_argument("--floor-reps", type=int, default=3)
    run.add_argument("--cpus", default=None, help="taskset CPU list (e.g. 2-3)")
    run.add_argument("--no-micro", action="store_true")
    run.add_argument("--min-ratio", type=float, default=3.0)
    run.add_argument("--max-slowdown", type=float, default=1.10)
    run.add_argument("--min-samples", type=int, default=5)
    run.add_argument("--max-cv", type=float, default=0.30)
    run.add_argument("--allow-identity-drift", action="store_true")
    run.add_argument("--allow-noisy", action="store_true")
    run.add_argument("--keep-fixtures", action="store_true", help="keep generated fixture trees for inspection")
    run.add_argument(
        "--diagnostic",
        action="store_true",
        help="exit 0 when all selected pairs are equivalent and measured, even though the full gate is BLOCKED",
    )
    run.add_argument("--quiet", action="store_true")
    run.add_argument("--json-stdout", action="store_true")
    run.set_defaults(func=command_run)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
