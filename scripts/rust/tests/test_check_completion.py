#!/usr/bin/env python3
"""Non-vacuity tests for ``check_completion.py``.

A completion checker is worthless if it either never passes or can be made to
pass by deleting/skipping required evidence.  These tests:

1. build a fully evidenced synthetic inventory and prove the checker accepts it;
2. prove the real inventory currently fails (the rewrite is incomplete);
3. mutate the green inventory in the ways a greenwasher would try — deleting
   rows, removing options, lowering thresholds, faking performance, skipping
   workflows, weakening review — and prove each mutation is rejected;
4. prove the strengthened evidence-binding seam rejects an old-schema bundle, a
   partial bundle, a coverage/bundle sample discrepancy, a deleted or rewritten
   payload, a deleted manifest/raw-samples artifact, a stale candidate identity
   and a raw-sample pair-alignment tamper.

The positive control is a *synthetic* certification run, built through the real
``spkbench.certify.certify_run`` / ``spkbench.report.write_evidence`` pipeline
with the real ``spkbench.selftest.synthetic_probe`` observation seam and
re-verified from disk with its raw samples artifact. Because the production
registry honestly marks 16 strict workloads absent, the fixture injects a
unit-scoped registry at the registry-loading boundary. That registry has the
same 20 ids, 17 strict gate ids, 2 startup ids and frozen policy; every one of
its rows carries real paired samples (including the startup and projection
rows), and its simulated rows say so explicitly and never touch the production
registry or coverage.json.

Only the *observations* are synthetic (the probe and the temp registry/bundle).
The checker, ``Report``, ``check_bundle``, ``certify_run``, ``write_evidence``,
``evaluate_bundle``, the payload/post-state/raw-sample readers and the 11 gates
all run for real; no gate or verdict is mocked.

Run with the oracle interpreter (stdlib only; no pytest needed):

    <oracle-python> scripts/rust/tests/test_check_completion.py -v
"""
from __future__ import annotations

import base64
import copy
import hashlib
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

# Development tooling must not leave bytecode in the checkout.
sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
ROOT = SCRIPTS.parents[1]
BENCH = ROOT / "benchmarks" / "rust"
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(BENCH))

import build_inventory  # noqa: E402
import check_completion as checker  # noqa: E402
import inventory_model  # noqa: E402
import performance_evidence  # noqa: E402
from spkbench import certify  # noqa: E402
from support import synthetic_certification as synthetic  # noqa: E402

NATIVE_REF = "crates/sk-cli/src/main.rs"
TEST_REF = "crates/sk-cli/tests/cli.rs"
REVIEW_REF = "docs/rust/inventory/README.md"


class GreenInventory:
    """Build a fully evidenced inventory that must pass the checker."""

    def __init__(self, inventory: dict, surface: dict, performance_section: dict) -> None:
        self.inventory = copy.deepcopy(inventory)
        self.surface = copy.deepcopy(surface)
        self.performance_section = copy.deepcopy(performance_section)
        self._make_entries_complete()
        self._install_performance()
        self._make_workflows_complete()
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
                entry["contract"] = copy.deepcopy(inventory_model.CONTRACTS["read_only"])

    def _install_performance(self) -> None:
        """Install the projected section from the real certification bundle."""
        self.inventory["performance"] = copy.deepcopy(self.performance_section)

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
        performance = self.inventory["performance"]
        self.inventory["counts"]["perf_workloads_total"] = len(performance["workloads"])
        self.inventory["counts"]["perf_workloads_required"] = sum(
            1 for workload in performance["workloads"] if workload.get("required")
        )
        self.inventory["counts"]["perf_workloads_strict"] = sum(
            1 for workload in performance["workloads"] if workload.get("strict_equivalence")
        )


class CompletionCheckerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.tempdir = tempfile.TemporaryDirectory(prefix="sk-checker-test-")
        cls.certification = synthetic.build_certification(Path(cls.tempdir.name))
        cls.inventory = json.loads(checker.DEFAULT_INVENTORY.read_text(encoding="utf-8"))
        cls.surface = json.loads(checker.DEFAULT_SURFACE.read_text(encoding="utf-8"))
        cls.green = GreenInventory(
            cls.inventory, cls.surface, cls.certification.performance_section
        ).inventory

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tempdir.cleanup()

    def check(
        self, inventory: dict, *, synthetic_registry: bool = True, probe=None
    ) -> checker.Report:
        if not synthetic_registry:
            return checker.check(inventory, self.surface, ROOT, live_surface=None)
        # The production registry honestly has 16 absent strict workloads. Swap
        # only the registry-loading seam for the unit-scoped synthetic registry
        # and the live observation seam for the declared synthetic environment;
        # Report, check_bundle, certify_run and evaluate_bundle stay real.
        probe = probe or self.certification.probe()
        with mock.patch.object(
            performance_evidence, "DEFAULT_REGISTRY", self.certification.registry_path
        ), mock.patch.object(
            certify, "NATIVE_PROBE_FACTORY", lambda _root: probe
        ), mock.patch.object(
            performance_evidence,
            "FIXTURE_OBSERVER",
            synthetic.synthetic_fixture_observer,
        ):
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

    def copy_bundle(self) -> tuple[Path, Path]:
        """Copy the certification artifacts so a test can mutate them in isolation."""
        target = Path(tempfile.mkdtemp(dir=self.tempdir.name, prefix="bundle-copy-"))
        evidence = target / "evidence"
        shutil.copytree(self.certification.evidence_dir, evidence)
        return evidence, evidence / "performance-evidence.json"

    def assert_bundle_rejected(
        self, inventory: dict, *, contains: str, probe=None
    ) -> checker.Report:
        report = self.check(inventory, probe=probe)
        self.assertFalse(report.summary["ok"], "mutation was accepted by the checker")
        failures = [
            error
            for error in report.errors
            if error["code"] == "PERF_BUNDLE_NOT_CERTIFIED"
        ]
        self.assertTrue(
            any(contains in error["message"] for error in failures),
            f"expected a bundle failure containing {contains!r}; got {failures[:6]}",
        )
        return report

    def point_at_bundle(self, inventory: dict, bundle_path: Path) -> None:
        inventory["performance"]["aggregate"]["evidence"] = {
            "path": str(bundle_path),
            "sha256": synthetic.sha256_file(bundle_path),
        }

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
    def test_synthetic_bundle_passes_real_certification(self) -> None:
        cert = self.certification
        self.assertEqual(cert.verification["gate"]["verdict"], "PASS")
        self.assertEqual(cert.bundle["gate"]["verdict"], "PASS")
        self.assertEqual(cert.bundle["aggregate"]["strict_workload_count"], 17)
        self.assertEqual(cert.bundle["aggregate"]["sample_count"], 17 * synthetic.PAIRS)
        # Every required row -- strict, projection and startup -- has 20 real
        # paired samples on each side, and the floor table covers every row.
        self.assertEqual(len(cert.bundle["workloads"]), 20)
        for row in cert.bundle["workloads"]:
            self.assertEqual(row["status"], "ok", row["id"])
            self.assertEqual(len(row["sample_records"]["rust"]), synthetic.PAIRS, row["id"])
            self.assertEqual(len(row["sample_records"]["python"]), synthetic.PAIRS, row["id"])
            self.assertEqual(row["sample_count"]["pairs"], synthetic.PAIRS, row["id"])
        floor_table = cert.bundle["environment"]["floors_seconds"]
        self.assertEqual(floor_table["rust"], synthetic.FLOOR_S)
        for entry in cert.registry.workloads:
            key = ",".join((entry.get("oracle") or {}).get("floor_modules") or [])
            self.assertEqual(
                floor_table["python_by_modules"][key], synthetic.FLOOR_S, entry["id"]
            )
        for name in (
            "performance-evidence.json",
            "samples.jsonl",
            "manifest.json",
            "report.json",
            "identity.json",
            "environment.json",
            "fixtures.json",
            "summary.md",
        ):
            self.assertTrue((cert.evidence_dir / name).is_file(), name)
        manifest = json.loads((cert.evidence_dir / "manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(
            manifest["bundle"]["sha256"], synthetic.sha256_file(cert.bundle_path)
        )
        for name, record in manifest["artifacts"].items():
            self.assertEqual(
                synthetic.sha256_file(cert.evidence_dir / record["path"]),
                record["sha256"],
                name,
            )
        self.assertTrue(cert.bundle["payload_artifacts"])
        for name, record in manifest["payloads"].items():
            self.assertTrue(record["stored"], name)
            self.assertEqual(
                synthetic.sha256_file(cert.evidence_dir / record["path"]),
                record["sha256"],
                name,
            )

        # The mutation post-state is a real stored artifact: full manifests, the
        # delta and the actual required-file bytes. The production registry's
        # status.json schema is satisfied by generated content, not lowered.
        post = cert.bundle["post_state_artifacts"]["status.replay.rust"]
        envelope = json.loads((cert.evidence_dir / post["path"]).read_text(encoding="utf-8"))
        self.assertEqual(envelope["schema"], "spkbench.post-state.v1")
        status_rel = "kitty-specs/002-bench-mission-002/status.json"
        self.assertIn(status_rel, envelope["files"])
        self.assertIn(status_rel, envelope["delta"]["added"])
        self.assertIn(status_rel, envelope["after"])
        data = base64.b64decode(envelope["files"][status_rel]["content_b64"])
        self.assertGreaterEqual(len(data), 2000)
        self.assertEqual(hashlib.sha256(data).hexdigest(), envelope["after"][status_rel])
        status_payload = json.loads(data)
        self.assertEqual(status_payload["event_count"], 2800)
        self.assertEqual(status_payload["summary"]["done"], 200)
        self.assertGreaterEqual(len(status_payload["work_packages"]), 200)
        # R1: the input manifest is the real regenerated fixture's manifest, not
        # a fabricated placeholder, and every unchanged file is bound to it.
        fixture_manifest = cert.bundle["fixtures"]["replay-project"]["manifest"]
        self.assertEqual(envelope["before"], fixture_manifest)
        log_rel = "kitty-specs/002-bench-mission-002/status.events.jsonl"
        self.assertEqual(envelope["before"][log_rel], fixture_manifest[log_rel])
        self.assertEqual(envelope["after"][log_rel], fixture_manifest[log_rel])

        # The recorded observation order is a valid global chronology: unique
        # across the run and adjacent within every pair.
        orders = []
        for row in cert.bundle["workloads"]:
            for side in ("rust", "python"):
                orders.extend(sample["order"] for sample in row["sample_records"][side])
        self.assertEqual(len(orders), len(set(orders)))
        for row in cert.bundle["workloads"]:
            rust = {sample["pair_index"]: sample["order"] for sample in row["sample_records"]["rust"]}
            python = {
                sample["pair_index"]: sample["order"] for sample in row["sample_records"]["python"]
            }
            for index in range(synthetic.PAIRS):
                self.assertEqual(abs(rust[index] - python[index]), 1, row["id"])

    def test_synthetic_registry_is_scoped_to_the_frozen_floor(self) -> None:
        cert = self.certification
        self.assertEqual(
            synthetic.sha256_file(synthetic.PRODUCTION_REGISTRY),
            synthetic.PRODUCTION_REGISTRY_SHA256,
            "the production registry must stay byte-identical",
        )
        self.assertEqual(set(cert.registry.required_ids), checker.REQUIRED_PERF_WORKLOAD_IDS)
        self.assertEqual(set(cert.registry.gate_ids), checker.REQUIRED_GATE_WORKLOADS)
        self.assertEqual(set(cert.registry.startup_ids), checker.REQUIRED_STARTUP_WORKLOADS)
        # The scoped registry keeps the production policy (a superset of the
        # curated checker policy); every frozen floor is identical.
        self.assertEqual(cert.registry.policy.get("minimum_speedup"), checker.MIN_PERF_SPEEDUP)
        self.assertEqual(
            cert.registry.policy.get("max_per_workload_regression_fraction"),
            checker.MAX_PERF_REGRESSION,
        )
        self.assertEqual(
            cert.registry.policy.get("minimum_samples_per_side"), checker.MIN_SAMPLES
        )
        self.assertEqual(
            cert.registry.policy.get("minimum_strict_workloads"),
            checker.MIN_STRICT_WORKLOADS,
        )
        for key, expected in inventory_model.PERF_POLICY.items():
            self.assertEqual(cert.registry.policy.get(key), expected, key)
        self.assertEqual(len(cert.production_absent_ids), 16)
        self.assertEqual(cert.simulated_ids, cert.production_absent_ids)
        production = json.loads(synthetic.PRODUCTION_REGISTRY.read_text(encoding="utf-8"))
        production_absent = {
            row["id"] for row in production["workloads"] if row.get("measurement") == "absent"
        }
        self.assertEqual(production_absent, set(cert.production_absent_ids))
        for workload_id in cert.simulated_ids:
            row = cert.registry.workload(workload_id)
            self.assertEqual(row["measurement"], "read")
            self.assertTrue(
                any(note.startswith("SYNTHETIC TEST SCAFFOLDING") for note in row["notes"]),
                workload_id,
            )

    def test_green_inventory_passes(self) -> None:
        report = self.check(self.green)
        self.assertTrue(report.summary["ok"], report.errors[:10])
        self.assertEqual(
            set(report.gates),
            {
                "surface",
                "structure",
                "modules",
                "contracts",
                "migrations",
                "status",
                "workflows",
                "performance",
                "dependencies",
                "native",
                "accounting",
            },
        )
        self.assertTrue(all(report.gates.values()), report.gates)
        self.assertEqual(report.summary["error_count"], 0)
        self.assertEqual(report.summary["warning_count"], 0)
        self.assertEqual(
            report.summary["entries_implemented"], report.summary["entries_required"]
        )
        self.assertEqual(
            report.summary["workflows_passing"], len(checker.REQUIRED_WORKFLOW_IDS)
        )
        self.assertEqual(report.summary["perf_status"], "pass")

        performance = self.green["performance"]
        self.assertEqual(
            {workload["id"] for workload in performance["workloads"]},
            checker.REQUIRED_PERF_WORKLOAD_IDS,
        )
        strict = [
            workload
            for workload in performance["workloads"]
            if workload["id"] in checker.REQUIRED_GATE_WORKLOADS
        ]
        self.assertEqual(len(strict), 17)
        for workload in strict:
            self.assertEqual(workload["status"], "pass", workload["id"])
            self.assertTrue(workload["strict_equivalence"], workload["id"])
            self.assertGreaterEqual(
                len(workload["samples"]["python"]), checker.MIN_SAMPLES, workload["id"]
            )
            self.assertGreaterEqual(
                len(workload["samples"]["rust"]), checker.MIN_SAMPLES, workload["id"]
            )
            self.assertGreaterEqual(workload["speedup"], checker.MIN_PERF_SPEEDUP)
        self.assertEqual(performance["aggregate"]["status"], "pass")
        self.assertGreaterEqual(
            performance["aggregate"]["geometric_mean_speedup"], checker.MIN_PERF_SPEEDUP
        )
        self.assertGreaterEqual(
            performance["aggregate"]["sample_count"],
            checker.MIN_STRICT_WORKLOADS * checker.MIN_SAMPLES,
        )
        self.assertEqual(
            set(performance["startup"]["workload_ids"]), checker.REQUIRED_STARTUP_WORKLOADS
        )
        self.assertEqual(performance["policy"], inventory_model.PERF_POLICY)

    def test_current_inventory_fails(self) -> None:
        report = self.check(self.inventory, synthetic_registry=False)
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
        with mock.patch.object(
            performance_evidence, "DEFAULT_REGISTRY", self.certification.registry_path
        ):
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

    # -- certification bundle binding -------------------------------------
    def test_old_schema_bundle_is_rejected(self) -> None:
        _, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        bundle["schema"] = "1.0.0"
        synthetic.reseal(bundle)
        synthetic.write_bundle(bundle_path, bundle)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        report = self.check(mutated)
        self.assertFalse(report.summary["ok"])
        self.assertIn("PERF_EVIDENCE_SCHEMA", {error["code"] for error in report.errors})

    def test_partial_bundle_missing_a_required_row_is_rejected(self) -> None:
        _, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        bundle["workloads"] = [
            row for row in bundle["workloads"] if row["id"] != "cli.mission.list"
        ]
        synthetic.reseal(bundle)
        synthetic.write_bundle(bundle_path, bundle)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        report = self.check(mutated)
        self.assertFalse(report.summary["ok"])
        codes = {error["code"] for error in report.errors}
        self.assertIn("PERF_EVIDENCE_INCOMPLETE", codes)
        # The checker's own arithmetic is untouched; only the bundle is partial.
        self.assertNotIn("PERF_WORKLOAD_MISSING", codes)

    def test_coverage_command_recipe_must_match_the_bundle(self) -> None:
        inventory = copy.deepcopy(self.green)
        row = next(item for item in inventory["performance"]["workloads"] if item["id"] == "status.replay")
        row["command_argv"] = json.dumps({"rust": ["/bin/true"], "python": ["/bin/true"]})
        self.assert_rejected(inventory, "PERF_PROJECTION_MISMATCH")

    def test_coverage_fixture_identity_must_match_the_bundle(self) -> None:
        inventory = copy.deepcopy(self.green)
        row = next(item for item in inventory["performance"]["workloads"] if item["id"] == "status.replay")
        row["fixture_id"] = "foreign-fixture"
        self.assert_rejected(inventory, "PERF_PROJECTION_MISMATCH")

    def test_coverage_bundle_sample_discrepancy_is_rejected(self) -> None:
        mutated = copy.deepcopy(self.green)
        workload = next(
            item for item in mutated["performance"]["workloads"] if item["id"] == "workflow.tasks"
        )
        # Perturb a non-median sample: the checker's own arithmetic still holds,
        # so only the bundle cross-check can catch the discrepancy.
        workload["samples"]["python"][0] = round(
            float(workload["samples"]["python"][0]) + 0.5, 6
        )
        report = self.check(mutated)
        self.assertFalse(report.summary["ok"])
        codes = {error["code"] for error in report.errors}
        self.assertIn("PERF_SECTION_BUNDLE_MISMATCH", codes)
        self.assertNotIn("PERF_SPEEDUP_MISMATCH", codes)
        self.assertNotIn("PERF_AGGREGATE_MISMATCH", codes)

    def test_failed_bundle_row_cannot_be_laundered_as_a_passing_section(self) -> None:
        _, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        row = next(item for item in bundle["workloads"] if item["id"] == "workflow.tasks")
        row["status"] = "schema_failed"
        synthetic.reseal(bundle)
        synthetic.write_bundle(bundle_path, bundle)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        # The coverage section still claims this row passed. Rejection must not
        # depend on the section status field being honest.
        report = self.check(mutated)
        self.assertFalse(report.summary["ok"])
        codes = {error["code"] for error in report.errors}
        self.assertIn("PERF_STATUS_NOT_BACKED", codes)
        self.assertIn("PERF_WORKLOAD_NOT_PASSING", codes)
        # The section-status cross-check is not tautological: a non-ok bundle row
        # can no longer be projected as a passing coverage row silently.
        self.assertIn("PERF_SECTION_BUNDLE_MISMATCH", codes)

    def test_deleted_raw_samples_artifact_is_rejected(self) -> None:
        evidence, bundle_path = self.copy_bundle()
        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        (evidence / "samples.jsonl").unlink()

        report = self.check(mutated)
        self.assertFalse(report.summary["ok"])
        codes = {error["code"] for error in report.errors}
        self.assertIn("PERF_BUNDLE_NOT_CERTIFIED", codes)
        self.assertTrue(
            any(
                error["code"] == "PERF_BUNDLE_NOT_CERTIFIED"
                and "samples" in error["message"]
                for error in report.errors
            ),
            [error for error in report.errors if error["code"] == "PERF_BUNDLE_NOT_CERTIFIED"],
        )

    def test_deleted_manifest_is_rejected(self) -> None:
        evidence, bundle_path = self.copy_bundle()
        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        (evidence / "manifest.json").unlink()
        self.assert_bundle_rejected(mutated, contains="manifest.json is missing")

    def test_deleted_payload_artifact_is_rejected(self) -> None:
        evidence, bundle_path = self.copy_bundle()
        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        victim = evidence / "payloads" / "state.move-task.rust.json"
        self.assertTrue(victim.is_file())
        victim.unlink()
        self.assert_bundle_rejected(mutated, contains="payload artifact")

    def test_rewritten_payload_content_is_rejected(self) -> None:
        evidence, bundle_path = self.copy_bundle()
        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        victim = evidence / "payloads" / "state.move-task.python.json"
        payload = json.loads(victim.read_text(encoding="utf-8"))
        payload["kind"] = "tampered-after-measurement"
        victim.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        self.assert_bundle_rejected(mutated, contains="canonical content hash")

    def test_raw_sample_pair_alignment_tamper_is_rejected(self) -> None:
        evidence, bundle_path = self.copy_bundle()
        mutated = copy.deepcopy(self.green)
        samples_path = evidence / "samples.jsonl"
        lines = samples_path.read_text(encoding="utf-8").splitlines()
        rows = [json.loads(line) for line in lines if line.strip()]
        targets = [
            row
            for row in rows
            if row["workload"] == "state.move-task" and row["side"] == "rust"
        ]
        self.assertEqual(len(targets), synthetic.PAIRS)
        # Swap the durations of two distinct pairs without touching pair_index:
        # the multiset of raw durations is unchanged, so only pair-aligned
        # verification can see the tamper.
        targets[0]["duration_s"], targets[-1]["duration_s"] = (
            targets[-1]["duration_s"],
            targets[0]["duration_s"],
        )
        samples_path.write_text(
            "\n".join(json.dumps(row, sort_keys=True) for row in rows) + "\n",
            encoding="utf-8",
        )
        # A forger would also re-seal the artifact hash table and manifest. The
        # pair-aligned comparison must still catch the swap, because the raw
        # duration multiset itself is unchanged.
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        bundle["artifacts"]["samples"]["sha256"] = synthetic.sha256_file(samples_path)
        synthetic.reseal(bundle, evidence)
        self.point_at_bundle(mutated, bundle_path)
        self.assert_bundle_rejected(mutated, contains="differs from samples.jsonl")

    def test_bundle_fixture_table_forgery_is_rejected(self) -> None:
        """B2: a self-consistent forged fixture table is still regenerated away."""
        evidence, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        forged = "0" * 64
        for row in bundle["workloads"]:
            if row.get("fixture_id"):
                row["fixture_content_digest"] = forged
        for record in bundle["fixtures"].values():
            record["content_digest"] = forged
        fixtures_path = evidence / "fixtures.json"
        fixtures = json.loads(fixtures_path.read_text(encoding="utf-8"))
        for record in fixtures.values():
            record["content_digest"] = forged
        fixtures_path.write_text(
            json.dumps(fixtures, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        bundle["artifacts"]["fixtures"]["sha256"] = synthetic.sha256_file(fixtures_path)
        synthetic.reseal(bundle, evidence)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        self.assert_bundle_rejected(mutated, contains="fixture")

    def test_bundle_post_state_contract_forgery_is_rejected(self) -> None:
        """B3: an empty post-state delta is rejected even when every hash agrees."""
        evidence, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        row = next(item for item in bundle["workloads"] if item["id"] == "status.replay")
        for side in ("rust", "python"):
            record = bundle["post_state_artifacts"][f"status.replay.{side}"]
            path = evidence / record["path"]
            envelope = json.loads(path.read_text(encoding="utf-8"))
            envelope["delta"] = {"added": [], "changed": [], "removed": []}
            path.write_text(
                json.dumps(envelope, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            record["sha256"] = synthetic.sha256_file(path)
            record["canonical_sha256"] = synthetic.canonical_digest(envelope)
            record["bytes"] = path.stat().st_size
            delta_sha = synthetic.canonical_digest(envelope["delta"])
            row["post_state"]["sha256"][side] = delta_sha
            for sample in row["sample_records"][side]:
                sample["post_state_sha256"] = delta_sha
        synthetic.reseal(bundle, evidence)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        self.assert_bundle_rejected(mutated, contains="post-state")

    def test_bundle_stored_status_identity_forgery_is_rejected(self) -> None:
        """R2: a re-sealed status document for another Mission must be rejected."""
        evidence, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        row = next(item for item in bundle["workloads"] if item["id"] == "status.replay")
        status_rel = "kitty-specs/002-bench-mission-002/status.json"
        for side in ("rust", "python"):
            record = bundle["post_state_artifacts"][f"status.replay.{side}"]
            path = evidence / record["path"]
            envelope = json.loads(path.read_text(encoding="utf-8"))
            data = base64.b64decode(envelope["files"][status_rel]["content_b64"])
            payload = json.loads(data)
            payload["mission_slug"] = "999-some-other-mission"
            rewritten = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")
            digest = hashlib.sha256(rewritten).hexdigest()
            envelope["files"][status_rel]["content_b64"] = base64.b64encode(rewritten).decode(
                "ascii"
            )
            envelope["files"][status_rel]["bytes"] = len(rewritten)
            envelope["files"][status_rel]["sha256"] = digest
            envelope["files"][status_rel]["canonical_sha256"] = synthetic.canonical_digest(
                payload
            )
            envelope["after"][status_rel] = digest
            envelope["delta"]["added_sha256"][status_rel] = digest
            path.write_text(
                json.dumps(envelope, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            record["sha256"] = synthetic.sha256_file(path)
            record["canonical_sha256"] = synthetic.canonical_digest(envelope)
            record["bytes"] = path.stat().st_size
            delta_sha = synthetic.canonical_digest(envelope["delta"])
            row["post_state"]["sha256"][side] = delta_sha
            for sample in row["sample_records"][side]:
                sample["post_state_sha256"] = delta_sha
        synthetic.reseal(bundle, evidence)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        self.assert_bundle_rejected(mutated, contains="mission_slug")

    def test_bundle_unchanged_input_hash_forgery_is_rejected(self) -> None:
        """R1: a re-sealed lie about an unchanged fixture file is rejected."""
        evidence, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        row = next(item for item in bundle["workloads"] if item["id"] == "status.replay")
        victim = "kitty-specs/002-bench-mission-002/status.events.jsonl"
        forged = "9" * 64
        for side in ("rust", "python"):
            record = bundle["post_state_artifacts"][f"status.replay.{side}"]
            path = evidence / record["path"]
            envelope = json.loads(path.read_text(encoding="utf-8"))
            envelope["before"][victim] = forged
            envelope["after"][victim] = forged
            path.write_text(
                json.dumps(envelope, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            record["sha256"] = synthetic.sha256_file(path)
            record["canonical_sha256"] = synthetic.canonical_digest(envelope)
            record["bytes"] = path.stat().st_size
            delta_sha = synthetic.canonical_digest(envelope["delta"])
            row["post_state"]["sha256"][side] = delta_sha
            for sample in row["sample_records"][side]:
                sample["post_state_sha256"] = delta_sha
        synthetic.reseal(bundle, evidence)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        self.assert_bundle_rejected(mutated, contains="regenerated fixture bytes")

    def test_bundle_sample_chronology_forgery_is_rejected(self) -> None:
        """H1: resetting every sample order to 0 no longer passes."""
        evidence, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        for row in bundle["workloads"]:
            for side in ("rust", "python"):
                for sample in row.get("sample_records", {}).get(side, []):
                    sample["order"] = 0
        samples_path = evidence / "samples.jsonl"
        rows = [
            json.loads(line)
            for line in samples_path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        for row in rows:
            row["order"] = 0
        samples_path.write_text(
            "".join(json.dumps(row, sort_keys=True) + "\n" for row in rows), encoding="utf-8"
        )
        bundle["artifacts"]["samples"]["sha256"] = synthetic.sha256_file(samples_path)
        synthetic.reseal(bundle, evidence)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        self.assert_bundle_rejected(mutated, contains="order")

    def test_stale_candidate_commit_is_rejected(self) -> None:
        evidence, bundle_path = self.copy_bundle()
        bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
        bundle["candidate"]["commit"] = "b" * 40
        synthetic.reseal(bundle, evidence)

        mutated = copy.deepcopy(self.green)
        self.point_at_bundle(mutated, bundle_path)
        probe = self.certification.probe(ancestor=False)
        report = self.assert_bundle_rejected(mutated, contains="foreign history", probe=probe)
        codes = {error["code"] for error in report.errors}
        self.assertIn("PERF_EVIDENCE_CANDIDATE", codes)


if __name__ == "__main__":
    unittest.main(verbosity=2)
