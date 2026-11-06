"""End-to-end bundle tests for the certification gate and checker adapter.

Every test builds a synthetic measurement *through the real pipeline*
(``report.write_evidence`` -> ``certify.evaluate_bundle``) with a synthetic
native-source probe, then tampers with it. The gate must recompute, refuse and
never accept a filtered, degenerate or forged bundle.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest import mock

import pytest

BENCH = Path(__file__).resolve().parents[1]
REPO = BENCH.parents[1]
sys.path.insert(0, str(BENCH))

from spkbench import certify, evidence as evidencemod, selftest  # noqa: E402
from spkbench.registry import PerformanceRegistry  # noqa: E402
from spkbench.report import write_evidence  # noqa: E402


def _probe(**kwargs) -> selftest._SyntheticProbe:
    return selftest._SyntheticProbe(selftest._synthetic_identity(), **kwargs)


def _demo_bundle() -> tuple[Path, dict]:
    out = Path(tempfile.mkdtemp(prefix="spkbench-bundle-test-"))
    identity = selftest._synthetic_identity()
    identity["machine"] = {"hostname": "synthetic"}
    written = write_evidence(
        out_dir=out,
        run=selftest._synthetic_run(),
        identity=identity,
        fixtures={
            "synthetic-fixture": selftest.synthetic_fixture_record("synthetic-fixture", 7)
        },
        registry=selftest._synthetic_registry(),
        harness={"name": "spkbench", "version": "test"},
        repo=REPO,
        packs_root=None,
        live_probe=_probe(),
        verify_fixtures=True,
        fixture_observer=selftest.synthetic_fixture_observer,
    )
    return out, written


def _adapter():
    spec = importlib.util.spec_from_file_location(
        "performance_evidence", REPO / "scripts" / "rust" / "performance_evidence.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _write_registry(tmp_path: Path) -> Path:
    registry_path = tmp_path / "synthetic-registry.json"
    registry_path.write_text(
        json.dumps(selftest._synthetic_registry().data), encoding="utf-8"
    )
    return registry_path


def test_write_then_verify_round_trips() -> None:
    out, written = _demo_bundle()
    try:
        assert written["verification"]["gate"]["verdict"] == "PASS"
        manifest = json.loads((out / "manifest.json").read_text())
        bundle = out / "performance-evidence.json"
        assert manifest["bundle"]["sha256"] == certify._sha256_file(bundle)
        for name, record in manifest["artifacts"].items():
            assert certify._sha256_file(out / record["path"]) == record["sha256"], name
        reloaded = certify.load_bundle(out)
        result = certify.evaluate_bundle(
            reloaded,
            selftest._synthetic_registry(),
            repo=REPO,
            live_probe=_probe(),
            artifacts_root=out,
            require_artifacts=True,
            require_manifest=True,
        )
        assert result["gate"]["verdict"] == "PASS", result["gate"]["reasons"]
        # The checker-facing projection carries the recomputed numbers.
        section = certify.performance_section(
            reloaded, bundle, verification=written["verification"]
        )
        assert section["status"] == "pass"
        assert section["aggregate"]["geometric_mean_speedup"] == pytest.approx(4.0, rel=0.01)
        assert len(section["workloads"]) == len(selftest._synthetic_registry().required_ids)
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_artifacts_are_required_for_certification() -> None:
    out, _ = _demo_bundle()
    try:
        bundle = certify.load_bundle(out)
        result = certify.evaluate_bundle(
            bundle,
            selftest._synthetic_registry(),
            repo=REPO,
            live_probe=_probe(),
            require_artifacts=True,
            artifacts_root=None,
        )
        assert result["gate"]["verdict"] == "BLOCKED"
        assert any("artifact" in reason for reason in result["gate"]["reasons"])
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_deleted_samples_jsonl_blocks() -> None:
    out, _ = _demo_bundle()
    try:
        (out / "samples.jsonl").unlink()
        bundle = certify.load_bundle(out)
        result = certify.evaluate_bundle(
            bundle,
            selftest._synthetic_registry(),
            repo=REPO,
            live_probe=_probe(),
            artifacts_root=out,
            require_artifacts=True,
            require_manifest=True,
        )
        assert result["gate"]["verdict"] in {"FAIL", "BLOCKED"}
        assert any("samples" in reason for reason in result["gate"]["reasons"])
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_historical_evidence_is_not_re_certifiable() -> None:
    historical = REPO / "benchmarks/rust/evidence/2026-11-02-initial/report.json"
    assert historical.exists()
    report = json.loads(historical.read_text())
    assert report["schema"] == "spkbench.report.v1"
    assert report["schema"] != certify.SCHEMA
    completed = subprocess.run(
        [sys.executable, str(BENCH / "bench.py"), "gate", str(historical)],
        capture_output=True,
        text=True,
        cwd=str(REPO),
    )
    assert completed.returncode == 2, completed.stderr
    assert "BLOCKED" in completed.stdout


def test_checker_adapter_accepts_a_complete_bundle(tmp_path: Path) -> None:
    out, written = _demo_bundle()
    module = _adapter()
    try:
        registry_path = _write_registry(tmp_path)
        section = certify.performance_section(
            certify.load_bundle(out),
            out / "performance-evidence.json",
            verification=written["verification"],
        )
        module.FIXTURE_OBSERVER = selftest.synthetic_fixture_observer
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(),
        )
        assert result["ok"] is True, result["errors"]
        assert result["verdict"] == "PASS"
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_rejects_a_blocked_bundle(tmp_path: Path) -> None:
    out, _ = _demo_bundle()
    module = _adapter()
    try:
        bundle_path = out / "performance-evidence.json"
        bundle = certify.load_bundle(out)
        for row in bundle["workloads"]:
            if row["id"] == "gate.alpha":
                row["status"] = "schema_failed"
        bundle["gate"] = {"verdict": "BLOCKED", "reasons": ["x"], "stage": "test"}
        bundle["aggregate"]["status"] = "pass"
        bundle["aggregate"]["geometric_mean_speedup"] = 4.0
        selftest._reseal(bundle, out)
        registry_path = _write_registry(tmp_path)
        section = certify.performance_section(bundle, bundle_path)
        # A fabricated "pass" over a blocked bundle must not survive the adapter.
        section["status"] = "pass"
        section["aggregate"]["status"] = "pass"
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(),
        )
        assert result["ok"] is False
        assert any(
            error["code"] in {"PERF_BUNDLE_NOT_CERTIFIED", "PERF_STATUS_NOT_BACKED"}
            for error in result["errors"]
        ), result["errors"]
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_cross_checks_section_against_bundle(tmp_path: Path) -> None:
    out, written = _demo_bundle()
    module = _adapter()
    try:
        registry_path = _write_registry(tmp_path)
        section = certify.performance_section(
            certify.load_bundle(out), out / "performance-evidence.json", verification=written["verification"]
        )
        section["workloads"][0]["samples"] = {"python": [], "rust": []}
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(),
        )
        assert result["ok"] is False
        assert any(
            error["code"] == "PERF_SECTION_BUNDLE_MISMATCH" for error in result["errors"]
        ), result["errors"]
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_rejects_projection_drift(tmp_path: Path) -> None:
    out, written = _demo_bundle()
    module = _adapter()
    try:
        registry_path = _write_registry(tmp_path)
        section = certify.performance_section(
            certify.load_bundle(out), out / "performance-evidence.json", verification=written["verification"]
        )
        assert section["status"] == "pass"
        section["aggregate"]["geometric_mean_ci95"] = [100.0, 200.0]
        section["aggregate"]["work_excess_geometric_mean"] = 999.0
        section["aggregate"]["strict_workload_count"] = 999
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(),
        )
        assert result["ok"] is False
        assert {error["code"] for error in result["errors"]} & {"PERF_PROJECTION_MISMATCH"}
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_binds_live_native_source(tmp_path: Path) -> None:
    out, written = _demo_bundle()
    module = _adapter()
    try:
        registry_path = _write_registry(tmp_path)
        section = certify.performance_section(
            certify.load_bundle(out), out / "performance-evidence.json", verification=written["verification"]
        )
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(head="b" * 40, ancestor=False),
        )
        assert result["ok"] is False
        assert any(
            "foreign history" in error["message"] for error in result["errors"]
        ), result["errors"]
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_verifies_fixtures_by_default(tmp_path: Path) -> None:
    """B2: the checker regenerates fixtures instead of trusting the table."""
    out, written = _demo_bundle()
    module = _adapter()
    try:
        registry_path = _write_registry(tmp_path)
        section = certify.performance_section(
            certify.load_bundle(out),
            out / "performance-evidence.json",
            verification=written["verification"],
        )
        # No observation seam is injected: the real FixtureBuilder runs and cannot
        # resolve the unit-scoped synthetic recipe, so the bundle is blocked.
        assert module.FIXTURE_OBSERVER is None
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(),
        )
        assert result["ok"] is False
        assert any(
            error["code"] == "PERF_BUNDLE_NOT_CERTIFIED" and "fixture" in error["message"]
            for error in result["errors"]
        ), result["errors"]
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_rejects_a_consistent_fixture_forgery(tmp_path: Path) -> None:
    """B2: a globally consistent forged fixture table is rejected."""
    out, written = _demo_bundle()
    module = _adapter()
    try:
        registry_path = _write_registry(tmp_path)
        forged = "0" * 64
        for row in written["bundle"]["workloads"]:
            if row.get("fixture_id") == "synthetic-fixture":
                row["fixture_content_digest"] = forged
        written["bundle"]["fixtures"]["synthetic-fixture"]["content_digest"] = forged
        fixtures_path = out / "fixtures.json"
        fixtures = json.loads(fixtures_path.read_text(encoding="utf-8"))
        fixtures["synthetic-fixture"]["content_digest"] = forged
        fixtures_path.write_text(json.dumps(fixtures, sort_keys=True) + "\n", encoding="utf-8")
        written["bundle"]["artifacts"]["fixtures"]["sha256"] = certify._sha256_file(fixtures_path)
        selftest._reseal(written["bundle"], out)
        section = certify.performance_section(
            written["bundle"],
            out / "performance-evidence.json",
            verification=written["verification"],
        )
        module.FIXTURE_OBSERVER = selftest.synthetic_fixture_observer
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(),
        )
        assert result["ok"] is False
        assert any(
            error["code"] == "PERF_BUNDLE_NOT_CERTIFIED" and "fixture" in error["message"]
            for error in result["errors"]
        ), result["errors"]
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_rejects_post_state_contract_forgery(tmp_path: Path) -> None:
    """B3: the checker re-derives the post-state contract from stored bytes."""
    out, written = _demo_bundle()
    module = _adapter()
    try:
        registry_path = _write_registry(tmp_path)
        bundle = written["bundle"]
        for side in ("rust", "python"):
            record = bundle["post_state_artifacts"][f"gate.mutation.{side}"]
            path = out / record["path"]
            envelope = json.loads(path.read_text(encoding="utf-8"))
            envelope["delta"] = {"added": [], "changed": [], "removed": []}
            path.write_text(json.dumps(envelope, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            record["sha256"] = certify._sha256_file(path)
            record["canonical_sha256"] = evidencemod.canonical_sha256(envelope)
            record["bytes"] = path.stat().st_size
            delta_sha = evidencemod.canonical_sha256(envelope["delta"])
            row = next(item for item in bundle["workloads"] if item["id"] == "gate.mutation")
            row["post_state"]["sha256"][side] = delta_sha
            for sample in row["sample_records"][side]:
                sample["post_state_sha256"] = delta_sha
        selftest._reseal(bundle, out)
        section = certify.performance_section(
            bundle, out / "performance-evidence.json", verification=written["verification"]
        )
        module.FIXTURE_OBSERVER = selftest.synthetic_fixture_observer
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(),
        )
        assert result["ok"] is False
        assert any(
            error["code"] == "PERF_BUNDLE_NOT_CERTIFIED" and "post-state" in error["message"]
            for error in result["errors"]
        ), result["errors"]
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_malformed_coverage_cannot_return_a_pass_verdict(tmp_path: Path) -> None:
    out, written = _demo_bundle()
    module = _adapter()
    try:
        section = certify.performance_section(
            certify.load_bundle(out), out / "performance-evidence.json", verification=written["verification"]
        )
        section["workloads"] = ["gate.alpha", 7]
        result = module.check_bundle(
            None, section, root=REPO, registry_path=_write_registry(tmp_path),
            enforce_checker_ids=False, live_probe=_probe(),
        )
        assert result["ok"] is False
        assert any(error["code"] == "PERF_EVIDENCE_MALFORMED" for error in result["errors"])
        assert result["verdict"] != "PASS"
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_checker_adapter_reports_malformed_evidence(tmp_path: Path) -> None:
    module = _adapter()
    registry_path = _write_registry(tmp_path)
    result = module.check_bundle(
        None,
        {"aggregate": {"evidence": {"path": "/nonexistent/bundle.json", "sha256": "0" * 64}}},
        root=REPO,
        registry_path=registry_path,
        enforce_checker_ids=False,
        live_probe=_probe(),
    )
    assert result["ok"] is False
    assert result["verdict"] == "BLOCKED"
    assert any(error["code"].startswith("PERF_") for error in result["errors"])


def test_live_source_mismatch_fails() -> None:
    out, _ = _demo_bundle()
    try:
        bundle = certify.load_bundle(out)
        foreign = certify.evaluate_bundle(
            bundle,
            selftest._synthetic_registry(),
            repo=REPO,
            live_probe=_probe(head="b" * 40, ancestor=False),
            artifacts_root=out,
            require_artifacts=True,
            require_manifest=True,
        )
        assert foreign["gate"]["verdict"] == "FAIL"
        assert any("foreign history" in reason for reason in foreign["gate"]["reasons"])

        drifted_identity = selftest._synthetic_identity()
        drifted_identity["cargo"]["build_inputs"]["digest"] = "9" * 64
        drift = certify.evaluate_bundle(
            bundle,
            selftest._synthetic_registry(),
            repo=REPO,
            live_probe=selftest._SyntheticProbe(drifted_identity),
            artifacts_root=out,
            require_artifacts=True,
            require_manifest=True,
        )
        assert drift["gate"]["verdict"] == "FAIL"
        assert any("build-input digest" in reason for reason in drift["gate"]["reasons"])
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_emit_verifies_before_projecting_a_pass(tmp_path: Path) -> None:
    registry_path = _write_registry(tmp_path)
    registry_sha = evidencemod.sha256_file(registry_path)
    registry = PerformanceRegistry(
        registry_path,
        json.loads(registry_path.read_text(encoding="utf-8")),
        file_sha256=registry_sha,
    )
    out = Path(tempfile.mkdtemp(prefix="spkbench-bundle-test-"))
    module = _adapter()
    try:
        write_evidence(
            out_dir=out,
            run=selftest._synthetic_run(),
            identity=selftest._synthetic_identity(),
            fixtures={
                "synthetic-fixture": selftest.synthetic_fixture_record("synthetic-fixture", 7)
            },
            registry=registry,
            harness={"name": "spkbench", "version": "test"},
            repo=REPO,
            packs_root=None,
            live_probe=_probe(registry_sha256=registry_sha),
            verify_fixtures=True,
            fixture_observer=selftest.synthetic_fixture_observer,
        )
        bundle_path = out / "performance-evidence.json"
        out_file = tmp_path / "section.json"
        with mock.patch.object(
            certify, "NATIVE_PROBE_FACTORY", lambda _root: _probe(registry_sha256=registry_sha)
        ), mock.patch.object(
            module, "FIXTURE_OBSERVER", selftest.synthetic_fixture_observer
        ):
            args = argparse.Namespace(
                bundle=str(bundle_path), out=str(out_file), registry=str(registry_path)
            )
            code = module._cmd_emit(args)
        assert code == 0
        section = json.loads(out_file.read_text())
        assert section["status"] == "pass"
        assert section["aggregate"]["geometric_mean_speedup"] == pytest.approx(4.0, rel=0.01)
        assert section["gate"]["verdict"] == "PASS"
        # The emitted section is the checker's shape and re-validates cleanly.
        module.FIXTURE_OBSERVER = selftest.synthetic_fixture_observer
        result = module.check_bundle(
            None,
            section,
            root=REPO,
            registry_path=registry_path,
            enforce_checker_ids=False,
            live_probe=_probe(registry_sha256=registry_sha),
        )
        assert result["ok"] is True, result["errors"]
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_emit_refuses_a_forged_bundle() -> None:
    out, _ = _demo_bundle()
    module = _adapter()
    try:
        bundle_path = out / "performance-evidence.json"
        bundle = certify.load_bundle(out)
        bundle["aggregate"]["geometric_mean_speedup"] = 99.0
        bundle_path.write_text(json.dumps(bundle, indent=2, sort_keys=True) + "\n")
        with mock.patch.object(certify, "NATIVE_PROBE_FACTORY", lambda _root: _probe()):
            args = argparse.Namespace(bundle=str(bundle_path), out=None, registry=None)
            code = module._cmd_emit(args)
        assert code == 2
    finally:
        shutil.rmtree(out, ignore_errors=True)


def test_emit_never_projects_pass_from_a_non_pass_gate(tmp_path: Path) -> None:
    out, _ = _demo_bundle()
    module = _adapter()
    try:
        bundle_path = out / "performance-evidence.json"
        bundle = certify.load_bundle(out)
        for row in bundle["workloads"]:
            if row["id"] == "gate.alpha":
                row["status"] = "schema_failed"
        bundle["gate"] = {"verdict": "PASS", "reasons": ["forged"]}
        bundle["aggregate"]["status"] = "pass"
        selftest._reseal(bundle, out)
        out_file = tmp_path / "section.json"
        with mock.patch.object(certify, "NATIVE_PROBE_FACTORY", lambda _root: _probe()):
            args = argparse.Namespace(bundle=str(bundle_path), out=str(out_file), registry=None)
            code = module._cmd_emit(args)
        assert code == 2
        section = json.loads(out_file.read_text())
        assert section["status"] != "pass"
    finally:
        shutil.rmtree(out, ignore_errors=True)
