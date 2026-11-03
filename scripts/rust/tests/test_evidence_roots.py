"""Evidence must belong to the checkout supplied by --root, not this tool's home."""
from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import check_completion as checker  # noqa: E402


class EvidenceRootTests(unittest.TestCase):
    def test_reference_resolution_uses_the_audited_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = root / "tests" / "only-in-this-checkout.rs"
            evidence.parent.mkdir()
            evidence.write_text("// evidence\n")
            self.assertEqual(checker._ref_path("tests/only-in-this-checkout.rs:12", root), evidence)
            self.assertTrue(checker._ref_exists("tests/only-in-this-checkout.rs", root))
            self.assertEqual(checker._ref_path(str(evidence), root), evidence)
            self.assertFalse(checker._ref_exists("crates/sk-cli/src/main.rs", root))

    def test_contract_evidence_cannot_be_borrowed_from_the_tool_checkout(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = checker.Report()
            inventory = {"entries": [{
                "id": "test-entry",
                "implementation": {"native_refs": ["crates/sk-cli/src/main.rs"]},
            }]}
            checker._check_contracts(report, inventory, root)
            self.assertIn("MISSING_NATIVE_REF", {error["code"] for error in report.errors})

    def test_native_evidence_is_classified_under_the_audited_crates_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "audited" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub fn audited() {}\n")
            inventory = {"entries": [{
                "id": "test-entry", "required": True,
                "implementation": {"status": "implemented", "native_refs": ["crates/audited/src/lib.rs"]},
                "conformance": {"status": "pass", "refs": ["tests/parity.rs"]},
                "review": {"status": "approved", "independent": True, "refs": ["docs/review.md"]},
            }]}
            report = checker.Report()
            checker._check_status_gates(report, inventory, root)
            self.assertEqual(report.errors, [])
            inventory["entries"][0]["implementation"]["native_refs"] = [str(checker.ROOT / "crates/sk-cli/src/main.rs")]
            report = checker.Report()
            checker._check_status_gates(report, inventory, root)
            self.assertIn("IMPLEMENTATION_NOT_NATIVE", {error["code"] for error in report.errors})


if __name__ == "__main__":
    unittest.main()
