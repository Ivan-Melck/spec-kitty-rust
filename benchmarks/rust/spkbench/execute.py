"""Subprocess execution with isolated user state and per-sample accounting."""

from __future__ import annotations

import os
import resource
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Observation:
    protocol: str
    argv: list[str]
    exit_code: int | None
    stdout: bytes
    stderr: bytes
    duration_s: float
    cpu_user_s: float
    cpu_sys_s: float
    timed_out: bool
    order: int | None = None
    warmup: bool = False
    load_before: dict | None = None
    load_after: dict | None = None

    @property
    def ok(self) -> bool:
        return self.exit_code == 0 and not self.timed_out

    def as_dict(self, *, max_text: int = 2000) -> dict:
        return {
            "protocol": self.protocol,
            "argv": self.argv,
            "exit_code": self.exit_code,
            "stdout_bytes": len(self.stdout),
            "stdout_sha256": __import__("hashlib").sha256(self.stdout).hexdigest(),
            "stderr_bytes": len(self.stderr),
            "stderr_head": self.stderr[:max_text].decode("utf-8", "replace"),
            "stdout_head": self.stdout[:max_text].decode("utf-8", "replace"),
            "duration_s": self.duration_s,
            "cpu_user_s": self.cpu_user_s,
            "cpu_sys_s": self.cpu_sys_s,
            "timed_out": self.timed_out,
            "order": self.order,
            "warmup": self.warmup,
            "load_before": self.load_before,
            "load_after": self.load_after,
        }


def build_env(run_dir: Path) -> dict[str, str]:
    """Environment shared by both protocols.

    User state is isolated (HOME/XDG/TMPDIR). Bytecode is written to a run-local
    cache so the repository's ``src`` tree and the installed tool environment are
    never modified. Hash randomisation is fixed for deterministic set ordering.
    """
    home = run_dir / "home"
    tmp = run_dir / "tmp"
    pycache = run_dir / "pycache"
    for path in (home, tmp, pycache):
        path.mkdir(parents=True, exist_ok=True)
    env = {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "XDG_STATE_HOME": str(home / ".local" / "state"),
        "TMPDIR": str(tmp),
        "TEMP": str(tmp),
        "TMP": str(tmp),
        "PYTHONHASHSEED": "0",
        "PYTHONPYCACHEPREFIX": str(pycache),
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONUNBUFFERED": "1",
        "PYTHONWARNINGS": "ignore",
        "SPEC_KITTY_NO_UPGRADE_CHECK": "1",
        "SPEC_KITTY_NO_NAG": "1",
        "SPEC_KITTY_NO_BANNER": "1",
        "SPEC_KITTY_SIMPLE_HELP": "1",
        "NO_COLOR": "1",
        "TERM": "dumb",
        "LC_ALL": "C.UTF-8",
        "LANG": "C.UTF-8",
        # Both protocols must use the same orchestrator-facing timezone.
        "TZ": "UTC",
    }
    if "SPEC_KITTY_PACKS_ROOT" in os.environ:
        env["SPEC_KITTY_PACKS_ROOT"] = os.environ["SPEC_KITTY_PACKS_ROOT"]
    if "PYTHONPATH" in os.environ:
        env["PYTHONPATH"] = os.environ["PYTHONPATH"]
    return env


class Executor:
    def __init__(self, *, run_dir: Path, cwd: Path | None = None, cpus: str | None = None):
        self.run_dir = run_dir
        self.cwd = cwd
        self.env = build_env(run_dir)
        self.cpus = cpus
        self._order = 0

    def _wrap(self, argv: list[str]) -> list[str]:
        if self.cpus:
            return ["taskset", "-c", self.cpus, *argv]
        return argv

    def run(
        self,
        *,
        protocol: str,
        argv: list[str],
        timeout_s: float = 180.0,
        warmup: bool = False,
        cwd: Path | None = None,
    ) -> Observation:
        self._order += 1
        command = self._wrap(argv)
        before = resource.getrusage(resource.RUSAGE_CHILDREN)
        start = time.perf_counter_ns()
        timed_out = False
        try:
            proc = subprocess.run(
                command,
                cwd=str(cwd or self.cwd),
                env=self.env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=timeout_s,
                check=False,
            )
            exit_code = proc.returncode
            stdout = proc.stdout
            stderr = proc.stderr
        except subprocess.TimeoutExpired as exc:
            timed_out = True
            exit_code = None
            stdout = exc.stdout or b""
            stderr = exc.stderr or b""
        duration = (time.perf_counter_ns() - start) / 1e9
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        return Observation(
            protocol=protocol,
            argv=argv,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            duration_s=duration,
            cpu_user_s=after.ru_utime - before.ru_utime,
            cpu_sys_s=after.ru_stime - before.ru_stime,
            timed_out=timed_out,
            order=self._order,
            warmup=warmup,
        )
