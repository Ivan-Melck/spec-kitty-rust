"""Live observations that bind a stored bundle to the source it claims.

A bundle records *what it saw*. Certification additionally needs the facts a
reader can re-observe from the checkout right now:

* the current commit and working-tree state,
* the digest of every tracked Rust build input (``Cargo.toml``, ``Cargo.lock``,
  ``rust-toolchain.toml``, ``crates/**``),
* the ``Cargo.lock`` hash and the live toolchain,
* the sha256 of both executables, and
* the build receipt, re-verified against the live tree.

It also hashes the program that produced the evidence (``bench.py``, the
``spkbench`` package, the adapters and the completion-checker adapter) so a
bundle identifies its producer instead of merely naming it.

:class:`NativeSourceProbe` is a *probe*, not a verdict: it returns observations
and lets :mod:`spkbench.certify` judge them. Tests inject a probe with the same
shape (see ``spkbench.selftest``) so a synthetic environment can exercise the
full verifier; the CLI and the completion checker always construct the real one.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

from . import identity
from .build_receipt import load_receipt, verify_receipt

#: Files whose content decides what a measurement means. A commit that changes
#: any of them invalidates the measurement even when it only touches docs; a
#: commit that touches none of them (evidence, docs, tests) may legitimately
#: follow the measured code.
MEASURED_INPUT_PATHS: tuple[str, ...] = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "crates",
    "benchmarks/rust/bench.py",
    "benchmarks/rust/spkbench",
    "benchmarks/rust/adapters",
    "benchmarks/rust/tools",
    "scripts/rust/performance_evidence.py",
    "docs/rust/inventory/performance-registry.json",
)

#: The producer identity recorded in ``bundle["harness"]["code"]``.
HARNESS_CODE_PATHS: tuple[str, ...] = (
    "benchmarks/rust/bench.py",
    "benchmarks/rust/spkbench",
    "benchmarks/rust/adapters",
    "benchmarks/rust/tools",
    "scripts/rust/performance_evidence.py",
)

DEFAULT_RECEIPT = "target/release/build-receipt.json"
BINARIES = ("spec-kitty", "spec-kitty-rust")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()  # noqa: TID251 - file-integrity check
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _files_under(root: Path, relative: str) -> list[Path]:
    target = root / relative
    if target.is_file():
        return [target]
    if not target.is_dir():
        return []
    return [
        path
        for path in sorted(target.rglob("*"))
        if path.is_file() and "__pycache__" not in path.parts and path.suffix in {".py", ".json", ".toml"}
    ]


def harness_manifest(root: Path) -> dict[str, str]:
    """Repo-relative sha256 of every file that can shape the evidence.

    Deterministic and root-relative, so an out-of-tree audit can compare the
    recorded producer with the checkout that is actually being certified.
    """
    manifest: dict[str, str] = {}
    for relative in HARNESS_CODE_PATHS:
        for path in _files_under(root, relative):
            try:
                key = path.resolve().relative_to(root.resolve()).as_posix()
            except ValueError:
                key = path.as_posix()
            manifest[key] = _sha256(path)
    return manifest


class NativeSourceProbe:
    """Observe the live native source, binaries, receipt and harness code."""

    def __init__(
        self,
        root: Path,
        *,
        receipt_path: Path | None = None,
        binaries: dict[str, Path] | None = None,
    ) -> None:
        self.root = Path(root)
        self.receipt_path = receipt_path
        self._binaries = {
            name: Path(value) for name, value in (binaries or {}).items() if name in BINARIES
        }

    # -- observations ----------------------------------------------------
    def binary_paths(self) -> dict[str, Path]:
        paths = {name: self.root / "target" / "release" / name for name in BINARIES}
        paths.update(self._binaries)
        return paths

    def observe(
        self,
        *,
        receipt_path: Path | None = None,
        binary_paths: dict[str, str] | None = None,
        registry_path: Path | None = None,
        oracle_python: Path | None = None,
        packs_root: Path | None = None,
        baseline_commit: str | None = None,
    ) -> dict:
        worktree = identity.source_worktree_state(self.root)
        build_inputs = identity.build_inputs_manifest(self.root)
        toolchain = identity.toolchain_identity(self.root)
        binaries = self.binary_paths()
        for name, value in (binary_paths or {}).items():
            if name in BINARIES and value:
                binaries[name] = Path(value)
        provenance = identity.binary_provenance(self.root, binaries, build_inputs)
        lock = self.root / "Cargo.lock"

        receipt_file = receipt_path or self.receipt_path
        if receipt_file is None:
            candidate = self.root / DEFAULT_RECEIPT
            receipt_file = candidate if candidate.exists() else None
        receipt = None
        if receipt_file is not None and Path(receipt_file).is_file():
            receipt = load_receipt(Path(receipt_file))
            receipt["path"] = str(receipt_file)
            receipt["verification"] = verify_receipt(
                receipt,
                repo=self.root,
                binaries=binaries,
                build_inputs=build_inputs,
                toolchain=toolchain,
            )

        return {
            "head": worktree.get("head"),
            "dirty_paths": list(worktree.get("dirty_paths") or []),
            "dirty_count": worktree.get("dirty_count"),
            "build_inputs_digest": build_inputs.get("digest"),
            "build_inputs_file_count": build_inputs.get("file_count"),
            "cargo_lock_sha256": _sha256(lock) if lock.is_file() else None,
            "toolchain": {
                "release": toolchain.get("release"),
                "host": toolchain.get("host"),
                "channel": toolchain.get("channel"),
                "file_sha256": toolchain.get("file_sha256"),
                "rustc_stdout": (toolchain.get("rustc") or {}).get("stdout"),
                "cargo_stdout": (toolchain.get("cargo") or {}).get("stdout"),
            },
            "binaries": provenance.get("binaries") or {},
            "binary_problems": list(provenance.get("problems") or []),
            "receipt": receipt,
            "receipt_path": str(receipt_file) if receipt_file else None,
            "harness": harness_manifest(self.root),
            "registry_sha256": _sha256(Path(registry_path))
            if registry_path and Path(registry_path).is_file()
            else None,
            "registry_path": str(registry_path) if registry_path else None,
            "oracle": self._observe_oracle(oracle_python, packs_root, baseline_commit),
            "packs": identity.packs_identity(packs_root),
            "uv_lock_sha256": _sha256(self.root / "uv.lock")
            if (self.root / "uv.lock").is_file()
            else None,
        }

    def _observe_oracle(
        self,
        oracle_python: Path | None,
        packs_root: Path | None,
        baseline_commit: str | None,
    ) -> dict | None:
        if oracle_python is None or not Path(oracle_python).exists():
            return None
        python = Path(oracle_python)
        resolved = python.resolve()
        return {
            "python": str(python),
            "python_real_sha256": _sha256(resolved) if resolved.is_file() else None,
            "verify": identity.oracle_verification(self.root, python, packs_root),
            "baseline": identity.baseline_source_identity(self.root, baseline_commit)
            if baseline_commit
            else None,
        }

    # -- history ---------------------------------------------------------
    def is_ancestor(self, candidate: str) -> bool:
        """True when ``candidate`` is an ancestor of (or equal to) HEAD."""
        proc = identity._git(self.root, "merge-base", "--is-ancestor", candidate, "HEAD")
        return proc.get("exit") == 0

    def changed_measured_inputs(self, candidate: str) -> list[str]:
        """Paths this bundle's measured inputs changed between candidate and HEAD."""
        proc = identity._git(
            self.root,
            "diff",
            "--name-only",
            f"{candidate}..HEAD",
            "--",
            *MEASURED_INPUT_PATHS,
        )
        if proc.get("exit") != 0:
            return [f"<git diff failed: {proc.get('stderr') or 'unknown error'}>"]
        return [line for line in (proc.get("stdout") or "").splitlines() if line.strip()]

    def commit_present(self, commit: str) -> bool:
        return identity._git(self.root, "cat-file", "-e", f"{commit}^{{commit}}").get("exit") == 0


def default_native_probe(root: Path) -> NativeSourceProbe:
    """Factory used by the verifier. Tests replace it with a synthetic probe."""
    return NativeSourceProbe(root)
