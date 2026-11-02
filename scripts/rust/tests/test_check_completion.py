#!/usr/bin/env python3
"""Non-vacuity tests for ``check_completion.py``.

A completion checker is worthless if it either never passes or can be made to
pass by deleting/skipping required evidence.  These tests:

1. build a fully evidenced synthetic inventory and prove the checker accepts it;
2. prove the real inventory currently fails (the rewrite is incomplete);
3. mutate the green inventory in the ways a greenwasher would try — deleting
   rows, removing options, lowering thresholds, faking performance, skipping
   workflows, weakening review — and prove each mutation is rejected.

Run with the oracle interpreter (stdlib only; no pytest needed):

    <oracle-python> scripts/rust/tests/test_check_completion.py -v
"""
from __future__ import annotations

import copy
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

# Development tooling must not leave bytecode in the checkout.
sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
ROOT = SCRIPTS.parents[1]
sys.path.insert(0, str(SCRIPTS))

import build_inventory  # noqa: E402
import check_completion as checker  # noqa: E402

NATIVE_REF = "crates/sk-cli/src/main.rs"
TEST_REF = "crates/sk-cli/tests/cli.rs"
REVIEW_REF = "docs/rust/inventory/README.md"


def _median_speedup() -> float:
    return 6.0


class GreenInventory:
    """Build a fully evidenced inventory that must pass the checker."""

    def __init__(self, inventory: dict, surface: dict, tempdir: Path) -> None:
        self.inventory = copy.deepcopy(inventory)
        self.surface = copy.deepcopy(surface)
        self.tempdir = tempdir
        self.evidence_path = tempdir / "performance-evidence.json"
        self._make_entries_complete()
        self._make_workflows_complete()
        self._make_performance_complete()
        self._refresh_counts()

    def _make_entries_complete(self) -> None:
        for entry in self.inventory["entries"]:
            if not entry.get("required"):
                continue
            entry["implementation"] = {
                "status": "implemented",
                "native_refs": [NATIVE_REF],
                "notes": "synthetic green evidence for checker self-test",
            }
            entry["conformance"] = {"status": "pass", "refs": [TEST_REF]}
            entry["review"] = {
                "status": "approved",
                "refs": [REVIEW_REF],
                "independent": True,
            }
            entry["performance"] = {"status": "measured", "workload_ids": []}
        self.inventory["unresolved_entry_ids"] = []
        for entry in self.inventory["entries"]:
            if entry.get("contract", {}).get("status") == "unresolved":
                entry["contract"] = copy.deepcopy(checker.inventory_model.CONTRACTS["read_only"])

    def _make_workflows_complete(self) -> None:
        for workflow in self.inventory["workflows"]:
            workflow["status"] = "pass"
            for category in checker.REQUIRED_WORKFLOW_EVIDENCE:
                workflow[category] = [TEST_REF]
            workflow["review"] = {
                "status": "approved",
                "refs": [REVIEW_REF],
                "independent": True,
            }
            # Every workflow references only workloads present in the green set.
            for workload_id in workflow.get("performance_workloads", []):
                assert any(
                    workload["id"] == workload_id
                    for workload in self.inventory["performance"]["workloads"]
                ), workload_id

    def _make_performance_complete(self) -> None:
        performance = self.inventory["performance"]
        baseline_commit = performance["baseline"]["commit"]
        candidate_commit = "1" * 40
        performance["candidate"]["commit"] = candidate_commit
        python_samples = [30.0 + index * 0.01 for index in range(20)]
        rust_samples = [5.0 + index * 0.001 for index in range(20)]
        for workload in performance["workloads"]:
            workload["status"] = "pass"
            workload["result_equivalent"] = True
            workload["samples"] = {
                "python": list(python_samples),
                "rust": list(rust_samples),
            }
            workload["speedup"] = _median_speedup()
            workload["fixture_id"] = "self-test-fixture"
            workload["command_argv"] = ["spec-kitty", "self-test"]
            workload["environment"] = {"os": "linux", "load": "idle"}
            workload["evidence_refs"] = [REVIEW_REF]
            workload["strict_equivalence"] = workload["id"] in checker.REQUIRED_GATE_WORKLOADS
            workload["equivalence"] = "strict" if workload["strict_equivalence"] else "projection"
        performance["status"] = "pass"
        performance["aggregate"] = {
            "status": "pass",
            "geometric_mean_speedup": _median_speedup(),
            "minimum_speedup": 3.0,
            "max_per_workload_regression_fraction": 0.10,
            "sample_count": len(checker.REQUIRED_GATE_WORKLOADS) * 40,
            "evidence": {
                "path": str(self.evidence_path),
                "sha256": None,
            },
        }
        performance["startup"] = {
            "status": "pass",
            "excluded_from_gate": True,
            "workload_ids": sorted(checker.REQUIRED_STARTUP_WORKLOADS),
            "measurements": {"version": {"python_ms": 40.0, "rust_ms": 2.0}},
        }
        bundle = {
            "schema_version": "1.0.0",
            "baseline": {"commit": baseline_commit},
            "candidate": {"commit": candidate_commit},
            "workloads": [
                {"id": workload_id, "samples": {"python": python_samples, "rust": rust_samples}}
                for workload_id in sorted(checker.REQUIRED_GATE_WORKLOADS)
            ],
        }
        self.evidence_path.write_text(json.dumps(bundle), encoding="utf-8")
        performance["aggregate"]["evidence"]["sha256"] = hashlib.sha256(
            self.evidence_path.read_bytes()
        ).hexdigest()

    def _refresh_counts(self) -> None:
        entries = self.inventory["entries"]
        required = [entry for entry in entries if entry.get("required")]
        self.inventory["counts"]["entries_total"] = len(entries)
        self.inventory["counts"]["entries_required"] = len(required)
        self.inventory["counts"]["entries_not_required"] = len(entries) - len(required)
        self.inventory["counts"]["by_implementation"]["implemented"] = len(required)
        self.inventory["counts"]["mutating_entries"] = sum(1 for entry in entries if entry.get("mutates"))
        self.inventory["counts"]["documented_mutation_contracts"] = sum(
            1 for entry in entries if entry.get("mutates") and entry["contract"]["status"] == "documented"
        )
        self.inventory["counts"]["unresolved_contracts"] = 0
        self.inventory["counts"]["pending_implementation"] = 0
        self.inventory["counts"]["pending_conformance"] = 0
        self.inventory["counts"]["pending_review"] = 0


class CompletionCheckerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.tempdir = tempfile.TemporaryDirectory(prefix="sk-checker-test-")
        cls.inventory = json.loads(checker.DEFAULT_INVENTORY.read_text(encoding="utf-8"))
        cls.surface = json.loads(checker.DEFAULT_SURFACE.read_text(encoding="utf-8"))
        cls.green = GreenInventory(cls.inventory, cls.surface, Path(cls.tempdir.name)).inventory

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tempdir.cleanup()

    def check(self, inventory: dict) -> checker.Report:
        return checker.check(inventory, self.surface, ROOT, live_surface=None)

    def codes(self, inventory: dict) -> set[str]:
        return {error["code"] for error in self.check(inventory).errors}

    def assert_rejected(self, inventory: dict, expected_code: str) -> None:
        report = self.check(inventory)
        self.assertFalse(report.summary["ok"], "mutation was accepted by the checker")
        self.assertIn(
            expected_code,
            {error["code"] for error in report.errors},
            f"expected {expected_code}, got {sorted({e['code'] for e in report.errors})}",
        )

    def test_rebuild_preserves_evidence(self) -> None:
        with tempfile.TemporaryDirectory(prefix="sk-rebuild-") as temp:
            path = Path(temp) / "coverage.json"
            first = build_inventory.build(self.surface, ROOT, existing_path=None)
            entry = next(item for item in first["entries"] if item["id"] == "CLI:accept")
            entry["implementation"] = {
                "status": "implemented",
                "native_refs": [NATIVE_REF],
                "notes": "preserved",
            }
            first["performance"]["workloads"][0]["status"] = "pass"
            first["delivery_slices"][0]["status"] = "done"
            path.write_text(json.dumps(first), encoding="utf-8")
            second = build_inventory.build(self.surface, ROOT, existing_path=path)
            rebuilt = next(item for item in second["entries"] if item["id"] == "CLI:accept")
            self.assertEqual(rebuilt["implementation"]["status"], "implemented")
            self.assertEqual(second["performance"]["workloads"][0]["status"], "pass")
            self.assertEqual(second["delivery_slices"][0]["status"], "done")
            self.assertIn("workflow", {entry["kind"] for entry in second["workflows"]})

    # -- positive control -------------------------------------------------
    def test_green_inventory_passes(self) -> None:
        report = self.check(self.green)
        self.assertTrue(report.summary["ok"], report.errors[:10])
        self.assertEqual(report.summary["entries_implemented"], len(checker.REQUIRED_CONTRACT_IDS) + 244 + 136 + 40 + 1 + 16 - 1)

    def test_current_inventory_fails(self) -> None:
        report = self.check(self.inventory)
        self.assertFalse(report.summary["ok"])
        self.assertIn("REQUIRED_ENTRY_NOT_COMPLETE", {error["code"] for error in report.errors})
        self.assertIn("PERF_NO_STRICT_SAMPLES", {error["code"] for error in report.errors})
        self.assertIn("PERF_INSUFFICIENT_WORKLOADS", {error["code"] for error in report.errors})

    # -- deleting/skipping required rows ---------------------------------
    def test_deleting_a_required_cli_leaf_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["entries"] = [
            entry for entry in mutated["entries"] if entry["id"] != "CLI:agent/tasks/move-task"
        ]
        mutated["counts"]["entries_total"] -= 1
        mutated["counts"]["entries_required"] -= 1
        self.assert_rejected(mutated, "MISSING_CLI_ENTRY")

    def test_marking_a_required_leaf_optional_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:agent/tasks/move-task")
        entry["required"] = False
        entry["exclusion"] = {"class": "internal_sentinel", "reason": "test"}
        self.assert_rejected(mutated, "UNEXCUSED_SKIP")

    def test_removing_an_option_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:consolidate")
        self.assertTrue(entry["surface"]["options"])
        entry["surface"]["options"].pop()
        self.assert_rejected(mutated, "SURFACE_DRIFT")

    def test_surface_and_inventory_tampering_still_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        surface = copy.deepcopy(self.surface)
        leaf = next(item for item in surface["leaves"] if item["id"] == "CLI:consolidate")
        leaf["options"].pop()
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:consolidate")
        entry["surface"]["options"].pop()
        report = checker.check(mutated, surface, ROOT, live_surface=self.surface)
        self.assertFalse(report.summary["ok"])
        self.assertIn("SURFACE_DRIFT", {error["code"] for error in report.errors})

    def test_stale_source_hash_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["baseline"]["surface_sha256"] = "0" * 64
        self.assert_rejected(mutated, "STALE_SURFACE")

    def test_duplicating_an_entry_id_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["entries"].append(copy.deepcopy(mutated["entries"][0]))
        self.assert_rejected(mutated, "DUPLICATE_ENTRY_ID")

    def test_deleting_a_module_row_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["modules"].pop()
        self.assert_rejected(mutated, "MODULE_ROW_MISSING")

    def test_module_hash_drift_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["modules"][0]["sha256"] = "0" * 64
        self.assert_rejected(mutated, "MODULE_HASH_DRIFT")

    def test_deleting_a_contract_entry_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["entries"] = [
            entry for entry in mutated["entries"] if entry["id"] != "WIRE-ORCH-ENVELOPE"
        ]
        mutated["counts"]["entries_total"] -= 1
        mutated["counts"]["entries_required"] -= 1
        self.assert_rejected(mutated, "MISSING_CONTRACT_ENTRY")

    def test_deleting_a_migration_row_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        victim = next(entry["id"] for entry in mutated["entries"] if entry["kind"] == "migration")
        mutated["entries"] = [entry for entry in mutated["entries"] if entry["id"] != victim]
        mutated["counts"]["entries_total"] -= 1
        mutated["counts"]["entries_required"] -= 1
        self.assert_rejected(mutated, "MIGRATION_ROW_MISSING")

    # -- skipping status/evidence ----------------------------------------
    def test_missing_implementation_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:agent/tasks/move-task")
        entry["implementation"]["status"] = "absent"
        self.assert_rejected(mutated, "REQUIRED_ENTRY_NOT_COMPLETE")

    def test_python_implementation_ref_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:accept")
        entry["implementation"]["native_refs"] = ["src/specify_cli/cli/commands/accept.py"]
        self.assert_rejected(mutated, "IMPLEMENTATION_NOT_NATIVE")

    def test_conformance_without_refs_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:accept")
        entry["conformance"]["refs"] = []
        self.assert_rejected(mutated, "CONFORMANCE_WITHOUT_EVIDENCE")

    def test_conformance_evidence_must_be_a_test(self) -> None:
        mutated = copy.deepcopy(self.green)
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:accept")
        entry["conformance"]["refs"] = [REVIEW_REF]
        self.assert_rejected(mutated, "CONFORMANCE_EVIDENCE_NOT_TEST")

    def test_review_must_be_independent(self) -> None:
        mutated = copy.deepcopy(self.green)
        entry = next(item for item in mutated["entries"] if item["id"] == "CLI:accept")
        entry["review"]["independent"] = False
        self.assert_rejected(mutated, "REVIEW_NOT_INDEPENDENT")

    def test_lowering_counts_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["counts"]["entries_required"] -= 1
        self.assert_rejected(mutated, "COUNT_REQUIRED")

    # -- workflow coverage ------------------------------------------------
    def test_deleting_a_workflow_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["workflows"] = [
            workflow for workflow in mutated["workflows"] if workflow["id"] != "WF-CONSOLIDATE"
        ]
        self.assert_rejected(mutated, "MISSING_WORKFLOW")

    def test_duplicate_workflow_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["workflows"].append(copy.deepcopy(mutated["workflows"][0]))
        self.assert_rejected(mutated, "WORKFLOW_DUPLICATE_ROW")

    def test_workflow_must_pass(self) -> None:
        mutated = copy.deepcopy(self.green)
        workflow = next(item for item in mutated["workflows"] if item["id"] == "WF-RECOVERY")
        workflow["status"] = "pending"
        self.assert_rejected(mutated, "WORKFLOW_NOT_PASSING")

    def test_workflow_evidence_category_cannot_be_dropped(self) -> None:
        mutated = copy.deepcopy(self.green)
        workflow = next(item for item in mutated["workflows"] if item["id"] == "WF-IMPLEMENT")
        workflow["concurrency_tests"] = []
        self.assert_rejected(mutated, "WORKFLOW_EVIDENCE_MISSING")

    def test_workflow_review_must_be_independent(self) -> None:
        mutated = copy.deepcopy(self.green)
        workflow = next(item for item in mutated["workflows"] if item["id"] == "WF-ACCEPT")
        workflow["review"]["independent"] = False
        self.assert_rejected(mutated, "WORKFLOW_REVIEW_NOT_INDEPENDENT")

    # -- performance ------------------------------------------------------
    def test_deleting_a_performance_workload_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["performance"]["workloads"] = [
            workload
            for workload in mutated["performance"]["workloads"]
            if workload["id"] != "workflow.recovery"
        ]
        self.assert_rejected(mutated, "PERF_WORKLOAD_MISSING")

    def test_duplicate_performance_workload_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["performance"]["workloads"].append(copy.deepcopy(mutated["performance"]["workloads"][0]))
        self.assert_rejected(mutated, "PERF_DUPLICATE_WORKLOAD")

    def test_lowering_performance_threshold_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["performance"]["policy"]["minimum_speedup"] = 2.0
        self.assert_rejected(mutated, "PERF_POLICY_MISMATCH")

    def test_slower_than_python_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        workload = next(
            item for item in mutated["performance"]["workloads"] if item["id"] == "state.move-task"
        )
        workload["samples"] = {
            "python": [30.0] * 20,
            "rust": [60.0] * 20,
        }
        workload["speedup"] = 0.5
        self.assert_rejected(mutated, "PERF_WORKLOAD_REGRESSION")

    def test_fewer_than_three_x_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        for workload in mutated["performance"]["workloads"]:
            if workload["id"] in checker.REQUIRED_GATE_WORKLOADS:
                workload["samples"] = {"python": [30.0] * 20, "rust": [15.0] * 20}
                workload["speedup"] = 2.0
        mutated["performance"]["aggregate"]["geometric_mean_speedup"] = 2.0
        self.assert_rejected(mutated, "PERF_TARGET_NOT_MET")

    def test_missing_raw_samples_fail(self) -> None:
        mutated = copy.deepcopy(self.green)
        workload = next(
            item for item in mutated["performance"]["workloads"] if item["id"] == "status.replay"
        )
        workload["samples"] = {}
        self.assert_rejected(mutated, "PERF_SAMPLES_MISSING")

    def test_fabricated_aggregate_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["performance"]["aggregate"]["geometric_mean_speedup"] = 9.0
        self.assert_rejected(mutated, "PERF_AGGREGATE_MISMATCH")

    def test_missing_evidence_bundle_fails(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["performance"]["aggregate"]["evidence"] = None
        self.assert_rejected(mutated, "PERF_EVIDENCE_BUNDLE_MISSING")

    def test_startup_cannot_be_the_only_measurement(self) -> None:
        mutated = copy.deepcopy(self.green)
        for workload in mutated["performance"]["workloads"]:
            if workload["id"] in checker.REQUIRED_GATE_WORKLOADS:
                workload["status"] = "pending"
                workload["samples"] = {}
        mutated["performance"]["status"] = "pass"
        self.assert_rejected(mutated, "PERF_WORKLOAD_NOT_PASSING")

    def test_startup_must_be_separate(self) -> None:
        mutated = copy.deepcopy(self.green)
        mutated["performance"]["startup"]["excluded_from_gate"] = False
        self.assert_rejected(mutated, "PERF_STARTUP_IN_GATE")


if __name__ == "__main__":
    unittest.main(verbosity=2)
