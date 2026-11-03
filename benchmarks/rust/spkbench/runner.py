"""Paired, equivalence-gated measurement loop.

Differences from the first harness (each one closes a review finding):

* **Paired interleaving.** Repetition *i* runs the Python and Rust side adjacent
  in a randomized order and records the pair index. Paired ratios let shared
  machine noise cancel; uncertainty is computed from pairs, not from two
  independent medians.
* **Mutation execution.** A mutating workload runs in a fresh clone of the
  pristine fixture for *every* invocation. The timer wraps only the command:
  clone and post-state scan are outside the measured window, so clone cost is
  credited to neither side.
* **Per-sample post-state.** Each mutation sample records the post-state delta
  hash, which must equal the side's preflight delta. A mutation that sometimes
  writes (or writes something else) is nondeterministic, never a valid sample.
* **Non-vacuous schemas.** Every preflight and every sample is validated against
  the workload's declared payload/state schema. Two empty documents, two
  unparseable blobs or two `{}` payloads fail before timing.
* **Explicit blocked states.** A frozen workload with no native behaviour, or a
  workload whose required binary is missing, is recorded as blocked with the
  reason. It is never silently dropped and never timed as an equal refusal.
"""

from __future__ import annotations

import base64
import json
import random
import shutil
import time
from dataclasses import dataclass, field
from pathlib import Path

from . import env as envmod
from . import expectations as expectmod
from .execute import Executor, Observation
from .fixtures import Fixture
from .mutation import clone_fixture, diff_manifests, ignored_artifacts, tree_manifest
from .stats import median, summarize
from .workloads import (
    CommandSpec,
    Workload,
    oracle_payload,
    rust_payload,
    sha_payload,
    workload_command_summary,
)


@dataclass
class SidePreflight:
    observation: Observation
    payload: object
    payload_sha: str
    problems: list[str] = field(default_factory=list)
    post_state: dict | None = None
    post_state_sha: str | None = None
    post_state_problems: list[str] = field(default_factory=list)


@dataclass
class WorkloadResult:
    workload: Workload
    status: str = "not_measured"
    reason: str = ""
    equivalence_diff: str = ""
    rust_preflight: SidePreflight | None = None
    oracle_preflight: SidePreflight | None = None
    rust_samples: list[dict] = field(default_factory=list)
    oracle_samples: list[dict] = field(default_factory=list)
    rejected: list[dict] = field(default_factory=list)
    truncated: bool = False
    extra: dict = field(default_factory=dict)

    def record(self, *, floors: dict) -> dict:
        w = self.workload
        record: dict = {
            "id": w.id,
            "description": w.description,
            "fixture": w.fixture,
            "fixture_id": w.fixture_id,
            "class": w.klass,
            "equivalence": w.equivalence,
            "required": w.required,
            "source": w.source,
            "measurement": w.measurement,
            "gate_eligible": w.counts_for_gate(),
            "status": self.status,
            "reason": self.reason,
            "blocked_reason": w.runtime_blocker or w.pending_reason,
            "pending_reason": w.pending_reason,
            "equivalence_diff": self.equivalence_diff,
            "notes": w.notes,
            "expected_exit": w.expected_exit,
            "repetitions_requested": w.repeats,
            "command_argv": workload_command_summary(
                w, fixture=str(fixture_path_for(w)) if w.fixture_id else None
            ),
            "samples": {
                "rust": list(self.rust_samples),
                "python": list(self.oracle_samples),
            },
            "rejected_samples": list(self.rejected),
            "schema_problems": {
                "rust": list(self.rust_preflight.problems) if self.rust_preflight else [],
                "python": list(self.oracle_preflight.problems) if self.oracle_preflight else [],
            },
            "truncated": self.truncated,
            "extra": self.extra,
        }
        if self.rust_preflight:
            record["preflight"] = {
                "rust": self.rust_preflight.observation.as_dict(),
                "python": self.oracle_preflight.observation.as_dict() if self.oracle_preflight else None,
                "payload_sha256": {
                    "rust": self.rust_preflight.payload_sha,
                    "python": self.oracle_preflight.payload_sha if self.oracle_preflight else None,
                },
                "payload": {
                    "rust": self.rust_preflight.payload,
                    "python": self.oracle_preflight.payload if self.oracle_preflight else None,
                },
                "post_state": {
                    "rust": self.rust_preflight.post_state,
                    "python": self.oracle_preflight.post_state if self.oracle_preflight else None,
                },
                "post_state_sha256": {
                    "rust": self.rust_preflight.post_state_sha,
                    "python": self.oracle_preflight.post_state_sha if self.oracle_preflight else None,
                },
                "post_state_problems": {
                    "rust": self.rust_preflight.post_state_problems,
                    "python": self.oracle_preflight.post_state_problems
                    if self.oracle_preflight
                    else [],
                },
            }
        rust_s = [item["duration_s"] for item in self.rust_samples]
        python_s = [item["duration_s"] for item in self.oracle_samples]
        if rust_s and python_s:
            floor_rust = floors.get("rust")
            floor_oracle = (
                floors.get(("python", tuple(w.oracle.floor_modules))) if w.oracle else None
            )
            rust_excess = (median(rust_s) - floor_rust) if floor_rust is not None else None
            python_excess = (median(python_s) - floor_oracle) if floor_oracle is not None else None
            work_ratio = None
            if (
                rust_excess is not None
                and python_excess is not None
                and rust_excess > 0
                and python_excess > 0
            ):
                work_ratio = python_excess / rust_excess
            record["timing"] = {
                "rust": summarize(rust_s),
                "python": summarize(python_s),
                "median_ratio_oracle_over_rust": median(python_s) / median(rust_s)
                if median(rust_s)
                else None,
                "floor_control": {
                    "rust_median_s": floor_rust,
                    "python_median_s": floor_oracle,
                    "rust_excess_s": rust_excess,
                    "python_excess_s": python_excess,
                    "work_excess_ratio": work_ratio,
                },
            }
        return record


def fixture_path_for(workload: Workload) -> Path | None:
    return workload.fixture_path


def _attach_fixture_path(workload: Workload, fixture_paths: dict[str, Path]) -> None:
    workload.fixture_path = fixture_paths.get(workload.fixture) if workload.fixture else None


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


def _extract_payload(observation: Observation, workload: Workload) -> object:
    if workload.equivalence == "contract":
        return {"_raw": observation.stdout.decode("utf-8", "replace")[:4000]}
    payload = (
        rust_payload(observation.stdout)
        if observation.protocol == "rust"
        else oracle_payload(observation.stdout)
    )
    if workload.normalize is not None:
        payload = workload.normalize(payload)
    return payload


@dataclass
class _Invocation:
    observation: Observation
    payload: object
    payload_sha: str
    post_state: dict | None
    post_state_sha: str | None
    post_state_problems: list[str]
    problems: list[str]
    clone: str | None


def _required_post_state_files(
    clone_path: Path, workload: Workload, before: dict[str, str], after: dict[str, str]
) -> dict[str, str]:
    """Base64 of the actual bytes of every schema-constrained post-state file.

    Storing the bytes (not only their hash) is what lets the verifier re-apply
    the registry's ``min_bytes`` and ``json_payload`` schema to what the
    implementation wrote, without the original working tree.
    """
    schema = (workload.expectations or {}).get("post_state") or {}
    stored: dict[str, str] = {}
    for rel in expectmod.required_post_state_paths(schema, workload.params):
        # Only files that the stored manifests describe are evidence; anything
        # else would be an unbound byte blob.
        if rel not in after:
            continue
        path = clone_path / rel
        try:
            stored[rel] = base64.b64encode(path.read_bytes()).decode("ascii")
        except OSError:
            continue
    return stored


def _invoke(
    *,
    executor: Executor,
    workload: Workload,
    side: str,
    fixture_path: Path | None,
    run_dir: Path,
    counter: int,
    warmup: bool,
) -> _Invocation:
    spec = workload.rust if side == "rust" else workload.oracle
    if spec is None:
        raise ValueError(f"{workload.id}: missing {side} command spec")
    post_state = None
    post_state_sha = None
    post_state_problems: list[str] = []
    clone_path: Path | None = None
    cwd = fixture_path or (run_dir / "empty-cwd")
    if not cwd.exists():
        cwd.mkdir(parents=True, exist_ok=True)
    argv = list(spec.argv)
    if workload.mutation is not None:
        if fixture_path is None:
            raise ValueError(f"{workload.id}: mutation workload has no fixture")
        prefixes = workload.mutation.ignored_post_state
        clone_path = clone_fixture(
            fixture_path,
            run_dir / "mutations" / workload.id.replace("/", "-") / f"{side}-{counter}",
        )
        argv = workload.mutation.rewrite(spec.argv, clone_path)
        content_roots = (fixture_path, clone_path)
        before = tree_manifest(
            clone_path, ignored_prefixes=prefixes, roots=(fixture_path,)
        )
        load_before = envmod.loadavg()
        observation = executor.run(
            protocol=spec.protocol,
            argv=argv,
            timeout_s=spec.timeout_s,
            warmup=warmup,
            cwd=clone_path,
        )
        observation.load_before = load_before
        observation.load_after = envmod.loadavg()
        # Post-state work happens outside the timed window.
        after = tree_manifest(
            clone_path, ignored_prefixes=prefixes, roots=(fixture_path,)
        )
        delta = diff_manifests(before, after)
        post_state = {
            "delta": delta,
            "before": before,
            "after": after,
            "params": dict(workload.params or {}),
            "content_roots": [str(item) for item in content_roots],
            "required_files": _required_post_state_files(clone_path, workload, before, after),
            "ignored_artifacts": ignored_artifacts(clone_path, prefixes),
        }
        post_state_sha = sha_payload(delta)
        post_state_problems = expectmod.validate_post_state(
            before=before,
            after=after,
            delta=delta,
            root=clone_path,
            schema=(workload.expectations or {}).get("post_state") or {},
            params=workload.params,
            content_roots=content_roots,
        )
        shutil.rmtree(clone_path, ignore_errors=True)
    else:
        observation = _run_once(executor, spec, cwd=cwd, warmup=warmup)

    payload = _extract_payload(observation, workload)
    payload_sha = sha_payload(payload)
    problems: list[str] = []
    expectations = workload.expectations or {}
    if workload.equivalence == "contract":
        problems.extend(expectmod.validate_stdout(observation.stdout, expectations.get("stdout") or {}))
    else:
        problems.extend(expectmod.validate_payload(payload, expectations.get("payload") or {}))
    problems.extend(post_state_problems)
    return _Invocation(
        observation=observation,
        payload=payload,
        payload_sha=payload_sha,
        post_state=post_state,
        post_state_sha=post_state_sha,
        post_state_problems=post_state_problems,
        problems=problems,
        clone=str(clone_path) if clone_path else None,
    )


def _side_preflight(invocation: _Invocation) -> SidePreflight:
    return SidePreflight(
        observation=invocation.observation,
        payload=invocation.payload,
        payload_sha=invocation.payload_sha,
        problems=invocation.problems,
        post_state=invocation.post_state,
        post_state_sha=invocation.post_state_sha,
        post_state_problems=invocation.post_state_problems,
    )


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
    invocation: _Invocation,
    expected: SidePreflight,
) -> tuple[bool, str]:
    observation = invocation.observation
    if observation.timed_out:
        return False, "measurement timed out"
    if observation.exit_code != expected.observation.exit_code:
        return False, f"exit code drifted: {observation.exit_code} != {expected.observation.exit_code}"
    if invocation.payload_sha != expected.payload_sha:
        return (
            False,
            f"normalized payload changed between preflight and sample "
            f"({invocation.payload_sha[:12]} != {expected.payload_sha[:12]})",
        )
    if workload.mutation is not None:
        if invocation.post_state_sha != expected.post_state_sha:
            return False, "post-state delta changed between preflight and sample"
        if invocation.post_state_problems:
            return False, "post-state violates the declared schema: " + invocation.post_state_problems[0]
    if invocation.problems:
        return False, "sample violates the declared schema: " + invocation.problems[0]
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


def _sample_record(invocation: _Invocation, *, side: str, repetition: int, order: int) -> dict:
    observation = invocation.observation
    return {
        "side": side,
        "pair_index": repetition,
        "order": order,
        "protocol": observation.protocol,
        "exit_code": observation.exit_code,
        "duration_s": observation.duration_s,
        "cpu_user_s": observation.cpu_user_s,
        "cpu_sys_s": observation.cpu_sys_s,
        "timed_out": observation.timed_out,
        "stdout_sha256": __import__("hashlib").sha256(observation.stdout).hexdigest(),
        "stdout_bytes": len(observation.stdout),
        "payload_sha256": invocation.payload_sha,
        "post_state_sha256": invocation.post_state_sha,
        "load_before": observation.load_before,
        "load_after": observation.load_after,
    }


def run_benchmark(
    *,
    workloads: list[Workload],
    executor: Executor,
    fixture_paths: dict[str, Path],
    fixtures: dict[str, Fixture],
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
    for workload in workloads:
        _attach_fixture_path(workload, fixture_paths)
    runnable = [workload for workload in workloads if workload.runnable]

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
    counter = 0
    for workload in runnable:
        result = WorkloadResult(workload=workload)
        fixture_path = fixture_paths.get(workload.fixture) if workload.fixture else None
        try:
            counter += 1
            rust_inv = _invoke(
                executor=executor, workload=workload, side="rust",
                fixture_path=fixture_path, run_dir=executor.run_dir, counter=counter, warmup=True,
            )
            counter += 1
            oracle_inv = _invoke(
                executor=executor, workload=workload, side="oracle",
                fixture_path=fixture_path, run_dir=executor.run_dir, counter=counter, warmup=True,
            )
        except Exception as exc:  # noqa: BLE001 - measurement boundary
            result.status = "error"
            result.reason = f"preflight execution failed: {exc}"
            prepared.append(result)
            step += 2
            continue
        step += 2
        result.rust_preflight = _side_preflight(rust_inv)
        result.oracle_preflight = _side_preflight(oracle_inv)
        ok_exit, exit_reason = _exit_matches(workload, rust_inv.observation, oracle_inv.observation)
        if not ok_exit:
            result.status = "preflight_failed"
            result.reason = exit_reason
            prepared.append(result)
            if progress:
                progress("preflight", step, total_preflight, f"{workload.id}: {exit_reason}")
            continue
        if rust_inv.problems or oracle_inv.problems:
            result.status = "schema_failed"
            result.reason = "preflight schema failure: " + "; ".join(
                (rust_inv.problems + oracle_inv.problems)[:3]
            )
            prepared.append(result)
            if progress:
                progress("preflight", step, total_preflight, f"{workload.id}: SCHEMA {result.reason[:80]}")
            continue
        equivalent, diff = (True, "")
        if workload.equivalence != "contract":
            assert workload.compare is not None
            equivalent, diff = workload.compare(rust_inv.payload, oracle_inv.payload)
        if equivalent and workload.mutation is not None:
            if rust_inv.post_state_sha != oracle_inv.post_state_sha:
                equivalent = False
                diff = "post-state delta differs between rust and python"
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
        fixture_path = fixture_paths.get(workload.fixture) if workload.fixture else None
        for _ in range(max(0, warmups - 1)):
            counter += 1
            _invoke(
                executor=executor, workload=workload, side="rust",
                fixture_path=fixture_path, run_dir=executor.run_dir, counter=counter, warmup=True,
            )
            counter += 1
            _invoke(
                executor=executor, workload=workload, side="oracle",
                fixture_path=fixture_path, run_dir=executor.run_dir, counter=counter, warmup=True,
            )

    # --- paired, interleaved, randomized-within-pair measurement
    tasks: list[tuple[WorkloadResult, str, int, int]] = []
    for result in prepared:
        if result.status != "measured_pending_samples":
            continue
        per_workload = result.workload.repeats or reps
        for repetition in range(per_workload):
            order = ["rust", "oracle"]
            rng.shuffle(order)
            for position, side in enumerate(order):
                tasks.append((result, side, repetition, position))

    measurement_start = time.time()
    processed = 0
    stopped = False
    for result, side, repetition, position in tasks:
        if stopped:
            result.truncated = True
            continue
        if time.time() - measurement_start > max_seconds:
            stopped = True
            result.truncated = True
            continue
        workload = result.workload
        fixture_path = fixture_paths.get(workload.fixture) if workload.fixture else None
        expected = result.rust_preflight if side == "rust" else result.oracle_preflight
        try:
            counter += 1
            invocation = _invoke(
                executor=executor, workload=workload, side=side,
                fixture_path=fixture_path, run_dir=executor.run_dir, counter=counter, warmup=False,
            )
        except Exception as exc:  # noqa: BLE001
            result.status = "error"
            result.reason = f"{side} sample {repetition} execution failed: {exc}"
            stopped = True
            continue
        valid, reason = _verify_sample(workload, invocation, expected)
        if not valid:
            result.rejected.append(
                {"side": side, "pair_index": repetition, "reason": reason}
            )
            result.status = "nondeterministic"
            result.reason = f"{side} sample {repetition}: {reason}"
            continue
        record = _sample_record(invocation, side=side, repetition=repetition, order=invocation.observation.order or 0)
        if side == "rust":
            result.rust_samples.append(record)
        else:
            result.oracle_samples.append(record)
        processed += 1
        if progress and processed % 10 == 0:
            progress(
                "measure", processed, len(tasks),
                f"{workload.id} {side} pair={repetition} {invocation.observation.duration_s:.3f}s",
            )

    elapsed = time.time() - started
    for result in prepared:
        if result.status == "measured_pending_samples":
            requested = result.workload.repeats or reps
            rust_n = len(result.rust_samples)
            python_n = len(result.oracle_samples)
            if rust_n == 0 or python_n == 0:
                if result.truncated or stopped:
                    result.status = "measured_incomplete"
                    result.reason = result.reason or (
                        "the time budget stopped the run before any valid sample was collected"
                    )
                else:
                    result.status = "not_measured"
                    result.reason = result.reason or "no valid samples collected"
            elif result.truncated or rust_n < requested or python_n < requested:
                result.status = "measured_incomplete"
                result.reason = (
                    f"collected rust={rust_n} python={python_n} of requested {requested}; "
                    "budget or interruption truncated the run"
                )
            else:
                result.status = "ok"

    records = [result.record(floors=floors) for result in prepared]
    by_id = {record["id"]: record for record in records}
    ordered: list[dict] = []
    for workload in workloads:
        if workload.id in by_id:
            ordered.append(by_id[workload.id])
        else:
            ordered.append(
                {
                    "id": workload.id,
                    "description": workload.description,
                    "fixture": workload.fixture,
                    "fixture_id": workload.fixture_id,
                    "class": workload.klass,
                    "equivalence": workload.equivalence,
                    "required": workload.required,
                    "source": workload.source,
                    "measurement": workload.measurement,
                    "gate_eligible": workload.counts_for_gate(),
                    "status": "blocked" if workload.runtime_blocker else (
                        "absent_native" if workload.measurement == "absent" else "not_run"
                    ),
                    "reason": workload.runtime_blocker or workload.pending_reason or "",
                    "blocked_reason": workload.runtime_blocker or workload.pending_reason,
                    "pending_reason": workload.pending_reason,
                    "notes": workload.notes,
                    "equivalence_diff": "",
                    "command_argv": workload_command_summary(workload),
                    "samples": {"rust": [], "python": []},
                }
            )
    return {
        "schema": "spkbench.run.v2",
        "run_id": f"{int(started)}-{seed}",
        "seed": seed,
        "repetitions": reps,
        "warmups": warmups,
        "budget_seconds": max_seconds,
        "started_at_epoch": started,
        "elapsed_seconds": elapsed,
        "truncated": stopped or any(result.truncated for result in prepared),
        "task_count": len(tasks),
        "processed_tasks": processed,
        "pairing": "interleaved-randomized-within-pair",
        "floors_seconds": {
            "rust": floors.get("rust"),
            "python_by_modules": {",".join(key): value for key, value in module_cache.items()},
        },
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
                f"{item['id']:<34} {item['class']:<7} {item['equivalence']:<10} "
                f"ratio={ratio:8.2f}x  rust={timing['rust']['median_s']*1000:9.2f}ms "
                f"py={timing['python']['median_s']*1000:10.2f}ms  n={len(item['samples']['rust'])}/"
                f"{len(item['samples']['python'])}  {item['status']}"
            )
        else:
            lines.append(
                f"{item['id']:<34} {item['status']:<20} {(item.get('reason') or '')[:80]}"
            )
    return "\n".join(lines)


def dump_payloads(record: dict, path: Path) -> None:
    path.write_text(json.dumps(record, indent=2, sort_keys=True), encoding="utf-8")
