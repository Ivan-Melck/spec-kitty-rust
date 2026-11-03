"""Real lock-exact controls for the development Unicode-table generator."""

from __future__ import annotations

import importlib.util
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/rust/generate_python_repr_unicode.py"
SPEC = importlib.util.spec_from_file_location("repr_unicode_generator", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class GeneratorTests(unittest.TestCase):
    def fixture(self, root: Path) -> Path:
        verifier = root / "scripts/rust/verify_oracle.py"
        verifier.parent.mkdir(parents=True)
        for name in ("uv.lock", "pyproject.toml"):
            shutil.copyfile(ROOT / name, root / name)
        shutil.copyfile(ROOT / "scripts/rust/verify_oracle.py", verifier)
        target = root / GENERATOR.OUTPUT
        target.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / GENERATOR.OUTPUT, target)
        return target

    def run_check(self, root: Path, cwd: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root), "--check"],
            cwd=cwd,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_verifier_does_not_switch_into_source_checkout(self) -> None:
        # Wrap the REAL verifier process, never synthesize a passing result.
        original = GENERATOR.subprocess.run
        with patch.object(GENERATOR.subprocess, "run", wraps=original) as observed:
            GENERATOR.verify(ROOT)
        self.assertEqual(observed.call_count, 1)
        self.assertNotEqual(Path(observed.call_args.kwargs["cwd"]).resolve(), ROOT)
        self.assertIn(str(ROOT), observed.call_args.args[0])

    def test_exact_oracle_and_committed_table_pass(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            result = self.run_check(ROOT, Path(scratch))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("up to date (701 ranges)", result.stdout)

    def test_missing_verifier_fails_without_creating_output(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            result = self.run_check(root, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing oracle verifier", result.stdout + result.stderr)
            self.assertFalse((root / GENERATOR.OUTPUT).exists())

    def test_real_lock_drift_fails_before_output(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            target = self.fixture(root)
            before = target.read_bytes()
            lock = root / "uv.lock"
            text = lock.read_text(encoding="utf-8")
            package = next(p for p in tomllib.loads(text)["package"] if p["name"] == "packaging")
            old = f'name = "packaging"\nversion = "{package["version"]}"'
            self.assertEqual(text.count(old), 1)
            lock.write_text(text.replace(old, 'name = "packaging"\nversion = "0.0.0"'), encoding="utf-8")
            result = self.run_check(root, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not lock-exact", result.stdout + result.stderr)
            self.assertEqual(target.read_bytes(), before)

    def test_altered_table_check_fails_without_rewriting(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            target = self.fixture(root)
            target.write_bytes(target.read_bytes() + b"\n// intentional fixture mutation\n")
            before = target.read_bytes()
            result = self.run_check(root, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("differs from a fresh verified generation", result.stdout + result.stderr)
            self.assertEqual(target.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
