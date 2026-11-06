#!/usr/bin/env python3
"""Spec Kitty Rust-vs-Python certification harness.

Usage::

    python3 benchmarks/rust/bench.py selftest
    python3 benchmarks/rust/bench.py registry
    python3 benchmarks/rust/bench.py list
    python3 benchmarks/rust/bench.py fixtures verify
    python3 benchmarks/rust/bench.py oracle-provision
    python3 benchmarks/rust/bench.py build --receipt target/release/build-receipt.json
    python3 benchmarks/rust/bench.py run --oracle-python .venv-rust-oracle/bin/python
    python3 benchmarks/rust/bench.py gate benchmarks/rust/evidence/<run>

Thresholds are frozen in ``docs/rust/inventory/performance-registry.json`` and
shared with ``scripts/rust/check_completion.py``. There are deliberately no
flags to lower them. The harness is development tooling: it never changes the
installed package, real user state, the repository ``src`` tree or a Rust
checkout.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

from spkbench import certify  # noqa: E402
from spkbench import env as envmod  # noqa: E402
from spkbench import identity as identitymod  # noqa: E402
from spkbench.build_receipt import create_receipt, write_receipt  # noqa: E402
from spkbench.execute import Executor  # noqa: E402
from spkbench.fixtures import FixtureBuilder, verify_manifest  # noqa: E402
from spkbench.live import NativeSourceProbe  # noqa: E402
from spkbench.registry import PerformanceRegistry, RegistryError  # noqa: E402
from spkbench.report import write_evidence  # noqa: E402
from spkbench.runner import run_benchmark, summarise_console  # noqa: E402
from spkbench.selftest import run_selftest  # noqa: E402
from spkbench.workloads import build_workloads, gate_workloads  # noqa: E402

DEFAULT_INSPECTION_BINARY = REPO_ROOT / "target" / "release" / "spec-kitty-rust"
DEFAULT_COMPAT_BINARY = REPO_ROOT / "target" / "release" / "spec-kitty"
DEFAULT_EVIDENCE = HERE / "evidence"
DEFAULT_RECEIPT = REPO_ROOT / "target" / "release" / "build-receipt.json"


def _utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def _utc_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _find_oracle_python(explicit: str | None) -> Path:
    candidates = []
    if explicit:
        candidates.append(Path(explicit))
    if os.environ.get("SPKBENCH_ORACLE_PYTHON"):
        candidates.append(Path(os.environ["SPKBENCH_ORACLE_PYTHON"]))
    candidates.append(REPO_ROOT / ".venv-rust-oracle" / "bin" / "python")
    candidates.append(Path("/tmp/spk-bench-oracle/venv/bin/python"))
    for candidate in candidates:
        if candidate.exists():
            return candidate.absolute()
    raise SystemExit(
        "no lock-exact oracle interpreter found.\n"
        "Provision one with: python3 benchmarks/rust/bench.py oracle-provision\n"
        "or pass --oracle-python .venv-rust-oracle/bin/python.\n"
        "An unavailable oracle is a hard error: there is no installed-tool fallback."
    )


def _registry(args: argparse.Namespace) -> PerformanceRegistry:
    try:
        return PerformanceRegistry.load(Path(args.registry) if getattr(args, "registry", None) else None)
    except RegistryError as exc:
        raise SystemExit(f"frozen performance registry error: {exc}") from None


def _compat_binary(inspection: Path) -> Path | None:
    candidate = inspection if inspection.name == "spec-kitty" else inspection.with_name("spec-kitty")
    return candidate if candidate.exists() else None


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


def command_registry(args: argparse.Namespace) -> int:
    registry = _registry(args)
    print(
        json.dumps(
            {
                "registry": registry.identity(),
                "policy": registry.policy,
                "checker": registry.checker,
                "workloads": [
                    {
                        "id": item["id"],
                        "category": item.get("category"),
                        "required": item.get("required"),
                        "gate_eligible": item.get("gate_eligible"),
                        "measurement": item.get("measurement"),
                        "native": (item.get("native") or {}).get("state"),
                        "blocked_by": (item.get("native") or {}).get("blocked_by"),
                    }
                    for item in registry.workloads
                ],
            },
            indent=2,
        )
    )
    return 0


def command_list(args: argparse.Namespace) -> int:
    registry = _registry(args)
    workloads = build_workloads(
        builder=FixtureBuilder(Path("/tmp/unused-spkbench-fixtures"), seed=0, dry_run=True),
        rust_binary=DEFAULT_INSPECTION_BINARY,
        oracle_python=Path("/usr/bin/python3"),
        adapters_dir=HERE / "adapters",
        registry=registry,
        repo_root=Path(args.repo).resolve(),
        include_micro=True,
    )
    rows = [
        {
            "id": workload.id,
            "source": workload.source,
            "class": workload.klass,
            "equivalence": workload.equivalence,
            "required": workload.required,
            "measurement": workload.measurement,
            "fixture": workload.fixture,
            "runnable": workload.runnable,
            "gate_eligible": workload.counts_for_gate(),
            "blocked_reason": workload.runtime_blocker or workload.pending_reason,
        }
        for workload in workloads
    ]
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
        uv, "sync", "--frozen", "--no-dev", "--no-install-project",
        "--project", str(target), "--python", "3.11",
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
    print(json.dumps({"oracle_python": str(python), "dependency_identity": deps["status"]}, indent=2))
    return 0 if deps["status"] == "exact" else 1


def command_identity(args: argparse.Namespace) -> int:
    registry = _registry(args)
    repo = Path(args.repo).resolve()
    binary = Path(args.rust_binary).resolve()
    oracle_python = _find_oracle_python(args.oracle_python)
    packs_root = Path(args.packs_root).resolve() if args.packs_root else repo / "packs"
    identity = identitymod.collect(
        repo=repo,
        binary=binary,
        oracle_python=oracle_python,
        packs_root=packs_root,
        registry=registry,
        compat_binary=_compat_binary(binary),
        baseline_commit=registry.data["frozen_baseline"]["commit"],
        require_clean=not args.allow_dirty,
        build_receipt_path=Path(args.build_receipt) if args.build_receipt else (
            DEFAULT_RECEIPT if DEFAULT_RECEIPT.exists() else None
        ),
    )
    identity["machine"] = envmod.capture_static(
        repo_root=repo, rust_binary=binary, oracle_python=oracle_python
    )
    print(json.dumps(identity, indent=2, sort_keys=True))
    return 0 if identity["status"] == "exact" else 1


def command_build(args: argparse.Namespace) -> int:
    """Run an explicit release build and write a receipt binding source to binary."""
    repo = Path(args.repo).resolve()
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
    if not Path(cargo).exists():
        raise SystemExit("cargo is unavailable")
    command = [cargo, "build", "--release", "-p", "sk-cli", "--locked"]
    if args.target:
        command.extend(["--target", args.target])
    source_before = identitymod.source_worktree_state(repo)
    if source_before["dirty"]:
        raise SystemExit(
            "refusing to build a receipt from a dirty tree: "
            f"{source_before['dirty_count']} paths changed"
        )
    toolchain = identitymod.toolchain_identity(repo)
    if not toolchain["rustc"].get("available"):
        raise SystemExit("rustc is unavailable")
    build_inputs_before = identitymod.build_inputs_manifest(repo)
    started_at = _utc_iso()
    print("+ " + " ".join(command))
    proc = subprocess.run(
        command,
        cwd=str(repo),
        check=False,
        env={
            **os.environ,
            "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2"),
            "CARGO_INCREMENTAL": os.environ.get("CARGO_INCREMENTAL", "0"),
        },
    )
    if proc.returncode != 0:
        raise SystemExit(f"cargo build failed with exit {proc.returncode}")
    finished_at = _utc_iso()
    source_after = identitymod.source_worktree_state(repo)
    build_inputs_after = identitymod.build_inputs_manifest(repo)
    if build_inputs_before["digest"] != build_inputs_after["digest"]:
        raise SystemExit("build inputs changed during the build; receipt refused")
    if source_after["dirty"]:
        raise SystemExit("the working tree became dirty during the build; receipt refused")
    binaries = {
        "spec-kitty": repo / "target" / "release" / "spec-kitty",
        "spec-kitty-rust": repo / "target" / "release" / "spec-kitty-rust",
    }
    receipt = create_receipt(
        repo=repo,
        binaries=binaries,
        command=command,
        source=source_before,
        build_inputs=build_inputs_before,
        toolchain=toolchain,
        started_at=started_at,
        finished_at=finished_at,
        target=args.target,
    )
    path = Path(args.receipt) if args.receipt else DEFAULT_RECEIPT
    write_receipt(path, receipt)
    print(json.dumps({"receipt": str(path), "digest": receipt.get("digest"), "binaries": {
        name: record["sha256"] for name, record in receipt["binaries"].items()
    }}, indent=2))
    return 0


def command_fixtures(args: argparse.Namespace) -> int:
    recipes = list(args.recipe) if args.recipe else _recipe_names()
    work = Path(tempfile.mkdtemp(prefix="spkbench-fixtures-verify-"))
    outcomes: dict[str, dict] = {}
    failures = 0
    try:
        for recipe in recipes:
            digests = []
            file_count = None
            for variant in ("short", "a-much-longer-audit-root-name-xxxxxxxxxxxxxxxx"):
                builder = FixtureBuilder(work / variant / recipe, seed=args.seed)
                fixture = builder.get(recipe)
                record = fixture.as_dict(include_manifest=False)
                digests.append(record["content_digest"])
                file_count = record["file_count"]
                if not verify_manifest(fixture)["unchanged"]:
                    failures += 1
            portable = len(set(digests)) == 1
            if not portable:
                failures += 1
            outcomes[recipe] = {
                "content_digest": digests[0],
                "portable": portable,
                "file_count": file_count,
            }
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print(json.dumps({"seed": args.seed, "fixtures": outcomes, "failures": failures}, indent=2, sort_keys=True))
    return 0 if failures == 0 else 1


def _recipe_names() -> list[str]:
    import spkbench.fixtures as fixtures_module

    return list(fixtures_module.RECIPES)


def command_selftest(args: argparse.Namespace) -> int:
    report = run_selftest()
    print(json.dumps(report, indent=2))
    return 0 if report["ok"] else 1


def command_gate(args: argparse.Namespace) -> int:
    registry = _registry(args)
    repo = Path(args.repo).resolve()
    path = Path(args.report).resolve()
    try:
        bundle = certify.load_bundle(path)
    except (OSError, ValueError) as exc:
        raise SystemExit(f"cannot read evidence bundle: {exc}") from None
    if bundle.get("schema") != certify.SCHEMA:
        print(
            json.dumps(
                {
                    "verdict": "BLOCKED",
                    "blockers": [
                        f"evidence schema {bundle.get('schema')!r} predates the certification bundle; "
                        "historical evidence stays BLOCKED and cannot be re-certified"
                    ],
                    "failures": [],
                },
                indent=2,
            )
        )
        return 2
    artifacts_root = path if path.is_dir() else path.parent
    if args.offline_audit:
        print(
            json.dumps(
                {
                    "verdict": "BLOCKED",
                    "blockers": [
                        "offline audit: live identity and artifact re-verification cannot be skipped"
                    ],
                },
                indent=2,
            )
        )
        return 2
    inspection = Path(args.rust_binary).resolve()
    receipt_path = Path(args.build_receipt) if args.build_receipt else (
        DEFAULT_RECEIPT if DEFAULT_RECEIPT.exists() else None
    )
    compat = _compat_binary(inspection)
    probe = NativeSourceProbe(
        repo,
        receipt_path=receipt_path,
        binaries={
            "spec-kitty-rust": inspection,
            **({"spec-kitty": compat} if compat else {}),
        },
    )
    result = certify.evaluate_bundle(
        bundle,
        registry,
        repo=repo,
        live_probe=probe,
        verify_fixtures=True,
        artifacts_root=artifacts_root,
        require_artifacts=True,
        require_manifest=True,
    )
    print(json.dumps({**result["gate"], "recomputed": result["recomputed"]}, indent=2))
    verdict = result["gate"]["verdict"]
    return 0 if verdict == "PASS" else (1 if verdict == "FAIL" else 2)


def command_run(args: argparse.Namespace) -> int:
    registry = _registry(args)
    repo = Path(args.repo).resolve()
    inspection = Path(args.rust_binary).resolve()
    if not inspection.exists():
        raise SystemExit(f"rust binary not found: {inspection}")
    compat = _compat_binary(inspection)
    oracle_python = _find_oracle_python(args.oracle_python)
    adapters_dir = HERE / "adapters"
    packs_root = Path(args.packs_root).resolve() if args.packs_root else repo / "packs"

    os.environ["PYTHONPATH"] = str(repo / "src")
    os.environ["SPEC_KITTY_PACKS_ROOT"] = str(packs_root)
    os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")

    receipt_path = Path(args.build_receipt) if args.build_receipt else (
        DEFAULT_RECEIPT if DEFAULT_RECEIPT.exists() else None
    )
    identity = identitymod.collect(
        repo=repo,
        binary=inspection,
        oracle_python=oracle_python,
        packs_root=packs_root,
        registry=registry,
        compat_binary=compat,
        baseline_commit=registry.data["frozen_baseline"]["commit"],
        require_clean=True,
        build_receipt_path=receipt_path,
    )
    identity["machine"] = envmod.capture_static(
        repo_root=repo, rust_binary=inspection, oracle_python=oracle_python
    )

    out_dir = (
        Path(args.out).resolve()
        if args.out
        else (DEFAULT_EVIDENCE / f"{_utc_stamp()}-seed{args.seed}").resolve()
    )
    out_dir.mkdir(parents=True, exist_ok=True)
    work_dir = Path(tempfile.mkdtemp(prefix="spkbench-work-"))
    fixtures_dir = work_dir / "fixtures"
    empty_cwd = work_dir / "empty-cwd"
    empty_cwd.mkdir(parents=True, exist_ok=True)

    try:
        workloads = build_workloads(
            builder=FixtureBuilder(fixtures_dir, seed=args.seed),
            rust_binary=inspection,
            oracle_python=oracle_python,
            adapters_dir=adapters_dir,
            registry=registry,
            repo_root=repo,
            include_micro=not args.no_micro,
        )
        workloads = _filter_workloads(workloads, args.workload)
        builder = FixtureBuilder(fixtures_dir, seed=args.seed)
        fixture_paths: dict[str, Path] = {}
        fixture_records: dict[str, dict] = {}
        fixture_start = time.time()
        for name in sorted({workload.fixture for workload in workloads if workload.fixture and workload.runnable}):
            fixture = builder.get(name)
            fixture_paths[name] = fixture.path
            fixture_records[name] = fixture.as_dict(include_manifest=False)
        fixture_seconds = time.time() - fixture_start

        # Portable evidence: the same recipe+seed at a second root must produce
        # the same root-canonicalised digest, otherwise the input cannot be
        # rebuilt at an audit location.
        portable_start = time.time()
        for name in fixture_records:
            twin = FixtureBuilder(fixtures_dir / "portable-twin" / name, seed=args.seed).get(name)
            twin_digest = twin.as_dict(include_manifest=False)["content_digest"]
            fixture_records[name]["portable_verified"] = twin_digest == fixture_records[name]["content_digest"]
            if not fixture_records[name]["portable_verified"]:
                identity["problems"].append(f"fixture {name} is not portable across roots")
                identity["status"] = "drift"
        portable_seconds = time.time() - portable_start

        executor = Executor(run_dir=work_dir, cwd=empty_cwd, cpus=args.cpus)

        def progress(phase: str, done: int, total: int, detail: str) -> None:
            if args.quiet:
                return
            print(f"[{phase}] {done}/{total} {detail}", file=sys.stderr)

        run = run_benchmark(
            workloads=workloads,
            executor=executor,
            fixture_paths=fixture_paths,
            fixtures={name: builder.get(name) for name in fixture_records},
            empty_cwd=empty_cwd,
            adapters_dir=adapters_dir,
            rust_binary=inspection,
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
        run["portable_fixture_seconds"] = portable_seconds
        run["fixture_integrity"] = fixture_integrity
        run["fixtures_unchanged"] = all(item["unchanged"] for item in fixture_integrity.values())
        run["cpus"] = args.cpus
        run["oracle_python"] = str(oracle_python)
        run["rust_binary"] = str(inspection)
        run["compat_binary"] = str(compat) if compat else None

        # Stability over the measurement window, recomputed from the live tree.
        binary_sha_start = identity["rust_binary"].get("sha256")
        binary_sha_end = identitymod.sha256_file(inspection) if inspection.exists() else None
        source_start = identity["cargo"]["build_inputs"]["digest"]
        source_end = identitymod.build_inputs_manifest(repo)["digest"]
        identity["stability"] = {
            "binary_sha256_start": binary_sha_start,
            "binary_sha256_end": binary_sha_end,
            "binary_stable": binary_sha_start == binary_sha_end,
            "source_digest_start": source_start,
            "source_digest_end": source_end,
            "source_stable": source_start == source_end,
        }
        if not identity["stability"]["binary_stable"]:
            identity["problems"].append("the Rust binary changed during the measurement window")
        if not identity["stability"]["source_stable"]:
            identity["problems"].append("Rust build inputs changed during the measurement window")
        if identity["problems"]:
            identity["status"] = "drift"

        harness = {
            "name": "spkbench",
            "version": "2.0.0",
            "entrypoint": str(Path(__file__).resolve()),
            "argv": sys.argv,
            "oracle_python": str(oracle_python),
            "rust_binary": str(inspection),
            "compat_binary": str(compat) if compat else None,
            "registry": registry.identity(),
            "frozen_floors": {
                "minimum_speedup": registry.policy.get("minimum_speedup"),
                "minimum_samples_per_side": registry.policy.get("minimum_samples_per_side"),
                "minimum_strict_workloads": registry.policy.get("minimum_strict_workloads"),
                "max_per_workload_regression_fraction": registry.policy.get(
                    "max_per_workload_regression_fraction"
                ),
            },
        }
        # Re-collect identity from the live tree after the measurement window and
        # verify the bundle against *that*, not against the start snapshot. A
        # mid-run edit, rebuild or oracle change therefore blocks instead of
        # being certified from a stale self-recording.
        end_identity = identitymod.collect(
            repo=repo,
            binary=inspection,
            oracle_python=oracle_python,
            packs_root=packs_root,
            registry=registry,
            compat_binary=compat,
            baseline_commit=registry.data["frozen_baseline"]["commit"],
            require_clean=True,
            build_receipt_path=receipt_path,
        )
        end_identity["machine"] = identity["machine"]
        # The probe below observes the source/binaries/receipt/toolchain/harness
        # at write time. The end snapshot additionally re-checks the oracle,
        # packs and dependency environment, which the probe does not cover.
        if end_identity.get("status") != "exact":
            new_problems = [
                problem
                for problem in (end_identity.get("problems") or [])
                if problem not in (identity.get("problems") or [])
            ]
            if new_problems:
                identity["status"] = "drift"
                identity["problems"] = list(identity.get("problems") or []) + new_problems

        probe = NativeSourceProbe(
            repo,
            receipt_path=receipt_path,
            binaries={
                "spec-kitty-rust": inspection,
                **({"spec-kitty": compat} if compat else {}),
            },
        )
        written = write_evidence(
            out_dir=out_dir,
            run=run,
            identity=identity,
            fixtures=fixture_records,
            registry=registry,
            harness=harness,
            repo=repo,
            packs_root=str(packs_root),
            live_probe=probe,
            verify_fixtures=True,
        )
        verification = written["verification"]
        gate = verification["gate"]

        if not args.quiet:
            print("")
            print(summarise_console(run))
            print("")
            print(f"gate: {gate['verdict']}")
            for reason in gate["reasons"]:
                print(f"  - {reason}")
            print("")
            print(f"bundle: {written['paths']['bundle_json']}")
            print(f"manifest: {written['paths']['manifest_json']}")
            print(f"summary: {written['paths']['summary_md']}")
            frozen = gate_workloads(workloads)
            print(
                f"frozen gate workloads with complete evidence: {gate['gate_workload_count']}/"
                f"{len(registry.gate_ids)} (registry gate ids); strict diagnostics measured: "
                f"{len(frozen)}"
            )
        if args.json_stdout:
            print(
                json.dumps(
                    {
                        "gate": gate,
                        "recomputed": verification["recomputed"],
                        "paths": {key: str(value) for key, value in written["paths"].items()},
                    }
                )
            )
        if args.diagnostic:
            # A diagnostic run reports whether the selected pairs are equivalent
            # and measured; it never turns the bundle verdict into a pass.
            bad = {"equivalence_failed", "nondeterministic", "error", "preflight_failed", "schema_failed"}
            ok = not run["truncated"] and not any(
                item.get("status") in bad for item in run["workloads"]
            )
            return 0 if ok else 1
        verdict = gate["verdict"]
        return 0 if verdict == "PASS" else (1 if verdict == "FAIL" else 2)
    finally:
        if args.keep_workdir:
            print(f"work directory kept: {work_dir}", file=sys.stderr)
        else:
            shutil.rmtree(work_dir, ignore_errors=True)
        # Fixtures live in the temp work directory, never in the checkout.
        if args.keep_fixtures and args.keep_workdir:
            print(f"fixtures kept: {fixtures_dir}", file=sys.stderr)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--registry", default=None, help="override the frozen registry path (audit only)")
    subparsers = parser.add_subparsers(dest="command", required=True)

    registry = subparsers.add_parser("registry", help="show and validate the frozen performance registry")
    registry.set_defaults(func=command_registry)

    listing = subparsers.add_parser("list", help="list workloads and their status")
    listing.add_argument("--repo", default=str(REPO_ROOT))
    listing.set_defaults(func=command_list)

    provision = subparsers.add_parser(
        "oracle-provision", help="build a lock-exact Python oracle environment with uv"
    )
    provision.add_argument("--dir", default="/tmp/spk-bench-oracle")
    provision.add_argument("--force", action="store_true")
    provision.set_defaults(func=command_oracle_provision)

    identity = subparsers.add_parser("identity", help="capture and validate source/binary/toolchain identity")
    identity.add_argument("--repo", default=str(REPO_ROOT))
    identity.add_argument("--rust-binary", default=str(DEFAULT_INSPECTION_BINARY))
    identity.add_argument("--oracle-python", default=None)
    identity.add_argument("--packs-root", default=None)
    identity.add_argument("--build-receipt", default=None)
    identity.add_argument("--allow-dirty", action="store_true", help="report dirty state instead of blocking")
    identity.set_defaults(func=command_identity)

    build = subparsers.add_parser(
        "build", help="cargo build --release --locked and write a source/binary receipt"
    )
    build.add_argument("--repo", default=str(REPO_ROOT))
    build.add_argument("--receipt", default=None)
    build.add_argument("--target", default=None)
    build.set_defaults(func=command_build)

    fixtures = subparsers.add_parser("fixtures", help="verify fixture recipes are portable")
    fixtures.add_argument("action", nargs="?", choices=["verify"], default="verify")
    fixtures.add_argument("--recipe", action="append", default=[])
    fixtures.add_argument("--seed", type=int, default=20261102)
    fixtures.set_defaults(func=command_fixtures)

    selftest = subparsers.add_parser("selftest", help="run harness negative/end-to-end tests")
    selftest.set_defaults(func=command_selftest)

    gate = subparsers.add_parser("gate", help="recompute and judge a saved evidence bundle")
    gate.add_argument("report", help="evidence directory or performance-evidence.json")
    gate.add_argument("--repo", default=str(REPO_ROOT))
    gate.add_argument("--rust-binary", default=str(DEFAULT_INSPECTION_BINARY))
    gate.add_argument("--oracle-python", default=None)
    gate.add_argument("--build-receipt", default=None)
    gate.add_argument("--offline-audit", action="store_true")
    gate.set_defaults(func=command_gate)

    run = subparsers.add_parser("run", help="run the certification measurement")
    run.add_argument("--repo", default=str(REPO_ROOT))
    run.add_argument("--rust-binary", default=str(DEFAULT_INSPECTION_BINARY))
    run.add_argument("--oracle-python", default=None)
    run.add_argument("--packs-root", default=None)
    run.add_argument("--build-receipt", default=None)
    run.add_argument("--out", default=None)
    run.add_argument("--workload", action="append", default=[], help="workload id glob; repeatable")
    run.add_argument("--reps", type=int, default=20, help="paired repetitions per side (frozen floor is 20)")
    run.add_argument("--warmups", type=int, default=1)
    run.add_argument("--seed", type=int, default=20261102)
    run.add_argument("--max-seconds", type=float, default=3600.0)
    run.add_argument("--floor-reps", type=int, default=5)
    run.add_argument("--cpus", default=None, help="taskset CPU list (e.g. 2-3)")
    run.add_argument("--no-micro", action="store_true")
    run.add_argument("--keep-workdir", action="store_true")
    run.add_argument("--keep-fixtures", action="store_true")
    run.add_argument(
        "--diagnostic",
        action="store_true",
        help="exit 0 when the selected pairs are equivalent/measured; the bundle verdict stays authoritative",
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
