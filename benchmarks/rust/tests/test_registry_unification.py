"""The frozen registry must be the *same* registry the completion checker uses.

The first harness declared 26 required workloads of its own, only four of which
appeared in ``check_completion.py``'s frozen 20. A harness PASS could therefore
never satisfy completion, while a harness run could drop the checker's real
workloads. These tests pin the shared contract.
"""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import pytest

BENCH = Path(__file__).resolve().parents[1]
REPO = BENCH.parents[1]
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(REPO / "scripts" / "rust"))

import inventory_model  # noqa: E402
import check_completion  # noqa: E402
from spkbench import expectations as expectmod  # noqa: E402
from spkbench.registry import PerformanceRegistry, RegistryError, registry_digest  # noqa: E402


def test_registry_loads_and_validates() -> None:
    registry = PerformanceRegistry.load()
    assert registry.data["frozen"] is True
    assert registry.declared_digest == "sha256:" + registry_digest(registry.data)
    assert len(registry.required_ids) == 20
    assert len(registry.gate_ids) == 17


def test_registry_ids_equal_the_curated_completion_model() -> None:
    registry = PerformanceRegistry.load()
    curated = {item["id"] for item in inventory_model.PERF_WORKLOADS}
    assert set(registry.required_ids) == curated
    assert set(registry.gate_ids) == set(inventory_model.REQUIRED_PERF_STRICT_IDS)
    assert set(registry.startup_ids) == {"startup.version", "startup.help"}


def test_registry_ids_equal_the_hard_coded_checker_floor() -> None:
    registry = PerformanceRegistry.load()
    assert set(registry.required_ids) == set(check_completion.REQUIRED_PERF_WORKLOAD_IDS)
    assert set(registry.gate_ids) == set(check_completion.REQUIRED_GATE_WORKLOADS)
    assert set(registry.startup_ids) == set(check_completion.REQUIRED_STARTUP_WORKLOADS)
    policy = registry.policy
    assert policy["minimum_speedup"] == check_completion.MIN_PERF_SPEEDUP
    assert policy["max_per_workload_regression_fraction"] == check_completion.MAX_PERF_REGRESSION
    assert policy["minimum_samples_per_side"] == check_completion.MIN_SAMPLES
    assert policy["minimum_strict_workloads"] == check_completion.MIN_STRICT_WORKLOADS


def _checker_mirror_policy() -> dict:
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "performance_evidence", REPO / "scripts" / "rust" / "performance_evidence.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.PERF_POLICY


def test_registry_policy_mirrors_the_curated_model() -> None:
    PERF_POLICY = _checker_mirror_policy()
    registry = PerformanceRegistry.load()
    for key, value in inventory_model.PERF_POLICY.items():
        assert registry.policy.get(key) == value, f"registry policy {key} drifted"
        assert PERF_POLICY.get(key) == value, f"checker mirror {key} drifted"


def test_gate_flags_match_the_frozen_strict_set() -> None:
    registry = PerformanceRegistry.load()
    for item in registry.workloads:
        expected_strict = item["id"] in check_completion.REQUIRED_GATE_WORKLOADS
        assert item["strict_equivalence"] is expected_strict
        assert item["gate_eligible"] is expected_strict


def test_registry_digest_tamper_is_rejected(tmp_path: Path) -> None:
    registry = PerformanceRegistry.load()
    data = copy.deepcopy(registry.data)
    data["policy"]["minimum_speedup"] = 1.0
    path = tmp_path / "registry.json"
    path.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(RegistryError, match="digest"):
        PerformanceRegistry.load(path)


def test_registry_rejects_a_dropped_workload(tmp_path: Path) -> None:
    registry = PerformanceRegistry.load()
    data = copy.deepcopy(registry.data)
    data["workloads"] = [item for item in data["workloads"] if item["id"] != "workflow.merge"]
    data["digest"] = "sha256:" + registry_digest(data)
    path = tmp_path / "registry.json"
    path.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(RegistryError, match="20 frozen workloads"):
        PerformanceRegistry.load(path)


def test_every_runnable_workload_has_a_non_vacuous_schema() -> None:
    registry = PerformanceRegistry.load()
    for item in registry.workloads:
        if item["measurement"] == "absent":
            continue
        ok, why = expectmod.non_vacuous(item.get("expectations") or {})
        assert ok, f"{item['id']}: {why}"
        if item["measurement"] == "mutation":
            post = item["expectations"].get("post_state") or {}
            assert post.get("required_paths")
            assert post.get("ignored_prefixes")
            assert post.get("exclusion_ruling")
            assert post.get("expected_additions")


def test_absent_workloads_are_explicitly_unimplemented() -> None:
    registry = PerformanceRegistry.load()
    absent = [item for item in registry.workloads if item["measurement"] == "absent"]
    assert len(absent) == 16
    assert {item["id"] for item in absent} == set(registry.gate_ids) - {"status.replay"}
    for item in absent:
        assert item["native"]["blocked_by"]
        assert item["native"]["argv"] is None
        assert item["oracle"] is None
        assert item["expectations"] is None


def test_absent_native_refusals_are_real_routes_not_fake_adapters(tmp_path: Path) -> None:
    """The absent ids map to explicit native refusals, not invented adapters."""
    from spkbench.fixtures import FixtureBuilder
    from spkbench.workloads import build_workloads

    workloads = {
        workload.id: workload
        for workload in build_workloads(
            builder=FixtureBuilder(tmp_path, seed=0, dry_run=True),
            rust_binary=REPO / "target/release/spec-kitty-rust",
            oracle_python=REPO / ".venv-rust-oracle/bin/python",
            adapters_dir=BENCH / "adapters",
            registry=PerformanceRegistry.load(),
            repo_root=REPO,
        )
    }
    for wid in ("state.event-append", "state.move-task"):
        workload = workloads[wid]
        assert workload.runnable is False
        assert workload.rust is None
        assert workload.oracle is None
        assert "refuses" in (workload.pending_reason or "").lower()
