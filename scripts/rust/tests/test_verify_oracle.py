"""Oracle identity cannot turn a missing dependency closure into a green check."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from scripts.rust.verify_oracle import verify


class OracleVerificationTests(unittest.TestCase):
    def report(self, packages, *, requirements=None, root_requirements=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dependencies = root_requirements or ["dep>=1"]
            import json

            (root / "pyproject.toml").write_text('[project]\nname="demo"\ndependencies=' + json.dumps(dependencies) + "\n")
            (root / "uv.lock").write_text('[[package]]\nname="demo"\nversion="0.1"\n[[package]]\nname="dep"\nversion="1.0"\n')
            distributions = [SimpleNamespace(metadata={"Name": name}, version=version) for name, version in packages]
            with (
                patch("scripts.rust.verify_oracle.metadata.distributions", return_value=distributions),
                patch("scripts.rust.verify_oracle.metadata.requires", side_effect=lambda name: (requirements or {}).get(name)),
                patch("scripts.rust.verify_oracle.sys.version_info", (3, 11)),
            ):
                return verify(root)

    def test_genuine_complete_pinned_environment_passes(self):
        self.assertTrue(self.report([("demo", "0.1"), ("dep", "1.0")])["ok"])

    def test_empty_and_incomplete_environments_fail(self):
        for packages in [[], [("demo", "0.1")]]:
            self.assertFalse(self.report(packages)["ok"])

    def test_wrong_unlocked_and_duplicate_versions_fail(self):
        for packages in [
            [("demo", "0.1"), ("dep", "2.0")],
            [("demo", "0.1"), ("dep", "1.0"), ("extra", "1.0")],
            [("demo", "0.1"), ("dep", "2.0"), ("dep", "1.0")],
        ]:
            self.assertFalse(self.report(packages)["ok"])

    def test_missing_transitive_runtime_dependency_fails(self):
        report = self.report([("demo", "0.1"), ("dep", "1.0")], requirements={"dep": ["missing>=1"]})
        self.assertFalse(report["ok"])
        self.assertEqual(report["missing"], ["missing"])

    def test_unsatisfied_requirement_fails_even_when_lock_matches(self):
        report = self.report([("demo", "0.1"), ("dep", "1.0")], root_requirements=["dep>=2"])
        self.assertFalse(report["ok"])
        self.assertEqual(report["unsatisfied"], ["dep>=2"])


if __name__ == "__main__":
    unittest.main()
