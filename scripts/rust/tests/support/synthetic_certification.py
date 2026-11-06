"""Synthetic certification evidence for completion-checker tests.

This module is **test scaffolding**, never a measurement. The production
registry (`docs/rust/inventory/performance-registry.json`) is read-only here and
honestly marks 16 of the 20 required workloads absent. The completion checker's
positive control cannot use that registry, because a genuinely complete bundle
is impossible today. Instead this module derives a *unit-scoped* registry from
it with:

* exactly the same 20 required ids, 17 strict gate ids, 2 startup ids;
* exactly the same frozen policy (3.0x, 20 samples/side, 14 strict workloads,
  10% regression, 4000 bootstrap iterations, quiet-machine load floors);
* the 16 absent rows explicitly re-labelled as *simulated* read workloads with
  synthetic native/oracle argv and payload expectations.

Every such row carries a note beginning ``SYNTHETIC TEST SCAFFOLDING`` so the
substitution cannot be mistaken for production coverage.

The synthetic run is built to the strengthened evidence-binding contract
(``benchmarks/rust/EVIDENCE-BINDING-REPORT.md``, "Integration required from the
separate test owner"):

1. the real writer receives the real ``spkbench.selftest.synthetic_probe``
   observation seam, so every acceptance check still runs against the declared
   synthetic environment;
2. every run record carries ``measurement``/``equivalence`` and per-side
   preflight observations (exit code, timeout);
3. **every** required row -- strict, projection and startup -- has 20 real
   paired sample records, and the recorded launch/import floor table covers
   every runnable row;
4. stored content is self-consistent: mutation post-state deltas are stored and
   their per-sample hashes are the canonical hash of that delta, contract rows
   store ``{"_raw": stdout}`` that satisfies the declared stdout schema, and
   payload schemas apply only to non-contract rows.

The bundle is written by the real :func:`spkbench.report.write_evidence` and
re-verified from disk by :func:`spkbench.certify.evaluate_bundle`. Nothing here
invents a second bundle format, mocks the checker verdict, weakens a floor or
writes production evidence; all output lives under a caller-supplied temporary
directory.
"""

from __future__ import annotations

import base64
import copy
import hashlib
import json
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path

# Development tooling must not leave bytecode in the checkout.
sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
TESTS = HERE.parent
SCRIPTS = TESTS.parent
ROOT = SCRIPTS.parents[1]
BENCH = ROOT / "benchmarks" / "rust"
for _path in (str(SCRIPTS), str(BENCH)):
    if _path not in sys.path:
        sys.path.insert(0, _path)

import inventory_model  # noqa: E402
from spkbench import certify  # noqa: E402
from spkbench import evidence as evidencemod  # noqa: E402
from spkbench import expectations as expectmod  # noqa: E402
from spkbench import identity as identitymod  # noqa: E402
from spkbench.registry import PerformanceRegistry, canonical_json, registry_digest  # noqa: E402
from spkbench.report import write_evidence  # noqa: E402
from spkbench.selftest import synthetic_fixture_observer  # noqa: E402
from spkbench.selftest import synthetic_fixture_manifest  # noqa: E402
from spkbench.selftest import synthetic_fixture_record  # noqa: E402
from spkbench.selftest import synthetic_probe as _synthetic_probe  # noqa: E402

PRODUCTION_REGISTRY = ROOT / "docs/rust/inventory/performance-registry.json"
PRODUCTION_REGISTRY_SHA256 = hashlib.sha256(PRODUCTION_REGISTRY.read_bytes()).hexdigest()

BASELINE_COMMIT = "9aed3a8012cb5eb22c0128b14d525039f95fea7a"
CANDIDATE_COMMIT = "a" * 40
SYNTHETIC_FIXTURE = "synthetic-checker-fixture"
SCAFFOLD_NOTE = (
    "SYNTHETIC TEST SCAFFOLDING: this row is a simulated workload used only by "
    "scripts/rust/tests/test_check_completion.py. It is not a native "
    "implementation, not a measurement and must never be copied into the "
    "production registry or coverage.json."
)

# Deterministic synthetic timings. They are generated values, not measurements;
# every note, artifact and report describes them as synthetic.
RUST_MEDIAN_S = 0.020
FLOOR_S = 0.005
GATE_RATIO = 4.0
PROJECTION_RATIO = 2.0
STARTUP_RATIO = 50.0
PAIRS = 20
SEED = 7

STARTUP_HELP_STDOUT = """Usage: spec-kitty [OPTIONS] COMMAND [ARGS]...

  Spec Kitty mission workflow CLI (synthetic fixture output).

Options:
  --version   Show the version and exit.
  --help      Show this message and exit.

Commands:
  accept       Accept a completed mission.
  agent        Agent workflow commands.
  charter      Charter governance commands.
  consolidate  Consolidate approved lanes.
  implement    Implement a work package.
  mission      Mission lifecycle commands.
  next         Advance the runtime loop.
  review       Review a work package.
  specify      Create a mission specification.
  tasks        Create mission work packages.
"""


@dataclass
class SyntheticCertification:
    """Everything a checker test needs to exercise the real bundle seam."""

    registry_path: Path
    registry: PerformanceRegistry
    identity: dict
    harness_code: dict
    simulated_ids: list[str]
    production_absent_ids: list[str]
    evidence_dir: Path
    bundle_path: Path
    bundle: dict
    performance_section: dict
    verification: dict

    def checker_performance(self) -> dict:
        """Return a deep copy of the checker-facing performance section."""
        return copy.deepcopy(self.performance_section)

    def probe(self, **overrides):
        """A fresh synthetic observation probe bound to this bundle's harness.

        The probe fabricates *observations* only (the sanctioned test seam); the
        recorded harness manifest is reused so a concurrent edit by the
        benchmark owner cannot make the fixture's own positive control flake.
        """
        return make_probe(self.identity, self.registry_path, self.harness_code, **overrides)


def make_probe(identity: dict, registry_path: Path, harness_code: dict | None = None, **overrides):
    """Build the real ``synthetic_probe`` and apply optional test overrides."""
    probe = _synthetic_probe(identity, registry_path)
    if harness_code is not None:
        probe.harness = copy.deepcopy(harness_code)
    for key, value in overrides.items():
        if not hasattr(probe, key):
            raise AttributeError(f"synthetic probe has no override {key!r}")
        setattr(probe, key, value)
    return probe


def build_scoped_registry(destination: Path) -> tuple[Path, list[str], list[str]]:
    """Derive the unit-scoped synthetic registry and write it to ``destination``.

    Returns ``(path, simulated_ids, production_absent_ids)``. Only the
    ``absent`` rows are changed; ids, flags, policy and the checker contract stay
    byte-identical to the production registry.
    """
    data = json.loads(PRODUCTION_REGISTRY.read_text(encoding="utf-8"))
    production_absent = [
        row["id"] for row in data["workloads"] if row.get("measurement") == "absent"
    ]
    simulated: list[str] = []
    for row in data["workloads"]:
        if row.get("measurement") != "absent":
            continue
        workload_id = row["id"]
        row["measurement"] = "read"
        row["fixture"] = SYNTHETIC_FIXTURE
        row["normalizer"] = "identity"
        row["comparator"] = "exact"
        row["native"] = {
            "state": "implemented",
            "binary": "spec-kitty",
            "route": f"SYNTHETIC simulated native route for {workload_id}",
            "argv": ["{binary:spec-kitty}", "synthetic", workload_id],
            "blocked_by": None,
        }
        row["oracle"] = {
            "kind": "py-adapter",
            "argv": ["{oracle}", "synthetic_adapter.py", "--workload", workload_id],
            "adapter": None,
            "floor_modules": ["synthetic"],
        }
        row["expectations"] = {
            "exit": 0,
            "payload": {
                "required_keys": ["kind"],
                "equals": {"kind": workload_id},
            },
        }
        row.setdefault("notes", []).append(SCAFFOLD_NOTE)
        simulated.append(workload_id)

    data["digest"] = "sha256:" + registry_digest(data)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    registry = PerformanceRegistry.load(destination)
    problems = registry.validate()
    if problems:  # pragma: no cover - a broken fixture must fail loudly
        raise AssertionError("synthetic registry failed validation: " + "; ".join(problems))
    if len(registry.required_ids) != 20 or len(registry.gate_ids) != 17:
        raise AssertionError(
            f"synthetic registry has {len(registry.required_ids)} required / "
            f"{len(registry.gate_ids)} gate ids; expected 20 / 17"
        )
    production_policy = json.loads(PRODUCTION_REGISTRY.read_text(encoding="utf-8"))["policy"]
    if registry.policy != production_policy:
        raise AssertionError("synthetic registry changed the frozen performance policy")
    return destination, simulated, production_absent


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _real_lock_sha256(name: str, fallback: str) -> str:
    path = ROOT / name
    return sha256_file(path) if path.is_file() else fallback


def _real_packs_identity() -> dict:
    try:
        record = identitymod.packs_identity(ROOT / "packs")
    except (OSError, RuntimeError):  # pragma: no cover - a broken repo fails loudly elsewhere
        return {"exists": True, "digest": "1" * 64}
    if not record.get("digest"):  # pragma: no cover
        return {"exists": True, "digest": "1" * 64}
    return {"exists": bool(record.get("exists")), "digest": record["digest"]}


def synthetic_identity() -> dict:
    """A complete, explicitly synthetic identity/build binding.

    The probe observes exactly these values. Cargo.lock, uv.lock and the repo
    packs digest are the real files' digests so the fixture does not invent
    input identity; every build/binary/receipt value is synthetic and labelled
    as such.
    """
    return {
        "status": "exact",
        "problems": [],
        "source": {"oracle_imports_from_repo_src": True, "synthetic": True},
        "worktree": {
            "head": CANDIDATE_COMMIT,
            "dirty": False,
            "dirty_paths": [],
            "dirty_count": 0,
        },
        "dependencies": {"status": "exact", "mismatches": [], "installed_not_in_lock": []},
        "cargo": {
            "build_inputs": {"file_count": 1, "digest": "b" * 64},
            "lock_sha256": _real_lock_sha256("Cargo.lock", "c" * 64),
            "toolchain": {
                "release": "1.99.1",
                "host": "x86_64-unknown-linux-gnu",
                "rustc": {"stdout": "rustc 1.99.1 (synthetic)"},
                "cargo": {"stdout": "cargo 1.99.1 (synthetic)"},
                "release_profile": {"lto": "thin"},
            },
        },
        "binary_provenance": {
            "binaries": {
                "spec-kitty": {
                    "path": "/synthetic/spec-kitty",
                    "sha256": "d" * 64,
                    "size": 1,
                    "mtime_ns": 1,
                    "predates_source": False,
                },
                "spec-kitty-rust": {
                    "path": "/synthetic/spec-kitty-rust",
                    "sha256": "e" * 64,
                    "size": 1,
                    "mtime_ns": 1,
                    "predates_source": False,
                },
            }
        },
        "oracle": {
            "baseline": {"baseline": BASELINE_COMMIT, "src_matches_baseline": True},
            "verify": {"ok": True},
            "version": {"stdout": "Python 3.11.15 (synthetic)"},
            "python_real_sha256": "f" * 64,
        },
        "packs": _real_packs_identity(),
        "build_receipt": {
            "digest": "2" * 64,
            "path": "/synthetic/build-receipt.json",
            "verification": {"ok": True, "problems": []},
        },
        "uv_lock_sha256": _real_lock_sha256("uv.lock", "3" * 64),
        "machine": {"hostname": "synthetic-checker-fixture"},
        "stability": {"binary_stable": True, "source_stable": True},
    }


def _series(base: float, *, phase: int) -> list[float]:
    """Deterministic, tiny jitter around ``base`` (still synthetic).

    Twenty values with four each of -2..+2 jitter units keep the median exactly
    at ``base`` and the coefficient of variation far below the noise floor.
    """
    values = []
    for index in range(PAIRS):
        wobble = (((index * 7 + phase) % 5) - 2) * 0.0005
        values.append(round(base * (1.0 + wobble), 9))
    return values


def _phase(workload_id: str) -> int:
    return sum(map(ord, workload_id)) % 5


def _fixtures(registry: PerformanceRegistry) -> dict[str, dict]:
    """The recorded fixture table, generated by the real fixture generators.

    Real recipes (`compat-project`, `replay-project`) regenerate through
    `FixtureBuilder`; the unit-scoped synthetic recipe regenerates through the
    test generator in `spkbench.selftest`. Both go through the same cache the
    observation seam uses, so the recorded digest is the *generated* digest.
    """
    names = {row.get("fixture") for row in registry.workloads if row.get("fixture")}
    names.add(SYNTHETIC_FIXTURE)
    return {name: synthetic_fixture_record(name, SEED) for name in sorted(names)}


def _payload(entry: dict) -> dict:
    """A small normalized payload that satisfies the entry's declared schema.

    The synthetic registry's simulated rows only require ``kind``. The real
    production rows keep their real schemas, so the fixture builds enough rows
    for ``status.replay``, ``cli.mission.list`` and the two startup rows to pass
    the same ``spkbench.expectations`` validators the runner uses. Contract rows
    store ``{"_raw": stdout}`` exactly as the runner does.
    """
    workload_id = entry["id"]
    expectations = entry.get("expectations") or {}
    if entry.get("equivalence") == "contract":
        if workload_id == "startup.help":
            raw = STARTUP_HELP_STDOUT
        elif workload_id == "startup.version":
            raw = "spec-kitty 4.0.0\n"
        else:  # pragma: no cover - only the two production startup rows exist
            raw = f"spec-kitty 4.0.0\nUsage: {workload_id}\n  synthetic\n"
        payload = {"_raw": raw}
        problems = expectmod.validate_stdout(raw.encode("utf-8"), expectations.get("stdout") or {})
        if problems:  # pragma: no cover - a broken fixture must fail loudly
            raise AssertionError(f"synthetic stdout for {workload_id} is invalid: {problems}")
        return payload

    if workload_id == "status.replay":
        work_packages = [
            {
                "lane": "done",
                "actor": "synthetic",
                "force_count": 0,
                "last_event_id": "evt-2800",
                "last_transition_at": "2026-01-01T00:00:00Z",
                "review_result": "approved",
            }
            for _ in range(200)
        ]
        payload = {
            "mission_slug": "002-bench-mission-002",
            "mission_number": "2",
            "mission_type": "software-dev",
            "materialized_at": "2026-01-01T00:00:00Z",
            "event_count": 2800,
            "last_event_id": "evt-2800",
            "work_packages": work_packages,
            "summary": {"done": 200},
            "schema_version": 2,
        }
    elif workload_id == "cli.mission.list":
        payload = {
            "count": 12,
            "missions": [
                {
                    "slug": f"synthetic-mission-{index:02d}",
                    "mid8": f"{index:08X}",
                    "friendly_name": f"Synthetic Mission {index}",
                }
                for index in range(12)
            ],
        }
    else:
        payload = {"kind": workload_id, "synthetic": True, "items": [workload_id]}

    payload_schema = expectations.get("payload") or {}
    if payload_schema:
        problems = expectmod.validate_payload(payload, payload_schema)
        if problems:  # pragma: no cover - a broken fixture must fail loudly
            raise AssertionError(f"synthetic payload for {workload_id} is invalid: {problems}")
    return payload


def _post_state(entry: dict) -> dict:
    """A complete post-state observation satisfying the unchanged registry schema.

    The production `status.replay` row requires `kitty-specs/<slug>/status.json`
    with >= 2000 bytes, `event_count == 2800`, 200 work packages and
    `summary.done == 200`. The generator writes *real generated bytes* for that
    path (it does not lower the expectation and does not mock the verdict), plus
    the full before/after manifests and the complete delta the writer stores.
    """
    if entry.get("measurement") != "mutation":
        return {}
    post_schema = (entry.get("expectations") or {}).get("post_state") or {}
    params = entry.get("params") or {}

    def render(values) -> list[str]:
        return [str(value).format(**params) for value in values or []]

    added = render(post_schema.get("expected_additions"))
    changed = render(post_schema.get("expected_changes"))
    removed = render(post_schema.get("expected_removals"))
    if not added:  # pragma: no cover - fixture must stay explicit
        added = ["synthetic/post-state.json"]

    payload = _payload(entry)
    rel = added[0]
    data = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")

    before: dict[str, str] = synthetic_fixture_manifest(
        entry.get("fixture") or SYNTHETIC_FIXTURE, SEED
    )
    after = dict(before)
    after[rel] = hashlib.sha256(data).hexdigest()
    delta = {
        "added": added,
        "changed": changed,
        "removed": removed,
        "added_sha256": {path: after.get(path, "0" * 64) for path in added},
        "removed_sha256": {path: before.get(path, "0" * 64) for path in removed},
        "changed_sha256": {
            path: {"before": before.get(path, "0" * 64), "after": after.get(path, "0" * 64)}
            for path in changed
        },
    }
    return {
        "delta": delta,
        "before": before,
        "after": after,
        "params": dict(params),
        "content_roots": [],
        "required_files": {rel: base64.b64encode(data).decode("ascii")},
    }


def _payload_sha256(payload: dict) -> str:
    return evidencemod.canonical_sha256(payload)


def _sample(
    index: int,
    duration_s: float,
    side: str,
    *,
    order: int,
    payload_sha256: str,
    post_state_sha256: str | None,
    stdout_sha256: str,
) -> dict:
    return {
        "side": side,
        "pair_index": index,
        "order": order,
        "protocol": "rust" if side == "rust" else "py-adapter",
        "duration_s": duration_s,
        "cpu_user_s": 0.0,
        "cpu_sys_s": 0.0,
        "timed_out": False,
        "exit_code": 0,
        "stdout_sha256": stdout_sha256,
        "payload_sha256": payload_sha256,
        "post_state_sha256": post_state_sha256,
        "load_before": {"1m": 1.0},
        "load_after": {"1m": 1.0},
    }


def _ratio(entry: dict) -> float:
    if entry.get("measurement") == "startup":
        return STARTUP_RATIO
    if entry.get("gate_eligible"):
        return GATE_RATIO
    return PROJECTION_RATIO


def _record(entry: dict, *, order_base: int) -> dict:
    """One run record with 20 real paired sample records for every row.

    This mirrors the shape :func:`spkbench.runner` writes: per-side preflight
    observations, normalized payloads, a canonical post-state delta and a
    per-sample payload/post-state hash. The values are synthetic; the contract
    is the production one.
    """
    workload_id = entry["id"]
    payload = _payload(entry)
    payload_sha256 = _payload_sha256(payload)
    stdout_sha256 = hashlib.sha256(
        (payload.get("_raw", "") if isinstance(payload, dict) else "").encode("utf-8")
    ).hexdigest()
    post_state = _post_state(entry)
    post_state_sha256 = (
        evidencemod.canonical_sha256(post_state["delta"]) if post_state else None
    )
    mutation = entry.get("measurement") == "mutation"

    rust_values = _series(RUST_MEDIAN_S, phase=_phase(workload_id))
    python_values = _series(
        RUST_MEDIAN_S * _ratio(entry), phase=(_phase(workload_id) + 2) % 5
    )
    command_argv = {
        "rust": list(entry["native"]["argv"]),
        "python": list((entry.get("oracle") or {}).get("argv") or []),
    }
    fixture_id = entry.get("fixture")

    return {
        "id": workload_id,
        "status": "ok",
        "reason": "",
        "equivalence_diff": "",
        "measurement": entry.get("measurement"),
        "equivalence": entry.get("equivalence"),
        "gate_eligible": bool(entry.get("gate_eligible")),
        "fixture_id": fixture_id,
        "command_argv": command_argv,
        "expected_exit": 0,
        "samples": {
            "rust": [
                _sample(
                    index,
                    value,
                    "rust",
                    order=order_base + index * 2,
                    payload_sha256=payload_sha256,
                    post_state_sha256=post_state_sha256,
                    stdout_sha256=stdout_sha256,
                )
                for index, value in enumerate(rust_values)
            ],
            "python": [
                _sample(
                    index,
                    value,
                    "python",
                    order=order_base + index * 2 + 1,
                    payload_sha256=payload_sha256,
                    post_state_sha256=post_state_sha256,
                    stdout_sha256=stdout_sha256,
                )
                for index, value in enumerate(python_values)
            ],
        },
        "rejected_samples": [],
        "schema_problems": {"rust": [], "python": []},
        "preflight": {
            "rust": {
                "protocol": "rust",
                "exit_code": 0,
                "timed_out": False,
                "stdout_sha256": stdout_sha256,
            },
            "python": {
                "protocol": "py-adapter",
                "exit_code": 0,
                "timed_out": False,
                "stdout_sha256": stdout_sha256,
            },
            "payload": {"rust": payload, "python": payload},
            "payload_sha256": {"rust": payload_sha256, "python": payload_sha256},
            "post_state": (
                {
                    "rust": copy.deepcopy(post_state),
                    "python": copy.deepcopy(post_state),
                }
                if mutation
                else {}
            ),
            "post_state_sha256": {
                "rust": post_state_sha256,
                "python": post_state_sha256,
            },
            "post_state_problems": {"rust": [], "python": []},
        },
        "timing": _timing(entry, rust_values, python_values),
    }


def _timing(entry: dict, rust_values: list[float], python_values: list[float]) -> dict:
    rust_median = statistics.median(rust_values)
    python_median = statistics.median(python_values)
    floor_python = FLOOR_S
    rust_excess = rust_median - FLOOR_S
    python_excess = python_median - floor_python
    ratio = python_excess / rust_excess if rust_excess > 0 and python_excess > 0 else None
    return {
        "rust": {"n": len(rust_values), "median_s": rust_median},
        "python": {"n": len(python_values), "median_s": python_median},
        "median_ratio_oracle_over_rust": python_median / rust_median,
        "floor_control": {
            "rust_median_s": FLOOR_S,
            "python_median_s": floor_python,
            "rust_excess_s": rust_excess,
            "python_excess_s": python_excess,
            "work_excess_ratio": ratio,
        },
    }


def _floor_table(registry: PerformanceRegistry) -> dict:
    """The launch/import floor of every runnable row, not only gate rows."""
    python_floors: dict[str, float] = {}
    for entry in registry.workloads:
        if entry.get("measurement") == "absent":
            continue
        modules = ",".join((entry.get("oracle") or {}).get("floor_modules") or [])
        python_floors[modules] = FLOOR_S
    return {"rust": FLOOR_S, "python_by_modules": python_floors}


def synthetic_run(registry: PerformanceRegistry) -> dict:
    """A complete synthetic run covering every registry row."""
    workloads = []
    # The executor observation counter is global. Start after a floor/preflight
    # offset so the synthetic records exercise a real chronology instead of
    # resetting to 0/1 per workload.
    order_cursor = 700
    for entry in registry.workloads:
        if entry.get("measurement") == "absent":
            continue
        workloads.append(_record(entry, order_base=order_cursor))
        order_cursor += PAIRS * 2 + 2
    return {
        "schema": "spkbench.run.v2",
        "run_id": "synthetic-checker-fixture",
        "seed": 7,
        "repetitions": PAIRS,
        "warmups": 1,
        "budget_seconds": 60.0,
        "truncated": False,
        "fixtures_unchanged": True,
        "pairing": "interleaved-randomized-within-pair",
        "floors_seconds": _floor_table(registry),
        "workloads": workloads,
        "load": {"loadavg": {"1m": 1.0, "5m": 1.0, "15m": 1.0}, "captured_at": "run_end"},
    }


def build_certification(temp_root: Path) -> SyntheticCertification:
    """Write the registry, run and evidence bundle under ``temp_root``.

    The real writer is used end to end through the real synthetic probe seam;
    the returned ``verification`` is the writer's authoritative re-verification
    from disk (artifacts and manifest included).
    """
    registry_path, simulated, production_absent = build_scoped_registry(
        temp_root / "synthetic-performance-registry.json"
    )
    registry = PerformanceRegistry.load(registry_path)
    identity = synthetic_identity()
    evidence_dir = temp_root / "evidence"
    written = write_evidence(
        out_dir=evidence_dir,
        run=synthetic_run(registry),
        identity=identity,
        fixtures=_fixtures(registry),
        registry=registry,
        harness={
            "name": "spkbench",
            "version": "synthetic-checker-fixture",
            "synthetic": True,
            "note": "test scaffolding only; generated timings are not measurements",
        },
        repo=ROOT,
        packs_root=None,
        live_probe=_synthetic_probe(identity, registry_path),
        verify_fixtures=True,
        fixture_observer=synthetic_fixture_observer,
    )
    verdict = written["verification"]["gate"]["verdict"]
    if verdict != "PASS":  # pragma: no cover - a broken fixture must fail loudly
        raise AssertionError(
            "synthetic certification bundle did not pass: "
            f"{verdict}: {written['verification']['gate']['reasons'][:5]}"
        )
    bundle_path = evidence_dir / "performance-evidence.json"
    bundle = written["bundle"]
    section = certify.performance_section(bundle, bundle_path, verification=written["verification"])
    # The checker compares performance.policy exactly against the curated model;
    # certify.performance_section deliberately omits the policy.
    section["policy"] = copy.deepcopy(inventory_model.PERF_POLICY)
    return SyntheticCertification(
        registry_path=registry_path,
        registry=registry,
        identity=identity,
        harness_code=copy.deepcopy((bundle.get("harness") or {}).get("code") or {}),
        simulated_ids=simulated,
        production_absent_ids=production_absent,
        evidence_dir=evidence_dir,
        bundle_path=bundle_path,
        bundle=bundle,
        performance_section=section,
        verification=written["verification"],
    )


def reseal(bundle: dict, root: Path | None = None) -> dict:
    """Recompute the bundle self-hash after a deliberate test mutation.

    When ``root`` is supplied the bundle and its manifest are rewritten the way
    a forger would, so a test can show that a consistent hash chain still does
    not rescue a forged claim.
    """
    bundle["self_sha256"] = certify.self_digest(bundle)
    if root is not None:
        bundle_path = Path(root) / "performance-evidence.json"
        write_bundle(bundle_path, bundle)
        manifest_path = Path(root) / "manifest.json"
        if manifest_path.is_file():
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["bundle"]["sha256"] = sha256_file(bundle_path)
            manifest["artifacts"] = bundle.get("artifacts") or {}
            manifest["payloads"] = bundle.get("payload_artifacts") or {}
            manifest["post_state"] = bundle.get("post_state_artifacts") or {}
            manifest_path.write_text(
                json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
    return bundle


def write_bundle(bundle_path: Path, bundle: dict) -> None:
    bundle_path.write_text(
        json.dumps(bundle, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def canonical_digest(value) -> str:
    """The canonical content hash the writer records for stored content."""
    return evidencemod.canonical_sha256(value)


__all__ = [
    "BASELINE_COMMIT",
    "CANDIDATE_COMMIT",
    "FLOOR_S",
    "GATE_RATIO",
    "PAIRS",
    "PRODUCTION_REGISTRY",
    "PRODUCTION_REGISTRY_SHA256",
    "PROJECTION_RATIO",
    "RUST_MEDIAN_S",
    "SCAFFOLD_NOTE",
    "STARTUP_RATIO",
    "SYNTHETIC_FIXTURE",
    "SyntheticCertification",
    "build_certification",
    "build_scoped_registry",
    "canonical_digest",
    "canonical_json",
    "make_probe",
    "reseal",
    "sha256_file",
    "synthetic_identity",
    "synthetic_run",
    "write_bundle",
]
