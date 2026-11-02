"""Source, binary, and dependency identity checks.

The benchmark is only meaningful if four things are pinned and recorded:

1. the repository source revision (and dirty state),
2. the Rust release binary actually measured (hash + mtime + size),
3. the Python oracle source tree (``<repo>/src`` on ``PYTHONPATH``), and
4. the Python oracle dependency versions, checked against ``uv.lock``.

Any mismatch is reported, never silently ignored. ``--require-exact-identity``
turns a mismatch into a blocking gate result.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import tomllib
from importlib import metadata as importlib_metadata
from pathlib import Path


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def tree_manifest(root: Path, *, suffixes: tuple[str, ...] | None = None) -> dict:
    """Deterministic manifest of a directory tree (relative POSIX paths)."""
    entries: dict[str, dict] = {}
    if not root.exists():
        return {"root": str(root), "exists": False, "entries": entries}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        if suffixes and path.suffix not in suffixes:
            continue
        rel = path.relative_to(root).as_posix()
        entries[rel] = {"sha256": sha256_file(path), "size": path.stat().st_size}
    digest = hashlib.sha256()
    for rel in sorted(entries):
        digest.update(rel.encode())
        digest.update(b"\0")
        digest.update(entries[rel]["sha256"].encode())
        digest.update(b"\n")
    return {
        "root": str(root),
        "exists": True,
        "file_count": len(entries),
        "digest": digest.hexdigest(),
        "entries": entries,
    }


def _git(repo: Path, *args: str) -> dict:
    try:
        proc = subprocess.run(
            ["git", *args],
            cwd=str(repo),
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        return {"exit": proc.returncode, "stdout": proc.stdout.strip(), "stderr": proc.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"exit": -1, "stdout": "", "stderr": str(exc)}


def _normalize_dist(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).strip().lower()


def lock_packages(lock_path: Path) -> dict[str, str]:
    data = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    return {
        _normalize_dist(pkg["name"]): pkg["version"]
        for pkg in data.get("package", [])
        if "name" in pkg and "version" in pkg
    }


def installed_distributions(python: Path) -> dict[str, str]:
    """Read installed versions from the oracle interpreter's site-packages.

    Runs a tiny program in the oracle interpreter; no benchmark workload is
    imported. Falls back to the current interpreter only when the call fails.
    """
    program = (
        "import json,re,importlib.metadata as m;"
        "print(json.dumps({re.sub(r'[-_.]+','-',d.metadata['Name']).lower():d.version"
        " for d in m.distributions() if d.metadata['Name']}))"
    )
    try:
        proc = subprocess.run(
            [str(python), "-c", program],
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        if proc.returncode == 0:
            return json.loads(proc.stdout)
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
        pass
    return {
        _normalize_dist(dist.metadata["Name"]): dist.version
        for dist in importlib_metadata.distributions()
        if dist.metadata["Name"]
    }


def dependency_identity(lock_path: Path, python: Path) -> dict:
    """Compare installed runtime distributions with ``uv.lock`` entries.

    The lock contains development tooling too; installed-only is the expected
    subset for a ``--no-dev`` oracle environment. Every *overlapping* name must
    match exactly, and no installed distribution may be absent from the lock.
    """
    locked = lock_packages(lock_path)
    installed = installed_distributions(python)
    common = sorted(set(locked) & set(installed))
    mismatches = [
        {"name": name, "lock": locked[name], "installed": installed[name]}
        for name in common
        if locked[name] != installed[name]
    ]
    installed_not_locked = sorted(set(installed) - set(locked))
    status = "exact" if not mismatches and not installed_not_locked else "drift"
    return {
        "status": status,
        "lock_path": str(lock_path),
        "lock_sha256": sha256_file(lock_path) if lock_path.exists() else None,
        "locked_package_count": len(locked),
        "installed_package_count": len(installed),
        "overlapping_package_count": len(common),
        "mismatches": mismatches,
        "installed_not_in_lock": installed_not_locked,
        "installed_versions": {name: installed[name] for name in common},
    }


def source_identity(repo: Path, python: Path) -> dict:
    src = repo / "src"
    manifest = tree_manifest(src, suffixes=(".py", ".yaml", ".yml", ".json", ".toml"))
    pyproject = repo / "pyproject.toml"
    project_version = None
    if pyproject.exists():
        try:
            project_version = tomllib.loads(pyproject.read_text(encoding="utf-8"))["project"]["version"]
        except (KeyError, tomllib.TOMLDecodeError):
            project_version = None
    # What the oracle actually imports: PYTHONPATH puts <repo>/src first, so
    # the source tree identity above is authoritative. Confirm it is not being
    # shadowed by an installed copy earlier on sys.path.
    probe = subprocess.run(
        [str(python), "-c", "import specify_cli,sys;print(specify_cli.__file__)"],
        env={
            **os.environ,
            "PYTHONPATH": str(src),
            # Never let an identity probe write bytecode into the source tree.
            "PYTHONDONTWRITEBYTECODE": "1",
        },
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    imported_from = probe.stdout.strip() if probe.returncode == 0 else None
    imported_from_repo = bool(imported_from and Path(imported_from).resolve().is_relative_to(src.resolve()))
    return {
        "repo_head": _git(repo, "rev-parse", "HEAD"),
        "repo_short_head": _git(repo, "rev-parse", "--short", "HEAD"),
        "git_status_porcelain": _git(repo, "status", "--porcelain"),
        "project_version": project_version,
        "src_tree": {k: manifest[k] for k in ("root", "exists", "file_count", "digest")},
        "src_manifest": manifest["entries"],
        "oracle_imports_specify_cli_from": imported_from,
        "oracle_imports_from_repo_src": imported_from_repo,
        "pythonpath": str(src),
    }


def binary_identity(binary: Path) -> dict:
    if not binary.exists():
        return {"path": str(binary), "exists": False}
    stat = binary.stat()
    version = None
    try:
        proc = subprocess.run(
            [str(binary), "--version"], capture_output=True, text=True, timeout=30, check=False
        )
        version = {"exit": proc.returncode, "stdout": proc.stdout.strip(), "stderr": proc.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired) as exc:
        version = {"error": str(exc)}
    return {
        "path": str(binary),
        "exists": True,
        "sha256": sha256_file(binary),
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "version": version,
    }


def packs_identity(packs_root: Path | None) -> dict:
    if packs_root is None:
        return {"path": None, "exists": False}
    manifest = tree_manifest(packs_root)
    return {
        "path": str(packs_root),
        "exists": manifest["exists"],
        "file_count": manifest.get("file_count"),
        "digest": manifest.get("digest"),
    }


def collect(*, repo: Path, binary: Path, oracle_python: Path, packs_root: Path | None) -> dict:
    lock = repo / "uv.lock"
    deps = dependency_identity(lock, oracle_python)
    src = source_identity(repo, oracle_python)
    binfo = binary_identity(binary)
    packs = packs_identity(packs_root)
    problems = []
    if deps["status"] != "exact":
        problems.append("python dependency versions do not match uv.lock")
    if not binfo.get("exists"):
        problems.append("rust binary missing")
    if not src.get("oracle_imports_from_repo_src"):
        problems.append("oracle interpreter does not import specify_cli from <repo>/src")
    return {
        "status": "exact" if not problems else "drift",
        "problems": problems,
        "source": src,
        "dependencies": deps,
        "rust_binary": binfo,
        "packs": packs,
        "uv_lock_sha256": sha256_file(lock) if lock.exists() else None,
        "pyproject_sha256": sha256_file(repo / "pyproject.toml")
        if (repo / "pyproject.toml").exists()
        else None,
    }
