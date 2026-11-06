"""Real lock-exact controls for the development router Unicode-fact generator."""

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
SCRIPT = ROOT / "scripts/rust/generate_router_unicode.py"
SPEC = importlib.util.spec_from_file_location("router_unicode_generator", SCRIPT)
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

    def run_script(
        self, root: Path, cwd: Path, *arguments: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root), *arguments],
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

    def test_rendered_tests_are_consistent_with_the_rendered_ranges(self) -> None:
        # A future explicit pin bump must not emit stale test counts.
        text = GENERATOR.render(
            [(0, 1), (9, 10)],
            [(0x41, 0x61)],
            [],
            [(0x41, 0x41)],
            [(0x27, 0x27)],
            version="test-version",
            identity=0x110000 - 1,
            neither=0x110000 - 2,
        )
        self.assertIn("WORD_RANGE_COUNT: usize = 2;", text)
        self.assertIn("assert_eq!(ranges.len(), count", text)
        self.assertIn('const WORD_DIGEST: &str = "', text)
        self.assertIn("LOWER_EXPANSIONS: &[(u32, &[u32])] = &[];", text)
        self.assertIn("test-version", text)

    def test_rendered_upper_tables_are_consistent_and_refuse_bad_coverage(self) -> None:
        # A future pin bump must not emit stale counts or a vacuous table.
        text = GENERATOR.render_upper(
            [(0x61, 0x41)],
            [(0xDF, [0x53, 0x53])],
            version="test-version",
            identity=0x110000 - 2,
        )
        self.assertIn("UPPER_SINGLE_COUNT: usize = 1;", text)
        self.assertIn("UPPER_EXPANSION_COUNT: usize = 1;", text)
        self.assertIn("(0x00df, &[0x0053, 0x0053])", text)
        self.assertIn('const UPPER_DIGEST: &str = "', text)
        self.assertIn("test-version", text)
        self.assertIn("identity code points", text)
        with self.assertRaises(SystemExit):
            GENERATOR.render_upper(
                [(0x61, 0x41)],
                [],
                version="test-version",
                identity=0x110000 - 2,  # 2 singles + 0 expansions + this != 0x110000
            )

    def test_upper_facts_cover_every_code_point_and_keep_surrogates(self) -> None:
        singles, expansions, identity = GENERATOR.upper_facts()
        self.assertEqual(identity + len(singles) + len(expansions), 0x110000)
        self.assertEqual(dict(singles).get(0x61), 0x41, "ASCII lower case still maps")
        self.assertNotIn(0x264, dict(singles), "U+0264 has no capital in Unicode 14")
        self.assertNotIn(0x19B, dict(singles), "U+019B has no capital in Unicode 14")
        self.assertEqual(dict(expansions).get(0xDF), [0x53, 0x53])
        for code in range(0xD800, 0xE000):
            self.assertNotIn(code, dict(singles))
            self.assertNotIn(code, dict(expansions))
        GENERATOR.verify_mapping("upper", singles)
        for code, mapped in expansions:
            self.assertGreaterEqual(len(mapped), 2, hex(code))

    def test_active_tables_are_unchanged_by_the_upper_extension(self) -> None:
        # The lower/word/cased tables keep their recorded digests; only the new
        # upper output is added.
        word = GENERATOR.word_ranges()
        singles, expansions, identity = GENERATOR.lower_facts()
        cased, ignorable, neither = GENERATOR.context_facts()
        self.assertEqual(len(word), 734)
        self.assertEqual(len(singles), 1432)
        self.assertEqual(len(expansions), 1)
        self.assertEqual(identity, 1112679)
        self.assertEqual(len(cased), 149)
        self.assertEqual(len(ignorable), 427)
        self.assertEqual(neither, 1107257)

    def test_rendered_coverage_defects_are_refused(self) -> None:
        with self.assertRaises(SystemExit):
            GENERATOR.render(
                [(0, 1)],
                [],
                [],
                [(0, 0)],
                [],
                version="test-version",
                identity=0x110000 - 1,
                neither=0,  # 2 + 1 + 0 != 0x110000
            )

    def test_range_and_mapping_guards_refuse_bad_tables(self) -> None:
        with self.assertRaises(SystemExit):
            GENERATOR.verify_ranges("x", [(5, 4)])
        with self.assertRaises(SystemExit):
            GENERATOR.verify_ranges("x", [(1, 2), (2, 3)])
        with self.assertRaises(SystemExit):
            GENERATOR.verify_ranges("x", [(0, 0x110000)])
        with self.assertRaises(SystemExit):
            GENERATOR.verify_mapping("x", [(2, 1), (1, 2)])
        with self.assertRaises(SystemExit):
            GENERATOR.verify_mapping("x", [(0x41, 0x41)])
        with self.assertRaises(SystemExit):
            GENERATOR.verify_mapping("x", [(0x41, 0x110000)])

    def test_tiny_input_still_covers_every_code_point(self) -> None:
        with patch.object(GENERATOR, "LAST_CODE_POINT", 0x100):
            word = GENERATOR.word_ranges()
            singles, expansions, identity = GENERATOR.lower_facts()
            cased, ignorable, neither = GENERATOR.context_facts()
        word_total = sum(end - start + 1 for start, end in word)
        self.assertEqual(word_total + (0x100 - word_total), 0x100)
        self.assertGreater(word_total, 0)
        self.assertEqual(identity + len(singles) + len(expansions), 0x100)
        cased_total = sum(end - start + 1 for start, end in cased)
        ignorable_total = sum(end - start + 1 for start, end in ignorable)
        self.assertEqual(cased_total + ignorable_total + neither, 0x100)
        self.assertEqual(singles[0], (0x41, 0x61), "ASCII upper-case still maps")
        self.assertTrue(all(source <= 0xFF for source, _ in singles))
        self.assertTrue(all(mapped <= 0xFF for _, mapped in singles))
        GENERATOR.verify_ranges("word", word)
        GENERATOR.verify_ranges("cased", cased)
        GENERATOR.verify_ranges("ignorable", ignorable)
        GENERATOR.verify_mapping("lower", singles)

    def test_interpreter_and_unicode_pins_are_enforced(self) -> None:
        with patch.object(GENERATOR, "EXPECTED_VERSION", (3, 12, 0)), self.assertRaises(SystemExit) as raised:
            GENERATOR.verify(ROOT)
        self.assertIn("is not the pinned", str(raised.exception))
        with patch.object(GENERATOR, "EXPECTED_UNIDATA", "15.0.0"), self.assertRaises(SystemExit) as raised:
            GENERATOR.verify(ROOT)
        self.assertIn("requires Unicode", str(raised.exception))
        with patch.object(GENERATOR, "EXPECTED_REQUIREMENTS", 999), self.assertRaises(SystemExit) as raised:
            GENERATOR.verify(ROOT)
        self.assertIn("pinned 999", str(raised.exception))

    def test_exact_oracle_and_committed_table_pass(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            result = self.run_script(ROOT, Path(scratch), "--check")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("up to date (734 word ranges, 1432 lower singles, 1 expansions)", result.stdout)
        self.assertIn(
            "crates/sk-kernel/src/python_text_upper_data.rs: up to date (1423 upper singles, 102 expansions)",
            result.stdout,
        )

    def test_missing_verifier_fails_without_creating_output(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            result = self.run_script(root, root, "--check")
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
            lock.write_text(
                text.replace(old, 'name = "packaging"\nversion = "0.0.0"'), encoding="utf-8"
            )
            result = self.run_script(root, root, "--check")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not lock-exact", result.stdout + result.stderr)
            self.assertEqual(target.read_bytes(), before)

    def test_altered_table_check_fails_without_rewriting(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            target = self.fixture(root)
            target.write_bytes(target.read_bytes() + b"\n// intentional fixture mutation\n")
            before = target.read_bytes()
            result = self.run_script(root, root, "--check")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("differs from a fresh verified generation", result.stdout + result.stderr)
            self.assertEqual(target.read_bytes(), before)

    def test_write_refuses_a_different_existing_table_without_force(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            target = self.fixture(root)
            target.write_bytes(target.read_bytes() + b"\n// intentional fixture mutation\n")
            before = target.read_bytes()
            result = self.run_script(root, root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("refusing to overwrite", result.stdout + result.stderr)
            self.assertEqual(target.read_bytes(), before)
            forced = self.run_script(root, root, "--force")
            self.assertEqual(forced.returncode, 0, forced.stdout + forced.stderr)
            self.assertIn("wrote 734 word ranges", forced.stdout)
            self.assertNotEqual(target.read_bytes(), before)
            self.assertNotIn(b"intentional fixture mutation", target.read_bytes())
            checked = self.run_script(root, root, "--check")
            self.assertEqual(checked.returncode, 0, checked.stdout + checked.stderr)


if __name__ == "__main__":
    unittest.main()
