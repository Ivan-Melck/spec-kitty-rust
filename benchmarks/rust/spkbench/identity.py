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

    This probe is fail-closed by design. An earlier version silently fell back
    to the *harness* interpreter when the oracle probe failed, which certified
    a Python environment that was never measured. A missing or broken oracle
    interpreter is now an error, never a substitute.
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
            env={**os.environ, "PYTHONNOUSERSITE": "1", "PYTHONDONTWRITEBYTECODE": "1"},
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise RuntimeError(f"cannot query oracle distributions: {exc}") from None
    if proc.returncode != 0:
        raise RuntimeError(
            "oracle distribution probe failed: " + (proc.stderr.strip() or "no stderr")[:400]
        )
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"oracle distribution probe returned invalid JSON: {exc}") from None


def dependency_identity(lock_path: Path, python: Path) -> dict:
    """Compare installed runtime distributions with ``uv.lock`` entries.

    The lock contains development tooling too; installed-only is the expected
    subset for a ``--no-dev`` oracle environment. Every *overlapping* name must
    match exactly, and no installed distribution may be absent from the lock.
    """
    locked = lock_packages(lock_path)
    try:
        installed = installed_distributions(python)
    except RuntimeError as exc:
        return {
            "status": "drift",
            "lock_path": str(lock_path),
            "lock_sha256": sha256_file(lock_path) if lock_path.exists() else None,
            "locked_package_count": len(locked),
            "installed_package_count": 0,
            "overlapping_package_count": 0,
            "mismatches": [],
            "installed_not_in_lock": [],
            "installed_versions": {},
            "probe_error": str(exc),
        }
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


def oracle_verification(repo: Path, oracle_python: Path, packs_root: Path | None) -> dict:
    """Run the pinned fail-closed oracle verifier under the oracle interpreter."""
    script = repo / "scripts/rust/verify_oracle.py"
    if not script.exists():
        return {"ok": False, "error": f"missing {script}"}
    env = {
        **os.environ,
        "PYTHONPATH": str(repo / "src"),
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONNOUSERSITE": "1",
        "TZ": "UTC",
        "NO_COLOR": "1",
    }
    if packs_root is not None:
        env["SPEC_KITTY_PACKS_ROOT"] = str(packs_root)
    try:
        proc = subprocess.run(
            [str(oracle_python), str(script), "--root", str(repo)],
            capture_output=True,
            text=True,
            timeout=300,
            check=False,
            env=env,
            cwd=str(repo),
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"ok": False, "error": str(exc)}
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError:
        report = {"ok": False, "error": (proc.stderr or proc.stdout)[-2000:]}
    report["exit"] = proc.returncode
    report["script"] = "scripts/rust/verify_oracle.py"
    return report


def collect(
    *,
    repo: Path,
    binary: Path,
    oracle_python: Path,
    packs_root: Path | None,
    registry: object | None = None,
    compat_binary: Path | None = None,
    baseline_commit: str | None = None,
    require_clean: bool = True,
    build_receipt_path: Path | None = None,
) -> dict:
    lock = repo / "uv.lock"
    deps = dependency_identity(lock, oracle_python)
    src = source_identity(repo, oracle_python)
    worktree = source_worktree_state(repo)
    build_inputs = build_inputs_manifest(repo)
    binaries = {"spec-kitty-rust": binary}
    if compat_binary is not None:
        binaries["spec-kitty"] = compat_binary
    elif binary.name == "spec-kitty":
        binaries["spec-kitty"] = binary
    provenance = binary_provenance(repo, binaries, build_inputs)
    toolchain = toolchain_identity(repo)
    oracle = {
        "python": str(oracle_python),
        "python_resolved": str(oracle_python.resolve()) if oracle_python.exists() else None,
        "python_real_sha256": sha256_file(oracle_python.resolve())
        if oracle_python.exists() and oracle_python.resolve().is_file()
        else None,
        "version": _tool_version([str(oracle_python), "-V"]),
        "verify": oracle_verification(repo, oracle_python, packs_root),
        "baseline": baseline_source_identity(repo, baseline_commit) if baseline_commit else None,
    }
    packs = packs_identity(packs_root)
    receipt = None
    if build_receipt_path is not None:
        from .build_receipt import load_receipt, verify_receipt

        receipt = load_receipt(build_receipt_path)
        receipt["path"] = str(build_receipt_path)
        receipt["verification"] = verify_receipt(
            receipt, repo=repo, binaries=binaries, build_inputs=build_inputs, toolchain=toolchain
        )

    problems: list[str] = []
    if deps["status"] != "exact":
        problems.append("python dependency versions do not match uv.lock")
    if not src.get("oracle_imports_from_repo_src"):
        problems.append("oracle interpreter does not import specify_cli from <repo>/src")
    if not oracle["verify"].get("ok"):
        problems.append("oracle environment fails scripts/rust/verify_oracle.py")
    if oracle["baseline"] is None:
        problems.append("no pinned oracle source baseline was recorded")
    elif not oracle["baseline"].get("src_matches_baseline"):
        problems.append("oracle <repo>/src differs from the pinned baseline commit")
    if not packs.get("exists"):
        problems.append("repo packs root is missing")
    elif Path(packs.get("path") or "").resolve() != (repo / "packs").resolve():
        problems.append("packs root is not <repo>/packs (installed tool trees are not certification inputs)")
    if require_clean and worktree["dirty"]:
        problems.append(
            f"source tree is dirty ({worktree['dirty_count']} paths); certification requires a clean commit"
        )
    problems.extend(provenance["problems"])
    if not toolchain["rustc"].get("available"):
        problems.append("rustc is unavailable; toolchain identity cannot be recorded")
    if not toolchain["cargo"].get("available"):
        problems.append("cargo is unavailable; build identity cannot be recorded")
    if toolchain.get("channel") and toolchain.get("release") and toolchain["channel"] != toolchain["release"]:
        problems.append(
            f"rustc release {toolchain['release']} does not match rust-toolchain.toml {toolchain['channel']}"
        )
    if receipt is not None and not receipt["verification"].get("ok"):
        problems.append("build receipt does not verify against the live source/binary")

    identity: dict = {
        "status": "exact" if not problems else "drift",
        "problems": problems,
        "source": src,
        "worktree": worktree,
        "dependencies": deps,
        "rust_binary": binary_identity(binary),
        "binaries": provenance["binaries"],
        "binary_provenance": provenance,
        "cargo": {
            "build_inputs": {k: build_inputs[k] for k in ("file_count", "digest")},
            "lock_sha256": sha256_file(repo / "Cargo.lock") if (repo / "Cargo.lock").exists() else None,
            "manifest_sha256": sha256_file(repo / "Cargo.toml") if (repo / "Cargo.toml").exists() else None,
            "toolchain": toolchain,
        },
        "oracle": oracle,
        "packs": packs,
        "build_receipt": receipt,
        "uv_lock_sha256": sha256_file(lock) if lock.exists() else None,
        "pyproject_sha256": sha256_file(repo / "pyproject.toml")
        if (repo / "pyproject.toml").exists()
        else None,
    }
    if registry is not None:
        identity["registry"] = registry.identity()
    return identity


# ---------------------------------------------------------------------------
# Rust build identity (Cargo.lock / toolchain / binary provenance)
#
# A binary sha256 alone does not prove the measured executable was built from
# the recorded source. Certification therefore binds four facts and recomputes
# them live:
#
#   * the git commit and a clean working tree,
#   * a digest of every tracked Rust build input (Cargo.toml, Cargo.lock,
#     rust-toolchain.toml, crates/**),
#   * the toolchain that must have built it (rust-toolchain.toml + rustc -vV +
#     cargo -V + the release profile flags),
#   * a build receipt written by an explicit `cargo build` run, and a binary
#     newer than every build input.
#
# Without a receipt the gate blocks: an unattested binary is not evidence.
# ---------------------------------------------------------------------------

BUILD_INPUT_ROOTS = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "crates")


def _git_ls_files(repo: Path, paths: tuple[str, ...]) -> list[str]:
    proc = subprocess.run(
        ["git", "ls-files", "-z", "--", *paths],
        cwd=str(repo),
        capture_output=True,
        check=False,
        timeout=60,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.decode("utf-8", "replace").strip() or "git ls-files failed")
    return [item.decode("utf-8") for item in proc.stdout.split(b"\0") if item]


def build_inputs_manifest(repo: Path) -> dict:
    """Content digest of every tracked Rust build input."""
    files = _git_ls_files(repo, BUILD_INPUT_ROOTS)
    entries = {}
    digest = hashlib.sha256()
    for rel in sorted(files):
        path = repo / rel
        if not path.is_file():
            entries[rel] = {"missing": True}
            digest.update(rel.encode() + b"\0missing\n")
            continue
        file_digest = sha256_file(path)
        entries[rel] = {"sha256": file_digest, "size": path.stat().st_size}
        digest.update(rel.encode() + b"\0" + file_digest.encode() + b"\n")
    return {"file_count": len(entries), "digest": digest.hexdigest(), "entries": entries}


def _tool_version(argv: list[str]) -> dict:
    exe = argv[0]
    if not Path(exe).is_absolute():
        found = None
        for root in (Path.home() / ".cargo" / "bin",):
            candidate = root / exe
            if candidate.exists():
                found = str(candidate)
                break
        exe = found or __import__("shutil").which(exe) or exe
    try:
        proc = subprocess.run([exe, *argv[1:]], capture_output=True, text=True, timeout=60, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"argv": argv, "available": False, "error": str(exc)}
    return {
        "argv": argv,
        "available": proc.returncode == 0,
        "exit": proc.returncode,
        "stdout": proc.stdout.strip(),
        "stderr": proc.stderr.strip(),
    }


def toolchain_identity(repo: Path) -> dict:
    toolchain_file = repo / "rust-toolchain.toml"
    cargo_toml = repo / "Cargo.toml"
    channel = None
    if toolchain_file.exists():
        try:
            channel = tomllib.loads(toolchain_file.read_text(encoding="utf-8"))["toolchain"]["channel"]
        except (KeyError, tomllib.TOMLDecodeError):
            channel = None
    profile = None
    if cargo_toml.exists():
        try:
            profile = tomllib.loads(cargo_toml.read_text(encoding="utf-8")).get("profile", {}).get("release")
        except tomllib.TOMLDecodeError:
            profile = None
    rustc = _tool_version(["rustc", "-vV"])
    cargo = _tool_version(["cargo", "-V"])
    rustc_text = rustc.get("stdout") or ""
    host = None
    release = None
    for line in rustc_text.splitlines():
        if line.startswith("host:"):
            host = line.split(":", 1)[1].strip()
        elif line.startswith("release:"):
            release = line.split(":", 1)[1].strip()
    return {
        "file_present": toolchain_file.exists(),
        "file_sha256": sha256_file(toolchain_file) if toolchain_file.exists() else None,
        "channel": channel,
        "release": release,
        "host": host,
        "rustc": rustc,
        "cargo": cargo,
        "rustc_matches_channel": bool(channel and release and channel in release or channel == release),
        "release_profile": profile,
    }


def source_worktree_state(repo: Path) -> dict:
    status = _git(repo, "status", "--porcelain")
    paths = [line for line in status.get("stdout", "").splitlines() if line.strip()]
    return {
        "head": _git(repo, "rev-parse", "HEAD").get("stdout"),
        "dirty": bool(paths),
        "dirty_paths": paths[:200],
        "dirty_count": len(paths),
    }


def baseline_source_identity(repo: Path, baseline: str) -> dict:
    """Prove the oracle's imported ``src`` is the pinned baseline, not a dirty tree."""
    exists = _git(repo, "cat-file", "-e", f"{baseline}^{{commit}}")
    if exists.get("exit") != 0:
        return {"baseline": baseline, "commit_present": False, "src_matches_baseline": False}
    diff = _git(repo, "diff", "--quiet", baseline, "--", "src")
    return {
        "baseline": baseline,
        "commit_present": True,
        "src_matches_baseline": diff.get("exit") == 0,
        "src_diff_head": (diff.get("stdout") or "")[:2000],
    }


def binary_provenance(repo: Path, binaries: dict[str, Path], build_inputs: dict) -> dict:
    """Check each binary is newer than every tracked build input."""
    newest_rel = None
    newest_mtime = 0
    for rel in build_inputs.get("entries", {}):
        path = repo / rel
        if not path.exists():
            continue
        mtime = path.stat().st_mtime_ns
        if mtime > newest_mtime:
            newest_mtime, newest_rel = mtime, rel
    records = {}
    problems = []
    for name, path in binaries.items():
        if not path or not path.exists():
            records[name] = {"exists": False}
            problems.append(f"binary {name} is missing")
            continue
        info = binary_identity(path)
        info["name"] = name
        if newest_mtime and path.stat().st_mtime_ns < newest_mtime:
            info["predates_source"] = True
            problems.append(
                f"binary {name} is older than build input {newest_rel!r}; rebuild before measuring"
            )
        else:
            info["predates_source"] = False
        records[name] = info
    return {
        "binaries": records,
        "newest_build_input": newest_rel,
        "newest_build_input_mtime_ns": newest_mtime,
        "problems": problems,
    }
