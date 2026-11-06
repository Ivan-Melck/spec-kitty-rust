"""Build receipts: bind a measured binary to the source and toolchain that built it.

A sha256 of an executable proves *what* ran, never *what it was built from*. The
review of the first evidence bundle found a recorded binary that no longer
existed and a repo HEAD that contradicted the identity file, because nothing tied
the two together.

A receipt is written by ``bench.py build`` immediately around an explicit
``cargo build --release --locked``. It records the clean source commit, the
digest of every tracked Rust build input before and after the build, the
toolchain, the command, and the resulting binary hashes. The certification gate
requires a receipt and re-verifies it against the live tree, so an unattested or
stale binary blocks instead of being measured.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

SCHEMA = "spkbench.build-receipt.v1"


def canonical_json(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def create_receipt(
    *,
    repo: Path,
    binaries: dict[str, Path],
    command: list[str],
    source: dict,
    build_inputs: dict,
    toolchain: dict,
    started_at: str,
    finished_at: str,
    target: str | None = None,
) -> dict:
    if source.get("dirty"):
        raise RuntimeError("refusing a build receipt for a dirty source tree")
    records = {}
    for name, path in binaries.items():
        if not path.exists():
            raise RuntimeError(f"binary {name} does not exist: {path}")
        records[name] = {
            "path": str(path),
            "sha256": _sha256(path),
            "size": path.stat().st_size,
            "mtime_ns": path.stat().st_mtime_ns,
        }
    return {
        "schema": SCHEMA,
        "repo": str(repo),
        "source": {
            "head": source.get("head"),
            "dirty": False,
            "build_inputs_digest": build_inputs.get("digest"),
            "build_inputs_file_count": build_inputs.get("file_count"),
            "cargo_lock_sha256": _sha256(repo / "Cargo.lock") if (repo / "Cargo.lock").exists() else None,
            "toolchain_file_sha256": toolchain.get("file_sha256"),
        },
        "toolchain": {
            "release": toolchain.get("release"),
            "host": toolchain.get("host"),
            "rustc_verbose": (toolchain.get("rustc") or {}).get("stdout"),
            "cargo_version": (toolchain.get("cargo") or {}).get("stdout"),
            "release_profile": toolchain.get("release_profile"),
        },
        "build": {
            "command": command,
            "target": target,
            "started_at": started_at,
            "finished_at": finished_at,
        },
        "binaries": records,
    }


def receipt_digest(receipt: dict) -> str:
    return hashlib.sha256(canonical_json(receipt).encode()).hexdigest()


def write_receipt(path: Path, receipt: dict) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path


def load_receipt(path: Path) -> dict:
    try:
        receipt = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return {"schema": None, "load_error": str(exc)}
    if not isinstance(receipt, dict):
        return {"schema": None, "load_error": "receipt is not an object"}
    receipt["digest"] = receipt_digest(receipt)
    return receipt


def verify_receipt(
    receipt: dict,
    *,
    repo: Path,
    binaries: dict[str, Path],
    build_inputs: dict,
    toolchain: dict,
) -> dict:
    problems: list[str] = []
    if receipt.get("schema") != SCHEMA:
        problems.append(f"unexpected build receipt schema {receipt.get('schema')!r}")
    if receipt.get("load_error"):
        problems.append(f"build receipt is unreadable: {receipt['load_error']}")
    if receipt.get("repo") and Path(str(receipt["repo"])).resolve() != repo.resolve():
        problems.append("build receipt was written for a different repository path")
    source = receipt.get("source") or {}
    if source.get("dirty") is not False:
        problems.append("build receipt records a dirty source tree")
    if source.get("build_inputs_digest") != build_inputs.get("digest"):
        problems.append("build receipt build-input digest does not match the live tree")
    recorded_toolchain = receipt.get("toolchain") or {}
    live_rustc = (toolchain.get("rustc") or {}).get("stdout")
    if recorded_toolchain.get("rustc_verbose") != live_rustc:
        problems.append("build receipt rustc differs from the live toolchain")
    if recorded_toolchain.get("release") != toolchain.get("release"):
        problems.append("build receipt rustc release differs from the live toolchain")
    command = " ".join(str(item) for item in (receipt.get("build") or {}).get("command") or [])
    for required in ("build", "--release", "--locked"):
        if required not in command:
            problems.append(f"build receipt command does not contain {required!r}")
    finished = (receipt.get("build") or {}).get("finished_at")
    for name, path in binaries.items():
        record = (receipt.get("binaries") or {}).get(name)
        if not record:
            problems.append(f"build receipt has no hash for {name}")
            continue
        if not path.exists():
            problems.append(f"binary {name} is missing")
            continue
        if record.get("sha256") != _sha256(path):
            problems.append(f"build receipt hash does not match binary {name}")
        if finished and path.stat().st_mtime_ns < int(record.get("mtime_ns") or 0):
            # The file is *older* than the build that supposedly produced it,
            # e.g. a replacement restored with an older timestamp. The sha256
            # check above is the real control; this is a provenance diagnostic.
            problems.append(
                f"binary {name} has an mtime older than the recorded build; it may have been "
                "replaced with a file that does not come from that build (sha256 is authoritative)"
            )
    return {
        "ok": not problems,
        "problems": problems,
        "digest": receipt.get("digest"),
        "head": source.get("head"),
        "build_inputs_digest": source.get("build_inputs_digest"),
        "finished_at": finished,
    }
