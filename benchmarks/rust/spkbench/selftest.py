"""End-to-end negative tests for the certification gate.

The original suite unit-tested ``evaluate_gate`` with hand-written dicts, which
is why it passed while the gate could be fooled by a filtered report, a deleted
``samples.jsonl``, two empty payloads or a startup command. Every test here now
goes through the real pipeline: a synthetic measurement run is turned into a
bundle by :func:`spkbench.report.write_evidence` (which stores payloads,
post-state deltas, raw samples and a manifest), the written evidence is
(optionally) tampered with, and :func:`spkbench.certify.evaluate_bundle` must
recompute the same numbers from the stored bytes and refuse.

The synthetic environment is fabricated only at the *probe* seam
(:class:`_SyntheticProbe`): the live native source is replaced by a probe with
the same shape, so every acceptance check still runs. Nothing here disables a
check, weakens a floor or invents a second bundle format.

``test_required_oracle_and_binaries_present`` deliberately **fails** when the
lock-exact oracle interpreter or the release binaries are missing. A skip would
report a green suite for an environment that cannot measure anything.
"""

from __future__ import annotations

import base64
import copy
import hashlib
import json
import shutil
import tempfile
from dataclasses import dataclass
from pathlib import Path

from . import certify, evidence as evidencemod, expectations as expectmod
from .live import harness_manifest
from .fixtures import RECIPES, Fixture, FixtureBuilder, content_manifest
from .registry import PerformanceRegistry, canonical_json, registry_digest
from .report import write_evidence
from .runner import _exit_matches
from .stats import regression_verdict
from .workloads import Workload, compare_canonical, drop_paths, first_difference

REPO = Path(__file__).resolve().parents[3]
ORACLE = REPO / ".venv-rust-oracle" / "bin" / "python"
BINARIES = {
    "spec-kitty": REPO / "target/release/spec-kitty",
    "spec-kitty-rust": REPO / "target/release/spec-kitty-rust",
}
BASELINE = "9aed3a8012cb5eb22c0128b14d525039f95fea7a"
CANDIDATE = "a" * 40


class CheckFailure(AssertionError):
    pass


def _check(condition: bool, message: str) -> None:
    if not condition:
        raise CheckFailure(message)


# ---------------------------------------------------------------------------
# Synthetic registry, run and environment
# ---------------------------------------------------------------------------

GATE_IDS = ("gate.alpha", "gate.beta", "gate.mutation")
OTHER_REQUIRED = ("cli.projection",)

SYNTHETIC_RECIPE_KIND = "synthetic"
SYNTHETIC_FIXTURE_FILES = 10

#: One cache entry per (fixture name, seed): the two-root regeneration is
#: deterministic, so the *observation* is computed once per test process.
_FIXTURE_FACTS_CACHE: dict[tuple[str, int], list[dict]] = {}


def _write_synthetic_fixture(root: Path, name: str, seed: int) -> Fixture:
    """Write real fixture bytes for a unit-scoped synthetic recipe."""
    path = root / name
    path.mkdir(parents=True, exist_ok=True)
    for index in range(SYNTHETIC_FIXTURE_FILES):
        (path / f"file-{index:02d}.txt").write_text(
            f"{name}:{seed}:{index}\n", encoding="utf-8"
        )
    fixture = Fixture(
        name=name,
        path=path,
        recipe={"kind": SYNTHETIC_RECIPE_KIND, "seed": seed},
    )
    fixture.content = content_manifest(path)
    return fixture


def _regenerate_fixture(name: str, seed: int, root: Path) -> dict:
    if name in RECIPES:
        return FixtureBuilder(root / name, seed=seed).get(name).as_dict(include_manifest=True)
    return _write_synthetic_fixture(root, name, seed).as_dict(include_manifest=True)


def _fixture_facts(name: str, seed: int) -> list[dict]:
    """Regenerate a fixture at two fresh audit roots (cached per process)."""
    key = (name, seed)
    if key not in _FIXTURE_FACTS_CACHE:
        root = Path(tempfile.mkdtemp(prefix="spkbench-fixture-facts-"))
        try:
            _FIXTURE_FACTS_CACHE[key] = [
                _regenerate_fixture(name, seed, root / variant)
                for variant in ("a", "b" + "x" * 24)
            ]
        finally:
            shutil.rmtree(root, ignore_errors=True)
    return copy.deepcopy(_FIXTURE_FACTS_CACHE[key])


def synthetic_fixture_record(name: str, seed: int) -> dict:
    """The recorded fixture-table row a real generator produces."""
    record = _fixture_facts(name, seed)[0]
    record.pop("path", None)
    return record


def synthetic_fixture_manifest(name: str, seed: int) -> dict[str, str]:
    """The real per-file portable manifest of a generated fixture.

    Synthetic post-state records use this instead of fabricating a before
    manifest, so the positive control exercises the same binding the production
    verifier applies to a measured mutation input.
    """
    return copy.deepcopy(_fixture_facts(name, seed)[0]["manifest"])


def synthetic_fixture_observer(bundle: dict) -> dict[str, dict]:
    """Fixture observation seam for unit-scoped synthetic recipes.

    It has the same shape and semantics as :func:`spkbench.certify._verify_fixtures`:
    every fixture is regenerated at two roots from the frozen (or synthetic test)
    recipe, and the digest, recipe facts and file count are compared with the
    recorded table. Only *observation* is injected; no comparison is skipped and
    no bundle flag can select this path.
    """
    seed = int((bundle.get("environment") or {}).get("seed") or 0)
    outcomes: dict[str, dict] = {}
    for name, recorded in sorted((bundle.get("fixtures") or {}).items()):
        record = recorded if isinstance(recorded, dict) else {}
        generated = _fixture_facts(name, seed)
        digests = [item.get("content_digest") for item in generated]
        manifests = [item.get("manifest") for item in generated]
        outcomes[name] = {
            "recorded": record.get("content_digest"),
            "regenerated": digests,
            "portable": len(set(digests)) == 1,
            "manifests_equal": manifests[0] == manifests[1],
            "manifest": manifests[0],
            "manifest_sha256": generated[0].get("manifest_sha256"),
            "matches": len(set(digests)) == 1 and digests[0] == record.get("content_digest"),
            "recorded_recipe": record.get("recipe"),
            "regenerated_recipe": generated[0].get("recipe"),
            "recorded_file_count": record.get("file_count"),
            "regenerated_file_count": generated[0].get("file_count"),
        }
    return outcomes


def _synthetic_registry(
    *,
    absent: tuple[str, ...] = (),
    gate_ids: tuple[str, ...] = GATE_IDS,
    minimum_strict: int | None = None,
) -> PerformanceRegistry:
    workloads = []
    for wid in gate_ids:
        measurement = "mutation" if wid.endswith("mutation") else "read"
        entry = {
            "id": wid,
            "category": "work",
            "required": True,
            "strict_equivalence": True,
            "gate_eligible": True,
            "equivalence": "strict",
            "measurement": measurement,
            "fixture": "synthetic-fixture",
            "params": {},
            "normalizer": "identity",
            "comparator": "exact",
            "native": {
                "state": "implemented",
                "binary": "spec-kitty",
                "route": f"{wid} route",
                "argv": ["{binary:spec-kitty}", "run", wid],
                "blocked_by": None,
            },
            "oracle": {
                "kind": "py-adapter",
                "argv": ["{oracle}", "adapter.py", "--workload", wid],
                "adapter": None,
                "floor_modules": ["synthetic"],
            },
            "expectations": {
                "exit": 0,
                "payload": {"required_keys": ["kind"], "equals": {"kind": wid}},
            },
        }
        if measurement == "mutation":
            entry["expectations"]["post_state"] = {
                "required_paths": ["out/state.json"],
                "json_payload": {"required_keys": ["event_count"]},
                "ignored_prefixes": ["locks/"],
                "expected_additions": ["out/state.json"],
                "expected_changes": [],
                "expected_removals": [],
                "exclusion_ruling": "synthetic lock exclusion",
            }
        workloads.append(entry)
    for wid in absent:
        workloads.append(
            {
                "id": wid,
                "category": "workflow",
                "required": True,
                "strict_equivalence": True,
                "gate_eligible": True,
                "equivalence": "strict",
                "measurement": "absent",
                "fixture": None,
                "params": {},
                "normalizer": None,
                "comparator": None,
                "native": {
                    "state": "absent",
                    "binary": None,
                    "route": None,
                    "argv": None,
                    "blocked_by": "synthetic: no native workflow",
                },
                "oracle": None,
                "expectations": None,
                "notes": [],
            }
        )
    for wid in OTHER_REQUIRED:
        workloads.append(
            {
                "id": wid,
                "category": "work",
                "required": True,
                "strict_equivalence": False,
                "gate_eligible": False,
                "equivalence": "projection",
                "measurement": "read",
                "fixture": "synthetic-fixture",
                "params": {},
                "normalizer": "identity",
                "comparator": "exact",
                "native": {
                    "state": "implemented",
                    "binary": "spec-kitty",
                    "route": "cli projection",
                    "argv": ["{binary:spec-kitty}", "projection"],
                    "blocked_by": None,
                },
                "oracle": {
                    "kind": "py-adapter",
                    "argv": ["{oracle}", "adapter.py", "--projection"],
                    "adapter": None,
                    "floor_modules": ["synthetic"],
                },
                "expectations": {"exit": 0, "payload": {"required_keys": ["count"]}},
            }
        )
    for wid in ("startup.version", "startup.help"):
        workloads.append(
            {
                "id": wid,
                "category": "startup",
                "required": True,
                "strict_equivalence": False,
                "gate_eligible": False,
                "equivalence": "contract",
                "measurement": "startup",
                "fixture": None,
                "params": {},
                "normalizer": "raw_stdout",
                "comparator": "contract",
                "native": {
                    "state": "implemented",
                    "binary": "spec-kitty",
                    "route": wid,
                    "argv": [
                        "{binary:spec-kitty}",
                        "--version" if wid.endswith("version") else "--help",
                    ],
                    "blocked_by": None,
                },
                "oracle": {
                    "kind": "py-cli",
                    "argv": ["{oracle}", "-m", "specify_cli", "--version"],
                    "adapter": None,
                    "floor_modules": [],
                },
                "expectations": {
                    "exit": 0,
                    "stdout": {"required_substrings": ["spec"], "min_bytes": 4},
                },
            }
        )
    required = [item["id"] for item in workloads]
    gate_ids = [item["id"] for item in workloads if item["gate_eligible"]]
    measured_gate = [wid for wid in gate_ids if wid not in absent]
    data = {
        "schema": "skit.rust.performance-registry.v1",
        "registry_version": 1,
        "frozen": True,
        "frozen_baseline": {"repository": "spec-kitty/spec-kitty", "commit": BASELINE},
        "checker": {
            "path": "scripts/rust/check_completion.py",
            "required_workload_ids": required,
            "gate_workload_ids": gate_ids,
            "startup_workload_ids": ["startup.version", "startup.help"],
            "min_samples_per_side": 20,
            "min_strict_workloads": len(measured_gate),
            "min_speedup": 3.0,
            "max_regression_fraction": 0.10,
        },
        "policy": {
            "gate": "geometric_mean_speedup",
            "minimum_speedup": 3.0,
            "max_per_workload_regression_fraction": 0.10,
            "minimum_strict_workloads": minimum_strict
            if minimum_strict is not None
            else len(measured_gate),
            "minimum_samples_per_side": 20,
            "confidence_level": 0.95,
            "bootstrap_iterations": 200,
            "max_side_cv": 1.0,
            "max_load_1m": 50.0,
            "max_load_spread_1m": 10.0,
            "min_work_excess_seconds": 0.005,
            "min_work_excess_ratio": 1.25,
            "require_paired_repetitions": True,
            "require_interleaved_repetitions": True,
            "require_machine_and_load_context": True,
            "require_result_equivalence": True,
            "require_payload_schema": True,
            "require_post_state_equivalence": True,
            "require_fixture_identity": True,
            "require_portable_fixtures": True,
            "require_toolchain_identity": True,
            "require_build_receipt": True,
            "require_clean_source": True,
            "require_registry_digest": True,
            "require_recomputation": True,
            "require_work_excess_geomean": True,
            "require_startup_separate": True,
            "projection_counts_toward_gate": False,
            "startup_counts_toward_gate": False,
            "require_raw_samples": True,
        },
        "workloads": workloads,
        "diagnostics": [],
    }
    data["digest"] = "sha256:" + registry_digest(data)
    return PerformanceRegistry(Path("synthetic-registry.json"), data, file_sha256="0" * 64)


def _synthetic_identity() -> dict:
    return {
        "status": "exact",
        "problems": [],
        "source": {"oracle_imports_from_repo_src": True},
        "worktree": {"head": CANDIDATE, "dirty": False, "dirty_paths": [], "dirty_count": 0},
        "dependencies": {"status": "exact", "mismatches": [], "installed_not_in_lock": []},
        "cargo": {
            "build_inputs": {"file_count": 1, "digest": "b" * 64},
            "lock_sha256": "c" * 64,
            "toolchain": {
                "release": "1.99.1",
                "host": "x86_64-unknown-linux-gnu",
                "rustc": {"stdout": "rustc 1.99.1"},
                "cargo": {"stdout": "cargo 1.99.1"},
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
            "baseline": {"baseline": BASELINE, "src_matches_baseline": True},
            "verify": {"ok": True},
            "version": {"stdout": "Python 3.11.15"},
            "python_real_sha256": "f" * 64,
        },
        "packs": {"exists": True, "digest": "1" * 64},
        "build_receipt": {
            "digest": "2" * 64,
            "path": "/synthetic/build-receipt.json",
            "verification": {"ok": True, "problems": []},
        },
        "uv_lock_sha256": "3" * 64,
        "machine": {"hostname": "synthetic"},
        "stability": {"binary_stable": True, "source_stable": True},
    }


class _SyntheticProbe:
    """A native-source probe over a fabricated environment (tests only).

    It implements the same contract as :class:`spkbench.live.NativeSourceProbe`
    and simply reports the synthetic environment consistently. The verifier runs
    every check against it, which is exactly the point: the positive control
    exercises the full acceptance path, it does not switch acceptance off.
    """

    def __init__(
        self,
        identity: dict | None = None,
        *,
        head: str | None = None,
        ancestor: bool = True,
        changed_inputs: tuple[str, ...] = (),
        dirty: tuple[str, ...] = (),
        receipt_ok: bool = True,
        harness: dict | None = None,
        registry_sha256: str | None = "0" * 64,
    ) -> None:
        self.identity = copy.deepcopy(identity or _synthetic_identity())
        self.head = head
        self.ancestor = ancestor
        self.changed_inputs = changed_inputs
        self.dirty = dirty
        self.receipt_ok = receipt_ok
        self.harness = harness
        self.registry_sha256 = registry_sha256

    def observe(  # noqa: ARG002
        self, *, receipt_path=None, binary_paths=None, registry_path=None,
        oracle_python=None, packs_root=None, baseline_commit=None,
    ) -> dict:
        del receipt_path, binary_paths, oracle_python, packs_root, baseline_commit
        identity = self.identity
        candidate = identity["worktree"]["head"]
        receipt = identity.get("build_receipt")
        receipt_record = None
        if receipt:
            receipt_record = {
                "digest": receipt.get("digest"),
                "repo": str(REPO),
                "source": {
                    "head": candidate,
                    "dirty": False,
                    "build_inputs_digest": identity["cargo"]["build_inputs"]["digest"],
                    "cargo_lock_sha256": identity["cargo"]["lock_sha256"],
                },
                "binaries": {
                    name: {
                        "sha256": record.get("sha256"),
                        "size": record.get("size"),
                        "mtime_ns": record.get("mtime_ns"),
                    }
                    for name, record in identity["binary_provenance"]["binaries"].items()
                },
                "verification": {
                    "ok": self.receipt_ok,
                    "problems": [] if self.receipt_ok else ["synthetic receipt mismatch"],
                },
            }
        return {
            "head": self.head or candidate,
            "dirty_paths": list(self.dirty),
            "dirty_count": len(self.dirty),
            "build_inputs_digest": identity["cargo"]["build_inputs"]["digest"],
            "build_inputs_file_count": identity["cargo"]["build_inputs"].get("file_count"),
            "cargo_lock_sha256": identity["cargo"]["lock_sha256"],
            "toolchain": {
                "release": identity["cargo"]["toolchain"].get("release"),
                "host": identity["cargo"]["toolchain"].get("host"),
                "rustc_stdout": identity["cargo"]["toolchain"]["rustc"].get("stdout"),
                "cargo_stdout": identity["cargo"]["toolchain"]["cargo"].get("stdout"),
            },
            "binaries": {
                name: {
                    "path": record.get("path"),
                    "exists": True,
                    "sha256": record.get("sha256"),
                    "size": record.get("size"),
                    "mtime_ns": record.get("mtime_ns"),
                    "predates_source": record.get("predates_source", False),
                }
                for name, record in identity["binary_provenance"]["binaries"].items()
            },
            "receipt": receipt_record,
            "receipt_path": (receipt or {}).get("path"),
            "harness": dict(self.harness) if self.harness is not None else harness_manifest(REPO),
            "registry_sha256": self.registry_sha256,
            "registry_path": str(registry_path) if registry_path else None,
            "oracle": {
                "python": "/synthetic/oracle/python",
                "python_real_sha256": identity["oracle"].get("python_real_sha256"),
                "verify": identity["oracle"].get("verify"),
                "baseline": identity["oracle"].get("baseline"),
            },
            "packs": identity.get("packs"),
            "uv_lock_sha256": identity.get("uv_lock_sha256"),
        }

    def is_ancestor(self, candidate: str) -> bool:  # noqa: ARG002 - probe contract
        return self.ancestor

    def changed_measured_inputs(self, candidate: str) -> list[str]:  # noqa: ARG002
        return list(self.changed_inputs)


def synthetic_probe(identity: dict, registry_path: Path | str | None = None) -> _SyntheticProbe:
    """Test seam: a native-source probe over an explicitly synthetic environment.

    Import this from a test fixture that already declares its own synthetic
    identity (for example ``scripts/rust/tests/support/synthetic_certification``)
    and pass it to ``report.write_evidence(live_probe=...)`` and
    ``certify.evaluate_bundle(live_probe=...)``. Every acceptance check still
    runs; only the observations are fabricated, never disabled.
    """
    registry_sha = None
    if registry_path is not None and Path(registry_path).is_file():
        registry_sha = evidencemod.sha256_file(Path(registry_path))
    return _SyntheticProbe(identity, registry_sha256=registry_sha)


@dataclass
class _Sample:
    duration_s: float
    pair_index: int
    load_before: dict
    load_after: dict


def _payload(wid: str) -> dict:
    if wid.startswith("startup."):
        return {"_raw": "spec-kitty 1.0.0\nUsage:\n  spec-kitty <command>\n"}
    if wid == "cli.projection":
        return {"count": 3}
    return {"kind": wid}


def _digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()  # noqa: TID251 - synthetic content hash


def _post_state(wid: str, entry: dict) -> dict:  # noqa: ARG001 - symmetric fixture helper
    """A complete, self-consistent mutation post-state observation.

    The registry's declared post-state schema is *not* lowered: the synthetic
    mutation writes real bytes for the required path, the before/after manifests
    are the full content diff that produced the delta, and the delta carries the
    per-path hashes so the verifier can re-derive it.
    """
    if entry.get("measurement") != "mutation":
        return {}
    post_schema = (entry.get("expectations") or {}).get("post_state") or {}
    required = [
        expectmod.substitute(item, entry.get("params") or {})
        for item in post_schema.get("required_paths") or []
    ]
    rel = required[0] if required else "out/state.json"
    content = {
        "event_count": 2800,
        "work_packages": [
            {"id": f"WP{index:03d}", "lane": "done"} for index in range(200)
        ],
        "summary": {"done": 200},
    }
    data = (json.dumps(content, indent=2, sort_keys=True) + "\n").encode("utf-8")
    fixture_name = entry.get("fixture") or "synthetic-fixture"
    before = synthetic_fixture_manifest(fixture_name, 7)
    after = {**before, rel: _digest(data)}
    delta = {
        "added": [rel],
        "changed": [],
        "removed": [],
        "added_sha256": {rel: after[rel]},
        "removed_sha256": {},
        "changed_sha256": {},
    }
    return {
        "delta": delta,
        "before": before,
        "after": after,
        "params": dict(entry.get("params") or {}),
        "content_roots": [],
        "required_files": {rel: base64.b64encode(data).decode("ascii")},
    }


def _record(
    wid: str,
    *,
    entry: dict | None = None,
    ratio: float = 4.0,
    pairs: int = 20,
    status: str = "ok",
    rust_median: float = 0.02,
    floor: float = 0.005,
    floor_rust: float | None = None,
    floor_python: float | None = None,
    python_argv: list[str] | None = None,
    python_values: list[float] | None = None,
    order_base: int = 400,
) -> dict:
    floor_rust = floor if floor_rust is None else floor_rust
    floor_python = floor if floor_python is None else floor_python
    rust = [_Sample(rust_median, index, {"1m": 1.0}, {"1m": 1.0}) for index in range(pairs)]
    python_durations = python_values or [rust_median * ratio] * pairs
    python = [
        _Sample(value, index, {"1m": 1.0}, {"1m": 1.0}) for index, value in enumerate(python_durations)
    ]
    payload = _payload(wid)
    payload_sha = hashlib.sha256(canonical_json(payload).encode()).hexdigest()  # noqa: TID251
    entry = entry or {"measurement": "read"}
    post_state = _post_state(wid, entry)
    post_sha = (
        hashlib.sha256(canonical_json(post_state["delta"]).encode()).hexdigest()  # noqa: TID251
        if post_state
        else None
    )

    def sample(item: _Sample, side: str) -> dict:
        return {
            "side": side,
            "pair_index": item.pair_index,
            "order": order_base + item.pair_index * 2 + (0 if side == "rust" else 1),
            "duration_s": item.duration_s,
            "cpu_user_s": 0.0,
            "cpu_sys_s": 0.0,
            "timed_out": False,
            "exit_code": 0,
            "stdout_sha256": hashlib.sha256(b"synthetic").hexdigest(),  # noqa: TID251
            "payload_sha256": payload_sha,
            "post_state_sha256": post_sha,
            "load_before": item.load_before,
            "load_after": item.load_after,
        }

    rust_median_actual = statistics_median([item.duration_s for item in rust])
    python_median_actual = statistics_median([item.duration_s for item in python])
    return {
        "id": wid,
        "status": status,
        "reason": "",
        "equivalence_diff": "",
        "fixture_id": None if wid.startswith("startup.") else "synthetic-fixture",
        "measurement": entry.get("measurement") or "read",
        "equivalence": entry.get("equivalence") or "strict",
        "command_argv": {
            "rust": list(entry["native"]["argv"]),
            "python": python_argv or list(entry["oracle"]["argv"]),
        },
        "expected_exit": 0,
        "samples": {
            "rust": [sample(item, "rust") for item in rust],
            "python": [sample(item, "python") for item in python],
        },
        "rejected_samples": [],
        "schema_problems": {"rust": [], "python": []},
        "preflight": {
            "rust": {"protocol": "rust", "exit_code": 0, "timed_out": False},
            "python": {"protocol": "py-adapter", "exit_code": 0, "timed_out": False},
            "payload": {"rust": payload, "python": payload},
            "payload_sha256": {"rust": payload_sha, "python": payload_sha},
            "post_state": {"rust": post_state or None, "python": post_state or None},
            "post_state_sha256": {
                "rust": post_sha,
                "python": post_sha,
            },
            "post_state_problems": {"rust": [], "python": []},
        },
        "timing": {
            "rust": {"n": pairs, "median_s": rust_median_actual},
            "python": {"n": pairs, "median_s": python_median_actual},
            "median_ratio_oracle_over_rust": python_median_actual / rust_median_actual,
            "floor_control": {
                "rust_median_s": floor_rust,
                "python_median_s": floor_python,
                "rust_excess_s": rust_median_actual - floor_rust,
                "python_excess_s": python_median_actual - floor_python,
                "work_excess_ratio": (python_median_actual - floor_python)
                / (rust_median_actual - floor_rust),
            },
        },
    }


def statistics_median(values: list[float]) -> float:
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle]) / 2


def _synthetic_run(
    *,
    ratios: dict[str, float] | None = None,
    pairs: int = 20,
    floor_rust: float | None = None,
    floor_python: float | None = None,
    python_values: dict[str, list[float]] | None = None,
) -> dict:
    ratios = ratios or dict.fromkeys(GATE_IDS, 4.0)
    registry = _synthetic_registry()
    entries = {item["id"]: item for item in registry.workloads}
    workloads = []
    # The executor's observation counter is global: floor probes, preflights and
    # warmups consume orders before and between measurement pairs. Start after an
    # offset instead of pretending the first sample is order 0.
    order_cursor = 400
    for wid in (*GATE_IDS, *OTHER_REQUIRED, "startup.version", "startup.help"):
        entry = entries[wid]
        values = None
        if python_values and wid in python_values:
            values = python_values[wid]
        workloads.append(
            _record(
                wid,
                entry=entry,
                ratio=ratios.get(
                    wid,
                    4.0 if wid in GATE_IDS else (2.0 if wid in OTHER_REQUIRED else 50.0),
                ),
                pairs=pairs,
                floor_rust=floor_rust,
                floor_python=floor_python,
                python_values=values,
                order_base=order_cursor,
            )
        )
        order_cursor += pairs * 2 + 3
    return {
        "schema": "spkbench.run.v2",
        "run_id": "synthetic-run",
        "seed": 7,
        "repetitions": pairs,
        "warmups": 1,
        "budget_seconds": 60.0,
        "truncated": False,
        "fixtures_unchanged": True,
        "pairing": "interleaved-randomized-within-pair",
        "floors_seconds": {
            "rust": floor_rust if floor_rust is not None else 0.005,
            "python_by_modules": {
                "synthetic": floor_python if floor_python is not None else 0.005
            },
        },
        "workloads": workloads,
        "load": {"loadavg": {"1m": 1.0}},
    }


def _bundle(
    *,
    registry: PerformanceRegistry | None = None,
    run: dict | None = None,
    identity: dict | None = None,
    probe: _SyntheticProbe | None = None,
) -> tuple[dict, Path]:
    registry = registry or _synthetic_registry()
    run = run or _synthetic_run()
    identity = identity or _synthetic_identity()
    probe = probe or _SyntheticProbe(identity)
    out = Path(tempfile.mkdtemp(prefix="spkbench-selftest-"))
    written = write_evidence(
        out_dir=out,
        run=run,
        identity=identity,
        fixtures={"synthetic-fixture": synthetic_fixture_record("synthetic-fixture", 7)},
        registry=registry,
        harness={"name": "spkbench", "version": "test"},
        repo=REPO,
        packs_root=None,
        live_probe=probe,
        verify_fixtures=True,
        fixture_observer=synthetic_fixture_observer,
    )
    return written["bundle"], out


def _reseal(bundle: dict, root: Path | None = None) -> dict:
    """Rewrite a mutated bundle and its manifest as the writer would.

    Resealing is what a forger would do, so the tests can show that a consistent
    hash chain still does not rescue a forged claim.
    """
    bundle["self_sha256"] = certify.self_digest(bundle)
    if root is not None:
        path = root / "performance-evidence.json"
        path.write_text(json.dumps(bundle, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        manifest_path = root / "manifest.json"
        if manifest_path.is_file():
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["bundle"]["sha256"] = certify._sha256_file(path)
            manifest["artifacts"] = bundle.get("artifacts") or {}
            manifest["payloads"] = bundle.get("payload_artifacts") or {}
            manifest["post_state"] = bundle.get("post_state_artifacts") or {}
            manifest_path.write_text(
                json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
    return bundle


def _evaluate(
    bundle: dict,
    root: Path,
    registry: PerformanceRegistry | None = None,
    probe: _SyntheticProbe | None = None,
) -> dict:
    registry = registry or _synthetic_registry()
    return certify.evaluate_bundle(
        bundle,
        registry,
        repo=REPO,
        live_probe=probe or _SyntheticProbe(),
        verify_fixtures=True,
        fixture_observer=synthetic_fixture_observer,
        artifacts_root=root,
        require_artifacts=True,
        require_manifest=True,
    )


def _row(bundle: dict, wid: str) -> dict:
    for row in bundle["workloads"]:
        if row["id"] == wid:
            return row
    raise KeyError(wid)


def _cleanup(root: Path) -> None:
    shutil.rmtree(root, ignore_errors=True)


# ---------------------------------------------------------------------------
# Environment
# ---------------------------------------------------------------------------


def test_required_oracle_and_binaries_present() -> str:
    missing = [str(BINARIES["spec-kitty"]), str(BINARIES["spec-kitty-rust"])]
    missing = [path for path in missing if not Path(path).exists()]
    hints = []
    if not ORACLE.exists():
        hints.append(f"oracle interpreter {ORACLE} is missing (provision .venv-rust-oracle)")
    if missing:
        hints.append(
            "release binaries missing: " + ", ".join(missing) + " (run `bench.py build`)"
        )
    _check(not hints, "required measurement environment is absent: " + "; ".join(hints))
    return "oracle interpreter and both release binaries are present"


# ---------------------------------------------------------------------------
# Positive control
# ---------------------------------------------------------------------------


def test_positive_control_passes() -> str:
    bundle, root = _bundle()
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "PASS",
            f"expected PASS, got {result['gate']['verdict']}: {result['gate']['reasons'][:5]}",
        )
        _check(result["recomputed"]["strict_workload_count"] == 3, "expected 3 gate workloads")
        _check(
            result["gate"]["evidence_recomputed"] is True,
            "the gate must report recomputed evidence",
        )
    finally:
        _cleanup(root)
    return "3 strict workloads at 4x, stored payloads, paired samples and a live probe pass"


# ---------------------------------------------------------------------------
# Completeness (missing rows / samples / evidence)
# ---------------------------------------------------------------------------


def test_pending_blocks_even_with_fast_pairs() -> str:
    registry = _synthetic_registry(absent=("gate.absent",))
    bundle, root = _bundle(registry=registry)
    try:
        for row in bundle["workloads"]:
            if row["id"] == "gate.absent":
                row["status"] = "absent_native"
        _reseal(bundle, root)
        result = _evaluate(bundle, root, registry)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"expected BLOCKED, got {result['gate']['verdict']}",
        )
        _check(
            any("gate.absent" in reason for reason in result["gate"]["reasons"]),
            "block reason should name the absent workload",
        )
    finally:
        _cleanup(root)
    return "an absent required workflow blocks even when every measured pair is 4x"


def test_filtered_subset_blocks() -> str:
    bundle, root = _bundle()
    try:
        bundle["workloads"] = [row for row in bundle["workloads"] if row["id"] != "gate.beta"]
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"subset run must BLOCK, got {result['gate']['verdict']}",
        )
        _check(
            any("gate.beta" in reason for reason in result["gate"]["reasons"]),
            "missing row must be named",
        )
    finally:
        _cleanup(root)
    return "deleting a required workload row blocks instead of certifying a subset"


def test_missing_raw_samples_blocks() -> str:
    run = _synthetic_run()
    for record in run["workloads"]:
        if record["id"] == "gate.alpha":
            record["samples"] = {"python": [], "rust": []}
            record["status"] = "not_measured"
    bundle, root = _bundle(run=run)
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"missing samples must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "removing raw samples blocks the gate"


def test_insufficient_samples_blocks() -> str:
    bundle, root = _bundle(run=_synthetic_run(pairs=19))
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"n=19 must BLOCK, got {result['gate']['verdict']}",
        )
        _check(
            any("need >= 20" in reason for reason in result["gate"]["reasons"]),
            "sample floor must be named",
        )
    finally:
        _cleanup(root)
    return "19 pairs per side does not satisfy the 20-sample floor"


def test_missing_build_receipt_blocks() -> str:
    identity = _synthetic_identity()
    identity["build_receipt"] = None
    bundle, root = _bundle(identity=identity, probe=_SyntheticProbe(identity))
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"missing receipt must BLOCK, got {result['gate']['verdict']}",
        )
        _check(
            any("build receipt" in reason for reason in result["gate"]["reasons"]),
            "receipt must be named",
        )
    finally:
        _cleanup(root)
    return "a binary without a build receipt cannot be certified"


def test_identity_drift_blocks() -> str:
    identity = _synthetic_identity()
    identity["status"] = "drift"
    identity["problems"] = ["source tree is dirty"]
    bundle, root = _bundle(identity=identity, probe=_SyntheticProbe(identity))
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"identity drift must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "recorded identity drift blocks the measurement"


# ---------------------------------------------------------------------------
# Degenerate / forged evidence
# ---------------------------------------------------------------------------


def test_degenerate_payloads_are_rejected() -> str:
    schema = {"required_keys": ["kind"], "nonempty_keys": ["kind"]}
    for payload in ({}, {"_unparseable": "x"}, {"other": 1}):
        problems = expectmod.validate_payload(payload, schema)
        _check(bool(problems), f"degenerate payload {payload!r} must fail the schema")
    _check(expectmod.validate_payload({"kind": "step"}, schema) == [], "valid payload must pass")
    return "empty, unparseable and unrelated payloads fail the declared schema"


def test_two_sided_empty_output_is_not_equivalence() -> str:
    spec = Workload(
        id="contract",
        description="",
        fixture=None,
        klass="startup",
        equivalence="contract",
        required=False,
    )
    empty = dataclass_observation(0, b"", False)
    ok, reason = _exit_matches(spec, empty, empty)  # type: ignore[arg-type]
    _check(not ok and "empty stdout" in reason, f"two empty sides must fail, got {ok} {reason}")
    return "two empty outputs are not equivalence"


def dataclass_observation(exit_code: int, stdout: bytes, timed_out: bool):
    @dataclass
    class Observation:
        exit_code: int
        stdout: bytes
        timed_out: bool

    return Observation(exit_code, stdout, timed_out)


def test_degenerate_schema_status_fails() -> str:
    bundle, root = _bundle()
    try:
        row = _row(bundle, "gate.alpha")
        row["schema_problems"] = {"rust": ["payload is missing required key 'kind'"], "python": []}
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"schema failure must FAIL, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a workload whose payload failed its schema cannot pass"


def test_forged_speedup_fails() -> str:
    bundle, root = _bundle()
    try:
        _row(bundle, "gate.alpha")["speedup"] = 42.0
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"forged speedup must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("does not match recomputed" in reason for reason in result["gate"]["reasons"]),
            "mismatch must be named",
        )
    finally:
        _cleanup(root)
    return "a speedup that does not recompute from its samples fails"


def test_forged_geomean_fails() -> str:
    bundle, root = _bundle()
    try:
        bundle["aggregate"]["geometric_mean_speedup"] = 99.0
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"forged geomean must FAIL, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a geometric mean that does not recompute fails"


def test_tampered_self_hash_fails() -> str:
    bundle, root = _bundle()
    try:
        bundle["aggregate"]["geometric_mean_speedup"] = 99.0
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"tampering must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("self-hash" in reason for reason in result["gate"]["reasons"]),
            "self-hash must be named",
        )
    finally:
        _cleanup(root)
    return "editing the bundle without resealing breaks the self-hash"


def test_tampered_raw_samples_artifact_fails() -> str:
    bundle, root = _bundle()
    try:
        samples = root / "samples.jsonl"
        samples.write_text(samples.read_text() + '{"workload": "gate.alpha", "side": "rust"}\n')
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] in {"FAIL", "BLOCKED"},
            f"artifact tamper must FAIL/BLOCK, got {result['gate']['verdict']}",
        )
        details = result["gate"]["blockers"] + result["gate"]["failures"]
        _check(
            any("sha256 does not match" in reason or "samples.jsonl" in reason for reason in details),
            "artifact hash mismatch must be named",
        )
    finally:
        _cleanup(root)
    return "a samples.jsonl edit breaks the artifact hash and the derivation"


def test_deleted_payload_artifact_blocks() -> str:
    bundle, root = _bundle()
    try:
        records = bundle["payload_artifacts"]
        for record in records.values():
            if record.get("stored"):
                (root / record["path"]).unlink()
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"deleted payloads must BLOCK, got {result['gate']['verdict']}",
        )
        _check(
            any("payload artifact" in reason for reason in result["gate"]["reasons"]),
            "missing payload must be named",
        )
    finally:
        _cleanup(root)
    return "deleting the stored normalized payloads blocks instead of leaving an unbound hash"


def test_rewritten_payload_hash_fails() -> str:
    bundle, root = _bundle()
    try:
        # Rewrite one stored payload's content but keep its declared hash: the
        # canonical hash is recomputed from the file, so the edit is visible.
        record = bundle["payload_artifacts"]["gate.alpha.rust"]
        path = root / record["path"]
        payload = json.loads(path.read_text())
        payload["kind"] = "tampered"
        path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"edited payload content must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("canonical content hash" in reason or "does not match" in reason for reason in result["gate"]["reasons"]),
            "canonical hash mismatch must be named",
        )
    finally:
        _cleanup(root)
    return "a normalized payload edit is detected from its declared canonical hash"


def test_pretty_printed_payload_still_verifies() -> str:
    bundle, root = _bundle()
    try:
        path = root / bundle["payload_artifacts"]["gate.beta.rust"]["path"]
        payload = json.loads(path.read_text())
        path.write_text(json.dumps(payload, indent=4))  # different bytes, same document
        bundle["payload_artifacts"]["gate.beta.rust"]["sha256"] = certify._sha256_file(path)
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "PASS",
            f"reformatting the payload JSON must not change the document: {result['gate']['reasons'][:3]}",
        )
    finally:
        _cleanup(root)
    return "payload representation is not the hash: canonical content is"


def test_forged_paired_arrays_fail() -> str:
    bundle, root = _bundle()
    try:
        row = _row(bundle, "gate.alpha")
        row["paired"]["python"] = [4.0] * 20
        row["paired"]["rust"] = [1.0] * 20
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"forged paired arrays must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("paired" in reason for reason in result["gate"]["reasons"]),
            "the pair reconstruction mismatch must be named",
        )
    finally:
        _cleanup(root)
    return "the paired arrays are reconstructed from sample pair_index values, not trusted"


def test_forged_sample_fields_fail() -> str:
    bundle, root = _bundle()
    try:
        row = _row(bundle, "gate.alpha")
        row["sample_records"]["rust"][0]["payload_sha256"] = "0" * 64
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"forged sample field must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("payload hash" in reason for reason in result["gate"]["reasons"]),
            "per-sample payload binding must be named",
        )
    finally:
        _cleanup(root)
    return "every sample's payload/post-state/exit/timeout is bound to its preflight"


def test_projection_regression_fails() -> str:
    manifest, root = _bundle(run=_synthetic_run(ratios={"cli.projection": 0.05}))
    try:
        result = _evaluate(manifest, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"a 20x slower required projection must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("slower than Python" in reason for reason in result["gate"]["reasons"]),
            "the projection regression must be named",
        )
    finally:
        _cleanup(root)
    return "required non-gate workloads are still subject to the >10% regression rule"


def test_projection_sample_floor_applies() -> str:
    run = _synthetic_run()
    for record in run["workloads"]:
        if record["id"] == "cli.projection":
            record["samples"] = {"python": [], "rust": []}
            record["status"] = "not_measured"
    bundle, root = _bundle(run=run)
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"projection without samples must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "the projection cannot be silently exempt from the raw-sample floor"


def test_startup_only_apparent_win_fails() -> str:
    run = _synthetic_run(ratios=dict.fromkeys(GATE_IDS, 200.0))
    for record in run["workloads"]:
        if record["id"] in GATE_IDS:
            record["command_argv"]["rust"] = ["{binary:spec-kitty}", "--version"]
    bundle, root = _bundle(run=run)
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"startup-as-work must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("startup command" in reason or "stdout-only" in reason for reason in result["gate"]["reasons"]),
            "startup substitution must be named",
        )
    finally:
        _cleanup(root)
    return "a 200x startup command cannot stand in for a work workload"


def test_startup_dominated_work_blocks() -> str:
    run = _synthetic_run(floor_rust=0.0198, floor_python=0.005)
    for record in run["workloads"]:
        if record["id"] in GATE_IDS:
            record["samples"]["rust"] = [{**sample, "duration_s": 0.02} for sample in record["samples"]["rust"]]
            record["samples"]["python"] = [{**sample, "duration_s": 0.08} for sample in record["samples"]["python"]]
    bundle, root = _bundle(run=run)
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"startup-dominated must BLOCK, got {result['gate']['verdict']}",
        )
        _check(
            any("startup-dominated" in reason or "work window" in reason for reason in result["gate"]["reasons"]),
            "startup dominance must be named",
        )
    finally:
        _cleanup(root)
    return "a pair whose work window is below the floor blocks"


def test_projection_cannot_rescue_gate() -> str:
    registry = _synthetic_registry(gate_ids=())
    bundle, root = _bundle(registry=registry)
    try:
        result = _evaluate(bundle, root, registry)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"projection-only must BLOCK, got {result['gate']['verdict']}",
        )
        _check(
            result["recomputed"]["strict_workload_count"] == 0,
            "projection must not enter the gate population",
        )
    finally:
        _cleanup(root)
    return "a 2x projection cannot substitute for strict work"


def test_projection_cannot_be_marked_gate_eligible() -> str:
    bundle, root = _bundle()
    try:
        for row in bundle["workloads"]:
            if row["id"] == "cli.projection":
                row["gate_eligible"] = True
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"registry mismatch must FAIL, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a projection cannot be promoted to gate evidence"


# ---------------------------------------------------------------------------
# Regression / aggregate uncertainty
# ---------------------------------------------------------------------------


def test_material_regression_fails() -> str:
    run = _synthetic_run(ratios={"gate.alpha": 0.5, "gate.beta": 8.0, "gate.mutation": 8.0})
    bundle, root = _bundle(run=run)
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"2x slowdown must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("slower than Python" in reason for reason in result["gate"]["reasons"]),
            "regression must be named",
        )
    finally:
        _cleanup(root)
    return "a statistically material >10% regression fails even with a high geomean"


def test_uncertain_regression_blocks() -> str:
    verdict, message = regression_verdict(0.85, 0.84, 0.95, 0.10)
    _check(verdict == "uncertain", f"expected uncertain, got {verdict}")
    _check("does not exclude" in message, "uncertainty must be explicit")
    return "a regression whose interval reaches the allowed region blocks instead of passing"


def test_noisy_bundle_blocks() -> str:
    bundle, root = _bundle()
    try:
        bundle["environment"]["load_context"] = {
            "samples": 200,
            "median_1m": 12.0,
            "max_1m": 20.0,
            "min_1m": 1.0,
            "spread_1m": 19.0,
        }
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"noisy load must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a loaded machine blocks certification instead of being averaged"


def test_geomean_below_target_fails() -> str:
    run = _synthetic_run(ratios=dict.fromkeys(GATE_IDS, 2.0), floor_rust=0.014, floor_python=0.005)
    bundle, root = _bundle(run=run)
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"2x geomean must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("below 3.0x" in reason for reason in result["gate"]["reasons"]),
            "target must be named",
        )
    finally:
        _cleanup(root)
    return "a sub-3x geometric mean fails"


def test_geomean_ci_below_target_blocks() -> str:
    # A wildly bimodal Python series makes the per-pair interval reach below 3x.
    spike = [0.115 if index % 2 else 0.005 for index in range(20)]
    run = _synthetic_run(
        ratios={"gate.alpha": 3.0, "gate.beta": 4.0, "gate.mutation": 4.0},
        python_values={"gate.alpha": spike},
    )
    bundle, root = _bundle(run=run)
    try:
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"uncertain geomean must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a geomean whose interval reaches below 3x blocks"


# ---------------------------------------------------------------------------
# Registry / schema integrity
# ---------------------------------------------------------------------------


def test_registry_digest_tamper_blocks() -> str:
    bundle, root = _bundle()
    try:
        bundle["registry"]["digest"] = "sha256:" + "0" * 64
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"registry tamper must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a bundle citing a different registry digest blocks"


def test_vacuous_schema_is_rejected() -> str:
    ok, why = expectmod.non_vacuous({"exit": 0, "payload": {"required_keys": []}})
    _check(not ok, "an empty schema must be rejected")
    _check(
        "no key/value/size requirement" in why or "no structural requirement" in why,
        f"reason should be explicit, got {why}",
    )
    _check(
        expectmod.non_vacuous({"exit": 0, "payload": {"required_keys": ["kind"]}})[0],
        "real schema must pass",
    )
    return "schemas that two empty payloads could satisfy are rejected"


def test_extra_required_workload_fails() -> str:
    bundle, root = _bundle()
    try:
        extra = copy.deepcopy(_row(bundle, "gate.alpha"))
        extra["id"] = "unfrozen.workload"
        bundle["workloads"].append(extra)
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"extra required row must FAIL, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a required workload outside the frozen floor fails"


def test_absent_workload_with_samples_fails() -> str:
    registry = _synthetic_registry(absent=("gate.absent",))
    bundle, root = _bundle(registry=registry)
    try:
        for row in bundle["workloads"]:
            if row["id"] == "gate.absent":
                row["status"] = "ok"
                row["samples"] = {"python": [1.0] * 20, "rust": [1.0] * 20}
                row["sample_records"] = {
                    "python": [{"pair_index": index, "duration_s": 0.001} for index in range(20)],
                    "rust": [{"pair_index": index, "duration_s": 0.001} for index in range(20)],
                }
        _reseal(bundle, root)
        result = _evaluate(bundle, root, registry)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"absent-with-samples must FAIL, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "an absent native workload cannot carry samples"


def test_mutation_post_state_must_be_compared() -> str:
    bundle, root = _bundle()
    try:
        row = _row(bundle, "gate.mutation")
        row["post_state"] = {"compared": False, "equal": False}
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"missing post-state must FAIL, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a mutation without post-state equivalence fails"


def test_equivalence_diff_fails() -> str:
    bundle, root = _bundle()
    try:
        _row(bundle, "gate.beta")["equivalence_diff"] = "$.kind: 'a' != 'b'"
        _reseal(bundle, root)
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"equivalence diff must FAIL, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a recorded equivalence difference fails"


def test_equivalence_diff_detects_nested_change() -> str:
    left = {"decision": {"kind": "step", "step_id": "step-0001", "context": {"options": ["a", "b"]}}}
    right = {"decision": {"kind": "step", "step_id": "step-0001", "context": {"options": ["a", "c"]}}}
    diff = first_difference(left, right)
    _check(diff is not None and "options[1]" in diff, f"nested diff missed: {diff}")
    return "nested payload changes are detected with a field path"


def test_drop_paths_is_narrow() -> str:
    payload = {"result": {"status": "resolved", "matches": [{"surface": "x"}]}}
    dropped = drop_paths(payload, ["result.status"])
    _check("status" not in dropped["result"], "declared derived field should be dropped")
    _check(
        dropped["result"]["matches"][0]["surface"] == "x",
        "drop must not remove unrelated fields",
    )
    return "only the declared derived field is removed"


def test_unparseable_rust_envelope_fails() -> str:
    from .workloads import rust_payload

    payload = rust_payload(b"not json")
    _check("_unparseable" in payload, "garbage stdout must be marked unparseable")
    _check("_unparseable" in rust_payload(b"[]"), "non-object stdout must be marked unparseable")
    return "non-JSON and non-object output is rejected, not treated as an empty payload"


def test_canonical_comparator_is_exact() -> str:
    ok, _ = compare_canonical({"a": 1}, {"a": 1})
    _check(ok, "identical payloads must compare equal")
    ok, diff = compare_canonical({"a": 1}, {"a": 2})
    _check(not ok and "$.a" in diff, "different payloads must not compare equal")
    return "the canonical comparator never treats differing payloads as equal"


# ---------------------------------------------------------------------------
# Native-source binding (P0-1)
# ---------------------------------------------------------------------------


def test_stale_candidate_commit_fails() -> str:
    bundle, root = _bundle()
    try:
        result = certify.evaluate_bundle(
            bundle,
            _synthetic_registry(),
            repo=REPO,
            live_probe=_SyntheticProbe(head="f" * 40, ancestor=False),
            artifacts_root=root,
            require_artifacts=True,
            require_manifest=True,
        )
        _check(
            result["gate"]["verdict"] == "FAIL",
            f"a foreign/stale candidate must FAIL, got {result['gate']['verdict']}",
        )
        _check(
            any("foreign history" in reason for reason in result["gate"]["reasons"]),
            "the stale-candidate reason must be named",
        )
    finally:
        _cleanup(root)
    return "a candidate commit that is not an ancestor of HEAD cannot certify"


def test_evidence_only_descendant_is_allowed() -> str:
    bundle, root = _bundle()
    try:
        result = certify.evaluate_bundle(
            bundle,
            _synthetic_registry(),
            repo=REPO,
            live_probe=_SyntheticProbe(head="f" * 40, ancestor=True, changed_inputs=()),
            artifacts_root=root,
            require_artifacts=True,
            require_manifest=True,
        )
        _check(
            result["gate"]["verdict"] == "PASS",
            f"an evidence-only descendant must pass: {result['gate']['reasons'][:5]}",
        )
        _check(
            any("evidence/docs-only" in warning for warning in result["gate"]["warnings"]),
            "the descendant allowance must be visible",
        )
    finally:
        _cleanup(root)
    return "a later commit is accepted only when every measured input is unchanged"


def test_descendant_changing_build_inputs_blocks() -> str:
    bundle, root = _bundle()
    try:
        result = certify.evaluate_bundle(
            bundle,
            _synthetic_registry(),
            repo=REPO,
            live_probe=_SyntheticProbe(
                head="f" * 40, ancestor=True, changed_inputs=("Cargo.lock",)
            ),
            artifacts_root=root,
            require_artifacts=True,
            require_manifest=True,
        )
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"a build-input change after the candidate must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "a later commit that touches a measured input blocks instead of inheriting the verdict"


def test_dirty_measured_input_blocks() -> str:
    bundle, root = _bundle()
    try:
        result = certify.evaluate_bundle(
            bundle,
            _synthetic_registry(),
            repo=REPO,
            live_probe=_SyntheticProbe(dirty=(" M crates/sk-core/src/lib.rs",)),
            artifacts_root=root,
            require_artifacts=True,
            require_manifest=True,
        )
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"uncommitted build-input changes must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "dirty measured code blocks even when the commit is unchanged"


def test_dirty_docs_do_not_block() -> str:
    bundle, root = _bundle()
    try:
        result = certify.evaluate_bundle(
            bundle,
            _synthetic_registry(),
            repo=REPO,
            live_probe=_SyntheticProbe(dirty=(" M docs/rust/VERIFICATION.md",)),
            artifacts_root=root,
            require_artifacts=True,
            require_manifest=True,
        )
        _check(
            result["gate"]["verdict"] == "PASS",
            f"dirty docs must not block: {result['gate']['reasons'][:5]}",
        )
    finally:
        _cleanup(root)
    return "uncommitted documentation does not invalidate a measurement"


def test_harness_code_change_blocks() -> str:
    bundle, root = _bundle()
    try:
        live_harness = dict(harness_manifest(REPO))
        victim = sorted(live_harness)[0]
        live_harness[victim] = "0" * 64
        result = certify.evaluate_bundle(
            bundle,
            _synthetic_registry(),
            repo=REPO,
            live_probe=_SyntheticProbe(harness=live_harness),
            artifacts_root=root,
            require_artifacts=True,
            require_manifest=True,
        )
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"harness drift must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "the harness/adapter code is hashed and cannot silently change"


def test_missing_manifest_blocks() -> str:
    bundle, root = _bundle()
    try:
        (root / "manifest.json").unlink()
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"a missing manifest must BLOCK, got {result['gate']['verdict']}",
        )
    finally:
        _cleanup(root)
    return "the manifest is part of the evidence and is verified, not decorative"


def test_malformed_evidence_blocks_without_crashing() -> str:
    bundle, root = _bundle()
    try:
        bundle["workloads"] = ["not-a-row"]
        result = _evaluate(bundle, root)
        _check(
            result["gate"]["verdict"] == "BLOCKED",
            f"malformed rows must BLOCK, got {result['gate']['verdict']}",
        )
        _check(
            any("malformed" in reason or "without a string id" in reason for reason in result["gate"]["reasons"]),
            "malformed evidence must produce a typed reason",
        )
    finally:
        _cleanup(root)
    return "malformed evidence blocks with a reason instead of raising"


TESTS = [
    test_required_oracle_and_binaries_present,
    test_positive_control_passes,
    test_pending_blocks_even_with_fast_pairs,
    test_filtered_subset_blocks,
    test_missing_raw_samples_blocks,
    test_insufficient_samples_blocks,
    test_missing_build_receipt_blocks,
    test_identity_drift_blocks,
    test_degenerate_payloads_are_rejected,
    test_two_sided_empty_output_is_not_equivalence,
    test_degenerate_schema_status_fails,
    test_forged_speedup_fails,
    test_forged_geomean_fails,
    test_tampered_self_hash_fails,
    test_tampered_raw_samples_artifact_fails,
    test_deleted_payload_artifact_blocks,
    test_rewritten_payload_hash_fails,
    test_pretty_printed_payload_still_verifies,
    test_forged_paired_arrays_fail,
    test_forged_sample_fields_fail,
    test_projection_regression_fails,
    test_projection_sample_floor_applies,
    test_startup_only_apparent_win_fails,
    test_startup_dominated_work_blocks,
    test_projection_cannot_rescue_gate,
    test_projection_cannot_be_marked_gate_eligible,
    test_material_regression_fails,
    test_uncertain_regression_blocks,
    test_noisy_bundle_blocks,
    test_geomean_below_target_fails,
    test_geomean_ci_below_target_blocks,
    test_registry_digest_tamper_blocks,
    test_vacuous_schema_is_rejected,
    test_extra_required_workload_fails,
    test_absent_workload_with_samples_fails,
    test_mutation_post_state_must_be_compared,
    test_equivalence_diff_fails,
    test_equivalence_diff_detects_nested_change,
    test_drop_paths_is_narrow,
    test_unparseable_rust_envelope_fails,
    test_canonical_comparator_is_exact,
    test_stale_candidate_commit_fails,
    test_evidence_only_descendant_is_allowed,
    test_descendant_changing_build_inputs_blocks,
    test_dirty_measured_input_blocks,
    test_dirty_docs_do_not_block,
    test_harness_code_change_blocks,
    test_missing_manifest_blocks,
    test_malformed_evidence_blocks_without_crashing,
]


def run_selftest() -> dict:
    results = []
    failures = 0
    for test in TESTS:
        try:
            message = test()
            results.append({"test": test.__name__, "ok": True, "message": message})
        except Exception as exc:  # noqa: BLE001 - self-test runner
            failures += 1
            results.append(
                {"test": test.__name__, "ok": False, "error": f"{type(exc).__name__}: {exc}"}
            )
    return {"ok": failures == 0, "failures": failures, "total": len(TESTS), "results": results}


def main() -> int:
    report = run_selftest()
    print(json.dumps(report, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
