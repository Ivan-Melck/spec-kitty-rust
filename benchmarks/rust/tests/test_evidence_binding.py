"""Focused unit tests for the stored-evidence derivation and the probe contract.

The end-to-end negatives live in ``spkbench.selftest`` and
``test_gate_bundle.py``. These tests pin the low-level definitions the whole
binding rests on: what a payload's canonical hash *means*, that the manifest is
verified, that per-sample fields are tied to the stored artifacts, and that the
harness manifest covers the program that produced the evidence.
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path


BENCH = Path(__file__).resolve().parents[1]
REPO = BENCH.parents[1]
sys.path.insert(0, str(BENCH))

from spkbench import certify, evidence as evidencemod, selftest  # noqa: E402
from spkbench.live import HARNESS_CODE_PATHS, MEASURED_INPUT_PATHS, harness_manifest  # noqa: E402


def _probe(**kwargs) -> selftest._SyntheticProbe:
    return selftest._SyntheticProbe(selftest._synthetic_identity(), **kwargs)


def _evaluate(bundle: dict, root: Path, probe=None) -> dict:
    # Fixture regeneration is on, with an explicit observation seam for the
    # unit-scoped synthetic fixture. No bundle flag, custom registry or asserted
    # verdict turns the check off.
    return certify.evaluate_bundle(
        bundle,
        selftest._synthetic_registry(),
        repo=REPO,
        live_probe=probe or _probe(),
        verify_fixtures=True,
        fixture_observer=selftest.synthetic_fixture_observer,
        artifacts_root=root,
        require_artifacts=True,
        require_manifest=True,
    )


def _rewrite_stored_post_state(bundle: dict, root: Path, wid: str, mutate) -> str:
    """Rewrite both sides' stored post-state envelopes and re-seal every hash.

    This is the *consistent* forgery a forger would produce: the envelope file,
    its artifact record, the preflight hash, every per-sample hash, the row
    projection, the bundle self-hash and the manifest all agree. Only the
    semantic re-derivation can reject it.
    """
    delta_sha = None
    for side in ("rust", "python"):
        key = f"{wid}.{side}"
        record = bundle["post_state_artifacts"][key]
        path = root / record["path"]
        envelope = json.loads(path.read_text(encoding="utf-8"))
        mutate(envelope)
        path.write_text(json.dumps(envelope, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        record["sha256"] = certify._sha256_file(path)
        record["canonical_sha256"] = evidencemod.canonical_sha256(envelope)
        record["bytes"] = path.stat().st_size
        delta_sha = evidencemod.canonical_sha256(envelope["delta"])
        row = next(item for item in bundle["workloads"] if item["id"] == wid)
        row["post_state"]["sha256"][side] = delta_sha
        for sample in row["sample_records"][side]:
            sample["post_state_sha256"] = delta_sha
    selftest._reseal(bundle, root)
    return delta_sha


def test_payload_canonical_hash_is_representation_independent() -> None:
    payload = {"b": [1, 2, {"z": None}], "a": "x"}
    compact = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    pretty = (json.dumps(payload, indent=4) + "\n").encode()
    assert compact != pretty
    assert evidencemod.sha256_bytes(compact) != evidencemod.sha256_bytes(pretty)
    assert evidencemod.canonical_sha256(payload) == evidencemod.sha256_bytes(compact)


def test_missing_payload_declaration_blocks() -> None:
    bundle, root = selftest._bundle()
    try:
        bundle["payload_artifacts"].pop("gate.alpha.rust")
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] == "BLOCKED"
        assert any(
            "stored normalized payload for rust is missing" in reason
            for reason in result["gate"]["blockers"]
        ), result["gate"]["blockers"]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_manifest_payload_table_tamper_fails() -> None:
    bundle, root = selftest._bundle()
    try:
        manifest = json.loads((root / "manifest.json").read_text())
        key = next(iter(manifest["payloads"]))
        manifest["payloads"][key]["sha256"] = "0" * 64
        (root / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] == "FAIL"
        assert any(
            "manifest.json payload table" in message for message in result["gate"]["failures"]
        ), result["gate"]["failures"]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_extra_raw_sample_row_fails() -> None:
    bundle, root = selftest._bundle()
    try:
        samples = root / "samples.jsonl"
        rows = [json.loads(line) for line in samples.read_text().splitlines() if line.strip()]
        rows.append(
            {
                "workload": "gate.alpha",
                "side": "rust",
                "pair_index": 999,
                "duration_s": 0.02,
                "timed_out": False,
                "exit_code": 0,
            }
        )
        samples.write_text("".join(json.dumps(row, sort_keys=True) + "\n" for row in rows))
        bundle["artifacts"]["samples"]["sha256"] = certify._sha256_file(samples)
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] == "FAIL"
        assert any(
            "pair indices in the bundle do not match samples.jsonl" in message
            for message in result["gate"]["failures"]
        ), result["gate"]["failures"]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_mutation_sample_post_state_hash_is_bound() -> None:
    bundle, root = selftest._bundle()
    try:
        row = next(item for item in bundle["workloads"] if item["id"] == "gate.mutation")
        row["sample_records"]["rust"][0]["post_state_sha256"] = "0" * 64
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] == "FAIL"
        assert any(
            "post-state hash differs from the preflight post-state" in message
            for message in result["gate"]["failures"]
        ), result["gate"]["failures"]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_registry_file_hash_is_checked() -> None:
    bundle, root = selftest._bundle()
    try:
        result = _evaluate(bundle, root, probe=_probe(registry_sha256=None))
        assert result["gate"]["verdict"] == "BLOCKED"
        assert any(
            "frozen registry file could not be read" in reason
            for reason in result["gate"]["blockers"]
        ), result["gate"]["blockers"]

        result = _evaluate(bundle, root, probe=_probe(registry_sha256="9" * 64))
        assert result["gate"]["verdict"] == "FAIL"
        assert any(
            "frozen registry file changed since the measurement" in message
            for message in result["gate"]["failures"]
        ), result["gate"]["failures"]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_harness_manifest_covers_producer_not_tests() -> None:
    manifest = harness_manifest(REPO)
    assert "benchmarks/rust/bench.py" in manifest
    assert "scripts/rust/performance_evidence.py" in manifest
    assert any(name.startswith("benchmarks/rust/spkbench/") for name in manifest)
    assert any(name.startswith("benchmarks/rust/adapters/") for name in manifest)
    assert not any(name.startswith("benchmarks/rust/tests/") for name in manifest)
    assert not any("__pycache__" in name for name in manifest)
    # The measured-input set must cover the recorded producer so a later commit
    # that changes the harness is visible to the descendant check.
    for relative in HARNESS_CODE_PATHS:
        assert any(root == relative or root.startswith(relative + "/") for root in MEASURED_INPUT_PATHS)


def test_command_recipe_must_match_the_frozen_registry() -> None:
    bundle, root = selftest._bundle()
    try:
        row = next(item for item in bundle["workloads"] if item["id"] == "gate.alpha")
        row["command_argv"] = {"rust": ["/bin/true"], "python": ["/bin/true"]}
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS"
        assert any("command_argv" in message for message in result["gate"]["failures"])
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_stored_fixture_table_must_match_the_bundle() -> None:
    bundle, root = selftest._bundle()
    try:
        path = root / "fixtures.json"
        fixtures = json.loads(path.read_text())
        fixtures["injected"] = dict(fixtures["synthetic-fixture"])
        path.write_text(json.dumps(fixtures, sort_keys=True) + "\n")
        bundle["artifacts"]["fixtures"]["sha256"] = certify._sha256_file(path)
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS"
        assert any(
            "fixtures.json content" in message
            for message in result["gate"]["failures"]
        ), result["gate"]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_fixture_identity_is_bound_to_registry_and_manifest() -> None:
    for mutation in ("foreign_recipe", "wrong_digest", "missing_record"):
        bundle, root = selftest._bundle()
        try:
            row = next(item for item in bundle["workloads"] if item["id"] == "gate.alpha")
            if mutation == "foreign_recipe":
                bundle["fixtures"]["foreign-recipe"] = dict(bundle["fixtures"]["synthetic-fixture"])
                row["fixture_id"] = "foreign-recipe"
            elif mutation == "wrong_digest":
                row["fixture_content_digest"] = "0" * 64
            else:
                bundle["fixtures"].pop("synthetic-fixture")
            selftest._reseal(bundle, root)
            result = _evaluate(bundle, root)
            assert result["gate"]["verdict"] != "PASS", mutation
            assert any(
                "gate.alpha: fixture" in message
                for message in result["gate"]["failures"]
            ), result["gate"]
        finally:
            shutil.rmtree(root, ignore_errors=True)


def test_consistent_fixture_table_forgery_is_rejected() -> None:
    """B2: row digest + bundle table + fixtures.json + manifest forged together."""
    bundle, root = selftest._bundle()
    try:
        forged = "0" * 64
        for row in bundle["workloads"]:
            if row.get("fixture_id") == "synthetic-fixture":
                row["fixture_content_digest"] = forged
        bundle["fixtures"]["synthetic-fixture"]["content_digest"] = forged
        fixtures_path = root / "fixtures.json"
        fixtures = json.loads(fixtures_path.read_text(encoding="utf-8"))
        fixtures["synthetic-fixture"]["content_digest"] = forged
        fixtures_path.write_text(json.dumps(fixtures, sort_keys=True) + "\n", encoding="utf-8")
        bundle["artifacts"]["fixtures"]["sha256"] = certify._sha256_file(fixtures_path)
        selftest._reseal(bundle, root)

        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("fixture" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_fixture_verification_cannot_be_disabled_by_bundle_flags() -> None:
    """A bundle cannot label itself synthetic to skip the fixture observation."""
    bundle, root = selftest._bundle()
    try:
        bundle["harness"]["synthetic"] = True
        bundle["environment"]["synthetic"] = True
        bundle["registry"]["synthetic"] = True
        forged = "1" * 64
        bundle["fixtures"]["synthetic-fixture"]["content_digest"] = forged
        for row in bundle["workloads"]:
            if row.get("fixture_id") == "synthetic-fixture":
                row["fixture_content_digest"] = forged
        fixtures_path = root / "fixtures.json"
        fixtures = json.loads(fixtures_path.read_text(encoding="utf-8"))
        fixtures["synthetic-fixture"]["content_digest"] = forged
        fixtures_path.write_text(json.dumps(fixtures, sort_keys=True) + "\n", encoding="utf-8")
        bundle["artifacts"]["fixtures"]["sha256"] = certify._sha256_file(fixtures_path)
        selftest._reseal(bundle, root)

        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        assert any(
            "fixture" in message
            for message in result["gate"]["blockers"] + result["gate"]["failures"]
        )
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_post_state_delta_is_re_derived_from_the_registry_contract() -> None:
    """B3: an empty or wrong-path delta is rejected even when self-consistent."""
    for label, mutation in (
        ("empty", lambda envelope: envelope.__setitem__("delta", {"added": [], "changed": [], "removed": []})),
        (
            "wrong-path",
            lambda envelope: envelope.__setitem__(
                "delta", {"added": ["somewhere/else.json"], "changed": [], "removed": []}
            ),
        ),
    ):
        bundle, root = selftest._bundle()
        try:
            _rewrite_stored_post_state(bundle, root, "gate.mutation", mutation)
            result = _evaluate(bundle, root)
            assert result["gate"]["verdict"] != "PASS", (label, result["gate"])
            reasons = result["gate"]["blockers"] + result["gate"]["failures"]
            assert any("post-state" in message for message in reasons), (label, reasons)
        finally:
            shutil.rmtree(root, ignore_errors=True)


def test_post_state_path_without_content_is_rejected() -> None:
    """B3: naming the required path is not evidence of what was written."""
    bundle, root = selftest._bundle()
    try:

        def mutate(envelope: dict) -> None:
            envelope["files"] = {}
            for rel in envelope["after"]:
                envelope["after"][rel] = rel + "#not-a-digest"
            envelope["delta"]["added_sha256"] = {
                rel: envelope["after"][rel] for rel in envelope["delta"]["added"]
            }

        _rewrite_stored_post_state(bundle, root, "gate.mutation", mutate)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("post-state" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_post_state_unexpected_addition_is_rejected() -> None:
    """B3: an undeclared write next to the required artifact fails."""
    bundle, root = selftest._bundle()
    try:

        def mutate(envelope: dict) -> None:
            evil = "unexpected/evil.txt"
            envelope["after"][evil] = "2" * 64
            envelope["delta"]["added"].append(evil)
            envelope["delta"].setdefault("added_sha256", {})[evil] = "2" * 64

        _rewrite_stored_post_state(bundle, root, "gate.mutation", mutate)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("undeclared post-state addition" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_post_state_required_file_bytes_are_bound_to_the_manifest() -> None:
    """B3: replacing the stored bytes breaks the manifest content binding."""
    bundle, root = selftest._bundle()
    try:

        def mutate(envelope: dict) -> None:
            for rel, record in envelope["files"].items():
                record["content_b64"] = "e30="  # `{}`
                record["bytes"] = 2
                record["sha256"] = "3" * 64

        _rewrite_stored_post_state(bundle, root, "gate.mutation", mutate)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("post-state" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_recorded_input_manifest_is_the_generated_fixture_manifest() -> None:
    """R1: the positive control records the real fixture bytes, not a placeholder."""
    bundle, root = selftest._bundle()
    try:
        expected = selftest.synthetic_fixture_manifest("synthetic-fixture", 7)
        for side in ("rust", "python"):
            record = bundle["post_state_artifacts"][f"gate.mutation.{side}"]
            envelope = json.loads((root / record["path"]).read_text(encoding="utf-8"))
            for rel, digest in expected.items():
                assert envelope["before"].get(rel) == digest, rel
                assert envelope["after"].get(rel) == digest, rel
        assert "keep.json" not in envelope["before"]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_consistent_forged_unchanged_input_hash_is_rejected() -> None:
    """R1: a re-sealed lie about an unchanged input must still be rejected.

    Both sides' ``before`` and ``after`` manifests are rewritten to the same
    forged digest, so the delta still re-derives and every stored hash is
    re-sealed. Only the comparison with the independently regenerated fixture
    bytes can reject it.
    """
    bundle, root = selftest._bundle()
    try:
        forged = "9" * 64

        def mutate(envelope: dict) -> None:
            victim = sorted(rel for rel in envelope["before"] if rel.endswith(".txt"))[0]
            envelope["before"][victim] = forged
            envelope["after"][victim] = forged

        _rewrite_stored_post_state(bundle, root, "gate.mutation", mutate)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("regenerated fixture bytes" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_pretty_printed_post_state_envelope_still_verifies() -> None:
    """The stored envelope is a document: reformatting it is not an edit."""
    bundle, root = selftest._bundle()
    try:
        record = bundle["post_state_artifacts"]["gate.mutation.rust"]
        path = root / record["path"]
        envelope = json.loads(path.read_text(encoding="utf-8"))
        path.write_text(json.dumps(envelope, indent=4, sort_keys=True), encoding="utf-8")
        record["sha256"] = certify._sha256_file(path)
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] == "PASS", result["gate"]["reasons"][:5]
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_sample_order_must_be_a_global_unique_counter() -> None:
    """H1: per-workload reset orders and duplicate orders are rejected."""
    bundle, root = selftest._bundle()
    try:
        for row in bundle["workloads"]:
            for side in ("rust", "python"):
                for index, sample in enumerate(row["sample_records"][side]):
                    sample["order"] = index
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("order" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_pair_observation_orders_must_be_adjacent() -> None:
    """H1: the two sides of a pair are adjacent executor observations."""
    bundle, root = selftest._bundle()
    try:
        row = next(item for item in bundle["workloads"] if item["id"] == "gate.alpha")
        for sample in row["sample_records"]["rust"]:
            sample["order"] += 2
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("pair" in message and "order" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_observation_orders_are_unique_across_workloads() -> None:
    """H1: the executor counter is global, not per workload."""
    bundle, root = selftest._bundle()
    try:
        alpha = next(item for item in bundle["workloads"] if item["id"] == "gate.alpha")
        beta = next(item for item in bundle["workloads"] if item["id"] == "gate.beta")
        for side in ("rust", "python"):
            base = alpha["sample_records"]["rust"][0]["order"]
            beta["sample_records"][side][0]["order"] = base if side == "rust" else base + 1
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("order" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_raw_sample_order_must_match_the_bundle_record() -> None:
    """H1: reconstruction uses the recorded order, not raw line position."""
    bundle, root = selftest._bundle()
    try:
        samples_path = root / "samples.jsonl"
        rows = [json.loads(line) for line in samples_path.read_text().splitlines() if line.strip()]
        victim = next(
            row for row in rows if row["workload"] == "gate.beta" and row["side"] == "rust"
        )
        victim["order"] = victim["order"] + 1000
        samples_path.write_text(
            "".join(json.dumps(row, sort_keys=True) + "\n" for row in rows), encoding="utf-8"
        )
        bundle["artifacts"]["samples"]["sha256"] = certify._sha256_file(samples_path)
        selftest._reseal(bundle, root)
        result = _evaluate(bundle, root)
        assert result["gate"]["verdict"] != "PASS", result["gate"]
        reasons = result["gate"]["blockers"] + result["gate"]["failures"]
        assert any("order" in message for message in reasons), reasons
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_required_registry_floor_is_unchanged() -> None:
    registry = selftest._synthetic_registry()
    assert len(registry.required_ids) == len(
        [entry for entry in registry.workloads if entry.get("required")]
    )
    # The production registry keeps its frozen 20/17/20 floor; the synthetic
    # fixture above is unit-scoped and never replaces it.
    from spkbench.registry import PerformanceRegistry

    production = PerformanceRegistry.load(REPO / "docs/rust/inventory/performance-registry.json")
    assert len(production.required_ids) == 20
    assert len(production.gate_ids) == 17
    assert production.policy["minimum_samples_per_side"] == 20
    assert production.policy["minimum_speedup"] == 3.0
    assert production.policy["max_per_workload_regression_fraction"] == 0.10
    assert production.policy["minimum_strict_workloads"] == 14
