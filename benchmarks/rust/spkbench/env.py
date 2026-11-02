"""Machine, toolchain, and load capture.

Every report embeds this so a reader can judge whether a comparison was made on
a quiet machine or on a shared VM with competing work. Load is sampled before
and after the measurement window, never inferred.
"""

from __future__ import annotations

import os
import platform
import shutil
import socket
import subprocess
import sys
from pathlib import Path


def _read(path: str) -> str | None:
    try:
        return Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None


def _cpu_model() -> str | None:
    text = _read("/proc/cpuinfo") or ""
    for line in text.splitlines():
        if line.lower().startswith("model name"):
            return line.split(":", 1)[1].strip()
    return None


def _mem_total_bytes() -> int | None:
    text = _read("/proc/meminfo") or ""
    for line in text.splitlines():
        if line.startswith("MemTotal:"):
            return int(line.split()[1]) * 1024
    return None


def _os_release() -> dict[str, str]:
    text = _read("/etc/os-release") or ""
    out: dict[str, str] = {}
    for line in text.splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            out[key] = value.strip().strip('"')
    return out


def loadavg() -> dict:
    try:
        one, five, fifteen = os.getloadavg()
        return {"1m": one, "5m": five, "15m": fifteen}
    except OSError:
        return {}


def _find_executable(name: str) -> str | None:
    if os.path.isabs(name):
        return name if Path(name).exists() else None
    found = shutil.which(name)
    if found:
        return found
    # rustup installs outside the default non-login PATH on this VM.
    if name in {"rustc", "cargo", "rustup"}:
        candidate = Path.home() / ".cargo" / "bin" / name
        if candidate.exists():
            return str(candidate)
    return None


def _cmd_version(argv: list[str], *, cwd: Path | None = None, env: dict | None = None) -> dict:
    exe = _find_executable(argv[0])
    if not exe:
        return {"argv": argv, "available": False}
    try:
        proc = subprocess.run(
            [exe, *argv[1:]],
            cwd=str(cwd) if cwd else None,
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        return {
            "argv": argv,
            "available": True,
            "exit": proc.returncode,
            "stdout": proc.stdout.strip()[:2000],
            "stderr": proc.stderr.strip()[:2000],
        }
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"argv": argv, "available": False, "error": str(exc)}


def capture_static(
    *,
    repo_root: Path,
    rust_binary: Path,
    oracle_python: Path,
) -> dict:
    """Capture facts that do not change during a run."""
    rustc = _cmd_version(["rustc", "-Vv"])
    cargo = _cmd_version(["cargo", "-V"])
    oracle_version = _cmd_version([str(oracle_python), "-VV"])
    return {
        "captured_at_utc": __import__("datetime").datetime.now(
            __import__("datetime").timezone.utc
        ).isoformat(),
        "hostname": socket.gethostname(),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python_version": platform.python_version(),
            "python_implementation": platform.python_implementation(),
            "os_release": _os_release(),
            "cpu_model": _cpu_model(),
            "cpu_count": os.cpu_count(),
            "cpu_affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
            "mem_total_bytes": _mem_total_bytes(),
        },
        "toolchains": {
            "rustc": rustc,
            "cargo": cargo,
            "oracle_python": oracle_version,
            "harness_python": sys.version,
        },
        "paths": {
            "PATH": os.environ.get("PATH", ""),
            "repo_root": str(repo_root),
            "rust_binary": str(rust_binary),
            "oracle_python": str(oracle_python),
        },
    }


def capture_load(label: str) -> dict:
    return {"label": label, "loadavg": loadavg(), "at_utc": __import__("datetime").datetime.now(
        __import__("datetime").timezone.utc
    ).isoformat()}
