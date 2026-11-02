"""Interleaved, equivalence-gated measurement loop."""

from __future__ import annotations

import json
import random
import time
from dataclasses import dataclass, field
from pathlib import Path

from . import env as envmod
from .execute import Executor, Observation
from .stats import (
    bootstrap_ratio_ci,
    geometric_mean,
    median,
    noise_level,
    summarize,
)
from .workloads import (
    CommandSpec,
    Workload,
    canonical,
    oracle_payload,
    rust_payload,
    sha_payload,
)


@dataclass
class SidePreflight:
    observation: Observation
    payload: object
    payload_sha: str


@dataclass
class WorkloadResult:
    workload: Workload
    status: str = "not_measured"
    reason: str = ""
    equivalence_diff: str = ""
    rust_preflight: SidePreflight | None = None
    oracle_preflight: SidePreflight | None = None
    rust_samples: list[Observation] = field(default_factory=list)
    oracle_samples: list[Observation] = field(default_factory=list)
    truncated: bool = False
    extra: dict = field(default_factory=dict)

    def record(self, *, floors: dict) -> dict:
        w = self.workload
        record: dict = {
            "id": w.id,
            "description": w.description,
            "fixture": w.fixture,
            "class": w.klass,
            "equivalence": w.equivalence,
            "required": w.required,
            "gate_eligible": w.counts_for_gate(),
            "status": self.status,
            "reason": self.reason,
            "equivalence_diff": self.equivalence_diff,
            "notes": w.notes,
            "pending_reason": w.pending_reason,
            "expected_exit": w.expected_exit,
            "repeats_requested": w.repeats,
        }
        if self.rust_preflight:
            record["rust_preflight"] = self.rust_preflight.observation.as_dict()
            record["oracle_preflight"] = self.oracle_preflight.observation.as_dict() if self.oracle_preflight else None
            record["payload_sha256"] = {
                "rust": self.rust_preflight.payload_sha,
                "oracle": self.oracle_preflight.payload_sha if self.oracle_preflight else None,
            }
        rust_s = [o.duration_s for o in self.rust_samples]
        oracle_s = [o.duration_s for o in self.oracle_samples]
        record["samples"] = {
            "rust": [o.as_dict() for o in self.rust_samples],
            "oracle": [o.as_dict() for o in self.oracle_samples],
        }
        if rust_s and oracle_s:
            rust_summary = summarize(rust_s)
            oracle_summary = summarize(oracle_s)
            rng = random.Random(20261102)
            ci_low, ci_high = bootstrap_ratio_ci(oracle_s, rust_s, rng=rng)
            ratio = median(oracle_s) / median(rust_s) if median(rust_s) else None
            floor_rust = floors.get("rust")
            floor_oracle = floors.get(("python", tuple(w.oracle.floor_modules))) if w.oracle else None
            rust_excess = (median(rust_s) - floor_rust) if floor_rust is not None else None
            oracle_excess = (median(oracle_s) - floor_oracle) if floor_oracle is not None else None
            work_ratio = None
            if (
                rust_excess is not None
                and oracle_excess is not None
                and rust_excess > 0.005
                and oracle_excess > 0.005
            ):
                work_ratio = oracle_excess / rust_excess
            noise = noise_level(rust_summary["cv"], oracle_summary["cv"], ci_low, ci_high)
            record["timing"] = {
                "rust": rust_summary,
                "oracle": oracle_summary,
                "median_ratio_oracle_over_rust": ratio,
                "median_ratio_ci95": [ci_low, ci_high],
                "noise": noise,
                "floor_control": {
                    "rust_median_s": floor_rust,
                    "oracle_median_s": floor_oracle,
                    "rust_excess_s": rust_excess,
                    "oracle_excess_s": oracle_excess,
                    "work_excess_ratio": work_ratio,
                    "work_excess_note": (
                        "floor-subtracted diagnostic; null when either side's work excess "
                        "is below 5 ms (import/launch cost dominates)"
                    ),
                },
            }
        record["truncated"] = self.truncated
        record["extra"] = self.extra
        return record


def _cwd_for(workload: Workload, fixture_paths: dict[str, Path], empty_cwd: Path) -> Path:
    if workload.fixture is None:
        return empty_cwd
    return fixture_paths[workload.fixture]


def _run_once(
    executor: Executor,
    spec: CommandSpec,
    *,
    cwd: Path,
    warmup: bool,
) -> Observation:
    before_load = envmod.loadavg()
    observation = executor.run(
        protocol=spec.protocol,
        argv=spec.argv,
        timeout_s=spec.timeout_s,
        warmup=warmup,
        cwd=cwd,
    )
    observation.load_before = before_load
    observation.load_after = envmod.loadavg()
    return observation

def _preflight_side(observation: Observation, workload: Workload) -> SidePreflight:
    if workload.equivalence == "contract":
        payload: object = {"_raw": observation.stdout.decode("utf-8", "replace")[:4000]}
    else:
        payload = rust_payload(observation.stdout) if observation.protocol == "rust" else oracle_payload(observation.stdout)
        if workload.normalize is not None:
            payload = workload.normalize(payload)
    return SidePreflight(observation=observation, payload=payload, payload_sha=sha_payload(payload))


def _exit_matches(workload: Workload, rust: Observation, oracle: Observation) -> tuple[bool, str]:
    if rust.timed_out or oracle.timed_out:
        return False, "a preflight timed out"
    if workload.equivalence == "contract":
        for side, obs in (("rust", rust), ("oracle", oracle)):
            if obs.exit_code != 0:
                return False, f"{side} preflight exit {obs.exit_code}, expected 0"
            if not obs.stdout.strip():
                return False, f"{side} preflight produced empty stdout"
        return True, ""
    if workload.expected_exit is not None:
        if rust.exit_code != workload.expected_exit or oracle.exit_code != workload.expected_exit:
            return False, (
                f"preflight exit codes rust={rust.exit_code} oracle={oracle.exit_code}, "
                f"expected {workload.expected_exit}"
            )
        return True, ""
    if rust.exit_code != oracle.exit_code:
        return False, f"preflight exit codes differ: rust={rust.exit_code} oracle={oracle.exit_code}"
    return True, ""


def _verify_sample(
    workload: Workload,
    observation: Observation,
    expected: SidePreflight,
) -> tuple[bool, str]:
    if observation.timed_out:
        return False, "measurement timed out"
    if observation.exit_code != expected.observation.exit_code:
        return False, f"exit code drifted: {observation.exit_code} != {expected.observation.exit_code}"
    if workload.equivalence == "contract":
        if observation.exit_code != 0 or not observation.stdout.strip():
            return False, "contract sample lost exit 0 or non-empty stdout"
        return True, ""
    payload = rust_payload(observation.stdout) if observation.protocol == "rust" else oracle_payload(observation.stdout)
    if workload.normalize is not None:
        payload = workload.normalize(payload)
    digest = sha_payload(payload)
    if digest != expected.payload_sha:
        return False, f"normalized payload changed between preflight and sample ({digest[:12]} != {expected.payload_sha[:12]})"
    return True, ""


def _measure_floor(
    executor: Executor,
    *,
    protocol: str,
    argv: list[str],
    reps: int,
    cwd: Path,
) -> float:
    samples = []
    for _ in range(max(1, reps)):
        obs = executor.run(protocol=protocol, argv=argv, timeout_s=120, cwd=cwd)
        if not obs.timed_out:
            samples.append(obs.duration_s)
    return median(samples) if samples else float("nan")


def run_benchmark(
    *,
    workloads: list[Workload],
    executor: Executor,
    fixture_paths: dict[str, Path],
    empty_cwd: Path,
    adapters_dir: Path,
    rust_binary: Path,
    oracle_python: Path,
    reps: int,
    warmups: int,
    seed: int,
    max_seconds: float,
    floor_reps: int = 3,
    progress: callable | None = None,
) -> dict:
    started = time.time()
    rng = random.Random(seed)
    results: list[WorkloadResult] = []
    runnable = [w for w in workloads if w.runnable]

    # --- floors (protocol launch/import cost; reported, never used to rescue a ratio)
    floors: dict = {
        "rust": _measure_floor(
            executor,
            protocol="rust",
            argv=[str(rust_binary), "--version"],
            reps=floor_reps,
            cwd=empty_cwd,
        )
    }
    module_cache: dict[tuple[str, ...], float] = {}
    for workload in runnable:
        if workload.oracle and workload.oracle.floor_modules:
            key = tuple(workload.oracle.floor_modules)
            if key not in module_cache:
                argv = [str(oracle_python), str(adapters_dir / "py_floor.py")]
                for module in key:
                    argv.extend(["--module", module])
                module_cache[key] = _measure_floor(
                    executor, protocol="py-adapter", argv=argv, reps=floor_reps, cwd=empty_cwd
                )
            floors[("python", key)] = module_cache[key]
    if progress:
        progress("floors", 0, 0, f"rust={floors['rust']:.4f}s python_modules={len(module_cache)}")

    # --- preflight + equivalence gate (before any timing is trusted)
    prepared: list[WorkloadResult] = []
    total_preflight = len(runnable) * 2
    step = 0
    for workload in runnable:
        result = WorkloadResult(workload=workload)
        cwd = _cwd_for(workload, fixture_paths, empty_cwd)
        try:
            rust_obs = _run_once(executor, workload.rust, cwd=cwd, warmup=True)
            oracle_obs = _run_once(executor, workload.oracle, cwd=cwd, warmup=True)
        except Exception as exc:  # noqa: BLE001 - measurement boundary
            result.status = "error"
            result.reason = f"preflight execution failed: {exc}"
            prepared.append(result)
            step += 2
            continue
        step += 2
        ok_exit, exit_reason = _exit_matches(workload, rust_obs, oracle_obs)
        if not ok_exit:
            result.status = "preflight_failed"
            result.reason = exit_reason
            result.rust_preflight = _preflight_side(rust_obs, workload)
            result.oracle_preflight = _preflight_side(oracle_obs, workload)
            prepared.append(result)
            if progress:
                progress("preflight", step, total_preflight, f"{workload.id}: {exit_reason}")
            continue
        rust_pre = _preflight_side(rust_obs, workload)
        oracle_pre = _preflight_side(oracle_obs, workload)
        result.rust_preflight = rust_pre
        result.oracle_preflight = oracle_pre
        equivalent, diff = (True, "")
        if workload.equivalence != "contract":
            assert workload.compare is not None
            equivalent, diff = workload.compare(rust_pre.payload, oracle_pre.payload)
        if not equivalent:
            result.status = "equivalence_failed"
            result.reason = "preflight outputs are not equivalent"
            result.equivalence_diff = diff
            prepared.append(result)
            if progress:
                progress("preflight", step, total_preflight, f"{workload.id}: NOT EQUIVALENT: {diff}")
            continue
        result.status = "measured_pending_samples"
        prepared.append(result)
        if progress:
            progress("preflight", step, total_preflight, f"{workload.id}: equivalent")

    # --- additional warmups (preflight already warmed once)
    for result in prepared:
        if result.status != "measured_pending_samples":
            continue
        workload = result.workload
        cwd = _cwd_for(workload, fixture_paths, empty_cwd)
        for _ in range(max(0, warmups - 1)):
            _run_once(executor, workload.rust, cwd=cwd, warmup=True)
            _run_once(executor, workload.oracle, cwd=cwd, warmup=True)

    # --- interleaved randomised measurement
    tasks: list[tuple[WorkloadResult, str, int]] = []
    for result in prepared:
        if result.status != "measured_pending_samples":
            continue
        per_workload = result.workload.repeats or reps
        for rep in range(per_workload):
            tasks.append((result, "rust", rep))
            tasks.append((result, "oracle", rep))
    rng.shuffle(tasks)
    measured_workloads = {id(result) for result, _, _ in tasks}
    done_counts: dict[int, dict[str, int]] = {
        key: {"rust": 0, "oracle": 0} for key in measured_workloads
    }
    requested_counts: dict[int, int] = {}
    for result, _, _ in tasks:
        requested_counts[id(result)] = (result.workload.repeats or reps)

    measurement_start = time.time()
    processed = 0
    stopped = False
    for result, side, rep in tasks:
        if stopped:
            result.truncated = True
            continue
        if time.time() - measurement_start > max_seconds:
            stopped = True
            result.truncated = True
            continue
        workload = result.workload
        cwd = _cwd_for(workload, fixture_paths, empty_cwd)
        spec = workload.rust if side == "rust" else workload.oracle
        expected = result.rust_preflight if side == "rust" else result.oracle_preflight
        try:
            observation = _run_once(executor, spec, cwd=cwd, warmup=False)
        except Exception as exc:  # noqa: BLE001
            result.status = "error"
            result.reason = f"{side} sample {rep} execution failed: {exc}"
            stopped = True
            continue
        valid, reason = _verify_sample(workload, observation, expected)
        if not valid:
            result.status = "nondeterministic"
            result.reason = f"{side} sample {rep}: {reason}"
            stopped = False
            continue
        if side == "rust":
            result.rust_samples.append(observation)
        else:
            result.oracle_samples.append(observation)
        done_counts[id(result)][side] += 1
        processed += 1
        if progress and processed % 10 == 0:
            progress(
                "measure",
                processed,
                len(tasks),
                f"{workload.id} {side} {rep} {observation.duration_s:.3f}s",
            )

    elapsed = time.time() - started
    for result in prepared:
        if result.status == "measured_pending_samples":
            requested = result.workload.repeats or reps
            rust_n = len(result.rust_samples)
            oracle_n = len(result.oracle_samples)
            if rust_n == 0 or oracle_n == 0:
                result.status = "not_measured"
                result.reason = result.reason or "no valid samples collected"
            elif result.truncated or rust_n < requested or oracle_n < requested:
                result.status = "measured_incomplete"
                result.reason = (
                    f"collected rust={rust_n} oracle={oracle_n} of requested {requested}; "
                    "budget or interruption truncated the run"
                )
            else:
                result.status = "ok"

    records = [result.record(floors=floors) for result in prepared]
    # Preserve registry order for readability, including workloads that were
    # never runnable (pending) so the report never silently omits a gap.
    by_id = {record["id"]: record for record in records}
    ordered = []
    for workload in workloads:
        if workload.id in by_id:
            ordered.append(by_id[workload.id])
        else:
            ordered.append(
                {
                    "id": workload.id,
                    "description": workload.description,
                    "fixture": workload.fixture,
                    "class": workload.klass,
                    "equivalence": workload.equivalence,
                    "required": workload.required,
                    "gate_eligible": workload.counts_for_gate(),
                    "status": "pending" if workload.equivalence == "pending" else "not_run",
                    "reason": workload.pending_reason or "",
                    "pending_reason": workload.pending_reason,
                    "notes": workload.notes,
                    "equivalence_diff": "",
                }
            )

    gate_eligible = [r for r in ordered if r["gate_eligible"] and r.get("timing")]
    geomean = None
    if gate_eligible:
        geomean = geometric_mean([r["timing"]["median_ratio_oracle_over_rust"] for r in gate_eligible])

    return {
        "schema": "spkbench.run.v1",
        "seed": seed,
        "repetitions": reps,
        "warmups": warmups,
        "budget_seconds": max_seconds,
        "started_at_epoch": started,
        "elapsed_seconds": elapsed,
        "truncated": stopped,
        "task_count": len(tasks),
        "processed_tasks": processed,
        "floors_seconds": {
            "rust": floors.get("rust"),
            "python_by_modules": {
                ",".join(key): value for key, value in module_cache.items()
            },
        },
        "gate_geomean_ratio": geomean,
        "workloads": ordered,
        "load": envmod.capture_load("run_end"),
    }


def summarise_console(record: dict) -> str:
    lines = []
    for item in record["workloads"]:
        timing = item.get("timing")
        if timing:
            ratio = timing["median_ratio_oracle_over_rust"]
            lines.append(
                f"{item['id']:<36} {item['class']:<8} {item['equivalence']:<10} "
                f"ratio={ratio:8.2f}x  rust={timing['rust']['median_s']*1000:9.2f}ms "
                f"py={timing['oracle']['median_s']*1000:10.2f}ms  noise={timing['noise']:<8} "
                f"{item['status']}"
            )
        elif item["status"] == "pending":
            lines.append(f"{item['id']:<36} pending   {item['equivalence']:<10} {item['reason'][:80]}")
        else:
            lines.append(f"{item['id']:<36} {item['status']:<24} {item.get('reason','')[:90]}")
    if record.get("gate_geomean_ratio"):
        lines.append(f"gate-eligible geomean ratio: {record['gate_geomean_ratio']:.3f}x")
    return "\n".join(lines)


def dump_payloads(record: dict, path: Path) -> None:
    path.write_text(json.dumps(record, indent=2, sort_keys=True), encoding="utf-8")
