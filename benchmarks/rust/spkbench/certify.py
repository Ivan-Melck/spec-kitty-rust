"""Certification: build, verify and re-derive a performance evidence bundle.

This module is the single implementation of "may this evidence support a
performance claim?". It is used by the harness (``bench.py run`` / ``gate``) and
by the completion-checker adapter (``scripts/rust/performance_evidence.py``).

Nothing is taken on trust. At verification time the gate:

* reloads the stored artifacts, verifies their hashes and ``manifest.json``;
* re-parses every normalized payload and post-state delta, checks its declared
  canonical hash and re-applies the workload's frozen schema;
* re-derives equivalence by comparing the two stored payloads (or the stored
  stdout contract), not by reading ``result_equivalent``;
* reconstructs the pairs from each stored sample's ``pair_index`` and derives
  medians, paired intervals, the aggregate geometric mean, work excess and the
  regression verdict exclusively from those samples;
* observes the live native source (HEAD, build-input digest, ``Cargo.lock``,
  toolchain, both binaries, the re-verified build receipt) and the harness /
  adapter code, and requires them to still be the measured ones;
* compares every recorded projection with the recomputed value.

A saved report is therefore never trusted at face value: editing a number, a
status, a threshold, a commit, a payload or a fixture digest either breaks a
hash or fails a recomputation. Missing evidence blocks; contradictory evidence
fails; malformed evidence blocks with a typed reason instead of crashing.
"""

from __future__ import annotations

import hashlib
import json
import math
import tempfile
from pathlib import Path
from typing import Any

from . import evidence as evidencemod
from . import expectations as expectmod
from . import stats
from .evidence import Evidence, EvidenceError
from .fixtures import FixtureBuilder
from .live import BINARIES, MEASURED_INPUT_PATHS, default_native_probe, harness_manifest
from .registry import PerformanceRegistry, canonical_json, registry_digest

SCHEMA = "spkbench.performance-evidence.v1"
REPORT_SCHEMA = "spkbench.report.v2"
STARTUP_IDS = ("startup.version", "startup.help")
ALLOWED_STATUS = {
    "ok",
    "absent_native",
    "blocked",
    "equivalence_failed",
    "preflight_failed",
    "schema_failed",
    "nondeterministic",
    "not_measured",
    "measured_incomplete",
    "error",
}

#: Factory the verifier uses to observe the live checkout. Tests replace this
#: with a probe of the same shape; the CLI and the completion checker never do.
NATIVE_PROBE_FACTORY = default_native_probe

#: Relative tolerance when comparing recorded floats with recomputed ones.
RELATIVE_TOLERANCE = 0.01
ABSOLUTE_TOLERANCE = 0.01

#: Per-sample fields carried from the measurement into the bundle. ``pair_index``
#: is what lets the verifier reconstruct the paired arrays instead of trusting a
#: second copy of the durations.
SAMPLE_FIELDS = (
    "pair_index",
    "order",
    "duration_s",
    "cpu_user_s",
    "cpu_sys_s",
    "timed_out",
    "exit_code",
    "stdout_sha256",
    "stdout_bytes",
    "payload_sha256",
    "post_state_sha256",
    "load_before",
    "load_after",
)


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()  # noqa: TID251 - canonical content hash


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()  # noqa: TID251 - file-integrity check
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def self_digest(bundle: dict) -> str:
    payload = {key: value for key, value in bundle.items() if key != "self_sha256"}
    return _sha256_bytes(canonical_json(payload).encode())


def _finite_positive(values: list[Any]) -> tuple[bool, list[float]]:
    out: list[float] = []
    for value in values:
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return False, []
        number = float(value)
        if not math.isfinite(number) or number <= 0:
            return False, []
        out.append(number)
    return True, out


def _close(recorded: Any, recomputed: float) -> bool:
    if isinstance(recorded, bool) or not isinstance(recorded, (int, float)):
        return False
    return abs(float(recorded) - recomputed) <= max(ABSOLUTE_TOLERANCE, abs(recomputed) * RELATIVE_TOLERANCE)


def _statistics_from_values(
    python: list[float],
    rust: list[float],
    paired_python: list[float],
    paired_rust: list[float],
    *,
    iterations: int,
    confidence: float,
    seed: int,
    problems: list[str] | None = None,
) -> dict:
    statistics: dict[str, Any] = {
        "problems": list(problems or []),
        "python_samples": python,
        "rust_samples": rust,
    }
    if not python or not rust:
        statistics["problems"].append("python/rust samples are missing")
    if not paired_python or not paired_rust:
        statistics["problems"].append("paired samples are missing")
    if len(paired_python) != len(paired_rust):
        statistics["problems"].append("paired samples are not the same length")
    if statistics["problems"]:
        return statistics
    statistics["python_n"] = len(python)
    statistics["rust_n"] = len(rust)
    statistics["pairs"] = len(paired_python)
    statistics["python_median_s"] = stats.median(python)
    statistics["rust_median_s"] = stats.median(rust)
    statistics["speedup"] = statistics["python_median_s"] / statistics["rust_median_s"]
    paired = stats.paired_ratio_ci(
        paired_python,
        paired_rust,
        iterations=iterations,
        confidence=confidence,
        seed=seed,
    )
    statistics["paired_ratio"] = paired["point"]
    statistics["paired_ci95"] = paired["ci95"]
    statistics["pair_ratios"] = paired["pair_ratios"]
    statistics["python_cv"] = stats.cv(python)
    statistics["rust_cv"] = stats.cv(rust)
    statistics["noise"] = stats.noise_level(
        statistics["rust_cv"], statistics["python_cv"], paired["ci95"][0], paired["ci95"][1]
    )
    return statistics


def _pairs_from_records(
    python_records: list[dict], rust_records: list[dict]
) -> tuple[list[float], list[float]]:
    """Pair-aligned samples reconstructed from ``pair_index`` in observation order.

    The pair sequence is ordered by the recorded global executor ``order``, never
    by the position a row happens to have in ``samples.jsonl`` (the writer groups
    rows by side). :func:`_chronology_problems` is what rejects order data that
    is not a valid global counter.
    """
    python_by_pair: dict[int, float] = {}
    rust_by_pair: dict[int, float] = {}
    first_order: dict[int, int] = {}
    for side, records, target in (
        ("python", python_records, python_by_pair),
        ("rust", rust_records, rust_by_pair),
    ):
        for item in records:
            index, duration = item.get("pair_index"), item.get("duration_s")
            if not isinstance(index, int) or isinstance(index, bool):
                continue
            if not isinstance(duration, (int, float)) or isinstance(duration, bool):
                continue
            target[index] = float(duration)
            order = item.get("order")
            if isinstance(order, int) and not isinstance(order, bool):
                current = first_order.get(index)
                first_order[index] = order if current is None else min(current, order)
    indices = sorted(set(python_by_pair) & set(rust_by_pair))
    indices.sort(key=lambda index: (first_order.get(index, index), index))
    return ([python_by_pair[index] for index in indices], [rust_by_pair[index] for index in indices])


def _chronology_problems(
    wid: str,
    by_side: dict[str, dict[int, dict]],
    *,
    seen_orders: dict[int, str] | None = None,
) -> list[str]:
    """Validate ``order`` as the global executor counter of the run.

    The executor increments one counter for every invocation (floor probes,
    preflights, warmups and measurements), so the two measurement observations of
    a pair are adjacent and every order is unique across the whole run. There is
    deliberately no ``0/1`` assumption: the absolute values include the floor and
    preflight offset.
    """
    problems: list[str] = []
    if set(by_side["python"]) != set(by_side["rust"]):
        only_python = sorted(set(by_side["python"]) - set(by_side["rust"]))[:5]
        only_rust = sorted(set(by_side["rust"]) - set(by_side["python"]))[:5]
        problems.append(
            f"{wid}: pair_index does not pair one rust and one python sample "
            f"(python-only {only_python}, rust-only {only_rust})"
        )
    pair_orders: dict[int, dict[str, int]] = {}
    for side in ("python", "rust"):
        for index, record in sorted(by_side[side].items()):
            order = record.get("order")
            if isinstance(order, bool) or not isinstance(order, int):
                problems.append(f"{wid}: {side} pair {index} has no integer observation order")
                continue
            if order < 1:
                problems.append(
                    f"{wid}: {side} pair {index} order {order} is not a positive executor counter"
                )
            if seen_orders is not None:
                label = f"{wid}/{side}/{index}"
                prior = seen_orders.setdefault(order, label)
                if prior != label:
                    problems.append(
                        f"{wid}: {side} pair {index} order {order} is already used by {prior}; "
                        "the executor observation order is global"
                    )
            pair_orders.setdefault(index, {})[side] = order
    previous: int | None = None
    for index in sorted(pair_orders):
        observed = pair_orders[index]
        if set(observed) != {"python", "rust"}:
            problems.append(f"{wid}: pair {index} does not contain one rust and one python observation")
            continue
        if abs(observed["rust"] - observed["python"]) != 1:
            problems.append(
                f"{wid}: pair {index} observations are not adjacent in the global order "
                f"(rust={observed['rust']}, python={observed['python']})"
            )
        base = min(observed.values())
        if previous is not None and base <= previous:
            problems.append(
                f"{wid}: pair {index} does not follow the previous pair in observation order"
            )
        previous = base
    return problems


def _sample_record(sample: dict) -> dict:
    record = {field: sample.get(field) for field in SAMPLE_FIELDS}
    for field in ("duration_s", "cpu_user_s", "cpu_sys_s"):
        value = record.get(field)
        if value is not None and not isinstance(value, bool) and isinstance(value, (int, float)):
            record[field] = float(value)
    return record


def _load_context(run: dict) -> dict:
    loads = []
    for record in run.get("workloads", []):
        for side in ("rust", "python"):
            for sample in record.get("samples", {}).get(side, []):
                load = sample.get("load_before") if isinstance(sample, dict) else None
                if isinstance(load, dict) and isinstance(load.get("1m"), (int, float)):
                    loads.append(float(load["1m"]))
    loads.sort()
    if not loads:
        return {"samples": 0}
    return {
        "samples": len(loads),
        "median_1m": stats.median(loads),
        "max_1m": max(loads),
        "min_1m": min(loads),
        "spread_1m": max(loads) - min(loads),
    }


def _work_excess_from_medians(rust_median_s: float, python_median_s: float, floors: dict) -> dict:
    rust = floors.get("rust")
    python = floors.get("python")
    rust_excess = (rust_median_s - rust) if rust is not None else None
    python_excess = (python_median_s - python) if python is not None else None
    ratio = None
    if rust_excess and python_excess and rust_excess > 0 and python_excess > 0:
        ratio = python_excess / rust_excess
    return {
        "rust_floor_s": rust,
        "python_floor_s": python,
        "rust_excess_s": rust_excess,
        "python_excess_s": python_excess,
        "work_excess_ratio": ratio,
    }


# ---------------------------------------------------------------------------
# Bundle construction
# ---------------------------------------------------------------------------


def _python_floor(floors: dict, entry: dict) -> float | None:
    modules_key = ",".join(((entry.get("oracle") or {}).get("floor_modules")) or [])
    return (floors.get("python_by_modules") or {}).get(modules_key)


def _workload_row(
    record: dict,
    *,
    entry: dict,
    floors: dict,
    run_id: str,
    evidence_refs: list[str],
) -> dict:
    wid = entry["id"]
    status = record.get("status")
    samples = record.get("samples") or {}
    python_records = [
        _sample_record(item) for item in samples.get("python", []) if isinstance(item, dict)
    ]
    rust_records = [
        _sample_record(item) for item in samples.get("rust", []) if isinstance(item, dict)
    ]
    preflight = record.get("preflight") or {}
    preflight = preflight if isinstance(preflight, dict) else {}
    payload_sha = preflight.get("payload_sha256") or {}
    post_state_sha = preflight.get("post_state_sha256") or {}
    preflight_exit: dict[str, Any] = {}
    preflight_timed_out: dict[str, Any] = {}
    for side in ("rust", "python"):
        observation = preflight.get(side)
        if isinstance(observation, dict):
            if "exit_code" in observation:
                preflight_exit[side] = observation.get("exit_code")
            if "timed_out" in observation:
                preflight_timed_out[side] = observation.get("timed_out")

    paired_python, paired_rust = _pairs_from_records(python_records, rust_records)
    python_values = [
        float(item.get("duration_s") or 0.0)
        for item in python_records
        if isinstance(item.get("duration_s"), (int, float)) and not isinstance(item.get("duration_s"), bool)
    ]
    rust_values = [
        float(item.get("duration_s") or 0.0)
        for item in rust_records
        if isinstance(item.get("duration_s"), (int, float)) and not isinstance(item.get("duration_s"), bool)
    ]
    statistics = _statistics_from_values(
        python_values,
        rust_values,
        paired_python,
        paired_rust,
        iterations=200,
        confidence=0.95,
        seed=0,
    )
    work_excess = _work_excess_from_medians(
        statistics.get("rust_median_s") or 0.0,
        statistics.get("python_median_s") or 0.0,
        {"rust": floors.get("rust"), "python": _python_floor(floors, entry)},
    )
    return {
        "id": wid,
        "category": entry.get("category"),
        "required": bool(entry.get("required")),
        "strict_equivalence": bool(entry.get("strict_equivalence")),
        "equivalence": entry.get("equivalence"),
        "gate_eligible": bool(entry.get("gate_eligible")),
        "measurement": entry.get("measurement"),
        "status": status,
        "reason": record.get("reason") or "",
        "blocked_reason": entry.get("native", {}).get("blocked_by")
        if entry.get("measurement") == "absent"
        else None,
        "result_equivalent": status == "ok" and record.get("equivalence_diff") in (None, ""),
        "equivalence_diff": record.get("equivalence_diff") or "",
        "fixture_id": record.get("fixture_id"),
        "fixture_content_digest": None,
        "command_argv": record.get("command_argv") or {},
        "environment": run_id,
        "samples": {
            "python": [float(item.get("duration_s") or 0.0) * 1000.0 for item in python_records],
            "rust": [float(item.get("duration_s") or 0.0) * 1000.0 for item in rust_records],
        },
        "sample_records": {"python": python_records, "rust": rust_records},
        "sample_count": {
            "python": len(python_records),
            "rust": len(rust_records),
            "pairs": len(paired_python),
        },
        "preflight_exit": preflight_exit,
        "preflight_timed_out": preflight_timed_out,
        "expected_exit": record.get("expected_exit")
        if record.get("expected_exit") is not None
        else (entry.get("expectations") or {}).get("exit"),
        "payload_sha256": payload_sha if isinstance(payload_sha, dict) else {},
        "schema_problems": record.get("schema_problems") or {},
        "post_state": {
            "compared": entry.get("measurement") == "mutation",
            "equal": bool(
                entry.get("measurement") == "mutation"
                and post_state_sha.get("rust")
                and post_state_sha.get("rust") == post_state_sha.get("python")
            ),
            "sha256": post_state_sha if isinstance(post_state_sha, dict) else {},
            "ignored_prefixes": list(
                ((entry.get("expectations") or {}).get("post_state") or {}).get("ignored_prefixes") or []
            ),
            "problems": preflight.get("post_state_problems"),
            "exclusion_ruling": (
                ((entry.get("expectations") or {}).get("post_state") or {}).get("exclusion_ruling")
            ),
        },
        "rejected_samples": record.get("rejected_samples") or [],
        "evidence_refs": list(evidence_refs),
        "registry_workload": True,
        "floor_modules": list(((entry.get("oracle") or {}).get("floor_modules")) or []),
        "paired": {
            "python": [value * 1000.0 for value in paired_python],
            "rust": [value * 1000.0 for value in paired_rust],
        },
        "medians_ms": {
            "python": (statistics.get("python_median_s") or 0.0) * 1000.0
            if statistics.get("python_median_s") is not None
            else None,
            "rust": (statistics.get("rust_median_s") or 0.0) * 1000.0
            if statistics.get("rust_median_s") is not None
            else None,
        },
        "speedup": statistics.get("speedup"),
        "paired_ratio": statistics.get("paired_ratio"),
        "paired_ci95": statistics.get("paired_ci95"),
        "pair_ratios": statistics.get("pair_ratios"),
        "cv": {"python": statistics.get("python_cv"), "rust": statistics.get("rust_cv")},
        "noise": statistics.get("noise"),
        "work_excess": work_excess,
    }


def certify_run(
    *,
    run: dict,
    identity: dict,
    fixtures: dict[str, dict],
    registry: PerformanceRegistry,
    repo: Path,
    bundle_path_rel: str,
    harness: dict,
) -> dict:
    """Build the checker-shaped evidence bundle from a measurement run.

    The bundle is *not* certified here: :func:`spkbench.report.write_evidence`
    verifies the written bytes (artifacts, manifest, live identity) and stamps
    the verdict. Keeping construction and verdict separate is what stops a
    recorded ``PASS`` from being an input to its own acceptance.
    """
    policy = registry.policy
    seed = int(run.get("seed") or 0)
    run_id = str(run.get("run_id") or "unknown")
    floors = run.get("floors_seconds") or {}
    records = {record["id"]: record for record in run.get("workloads", [])}
    bundle_dir = str(Path(bundle_path_rel).parent)
    evidence_refs = [f"{bundle_dir}/samples.jsonl", bundle_path_rel]

    rows = []
    for entry in registry.workloads:
        record = records.get(entry["id"])
        if record is None:
            rows.append(
                {
                    "id": entry["id"],
                    "category": entry.get("category"),
                    "required": bool(entry.get("required")),
                    "strict_equivalence": bool(entry.get("strict_equivalence")),
                    "equivalence": entry.get("equivalence"),
                    "gate_eligible": bool(entry.get("gate_eligible")),
                    "measurement": entry.get("measurement"),
                    "status": "absent_native"
                    if entry.get("measurement") == "absent"
                    else "not_measured",
                    "reason": "workload row missing from the run",
                    "result_equivalent": False,
                    "samples": {"python": [], "rust": []},
                    "sample_records": {"python": [], "rust": []},
                    "sample_count": {"python": 0, "rust": 0, "pairs": 0},
                    "evidence_refs": evidence_refs,
                }
            )
            continue
        row = _workload_row(
            record,
            entry=entry,
            floors=floors,
            run_id=run_id,
            evidence_refs=evidence_refs,
        )
        fixture_name = record.get("fixture_id")
        if fixture_name and fixture_name in fixtures:
            row["fixture_content_digest"] = fixtures[fixture_name].get("content_digest")
        rows.append(row)

    # Aggregate projections recorded for the report. The verifier recomputes
    # them from the stored samples and refuses on any difference.
    policy = registry.policy
    iterations = int(policy.get("bootstrap_iterations") or 2000)
    confidence = float(policy.get("confidence_level") or 0.95)
    gate_rows = [row for row in rows if row.get("gate_eligible") and row.get("status") == "ok"]
    per_workload_pairs = {
        row["id"]: list(row.get("pair_ratios") or [])
        for row in gate_rows
        if row.get("pair_ratios")
    }
    aggregate_ci = (
        stats.geomean_ci(
            per_workload_pairs, iterations=iterations, confidence=confidence, seed=seed
        )
        if per_workload_pairs
        else None
    )
    work_ratios = {
        row["id"]: (row.get("work_excess") or {}).get("work_excess_ratio")
        for row in gate_rows
        if (row.get("work_excess") or {}).get("work_excess_ratio")
    }
    work_geomean = stats.geometric_mean(list(work_ratios.values())) if work_ratios else None
    strict_rows = [row for row in gate_rows if row.get("speedup")]
    speedups = [row["speedup"] for row in strict_rows]
    projected_geomean = stats.geometric_mean(speedups) if speedups else None
    projected_pair_count = sum((row.get("sample_count") or {}).get("pairs") or 0 for row in gate_rows)

    startup_rows = [row for row in rows if row["id"] in STARTUP_IDS]
    startup = {
        "excluded_from_gate": True,
        "workload_ids": list(STARTUP_IDS),
        "status": "pass"
        if startup_rows
        and all(row["status"] == "ok" and row["result_equivalent"] for row in startup_rows)
        else "pending",
        "measurements": {},
        "cannot_substitute": True,
    }
    # Startup medians are derived from the same records the verifier uses.
    for row in startup_rows:
        if row["status"] != "ok":
            continue
        python_values = [value / 1000.0 for value in row["samples"]["python"]]
        rust_values = [value / 1000.0 for value in row["samples"]["rust"]]
        startup["measurements"][row["id"]] = {
            "python_median_ms": stats.median(python_values) * 1000.0 if python_values else None,
            "rust_median_ms": stats.median(rust_values) * 1000.0 if rust_values else None,
            "pairs": row["sample_count"]["pairs"],
        }

    environment = {
        "run_id": run_id,
        "seed": run.get("seed"),
        "repetitions": run.get("repetitions"),
        "warmups": run.get("warmups"),
        "budget_seconds": run.get("budget_seconds"),
        "truncated": run.get("truncated"),
        "fixtures_unchanged": run.get("fixtures_unchanged"),
        "pairing": run.get("pairing"),
        "machine": identity.get("machine"),
        "load": run.get("load"),
        "load_context": _load_context(run),
        "floors_seconds": run.get("floors_seconds"),
        "isolation": {
            "home": "run-local",
            "xdg": "run-local",
            "tmp": "run-local",
            "pycache": "run-local",
            "cwd": "run-local or fixture clone",
            "pythonhashseed": "0",
            "tz": "UTC",
        },
    }

    return {
        "schema": SCHEMA,
        "registry": registry.identity(),
        "checker_contract": {
            "min_samples_per_side": int(policy.get("minimum_samples_per_side") or 20),
            "min_strict_workloads": int(policy.get("minimum_strict_workloads") or 14),
            "min_speedup": float(policy.get("minimum_speedup") or 3.0),
            "max_regression_fraction": float(
                policy.get("max_per_workload_regression_fraction") or 0.10
            ),
            "required_workload_ids": registry.required_ids,
            "gate_workload_ids": registry.gate_ids,
            "startup_workload_ids": registry.startup_ids,
        },
        "policy": policy,
        "baseline": {
            "commit": identity.get("oracle", {}).get("baseline", {}).get("baseline")
            if isinstance(identity.get("oracle", {}).get("baseline"), dict)
            else None,
            "python_version": (identity.get("oracle", {}).get("version") or {}).get("stdout"),
            "uv_lock_sha256": identity.get("uv_lock_sha256"),
            "oracle_python_real_sha256": identity.get("oracle", {}).get("python_real_sha256"),
            "packs_digest": (identity.get("packs") or {}).get("digest"),
            "oracle_verified": bool((identity.get("oracle", {}).get("verify") or {}).get("ok")),
        },
        "candidate": {
            "commit": (identity.get("worktree") or {}).get("head"),
            "source_clean": not bool((identity.get("worktree") or {}).get("dirty")),
            "source_dirty_paths": (identity.get("worktree") or {}).get("dirty_paths") or [],
            "source_digest": ((identity.get("cargo") or {}).get("build_inputs") or {}).get("digest"),
            "cargo_lock_sha256": (identity.get("cargo") or {}).get("lock_sha256")
            or (
                _sha256_file(repo / "Cargo.lock")
                if (repo / "Cargo.lock").exists()
                else None
            ),
            "toolchain": (identity.get("cargo") or {}).get("toolchain"),
            "binaries": {
                name: {
                    "path": record.get("path"),
                    "sha256": record.get("sha256"),
                    "size": record.get("size"),
                    "mtime_ns": record.get("mtime_ns"),
                    "predates_source": record.get("predates_source"),
                }
                for name, record in (
                    (identity.get("binary_provenance") or {}).get("binaries") or {}
                ).items()
            },
            "build_receipt": identity.get("build_receipt"),
            "stability": identity.get("stability"),
        },
        "identity_status": identity.get("status"),
        "identity_problems": identity.get("problems") or [],
        "harness": {**harness, "code": harness_manifest(repo)},
        "fixtures": fixtures,
        "environment": environment,
        "workloads": rows,
        "aggregate": {
            "status": "pending",
            "geometric_mean_speedup": projected_geomean,
            "geometric_mean_ci95": aggregate_ci["ci95"] if aggregate_ci else None,
            "work_excess_geometric_mean": work_geomean,
            "strict_workload_count": len(strict_rows),
            "sample_count": projected_pair_count,
            "minimum_speedup": float(policy.get("minimum_speedup") or 3.0),
            "max_per_workload_regression_fraction": float(
                policy.get("max_per_workload_regression_fraction") or 0.10
            ),
            "evidence": {"path": bundle_path_rel, "sha256": None},
        },
        "startup": startup,
        "gate": None,
    }


# ---------------------------------------------------------------------------
# Evidence derivation
# ---------------------------------------------------------------------------


def _fail(result: dict, message: str) -> None:
    result["failures"].append(message)


def _block(result: dict, message: str) -> None:
    result["blockers"].append(message)


def _warn(result: dict, message: str) -> None:
    result["warnings"].append(message)


def _input_manifest_problems(
    *,
    envelope: dict,
    schema: dict,
    params: dict,
    expected_manifest: dict,
) -> list[str]:
    """Bind the recorded input manifest to the regenerated fixture's real bytes.

    ``before`` is the measurement's input tree, ``after`` the post-state tree.
    Both are portable manifests (the shared fixture/clone root canonicalisation),
    so they are directly comparable with the independently regenerated fixture
    manifest. Only the registry's declared implementation-private prefixes are
    excluded; every user-observable file must match path-for-path.
    """
    ignored = tuple(schema.get("ignored_prefixes") or ())

    def visible(manifest: dict[str, str]) -> dict[str, str]:
        return {
            rel: digest
            for rel, digest in manifest.items()
            if not any(rel == item.rstrip("/") or rel.startswith(item) for item in ignored)
        }

    problems: list[str] = []
    before = envelope.get("before")
    after = envelope.get("after")
    if not isinstance(before, dict):
        return ["post-state before manifest is not an object"]
    expected = visible(expected_manifest)
    recorded = visible(before)
    missing = sorted(set(expected) - set(recorded))
    extra = sorted(set(recorded) - set(expected))
    if missing:
        problems.append(
            "recorded before manifest omits regenerated fixture paths " + str(missing[:5])
        )
    if extra:
        problems.append(
            "recorded before manifest names paths the regenerated fixture does not contain "
            + str(extra[:5])
        )
    differing = sorted(
        rel for rel in set(expected) & set(recorded) if expected[rel] != recorded[rel]
    )
    if differing:
        problems.append(
            "recorded before manifest does not match the regenerated fixture bytes for "
            + str(differing[:5])
        )
    unchanged = [
        expectmod.substitute(template, params)
        for template in schema.get("unchanged_paths") or []
    ]
    for rel in unchanged:
        if any(rel == item.rstrip("/") or rel.startswith(item) for item in ignored):
            continue
        recorded_digest = after.get(rel) if isinstance(after, dict) else None
        expected_digest = expected.get(rel)
        if expected_digest is None:
            problems.append(
                f"unchanged path {rel!r} is not present in the regenerated fixture"
            )
        elif recorded_digest != expected_digest:
            problems.append(
                f"recorded unchanged after manifest for {rel!r} does not match the "
                "regenerated fixture bytes"
            )
    return problems


def _derive_row(
    row: dict,
    entry: dict,
    *,
    evidence: Evidence | None,
    iterations: int,
    confidence: float,
    seed: int,
    seen_orders: dict[int, str] | None = None,
    fixtures: dict | None = None,
) -> dict:
    """Re-derive one workload's evidence from its samples and stored files."""
    wid = row.get("id")
    derived: dict[str, Any] = {
        "id": wid,
        "blockers": [],
        "failures": [],
        "samples": {},
        "pairs": ([], []),
        "statistics": {},
        "payloads": {},
        "post_states": {},
        "equivalence": None,
    }
    records = row.get("sample_records")
    if not isinstance(records, dict):
        derived["blockers"].append(f"{wid}: the bundle stores no per-sample records")
        return derived

    by_pair: dict[str, dict[int, dict]] = {}
    for side in ("python", "rust"):
        items = records.get(side)
        if not isinstance(items, list):
            derived["blockers"].append(f"{wid}: {side} sample records are not a list")
            return derived
        pairs: dict[int, dict] = {}
        for item in items:
            if not isinstance(item, dict):
                derived["failures"].append(f"{wid}: {side} sample records contain a non-object row")
                continue
            index = item.get("pair_index")
            if isinstance(index, bool) or not isinstance(index, int):
                derived["failures"].append(f"{wid}: {side} sample record has no integer pair_index")
                continue
            if index in pairs:
                derived["failures"].append(f"{wid}: {side} pair_index {index} appears twice")
                continue
            pairs[index] = item
        by_pair[side] = pairs

    if evidence is not None:
        for side in ("python", "rust"):
            stored = evidence.samples.get((wid, side), [])
            if not stored:
                derived["blockers"].append(f"{wid}: raw samples are missing from samples.jsonl")
                continue
            stored_by_pair: dict[int, dict] = {}
            for item in stored:
                index = item.get("pair_index")
                if isinstance(index, bool) or not isinstance(index, int):
                    derived["failures"].append(
                        f"{wid}: {side} raw sample in samples.jsonl has no integer pair_index"
                    )
                    continue
                if index in stored_by_pair:
                    derived["failures"].append(
                        f"{wid}: {side} raw sample pair_index {index} appears twice in samples.jsonl"
                    )
                    continue
                stored_by_pair[index] = item
            if set(by_pair[side]) != set(stored_by_pair):
                derived["failures"].append(
                    f"{wid}: {side} pair indices in the bundle do not match samples.jsonl"
                )
                continue
            for index in sorted(by_pair[side]):
                record = by_pair[side][index]
                raw = stored_by_pair[index]
                duration = record.get("duration_s")
                raw_duration = raw.get("duration_s")
                if not isinstance(duration, (int, float)) or isinstance(duration, bool):
                    derived["failures"].append(f"{wid}: {side} pair {index} has no numeric duration")
                    continue
                if not isinstance(raw_duration, (int, float)) or isinstance(raw_duration, bool):
                    derived["failures"].append(
                        f"{wid}: {side} pair {index} has no numeric duration in samples.jsonl"
                    )
                    continue
                if abs(float(duration) - float(raw_duration)) > 1e-6:
                    derived["failures"].append(
                        f"{wid}: {side} pair {index} duration differs from samples.jsonl"
                    )
                if raw.get("order") != record.get("order"):
                    derived["failures"].append(
                        f"{wid}: {side} pair {index} observation order differs from samples.jsonl"
                    )
                if raw.get("timed_out") is not False:
                    derived["failures"].append(
                        f"{wid}: {side} pair {index} timed out in samples.jsonl"
                    )

    # --- observation chronology: the executor counter is global and pairs are
    # adjacent. This is what makes `interleaved-randomized-within-pair` a
    # measured claim rather than a recorded string.
    for message in _chronology_problems(wid, by_pair, seen_orders=seen_orders):
        derived["failures"].append(message)

    expected_exit = row.get("expected_exit")
    recorded_expected = (entry.get("expectations") or {}).get("exit")
    if recorded_expected is not None and expected_exit != recorded_expected:
        derived["failures"].append(
            f"{wid}: recorded expected exit {expected_exit!r} differs from the frozen registry "
            f"value {recorded_expected!r}"
        )
    payload_sha = row.get("payload_sha256") or {}
    post_state_sha = (row.get("post_state") or {}).get("sha256") or {}
    preflight_exit = row.get("preflight_exit") or {}
    mutation = entry.get("measurement") == "mutation"

    if evidence is not None:
        for side in ("python", "rust"):
            if (wid, side) not in evidence.payloads:
                derived["blockers"].append(f"{wid}: stored normalized payload for {side} is missing")
            else:
                payload = evidence.payloads[(wid, side)]
                derived["payloads"][side] = payload
                computed = evidencemod.canonical_sha256(payload)
                recorded = payload_sha.get(side)
                if not recorded:
                    derived["failures"].append(f"{wid}: the bundle records no {side} payload hash")
                elif recorded != computed:
                    derived["failures"].append(
                        f"{wid}: recorded {side} payload hash does not match the stored payload"
                    )
            if mutation:
                if (wid, side) not in evidence.post_states:
                    derived["blockers"].append(f"{wid}: stored post-state delta for {side} is missing")
                else:
                    delta = evidence.post_states[(wid, side)]
                    derived["post_states"][side] = delta
                    computed_delta = evidencemod.canonical_sha256(delta)
                    recorded_delta = post_state_sha.get(side)
                    if not recorded_delta:
                        derived["failures"].append(
                            f"{wid}: the bundle records no {side} post-state hash"
                        )
                    elif recorded_delta != computed_delta:
                        derived["failures"].append(
                            f"{wid}: recorded {side} post-state hash does not match the stored delta"
                        )
                    envelope = evidence.post_state_envelopes.get((wid, side))
                    if envelope is None:
                        derived["blockers"].append(
                            f"{wid}: stored post-state envelope for {side} is missing"
                        )
                        continue
                    expected_params = entry.get("params") or {}
                    if (envelope.get("params") or {}) != expected_params:
                        derived["failures"].append(
                            f"{wid}: {side} stored post-state params do not match the frozen registry"
                        )
                    post_schema = (entry.get("expectations") or {}).get("post_state") or {}
                    for problem in expectmod.validate_post_state(
                        before=envelope.get("before") or {},
                        after=envelope.get("after") or {},
                        delta=delta,
                        schema=post_schema,
                        params=expected_params,
                        files=evidence.post_state_files.get((wid, side), {}),
                        content_roots=envelope.get("content_roots") or (),
                    ):
                        derived["failures"].append(f"{wid}: {side} {problem}")
                    if fixtures is not None:
                        fixture_id = row.get("fixture_id") or entry.get("fixture")
                        outcome = fixtures.get(fixture_id) if fixture_id else None
                        manifest = (outcome or {}).get("manifest")
                        if not isinstance(manifest, dict):
                            derived["blockers"].append(
                                f"{wid}: {side} input manifest cannot be bound to a "
                                f"regenerated fixture ({fixture_id!r})"
                            )
                        else:
                            for problem in _input_manifest_problems(
                                envelope=envelope,
                                schema=post_schema,
                                params=expected_params,
                                expected_manifest=manifest,
                            ):
                                derived["failures"].append(f"{wid}: {side} {problem}")

    # --- per-sample field binding (payload / post-state / exit / timeout)
    for side in ("python", "rust"):
        for index, record in sorted(by_pair[side].items()):
            if record.get("timed_out") is not False:
                derived["failures"].append(f"{wid}: {side} pair {index} timed out")
            if expected_exit is not None and record.get("exit_code") != expected_exit:
                derived["failures"].append(
                    f"{wid}: {side} pair {index} exit {record.get('exit_code')!r} is not the "
                    f"declared exit {expected_exit!r}"
                )
            if side in preflight_exit and record.get("exit_code") != preflight_exit.get(side):
                derived["failures"].append(
                    f"{wid}: {side} pair {index} exit differs from the preflight exit"
                )
            recorded_payload = payload_sha.get(side)
            if recorded_payload and record.get("payload_sha256") != recorded_payload:
                derived["failures"].append(
                    f"{wid}: {side} pair {index} payload hash differs from the preflight payload"
                )
            if evidence is not None and recorded_payload and (wid, side) in evidence.payloads:
                stored_hash = evidencemod.canonical_sha256(evidence.payloads[(wid, side)])
                if stored_hash != recorded_payload:
                    derived["failures"].append(
                        f"{wid}: {side} pair {index} payload hash is not the stored payload"
                    )
            if mutation and post_state_sha.get(side) and record.get("post_state_sha256") != post_state_sha.get(side):
                derived["failures"].append(
                    f"{wid}: {side} pair {index} post-state hash differs from the preflight post-state"
                )

    # --- statistics exclusively from the reconstructed samples
    python_records = [by_pair["python"][index] for index in sorted(by_pair["python"])]
    rust_records = [by_pair["rust"][index] for index in sorted(by_pair["rust"])]
    python_values = [
        float(item["duration_s"])
        for item in python_records
        if isinstance(item.get("duration_s"), (int, float))
        and not isinstance(item.get("duration_s"), bool)
    ]
    rust_values = [
        float(item["duration_s"])
        for item in rust_records
        if isinstance(item.get("duration_s"), (int, float))
        and not isinstance(item.get("duration_s"), bool)
    ]
    positive_python, python_values = _finite_positive(python_values)
    positive_rust, rust_values = _finite_positive(rust_values)
    problems: list[str] = []
    if not positive_python:
        problems.append("python samples are not all finite positive numbers")
    if not positive_rust:
        problems.append("rust samples are not all finite positive numbers")
    paired_python, paired_rust = _pairs_from_records(python_records, rust_records)
    derived["pairs"] = (paired_python, paired_rust)
    derived["statistics"] = _statistics_from_values(
        python_values,
        rust_values,
        paired_python,
        paired_rust,
        iterations=iterations,
        confidence=confidence,
        seed=seed,
        problems=problems,
    )
    return derived


def _judge_row(
    result: dict,
    row: dict,
    entry: dict,
    derived: dict,
    *,
    policy: dict,
    floors: dict,
) -> dict:
    """Compare one row's recorded projections with its derived evidence."""
    wid = row.get("id")
    for message in derived.get("blockers") or []:
        _block(result, message)
    for message in derived.get("failures") or []:
        _fail(result, message)
    statistics = derived.get("statistics") or {}
    for problem in statistics.get("problems") or []:
        _block(result, f"{wid}: {problem}")

    # --- equivalence from stored data when the payloads are available
    payloads = derived.get("payloads") or {}
    if payloads:
        equivalence_problems = _payload_equivalence(entry, payloads)
        for problem in equivalence_problems:
            _fail(result, f"{wid}: {problem}")
        equivalent = not equivalence_problems
        if row.get("result_equivalent") is not equivalent:
            _fail(
                result,
                f"{wid}: recorded result_equivalent={row.get('result_equivalent')!r} but the "
                f"stored payloads give {equivalent}",
            )
        if row.get("equivalence_diff"):
            _fail(result, f"{wid}: equivalence diff recorded: {str(row['equivalence_diff'])[:200]}")
        derived["equivalence"] = equivalent
    elif row.get("equivalence_diff"):
        _fail(result, f"{wid}: equivalence diff recorded: {str(row['equivalence_diff'])[:200]}")

    recorded_schema = row.get("schema_problems") or {}
    for side in ("python", "rust"):
        if recorded_schema.get(side):
            _fail(result, f"{wid}: {side} recorded a payload/state schema failure")

    # --- mutation post-state
    if entry.get("measurement") == "mutation":
        post = row.get("post_state") or {}
        if post.get("compared") is not True:
            _fail(result, f"{wid}: mutation post-state was not compared")
        if not post.get("exclusion_ruling"):
            _fail(result, f"{wid}: mutation post-state exclusions have no recorded ruling")
        post_problems = post.get("problems") or {}
        for side in ("python", "rust"):
            if (post_problems or {}).get(side):
                _fail(result, f"{wid}: {side} post-state schema failure")
        stored_post = derived.get("post_states") or {}
        if stored_post:
            rust_delta = stored_post.get("rust")
            python_delta = stored_post.get("python")
            equal = evidencemod.canonical_json(rust_delta) == evidencemod.canonical_json(python_delta)
            if post.get("equal") is not equal:
                _fail(
                    result,
                    f"{wid}: recorded post-state equality={post.get('equal')!r} but the stored "
                    f"deltas give {equal}",
                )
            if not equal:
                _fail(result, f"{wid}: stored mutation post-state differs between implementations")

    gate_eligible = bool(entry.get("gate_eligible"))
    work_ratio = None
    if statistics and not statistics.get("problems"):
        _compare_statistics(result, row, statistics, derived.get("pairs") or ([], []), wid)
        min_samples = int(policy.get("minimum_samples_per_side") or 20)
        if statistics["python_n"] < min_samples or statistics["rust_n"] < min_samples:
            _block(
                result,
                f"{wid}: python={statistics['python_n']} rust={statistics['rust_n']} samples; "
                f"need >= {min_samples} per side",
            )
        if statistics["pairs"] < min_samples:
            _block(result, f"{wid}: only {statistics['pairs']} paired repetitions; need >= {min_samples}")
        recorded_count = row.get("sample_count") or {}
        if recorded_count.get("python") != statistics["python_n"] or recorded_count.get(
            "rust"
        ) != statistics["rust_n"]:
            _fail(result, f"{wid}: recorded sample_count does not match the stored samples")
        if recorded_count.get("pairs") != statistics["pairs"]:
            _fail(result, f"{wid}: recorded pair count does not match the stored pairs")

        if gate_eligible:
            rust_argv = " ".join((row.get("command_argv") or {}).get("rust") or [])
            if any(flag in rust_argv for flag in ("--version", "--help")):
                _fail(result, f"{wid}: startup command used as a work workload")
            if (entry.get("expectations") or {}).get("stdout"):
                _fail(result, f"{wid}: a stdout-only contract workload cannot be gate evidence")
            work_ratio = _verify_work_excess(
                result, row, statistics, wid, policy, floors
            )
        else:
            # Required non-gate rows (startup pair, projection) are still
            # required measurements: same raw-sample floor, same equivalence and
            # the same >10% regression rule. They only stay out of the geomean,
            # but their recorded floor table is still checked so a projection
            # cannot carry an arbitrary launch/import baseline into coverage.json.
            wex = row.get("work_excess") or {}
            if _check_floor_table(result, row, wid, floors) is not None:
                expected = _work_excess_from_medians(
                    statistics["rust_median_s"],
                    statistics["python_median_s"],
                    {"rust": wex.get("rust_floor_s"), "python": wex.get("python_floor_s")},
                )
                if expected.get("work_excess_ratio") is not None and not _close(
                    wex.get("work_excess_ratio"), expected["work_excess_ratio"]
                ):
                    _fail(
                        result,
                        f"{wid}: recorded work-excess ratio does not match recomputation",
                    )
        max_regression = float(policy.get("max_per_workload_regression_fraction") or 0.10)
        verdict, message = stats.regression_verdict(
            statistics["speedup"],
            statistics["paired_ci95"][0],
            statistics["paired_ci95"][1],
            max_regression,
        )
        if verdict == "material":
            _fail(result, f"{wid}: {message}")
        elif verdict == "uncertain":
            _block(result, f"{wid}: {message}")
    derived["strict"] = gate_eligible and bool(statistics) and not statistics.get("problems")
    derived["work_ratio"] = work_ratio
    return derived


def _payload_equivalence(entry: dict, payloads: dict) -> list[str]:
    from .workloads import COMPARATORS

    problems: list[str] = []
    expectations = entry.get("expectations") or {}
    if entry.get("equivalence") == "contract":
        stdout_schema = expectations.get("stdout") or {}
        for side in ("python", "rust"):
            payload = payloads.get(side)
            raw = payload.get("_raw") if isinstance(payload, dict) else None
            if not isinstance(raw, str) or not raw.strip():
                problems.append(f"{side} stored stdout contract is empty")
                continue
            problems.extend(
                f"{side}: {problem}"
                for problem in expectmod.validate_stdout(raw.encode("utf-8"), stdout_schema)
            )
        return problems
    payload_schema = expectations.get("payload") or {}
    if payload_schema:
        for side in ("python", "rust"):
            problems.extend(
                f"{side}: {problem}"
                for problem in expectmod.validate_payload(payloads.get(side), payload_schema)
            )
    comparator = COMPARATORS.get(entry.get("comparator") or "exact")
    if comparator is None:
        problems.append(f"unknown comparator {entry.get('comparator')!r}")
    else:
        equal, diff = comparator(payloads.get("rust"), payloads.get("python"))
        if not equal:
            problems.append(f"stored payloads are not equivalent: {diff[:200]}")
    return problems


def _compare_statistics(
    result: dict,
    row: dict,
    statistics: dict,
    pairs: tuple[list[float], list[float]],
    wid: str,
) -> None:
    if not _close(row.get("speedup"), statistics["speedup"]):
        _fail(
            result,
            f"{wid}: recorded speedup {row.get('speedup')!r} does not match recomputed "
            f"{statistics['speedup']:.6f}",
        )
    recorded_medians = row.get("medians_ms") or {}
    for side, key in (("python", "python_median_s"), ("rust", "rust_median_s")):
        if not _close(recorded_medians.get(side), statistics[key] * 1000.0):
            _fail(result, f"{wid}: recorded {side} median does not match the stored samples")
    if not _close(row.get("paired_ratio"), statistics["paired_ratio"]):
        _fail(result, f"{wid}: recorded paired ratio does not match the stored pairs")
    recorded_ci = row.get("paired_ci95")
    recomputed_ci = statistics["paired_ci95"]
    if (
        not isinstance(recorded_ci, list)
        or len(recorded_ci) != 2
        or not _close(recorded_ci[0], recomputed_ci[0])
        or not _close(recorded_ci[1], recomputed_ci[1])
    ):
        _fail(result, f"{wid}: recorded paired CI does not match recomputation")
    recorded_ratios = row.get("pair_ratios")
    recomputed_ratios = statistics["pair_ratios"]
    recorded_ratio_values = (
        [float(item) for item in recorded_ratios] if isinstance(recorded_ratios, list) else []
    )
    if len(recorded_ratio_values) != len(recomputed_ratios) or any(
        not _close(a, b) for a, b in zip(recorded_ratio_values, recomputed_ratios, strict=True)
    ):
        _fail(result, f"{wid}: recorded pair ratios do not match the stored pairs")
    recorded_cv = row.get("cv") or {}
    for side, key in (("python", "python_cv"), ("rust", "rust_cv")):
        if not _close(recorded_cv.get(side), statistics[key]):
            _fail(result, f"{wid}: recorded {side} CV does not match the stored samples")
    recorded_paired = row.get("paired") or {}
    recorded_python = [float(item) for item in recorded_paired.get("python") or []]
    recorded_rust = [float(item) for item in recorded_paired.get("rust") or []]
    recorded_samples = row.get("samples") or {}
    if len(recorded_samples.get("python") or []) != statistics["python_n"] or any(
        not _close(a, b * 1000.0)
        for a, b in zip(
            [float(item) for item in recorded_samples.get("python") or []],
            statistics["python_samples"], strict=False,
        )
    ):
        _fail(result, f"{wid}: recorded python samples do not match the stored samples.jsonl")
    if len(recorded_samples.get("rust") or []) != statistics["rust_n"] or any(
        not _close(a, b * 1000.0)
        for a, b in zip(
            [float(item) for item in recorded_samples.get("rust") or []],
            statistics["rust_samples"], strict=False,
        )
    ):
        _fail(result, f"{wid}: recorded rust samples do not match the stored samples.jsonl")
    paired_python, paired_rust = pairs
    if len(recorded_python) != len(paired_python) or any(
        not _close(a, b * 1000.0) for a, b in zip(recorded_python, paired_python, strict=False)
    ):
        _fail(result, f"{wid}: recorded paired python samples do not match pair reconstruction")
    if len(recorded_rust) != len(paired_rust) or any(
        not _close(a, b * 1000.0) for a, b in zip(recorded_rust, paired_rust, strict=False)
    ):
        _fail(result, f"{wid}: recorded paired rust samples do not match pair reconstruction")


def _check_floor_table(
    result: dict, row: dict, wid: str, floors: dict
) -> tuple[float, float] | None:
    """The recorded launch/import floors must match the run's measured table."""
    wex = row.get("work_excess") or {}
    floor_rust = wex.get("rust_floor_s")
    floor_python = wex.get("python_floor_s")
    measured_rust_floor = floors.get("rust")
    modules_key = ",".join(row.get("floor_modules") or [])
    measured_python_floor = (floors.get("python_by_modules") or {}).get(modules_key)
    if floor_rust is None or floor_python is None:
        if modules_key:
            _block(result, f"{wid}: launch/import floor measurements are missing")
        return None
    if measured_rust_floor is None:
        _block(result, f"{wid}: no recorded Rust launch floor to check against")
        return None
    if modules_key and measured_python_floor is None:
        _block(result, f"{wid}: no recorded Python import floor for modules [{modules_key}]")
        return None
    if abs(float(floor_rust) - float(measured_rust_floor)) > max(
        0.001, float(measured_rust_floor) * 0.2
    ) or (
        modules_key
        and abs(float(floor_python) - float(measured_python_floor))
        > max(0.001, float(measured_python_floor) * 0.2)
    ):
        _fail(result, f"{wid}: recorded floors do not match the measured floor table")
        return None
    return float(floor_rust), float(floor_python)


def _verify_work_excess(
    result: dict,
    row: dict,
    statistics: dict,
    wid: str,
    policy: dict,
    floors: dict,
) -> float | None:
    table = _check_floor_table(result, row, wid, floors)
    if table is None:
        return None
    floor_rust, floor_python = table
    wex = row.get("work_excess") or {}
    work = _work_excess_from_medians(
        statistics["rust_median_s"],
        statistics["python_median_s"],
        {"rust": floor_rust, "python": floor_python},
    )
    work_ratio = work.get("work_excess_ratio")
    if work_ratio is None:
        _block(result, f"{wid}: measured median is below its own launch/import floor")
        return None
    if not _close(wex.get("work_excess_ratio"), work_ratio):
        _fail(
            result,
            f"{wid}: recorded work-excess ratio {wex.get('work_excess_ratio')!r} does not match "
            f"recomputed {work_ratio:.6f}",
        )
    if statistics["speedup"] >= 1.0 and work_ratio < float(
        policy.get("min_work_excess_ratio") or 1.25
    ):
        _block(result, f"{wid}: startup-dominated (work-excess ratio {work_ratio:.3f}x)")
    for label, value in (
        ("rust_excess_s", work["rust_excess_s"]),
        ("python_excess_s", work["python_excess_s"]),
    ):
        if value is None or value < float(policy.get("min_work_excess_seconds") or 0.005):
            _block(result, f"{wid}: {label} is below the minimum work window")
    return work_ratio


# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------


def _verify_fixtures(
    bundle: dict, *, temp_root: Path | None = None
) -> dict[str, dict]:
    """Regenerate every fixture at two fresh roots and compare frozen facts.

    This is the real observation: the portable content digest is recomputed from
    the frozen recipe + seed, at two different audit paths, and compared with the
    recorded digest, recipe facts and file count. A forger has to change the
    recorded table *and* produce bytes that the frozen recipe actually generates.

    The actual per-file manifest is returned too. It is the independent fact the
    mutation post-state's recorded input manifest is bound against: digest
    self-consistency inside a bundle is not evidence of what the fixture reads.
    """
    seed = int(bundle.get("environment", {}).get("seed") or 0)
    outcomes: dict[str, dict] = {}
    root = temp_root or Path(tempfile.mkdtemp(prefix="spkbench-fixverify-"))
    try:
        for index, (name, recorded) in enumerate(sorted((bundle.get("fixtures") or {}).items())):
            record = recorded if isinstance(recorded, dict) else {}
            generated: list[dict] = []
            for variant in ("a", "b" + "x" * 24):
                builder = FixtureBuilder(root / f"{index}-{variant}" / name, seed=seed)
                fixture = builder.get(name)
                generated.append(fixture.as_dict(include_manifest=True))
            digests = [item.get("content_digest") for item in generated]
            manifests = [item.get("manifest") for item in generated]
            outcomes[name] = {
                "recorded": record.get("content_digest"),
                "regenerated": digests,
                "portable": len(set(digests)) == 1,
                "manifests_equal": manifests[0] == manifests[1],
                "manifest": manifests[0],
                "manifest_sha256": generated[0].get("manifest_sha256"),
                "matches": len(set(digests)) == 1
                and digests[0] == record.get("content_digest"),
                "recorded_recipe": record.get("recipe"),
                "regenerated_recipe": generated[0].get("recipe"),
                "recorded_file_count": record.get("file_count"),
                "regenerated_file_count": generated[0].get("file_count"),
            }
    finally:
        if temp_root is None:
            import shutil

            shutil.rmtree(root, ignore_errors=True)
    return outcomes


def _porcelain_paths(lines: list[str]) -> list[str]:
    """Extract repo-relative paths from ``git status --porcelain`` lines."""
    paths = []
    for line in lines:
        text = line.rstrip()
        if len(text) > 3:
            text = text[3:]
        if " -> " in text:
            text = text.split(" -> ", 1)[1]
        if text:
            paths.append(text.strip().strip('"'))
    return paths


def _under(path: str, roots: tuple[str, ...]) -> bool:
    normalized = path.strip("./")
    return any(normalized == root or normalized.startswith(root + "/") for root in roots)


def _compare_live_native(
    result: dict,
    bundle: dict,
    live: dict,
    probe,
    *,
    registry: PerformanceRegistry,
) -> None:
    """Require the live checkout, binaries, receipt and harness to be the measured ones."""
    if live.get("error"):
        _block(result, f"live native source could not be observed: {live['error']}")
        return
    candidate = bundle.get("candidate") or {}
    commit = candidate.get("commit")

    head = live.get("head")
    if head != commit:
        if not isinstance(commit, str) or not commit:
            _block(result, "the bundle records no candidate commit to compare with HEAD")
        elif not probe.is_ancestor(commit):
            _fail(
                result,
                f"candidate commit {commit[:12]} is not an ancestor of live HEAD "
                f"{(head or 'unknown')[:12]}; the evidence belongs to a foreign history",
            )
        else:
            changed = probe.changed_measured_inputs(commit)
            if changed:
                _block(
                    result,
                    "measured build/harness inputs changed after the candidate commit: "
                    + ", ".join(changed[:10])
                    + (" …" if len(changed) > 10 else ""),
                )
            else:
                _warn(
                    result,
                    f"HEAD {(head or '')[:12]} follows the candidate {commit[:12]} with "
                    "evidence/docs-only commits; every measured input is unchanged",
                )

    dirty = _porcelain_paths(list(live.get("dirty_paths") or []))
    dirty_measured = [path for path in dirty if _under(path, MEASURED_INPUT_PATHS)]
    if dirty_measured:
        _block(
            result,
            "the working tree has uncommitted changes to measured inputs: "
            + ", ".join(dirty_measured[:10])
            + (" …" if len(dirty_measured) > 10 else ""),
        )

    if candidate.get("source_digest") != live.get("build_inputs_digest"):
        _fail(result, "live Rust build-input digest does not match the recorded candidate source")
    if candidate.get("cargo_lock_sha256") != live.get("cargo_lock_sha256"):
        _fail(result, "live Cargo.lock digest does not match the recorded candidate")
    recorded_toolchain = candidate.get("toolchain") or {}
    live_toolchain = live.get("toolchain") or {}
    if recorded_toolchain.get("release") != live_toolchain.get("release"):
        _fail(result, "live rustc release does not match the recorded toolchain")
    if (recorded_toolchain.get("rustc") or {}).get("stdout") != live_toolchain.get("rustc_stdout"):
        _fail(result, "live rustc version text does not match the recorded toolchain")
    if (recorded_toolchain.get("cargo") or {}).get("stdout") != live_toolchain.get("cargo_stdout"):
        _fail(result, "live cargo version text does not match the recorded toolchain")

    recorded_binaries = candidate.get("binaries") or {}
    live_binaries = live.get("binaries") or {}
    for name in BINARIES:
        recorded = recorded_binaries.get(name) or {}
        observed = live_binaries.get(name) or {}
        if not recorded.get("sha256"):
            _block(result, f"binary {name} has no recorded sha256")
            continue
        if not observed.get("exists"):
            _block(result, f"binary {name} is missing from the live tree")
            continue
        if recorded.get("sha256") != observed.get("sha256"):
            _fail(result, f"live binary {name} is not the measured binary (sha256 differs)")
        if observed.get("predates_source"):
            _block(result, f"binary {name} predates the live build inputs; rebuild before certifying")

    receipt = live.get("receipt")
    if not receipt:
        _block(result, "no build receipt on disk binds the measured binary to the recorded source")
    else:
        verification = receipt.get("verification") or {}
        if verification.get("ok") is not True:
            _fail(
                result,
                "the live build receipt does not verify: "
                + "; ".join((verification.get("problems") or [])[:5] or ["unknown"]),
            )
        recorded_receipt = candidate.get("build_receipt") or {}
        if not recorded_receipt:
            _block(result, "the bundle records no build receipt")
        else:
            if recorded_receipt.get("digest") != receipt.get("digest"):
                _fail(result, "the live build receipt digest differs from the recorded receipt")
            receipt_source = receipt.get("source") or {}
            if receipt_source.get("head") != commit:
                _fail(result, "the build receipt was written for a different commit")
            if receipt_source.get("build_inputs_digest") != candidate.get("source_digest"):
                _fail(result, "the build receipt was written for different build inputs")
            if receipt_source.get("cargo_lock_sha256") != candidate.get("cargo_lock_sha256"):
                _fail(result, "the build receipt was written for a different Cargo.lock")
            for name in BINARIES:
                receipt_binary = (receipt.get("binaries") or {}).get(name) or {}
                bundle_binary = recorded_binaries.get(name) or {}
                if not receipt_binary.get("sha256"):
                    _block(result, f"the build receipt has no hash for {name}")
                elif receipt_binary.get("sha256") != bundle_binary.get("sha256"):
                    _fail(result, f"the build receipt binds a different {name} binary")

    recorded_harness = (bundle.get("harness") or {}).get("code")
    if not isinstance(recorded_harness, dict) or not recorded_harness:
        _block(result, "the bundle does not record the harness / adapter code that produced it")
    else:
        live_harness = live.get("harness") or {}
        missing = sorted(set(recorded_harness) - set(live_harness))
        extra = sorted(set(live_harness) - set(recorded_harness))
        changed = sorted(
            name
            for name in set(recorded_harness) & set(live_harness)
            if recorded_harness[name] != live_harness[name]
        )
        if missing:
            _fail(
                result,
                "recorded harness files are missing from the live tree: " + ", ".join(missing[:6]),
            )
        if extra:
            _fail(
                result,
                "the live harness has files the measurement did not record: "
                + ", ".join(extra[:6]),
            )
        if changed:
            _block(
                result,
                "harness/adapter code changed since the measurement: " + ", ".join(changed[:6]),
            )

    declared_registry = bundle.get("registry") or {}
    registry_path = declared_registry.get("path")
    if not registry_path:
        _block(result, "the bundle does not record the frozen registry path")
    elif live.get("registry_sha256") is None:
        _block(result, f"the frozen registry file could not be read at {live.get('registry_path')}")
    elif live["registry_sha256"] != declared_registry.get("sha256"):
        _fail(result, "the frozen registry file changed since the measurement")

    if declared_registry.get("digest") != registry.declared_digest:
        _block(result, "bundle cites a different frozen registry digest")

    _compare_live_environment(result, bundle, live)


def _compare_live_environment(result: dict, bundle: dict, live: dict) -> None:
    """Re-observe the oracle environment, packs and ``uv.lock`` live."""
    baseline = bundle.get("baseline") or {}
    recorded_oracle_python = (bundle.get("harness") or {}).get("oracle_python")
    oracle = live.get("oracle")
    if baseline.get("oracle_verified") is not True:
        return  # already blocked as a recorded-identity problem
    if oracle is None:
        _block(
            result,
            "the oracle interpreter recorded for the measurement could not be observed live "
            f"({recorded_oracle_python!r})",
        )
        return
    verification = oracle.get("verify") or {}
    if verification.get("ok") is not True:
        _fail(
            result,
            "the live oracle environment fails scripts/rust/verify_oracle.py: "
            + "; ".join((verification.get("problems") or [verification.get("error") or "unknown"])[:5]),
        )
    if oracle.get("baseline") is not None and oracle["baseline"].get("src_matches_baseline") is not True:
        _fail(result, "the live oracle <repo>/src differs from the pinned baseline commit")
    if (
        baseline.get("oracle_python_real_sha256")
        and oracle.get("python_real_sha256")
        and baseline["oracle_python_real_sha256"] != oracle["python_real_sha256"]
    ):
        _fail(result, "the live oracle interpreter is not the measured interpreter")
    packs = live.get("packs") or {}
    if not packs.get("exists"):
        _block(result, "the repo packs root is missing")
    elif baseline.get("packs_digest") and packs.get("digest") != baseline.get("packs_digest"):
        _fail(result, "the live packs digest does not match the measured packs")
    if baseline.get("uv_lock_sha256") and live.get("uv_lock_sha256") != baseline.get("uv_lock_sha256"):
        _fail(result, "the live uv.lock does not match the measured lock")


def evaluate_bundle(
    bundle: dict,
    registry: PerformanceRegistry,
    *,
    repo: Path | None = None,
    live_probe=None,
    verify_fixtures: bool = False,
    fixture_observer=None,
    artifacts_root: Path | None = None,
    require_artifacts: bool = True,
    require_manifest: bool | None = None,
    live_identity: dict | None = None,
) -> dict:
    """Recompute and judge a bundle. Returns ``{gate, recomputed, fixtures}``.

    ``live_identity`` is accepted for backwards compatibility only: a *recorded*
    identity is data the bundle asserts about itself, so it is never enough to
    certify. The live checkout is always observed through a probe.

    ``fixture_observer`` is an explicit *observation* seam for unit-scoped
    synthetic fixtures (the same principle as the native-source probe): tests
    supply a regenerator with the same shape as :func:`_verify_fixtures`, and
    every fixture comparison still runs. There is no flag that skips fixtures.

    ``require_artifacts`` controls only whether the (last-written) artifact table
    and ``manifest.json`` must already exist; the stored payloads, post-state,
    raw samples and per-sample fields are always re-derived when an artifact
    root is supplied, and the gate always blocks when it is not. The live source
    is always observed: there is no switch that turns that check off.
    """
    result: dict[str, Any] = {
        "blockers": [],
        "failures": [],
        "warnings": [],
        "recomputed": {},
        "fixtures": {},
    }
    policy = registry.policy
    if live_identity is not None:
        _warn(result, "live_identity is recorded data and was ignored; a live probe was used")
    if require_manifest is None:
        require_manifest = require_artifacts
    iterations = int(policy.get("bootstrap_iterations") or 2000)
    confidence = float(policy.get("confidence_level") or 0.95)
    seed = 0
    if isinstance(bundle, dict):
        try:
            seed = int((bundle.get("environment") or {}).get("seed") or 0)
        except (TypeError, ValueError):
            _fail(result, "the bundle seed is not an integer")
    if not isinstance(bundle, dict):
        _block(result, "evidence bundle is not an object")
        return _finish(
            result,
            None,
            None,
            {},
            policy,
            verify_fixtures=verify_fixtures,
            require_live=False,
            fixture_observer=fixture_observer,
        )

    if bundle.get("schema") != SCHEMA:
        _block(result, f"unsupported evidence schema {bundle.get('schema')!r}")

    derived: dict[str, dict] = {}
    recomputed: dict[str, Any] = {}
    evidence: Evidence | None = None
    fixtures_observed: dict = {}
    fixture_observation = "recorded-only"
    try:
        _verify_registry_binding(result, bundle, registry, policy)
        if bundle.get("self_sha256") != self_digest(bundle):
            _fail(result, "bundle self-hash does not match its contents")
        _verify_recorded_identity(result, bundle)
        _verify_fixture_records(result, bundle)
        _verify_rows_structure(result, bundle, registry)
        if artifacts_root is not None:
            try:
                evidence = evidencemod.load_evidence(
                    bundle,
                    artifacts_root,
                    require_manifest=bool(require_manifest),
                    require_artifact_coverage=bool(require_artifacts),
                    required_ids=set(registry.required_ids),
                )
            except EvidenceError as exc:
                _block(result, f"evidence artifacts are unusable: {exc}")
            except (OSError, KeyError, TypeError, AttributeError, ValueError) as exc:
                _block(result, f"evidence artifacts are malformed: {type(exc).__name__}: {exc}")
            if evidence is not None:
                for message in evidence.problems:
                    _block(result, message)
                for message in evidence.failures:
                    _fail(result, message)
                for message in evidence.warnings:
                    _warn(result, message)
        fixtures_observed, fixture_observation = _observe_fixtures(
            result,
            bundle,
            verify_fixtures=verify_fixtures,
            repo=repo,
            registry_obj=registry,
            fixture_observer=fixture_observer,
        )
        derived = _judge_workloads(
            result,
            bundle,
            registry,
            evidence,
            policy,
            iterations=iterations,
            confidence=confidence,
            seed=seed,
            fixtures=fixtures_observed if verify_fixtures else None,
        )
        recomputed = _verify_aggregate(
            result, bundle, registry, derived, policy, iterations, confidence, seed
        )
        _verify_startup(result, bundle, derived, recomputed)
    except (EvidenceError, KeyError, TypeError, AttributeError, ValueError, IndexError) as exc:
        _block(
            result,
            f"evidence is malformed and could not be re-derived: {type(exc).__name__}: {exc}",
        )

    if artifacts_root is None:
        _block(
            result,
            "artifact directory was not supplied; stored payloads, post-state, raw samples and "
            "manifest cannot be re-checked",
        )

    verify_live = True
    if verify_live:
        if repo is None:
            _block(result, "no repository was supplied; the live native source cannot be observed")
        else:
            probe = live_probe
            live: dict
            try:
                probe = probe or NATIVE_PROBE_FACTORY(repo)
                receipt_hint = (bundle.get("candidate") or {}).get("build_receipt") or {}
                recorded_receipt = receipt_hint.get("path")
                receipt_path = None
                if isinstance(recorded_receipt, str) and recorded_receipt:
                    candidate_path = Path(recorded_receipt)
                    receipt_path = (
                        candidate_path
                        if candidate_path.is_absolute()
                        else Path(repo) / candidate_path
                    )
                binary_hint = {
                    name: str(record.get("path"))
                    for name, record in (
                        (bundle.get("candidate") or {}).get("binaries") or {}
                    ).items()
                    if isinstance(record, dict) and record.get("path")
                }
                declared_registry_path = (bundle.get("registry") or {}).get("path")
                registry_hint = None
                if isinstance(declared_registry_path, str) and declared_registry_path:
                    candidate_path = Path(declared_registry_path)
                    registry_hint = (
                        candidate_path
                        if candidate_path.is_absolute()
                        else Path(repo) / candidate_path
                    )
                baseline = bundle.get("baseline") or {}
                harness = bundle.get("harness") or {}
                oracle_hint = harness.get("oracle_python")
                live = probe.observe(
                    receipt_path=receipt_path,
                    binary_paths=binary_hint,
                    registry_path=registry_hint,
                    oracle_python=Path(oracle_hint)
                    if isinstance(oracle_hint, str) and oracle_hint
                    else None,
                    packs_root=Path(repo) / "packs",
                    baseline_commit=baseline.get("commit")
                    if isinstance(baseline.get("commit"), str)
                    else None,
                )
            except (OSError, RuntimeError, TypeError, ValueError) as exc:
                live = {"error": f"{type(exc).__name__}: {exc}"}
                probe = probe or NATIVE_PROBE_FACTORY(Path(repo))
            _compare_live_native(result, bundle, live, probe, registry=registry)

    return _finish(
        result,
        bundle,
        registry,
        recomputed,
        policy,
        verify_fixtures=verify_fixtures,
        require_live=verify_live,
        repo=repo,
        registry_obj=registry,
        fixture_observer=fixture_observer,
        fixtures_observed=fixtures_observed,
        fixture_observation=fixture_observation,
    )


def _verify_registry_binding(
    result: dict, bundle: dict, registry: PerformanceRegistry, policy: dict
) -> None:
    declared = bundle.get("registry") or {}
    if declared.get("digest") != registry.declared_digest:
        _block(result, "bundle cites a different frozen registry digest")
    elif declared.get("path") != registry.identity()["path"]:
        _warn(result, "registry path differs from the cited path (digest matches; audit copy)")
    if registry.declared_digest != "sha256:" + registry_digest(registry.data):
        _block(result, "frozen registry digest does not match its contents")
    if sorted(declared.get("workload_ids") or []) != sorted(registry.required_ids):
        _block(result, "bundle registry workload ids do not match the frozen floor")
    contract = bundle.get("checker_contract") or {}
    if sorted(contract.get("required_workload_ids") or []) != sorted(registry.required_ids):
        _block(result, "checker contract workload ids do not match the frozen floor")
    if sorted(contract.get("gate_workload_ids") or []) != sorted(registry.gate_ids):
        _block(result, "checker contract gate ids do not match the frozen floor")
    if sorted(contract.get("startup_workload_ids") or []) != sorted(registry.startup_ids):
        _block(result, "checker contract startup ids do not match the frozen floor")
    if contract.get("min_samples_per_side") != int(policy.get("minimum_samples_per_side") or 20):
        _block(result, "checker contract lowered the sample floor")
    if contract.get("min_speedup") != float(policy.get("minimum_speedup") or 3.0):
        _block(result, "checker contract lowered the speed floor")
    bundle_policy = bundle.get("policy") or {}
    for key in ("minimum_speedup", "minimum_samples_per_side", "minimum_strict_workloads"):
        if bundle_policy.get(key) != policy.get(key):
            _block(result, f"bundle policy {key} differs from the frozen registry")
    if bundle_policy.get("max_per_workload_regression_fraction") != policy.get(
        "max_per_workload_regression_fraction"
    ):
        _block(result, "bundle policy regression allowance differs from the frozen registry")


def _verify_recorded_identity(result: dict, bundle: dict) -> None:
    identity_status = bundle.get("identity_status")
    if identity_status != "exact":
        _block(
            result,
            "recorded identity is not exact: "
            + "; ".join((bundle.get("identity_problems") or [])[:5] or ["unknown"]),
        )
    candidate = bundle.get("candidate") or {}
    commit = candidate.get("commit")
    if not isinstance(commit, str) or not _is_commit(commit):
        _block(result, "candidate commit is not a 40-hex commit id")
    baseline = bundle.get("baseline") or {}
    if not isinstance(baseline.get("commit"), str) or not _is_commit(baseline.get("commit") or ""):
        _block(result, "baseline commit is not a 40-hex commit id")
    if candidate.get("source_clean") is not True:
        _block(result, "candidate source was not clean at measurement time")
    stability = candidate.get("stability") or {}
    if stability.get("binary_stable") is not True:
        _block(result, "the measured binary was not stable across the measurement window")
    if stability.get("source_stable") is not True:
        _block(result, "Rust build inputs changed during the measurement window")
    if not candidate.get("source_digest"):
        _block(result, "candidate source digest is missing")
    if not candidate.get("cargo_lock_sha256"):
        _block(result, "Cargo.lock digest is missing")
    binaries = candidate.get("binaries") or {}
    for name in BINARIES:
        record = binaries.get(name) or {}
        if not record.get("sha256"):
            _block(result, f"binary {name} has no recorded sha256")
        if record.get("predates_source"):
            _block(result, f"binary {name} predates the recorded source")
    toolchain = candidate.get("toolchain") or {}
    if not toolchain.get("release") or not (toolchain.get("rustc") or {}).get("stdout"):
        _block(result, "rustc toolchain identity is missing")
    if not (toolchain.get("cargo") or {}).get("stdout"):
        _block(result, "cargo identity is missing")
    receipt = candidate.get("build_receipt") or {}
    if not receipt:
        _block(result, "no build receipt binds the binary to the recorded source")
    elif not (receipt.get("verification") or {}).get("ok"):
        _block(
            result,
            "build receipt does not verify: "
            + "; ".join((receipt.get("verification") or {}).get("problems", [])[:3]),
        )
    if baseline.get("oracle_verified") is not True:
        _block(result, "oracle environment was not verified with scripts/rust/verify_oracle.py")
    if not baseline.get("packs_digest"):
        _block(result, "repo packs digest is missing")


def _verify_fixture_records(result: dict, bundle: dict) -> None:
    fixture_records = bundle.get("fixtures") or {}
    if not fixture_records:
        _block(result, "no fixture identity is recorded")
        return
    for name, record in fixture_records.items():
        if not isinstance(record, dict):
            _fail(result, f"fixture {name} is not an object")
            continue
        if not record.get("content_digest"):
            _block(result, f"fixture {name} has no portable content digest")
        if record.get("portable") is not True:
            _block(result, f"fixture {name} is not declared portable")
        if not record.get("file_count"):
            _block(result, f"fixture {name} has no files")


def _verify_rows_structure(
    result: dict, bundle: dict, registry: PerformanceRegistry
) -> None:
    rows = bundle.get("workloads") or []
    if not isinstance(rows, list):
        raise EvidenceError("bundle workloads is not a list")
    ids = [row.get("id") if isinstance(row, dict) else None for row in rows]
    if any(not isinstance(entry, str) for entry in ids):
        raise EvidenceError("bundle contains a workload row without a string id")
    if len(set(ids)) != len(ids):
        _fail(result, "bundle contains duplicate workload ids")
    by_id = {row.get("id"): row for row in rows if isinstance(row, dict)}
    missing = sorted(set(registry.required_ids) - set(by_id))
    if missing:
        _block(result, "required workloads are absent from the bundle: " + ", ".join(missing))
    extra_required = sorted(
        row["id"]
        for row in rows
        if row.get("required") and row.get("id") not in registry.required_ids
    )
    if extra_required:
        _fail(
            result,
            "bundle declares required workloads outside the frozen floor: "
            + ", ".join(extra_required),
        )
    for row in rows:
        wid = row.get("id")
        if wid in registry.required_ids:
            entry = registry.workload(wid)
            for key in ("required", "strict_equivalence", "equivalence", "gate_eligible", "category"):
                if row.get(key) != entry.get(key):
                    _fail(result, f"{wid}: {key} does not match the frozen registry")
            if row.get("measurement") != entry.get("measurement"):
                _fail(result, f"{wid}: measurement kind does not match the frozen registry")
            if row.get("status") == "ok":
                command = row.get("command_argv") or {}
                for side, producer in (("rust", "native"), ("python", "oracle")):
                    expected_argv = (entry.get(producer) or {}).get("argv")
                    if command.get(side) != expected_argv:
                        _fail(result, f"{wid}: {side} command_argv differs from the frozen registry")
            expected_fixture = entry.get("fixture")
            if row.get("status") == "ok" and expected_fixture is not None:
                fixture_id = row.get("fixture_id")
                if fixture_id != expected_fixture:
                    _fail(result, f"{wid}: fixture_id does not match the frozen registry")
                fixture = (bundle.get("fixtures") or {}).get(fixture_id)
                if not isinstance(fixture, dict):
                    _fail(result, f"{wid}: fixture_id is absent from the bundle fixture table")
                elif row.get("fixture_content_digest") != fixture.get("content_digest"):
                    _fail(result, f"{wid}: fixture content digest differs from the bundle fixture table")
        else:
            if row.get("required") or row.get("gate_eligible"):
                _fail(result, f"{wid}: non-frozen diagnostics cannot be required or gate-eligible")
        if row.get("status") not in ALLOWED_STATUS:
            _fail(result, f"{wid}: unknown status {row.get('status')!r}")


def _judge_workloads(
    result: dict,
    bundle: dict,
    registry: PerformanceRegistry,
    evidence: Evidence | None,
    policy: dict,
    *,
    iterations: int,
    confidence: float,
    seed: int,
    fixtures: dict | None = None,
) -> dict[str, dict]:
    by_id = {
        row.get("id"): row for row in bundle.get("workloads") or [] if isinstance(row, dict)
    }
    floors = (bundle.get("environment") or {}).get("floors_seconds") or {}
    pairing = (bundle.get("environment") or {}).get("pairing")
    measured = any(
        (row.get("sample_records") or {}).get("python") or (row.get("sample_records") or {}).get("rust")
        for row in by_id.values()
        if isinstance(row, dict)
    )
    if measured and pairing != "interleaved-randomized-within-pair":
        _fail(
            result,
            f"the run declares pairing={pairing!r}; measured repetitions must be adjacent and "
            "randomized within each rust/python pair",
        )
    derived: dict[str, dict] = {}
    seen_orders: dict[int, str] = {}
    for wid in registry.required_ids:
        row = by_id.get(wid)
        if row is None:
            continue
        entry = registry.workload(wid)
        if entry.get("measurement") == "absent":
            records = row.get("sample_records") or row.get("samples") or {}
            fabricated = bool(records.get("python") or records.get("rust"))
            if fabricated:
                _fail(result, f"{wid}: absent native workload carries samples (invented adapter)")
            elif row.get("status") != "absent_native":
                _fail(result, f"{wid}: absent native workload claims status {row.get('status')!r}")
            else:
                blocked_by = (entry.get("native") or {}).get("blocked_by") or "no native implementation"
                _block(result, f"{wid}: native workload is absent: {blocked_by}")
            continue
        if row.get("status") != "ok":
            _block(
                result,
                f"{wid}: status is {row.get('status')!r} ({row.get('reason') or 'no reason'})",
            )
            continue
        derived[wid] = _judge_row(
            result,
            row,
            entry,
            _derive_row(
                row,
                entry,
                evidence=evidence,
                iterations=iterations,
                confidence=confidence,
                seed=seed,
                seen_orders=seen_orders,
                fixtures=fixtures,
            ),
            policy=policy,
            floors=floors,
        )
    return derived


def _verify_aggregate(
    result: dict,
    bundle: dict,
    registry: PerformanceRegistry,
    derived: dict[str, dict],
    policy: dict,
    iterations: int,
    confidence: float,
    seed: int,
) -> dict:
    strict_speedups: list[float] = []
    per_workload_pairs: dict[str, list[float]] = {}
    work_ratios: dict[str, float] = {}
    pair_count = 0
    for wid in registry.required_ids:
        item = derived.get(wid)
        if not item or not item.get("strict"):
            continue
        statistics = item.get("statistics") or {}
        if statistics.get("problems"):
            continue
        strict_speedups.append(statistics["speedup"])
        per_workload_pairs[wid] = statistics["pair_ratios"]
        pair_count += statistics["pairs"]
        if item.get("work_ratio"):
            work_ratios[wid] = item["work_ratio"]
    geomean = stats.geometric_mean(strict_speedups) if strict_speedups else None
    aggregate_ci = (
        stats.geomean_ci(per_workload_pairs, iterations=iterations, confidence=confidence, seed=seed)
        if per_workload_pairs
        else None
    )
    work_geomean = stats.geometric_mean(list(work_ratios.values())) if work_ratios else None
    recomputed = {
        "strict_workload_count": len(strict_speedups),
        "sample_count": pair_count,
        "geometric_mean_speedup": geomean,
        "geometric_mean_ci95": aggregate_ci["ci95"] if aggregate_ci else None,
        "work_excess_geometric_mean": work_geomean,
    }
    result["recomputed"] = recomputed
    minimum_strict = int(policy.get("minimum_strict_workloads") or 14)
    min_speedup = float(policy.get("minimum_speedup") or 3.0)
    if len(strict_speedups) < minimum_strict:
        _block(
            result,
            f"{len(strict_speedups)} strict gate workloads have complete evidence; need {minimum_strict}",
        )
    environment = bundle.get("environment") or {}
    if environment.get("truncated") is not False:
        _block(result, "the measurement was truncated; partial samples cannot certify")
    if environment.get("fixtures_unchanged") is not True:
        _fail(result, "a pristine fixture changed during the run; read-only isolation is broken")

    load_context = environment.get("load_context") or {}
    max_load = float(policy.get("max_load_1m") or 6.0)
    max_spread = float(policy.get("max_load_spread_1m") or 4.0)
    if not load_context.get("samples"):
        _block(result, "no load context was recorded for the measurement window")
    else:
        if load_context.get("median_1m", 0) > max_load:
            _block(
                result,
                f"median load1m {load_context.get('median_1m'):.2f} exceeds the quiet-machine "
                f"floor {max_load:.2f}; rerun when idle",
            )
        if load_context.get("spread_1m", 0) > max_spread:
            _block(
                result,
                f"load1m spread {load_context.get('spread_1m'):.2f} exceeds {max_spread:.2f}; "
                "noise was not controlled",
            )
    for wid, item in derived.items():
        if not item.get("strict"):
            continue
        statistics = item.get("statistics") or {}
        for side, key in (("python", "python_cv"), ("rust", "rust_cv")):
            value = statistics.get(key)
            if isinstance(value, float) and value > float(policy.get("max_side_cv") or 0.30):
                _block(result, f"{wid}: {side} CV {value:.3f} exceeds the noise floor")

    aggregate = bundle.get("aggregate") or {}
    if geomean is None:
        _block(result, "no strict gate workload produced a speedup")
    else:
        if not _close(aggregate.get("geometric_mean_speedup"), geomean):
            _fail(
                result,
                f"recorded geometric mean {aggregate.get('geometric_mean_speedup')!r} does not "
                f"match recomputed {geomean:.6f}",
            )
        if geomean < min_speedup:
            _fail(result, f"geometric mean {geomean:.4f}x is below {min_speedup}x")
        ci_low, ci_high = (aggregate_ci or {}).get("ci95") or [0.0, 0.0]
        if ci_high < min_speedup:
            _fail(
                result,
                f"geometric-mean 95% CI {ci_low:.3f}-{ci_high:.3f} is entirely below {min_speedup}x",
            )
        elif ci_low < min_speedup:
            _block(
                result,
                f"geometric-mean 95% CI {ci_low:.3f}-{ci_high:.3f} reaches below {min_speedup}x",
            )
        if float(policy.get("require_work_excess_geomean") or 0) and (
            work_geomean is None or work_geomean < min_speedup
        ):
            _block(
                result,
                "work-excess geometric mean "
                + (f"{work_geomean:.3f}x" if work_geomean is not None else "is missing")
                + f" is below {min_speedup}x (startup cannot stand in for work)",
            )
    required_samples = minimum_strict * int(policy.get("minimum_samples_per_side") or 20)
    if pair_count < required_samples:
        _block(result, f"aggregate paired sample count {pair_count} is below {required_samples}")
    if aggregate.get("sample_count") != pair_count:
        _fail(
            result,
            f"recorded sample_count {aggregate.get('sample_count')!r} does not match recomputed "
            f"{pair_count}",
        )
    if aggregate.get("strict_workload_count") != len(strict_speedups):
        _fail(
            result,
            f"recorded strict_workload_count {aggregate.get('strict_workload_count')!r} does not "
            f"match recomputed {len(strict_speedups)}",
        )
    recorded_ci = aggregate.get("geometric_mean_ci95")
    if aggregate_ci is not None:
        if (
            not isinstance(recorded_ci, list)
            or len(recorded_ci) != 2
            or not _close(recorded_ci[0], aggregate_ci["ci95"][0])
            or not _close(recorded_ci[1], aggregate_ci["ci95"][1])
        ):
            _fail(result, "recorded aggregate CI does not match recomputation")
    elif recorded_ci is not None:
        _fail(result, "recorded aggregate CI exists but could not be recomputed")
    if work_geomean is None:
        if aggregate.get("work_excess_geometric_mean") is not None:
            _fail(result, "recorded work-excess geometric mean could not be recomputed")
    elif not _close(aggregate.get("work_excess_geometric_mean"), work_geomean):
        _fail(result, "recorded work-excess geometric mean does not match recomputation")
    if not _close(aggregate.get("minimum_speedup"), min_speedup):
        _fail(result, "recorded aggregate minimum_speedup differs from the frozen policy")
    if not _close(
        aggregate.get("max_per_workload_regression_fraction"),
        float(policy.get("max_per_workload_regression_fraction") or 0.10),
    ):
        _fail(result, "recorded aggregate regression allowance differs from the frozen policy")
    return recomputed


def _verify_startup(
    result: dict, bundle: dict, derived: dict[str, dict], recomputed: dict
) -> None:
    startup = bundle.get("startup") or {}
    if startup.get("excluded_from_gate") is not True:
        _fail(result, "startup is not excluded from the gate")
    if sorted(startup.get("workload_ids") or []) != sorted(STARTUP_IDS):
        _fail(result, "startup workload set is not the frozen pair")
    rows = {
        row.get("id"): row for row in bundle.get("workloads") or [] if isinstance(row, dict)
    }
    for wid in STARTUP_IDS:
        row = rows.get(wid)
        item = derived.get(wid)
        if row is None or item is None:
            _block(result, f"startup workload {wid} has no derived evidence")
            continue
        if item.get("strict"):
            _fail(result, "a startup workload entered the strict gate population")
        if row.get("gate_eligible") is not False:
            _fail(result, f"{wid}: startup row is gate-eligible")
    derived_ok = all(
        rows.get(wid) is not None
        and rows[wid].get("status") == "ok"
        and (derived.get(wid) or {}).get("statistics")
        and not ((derived.get(wid) or {}).get("statistics") or {}).get("problems")
        for wid in STARTUP_IDS
    )
    expected_status = "pass" if derived_ok else "pending"
    if startup.get("status") != expected_status:
        if expected_status == "pass":
            _fail(result, f"startup status {startup.get('status')!r} is not backed by its samples")
        else:
            _block(
                result,
                f"startup was not measured successfully (status={startup.get('status')!r})",
            )
    measurements = startup.get("measurements") or {}
    if not measurements:
        _block(result, "startup measurements are missing")
    for wid in STARTUP_IDS:
        statistics = ((derived.get(wid) or {}).get("statistics")) or {}
        recorded = measurements.get(wid) or {}
        if not statistics or statistics.get("problems"):
            continue
        if recorded.get("pairs") != statistics["pairs"]:
            _fail(result, f"{wid}: recorded startup pair count does not match the samples")
        if not _close(recorded.get("python_median_ms"), statistics["python_median_s"] * 1000.0):
            _fail(result, f"{wid}: recorded startup python median does not match the samples")
        if not _close(recorded.get("rust_median_ms"), statistics["rust_median_s"] * 1000.0):
            _fail(result, f"{wid}: recorded startup rust median does not match the samples")
    del recomputed


def _observe_fixtures(
    result: dict,
    bundle: dict,
    *,
    verify_fixtures: bool,
    repo: Path | None,
    registry_obj: PerformanceRegistry | None,
    fixture_observer=None,
) -> tuple[dict[str, dict], str]:
    """Run the fixture observation and record its verdict contributions.

    Returns ``(outcomes, marker)``. The marker is derived from whether the real
    observation ran, never from a caller's intent, so a skipped or failed
    regeneration cannot be labelled ``regenerated``.
    """
    if not (verify_fixtures and bundle is not None and repo is not None and registry_obj is not None):
        return {}, "recorded-only"
    observer = fixture_observer or _verify_fixtures
    try:
        outcomes = observer(bundle)
    except (OSError, RuntimeError, KeyError, ValueError, TypeError) as exc:
        _block(result, f"fixture regeneration failed: {type(exc).__name__}: {exc}")
        return {}, "failed"
    result["fixtures"] = outcomes
    recorded_names = set((bundle.get("fixtures") or {}).keys())
    observed_names = set(outcomes.keys())
    if recorded_names - observed_names:
        _block(
            result,
            "these recorded fixtures were not regenerated: "
            + ", ".join(sorted(recorded_names - observed_names)[:6]),
        )
    if observed_names - recorded_names:
        _fail(
            result,
            "the fixture observation produced records the bundle does not cite: "
            + ", ".join(sorted(observed_names - recorded_names)[:6]),
        )
    for name, outcome in outcomes.items():
        if not outcome.get("portable"):
            _block(result, f"fixture {name} does not regenerate identically at two roots")
        elif not outcome.get("manifests_equal"):
            _block(result, f"fixture {name} per-file manifest differs between audit roots")
        elif not outcome.get("matches"):
            _block(result, f"fixture {name} does not match its recorded content digest")
        if outcome.get("recorded_recipe") != outcome.get("regenerated_recipe"):
            _block(result, f"fixture {name} recipe differs from the frozen recipe")
        if (
            outcome.get("recorded_file_count") is not None
            and outcome.get("recorded_file_count") != outcome.get("regenerated_file_count")
        ):
            _fail(result, f"fixture {name} file count differs from the frozen recipe")
    return outcomes, "regenerated"


def _finish(
    result: dict,
    bundle: dict | None,
    registry: PerformanceRegistry | None,  # accepted for symmetry; unused
    recomputed: dict | None,
    policy: dict,
    *,
    verify_fixtures: bool,
    require_live: bool,
    repo: Path | None = None,
    registry_obj: PerformanceRegistry | None = None,
    fixture_observer=None,
    fixtures_observed: dict | None = None,
    fixture_observation: str = "recorded-only",
) -> dict:
    del registry, bundle, repo, registry_obj, fixture_observer, verify_fixtures
    if fixtures_observed:
        result["fixtures"] = fixtures_observed
    if result["blockers"]:
        verdict = "BLOCKED"
    elif result["failures"]:
        verdict = "FAIL"
    else:
        verdict = "PASS"
    recomputed = recomputed or {}
    geomean = recomputed.get("geometric_mean_speedup")
    ci = recomputed.get("geometric_mean_ci95") or [0.0, 0.0]
    reasons = [f"blocked: {item}" for item in result["blockers"]] or [
        f"failed: {item}" for item in result["failures"]
    ]
    if not reasons:
        if geomean:
            reasons = [
                f"geometric mean {geomean:.3f}x over "
                f"{recomputed.get('strict_workload_count', 0)} strict workloads "
                f"(95% CI {ci[0]:.2f}-{ci[1]:.2f}), recomputed from stored evidence"
            ]
        else:
            reasons = ["no verdict reason recorded"]
    result["gate"] = {
        "verdict": verdict,
        "geomean_ratio": geomean,
        "gate_workload_count": recomputed.get("strict_workload_count", 0),
        "sample_count": recomputed.get("sample_count", 0),
        "blockers": result["blockers"],
        "failures": result["failures"],
        "warnings": result["warnings"],
        "reasons": reasons,
        "policy": {
            "minimum_speedup": float(policy.get("minimum_speedup") or 3.0),
            "minimum_samples_per_side": int(policy.get("minimum_samples_per_side") or 20),
            "minimum_strict_workloads": int(policy.get("minimum_strict_workloads") or 14),
            "max_per_workload_regression_fraction": float(
                policy.get("max_per_workload_regression_fraction") or 0.10
            ),
        },
        "fixture_verification": fixture_observation,
        "identity_verified_live": bool(require_live),
        "evidence_recomputed": True,
    }
    return result


def _is_commit(value: str) -> bool:
    return len(value) == 40 and all(char in "0123456789abcdef" for char in value)


# ---------------------------------------------------------------------------
# Checker-facing projection
# ---------------------------------------------------------------------------


def performance_section(
    bundle: dict,
    bundle_path: Path,
    *,
    verification: dict | None = None,
) -> dict:
    """Project a *verified* bundle into the checker's ``performance`` section.

    The status is derived from the recomputed verdict, never from
    ``aggregate.status``: a non-PASS gate can never be projected as a pass.
    """
    gate = (verification or {}).get("gate") or {}
    passed = gate.get("verdict") == "PASS"
    recomputed = (verification or {}).get("recomputed") or {}
    registry_rows = {
        row["id"]: row for row in bundle.get("workloads") or [] if isinstance(row, dict)
    }
    workloads = []
    for entry_id in (bundle.get("registry") or {}).get("workload_ids") or []:
        row = registry_rows.get(entry_id)
        if row is None:
            workloads.append({"id": entry_id, "status": "pending"})
            continue
        command = row.get("command_argv") or {}
        workloads.append(
            {
                "id": entry_id,
                "category": row.get("category"),
                "required": bool(row.get("required")),
                "strict_equivalence": bool(row.get("strict_equivalence")),
                "equivalence": row.get("equivalence"),
                "status": "pass" if (passed and row.get("status") == "ok") else "pending",
                "result_equivalent": bool(passed and row.get("result_equivalent")),
                "fixture_id": row.get("fixture_id"),
                "command_argv": json.dumps(command, sort_keys=True),
                "environment": row.get("environment"),
                "samples": {
                    "python": [
                        round(float(value), 6) for value in (row.get("samples") or {}).get("python") or []
                    ],
                    "rust": [
                        round(float(value), 6) for value in (row.get("samples") or {}).get("rust") or []
                    ],
                },
                "speedup": row.get("speedup") if passed else None,
                "paired_ci95": row.get("paired_ci95") if passed else None,
                "work_excess": row.get("work_excess") if passed else None,
                "work_excess_ratio": (row.get("work_excess") or {}).get("work_excess_ratio")
                if passed
                else None,
                "post_state": row.get("post_state") if passed else None,
                "post_state_equal": (row.get("post_state") or {}).get("equal") if passed else None,
                "evidence_refs": row.get("evidence_refs") or [],
                "notes": row.get("reason") or "",
            }
        )
    startup = bundle.get("startup") or {}
    aggregate = bundle.get("aggregate") or {}
    verdict = gate.get("verdict")
    status = "pass" if passed else ("fail" if verdict == "FAIL" else "blocked")
    return {
        "status": status,
        "baseline": {
            "commit": (bundle.get("baseline") or {}).get("commit"),
            "python_version": (bundle.get("baseline") or {}).get("python_version"),
            "oracle_interpreter": bundle.get("harness", {}).get("oracle_python"),
        },
        "candidate": {
            "commit": (bundle.get("candidate") or {}).get("commit"),
            "rust_toolchain": {
                "channel": ((bundle.get("candidate") or {}).get("toolchain") or {}).get("release"),
                "profile": ((bundle.get("candidate") or {}).get("toolchain") or {}).get(
                    "release_profile"
                ),
            },
            "binary_sha256": {
                name: record.get("sha256")
                for name, record in ((bundle.get("candidate") or {}).get("binaries") or {}).items()
            },
            "source_digest": (bundle.get("candidate") or {}).get("source_digest"),
            "cargo_lock_sha256": (bundle.get("candidate") or {}).get("cargo_lock_sha256"),
            "build_receipt_digest": ((bundle.get("candidate") or {}).get("build_receipt") or {}).get(
                "digest"
            ),
        },
        "harness": bundle.get("harness") or {},
        "workloads": workloads,
        "aggregate": {
            "status": status,
            "geometric_mean_speedup": recomputed.get("geometric_mean_speedup") if passed else None,
            "geometric_mean_ci95": recomputed.get("geometric_mean_ci95") if passed else None,
            "work_excess_geometric_mean": recomputed.get("work_excess_geometric_mean")
            if passed
            else None,
            "strict_workload_count": recomputed.get("strict_workload_count") if passed else None,
            "sample_count": recomputed.get("sample_count") if passed else None,
            "minimum_speedup": aggregate.get("minimum_speedup"),
            "max_per_workload_regression_fraction": aggregate.get(
                "max_per_workload_regression_fraction"
            ),
            "evidence": {"path": _relativise(bundle_path), "sha256": _sha256_file(bundle_path)},
        },
        "startup": {
            "status": "pass" if passed else "pending",
            "excluded_from_gate": startup.get("excluded_from_gate"),
            "workload_ids": startup.get("workload_ids"),
            "measurements": startup.get("measurements") if passed else None,
        },
        "gate": gate or (bundle.get("gate") or {}),
    }


def _relativise(path: Path) -> str:
    """Repo-relative when possible, independent of the caller's cwd."""
    root = Path(__file__).resolve().parents[3]
    try:
        return path.resolve().relative_to(root).as_posix()
    except ValueError:
        return str(path.resolve())


def write_bundle(bundle: dict, directory: Path) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "performance-evidence.json"
    path.write_text(json.dumps(bundle, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path


def load_bundle(path: Path) -> dict:
    if path.is_dir():
        path = path / "performance-evidence.json"
    return json.loads(path.read_text(encoding="utf-8"))
