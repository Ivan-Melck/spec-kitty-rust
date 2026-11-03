"""Derive certified facts from the evidence files a bundle points at.

The gate must not read its own claims back. Everything the verdict depends on is
re-derived here from stored bytes:

* the artifact hash table, the bundle hash in ``manifest.json`` and the payload
  file hashes;
* the normalized payload of every side of every measured workload, whose
  canonical sha256 must equal the recorded ``payload_sha256`` and whose content
  must satisfy the workload's declared payload/stdout schema;
* the mutation post-state deltas, likewise content-addressed;
* every raw sample row in ``samples.jsonl`` (duration, pair index, exit code,
  timeout flag, payload and post-state hashes).

Equivalence, sample statistics, confidence intervals and regression verdicts are
computed from these derived objects only; :mod:`spkbench.certify` compares the
bundle's recorded projections against them and refuses on any difference.

A payload file may be pretty-printed JSON: the *declared* hash is defined here
as ``sha256(canonical_json(parsed_payload))``, never the serialized bytes, so a
reformatted file is legitimate while edited content is not.
"""

from __future__ import annotations

import base64
import hashlib
import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .registry import canonical_json

REQUIRED_ARTIFACTS = ("report", "samples", "identity", "environment", "fixtures", "summary")
MANIFEST_SCHEMA = "spkbench.bundle-manifest.v1"
POST_STATE_SCHEMA = "spkbench.post-state.v1"


class EvidenceError(RuntimeError):
    """Stored evidence is missing, unreadable or structurally impossible to judge."""


@dataclass
class StoredFile:
    name: str
    path: str
    exists: bool
    declared_sha256: str | None
    actual_sha256: str | None
    problems: list[str] = field(default_factory=list)


def sha256_bytes(data: bytes) -> str:
    # File-integrity hashing, not a charter semantic hash: TID251's own
    # justification for integrity checks applies.
    return hashlib.sha256(data).hexdigest()  # noqa: TID251


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()  # noqa: TID251 - file-integrity check
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_sha256(value: Any) -> str:
    """The hash a normalized payload declares: sha256 of its canonical JSON."""
    return sha256_bytes(canonical_json(value).encode("utf-8"))


def _side(name: str) -> str:
    return "python" if name in {"oracle", "python"} else "rust"


@dataclass
class Evidence:
    root: Path
    files: dict[str, StoredFile] = field(default_factory=dict)
    manifest: dict | None = None
    samples: dict[tuple[str, str], list[dict]] = field(default_factory=dict)
    sample_rows: list[dict] = field(default_factory=list)
    payloads: dict[tuple[str, str], Any] = field(default_factory=dict)
    post_states: dict[tuple[str, str], Any] = field(default_factory=dict)
    post_state_envelopes: dict[tuple[str, str], dict] = field(default_factory=dict)
    post_state_files: dict[tuple[str, str], dict[str, bytes]] = field(default_factory=dict)
    payload_problems: dict[tuple[str, str], list[str]] = field(default_factory=dict)
    post_state_problems: dict[tuple[str, str], list[str]] = field(default_factory=dict)
    problems: list[str] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    def payload_problem(self, workload_id: str, side: str) -> list[str]:
        return list(self.payload_problems.get((workload_id, side), []))


def _load_json(path: Path, label: str, evidence: Evidence) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, UnicodeDecodeError) as exc:
        evidence.problems.append(f"{label} is not readable JSON: {exc}")
        return None


def _check_declared_file(
    evidence: Evidence,
    *,
    name: str,
    role: str,
    declared: dict,
) -> StoredFile | None:
    declared_path = declared.get("path")
    declared_sha = declared.get("sha256")
    if not isinstance(declared_path, str) or not declared_path:
        evidence.problems.append(f"{role} {name} has no stored path")
        return None
    path = Path(declared_path)
    path = path if path.is_absolute() else evidence.root / path
    stored = StoredFile(
        name=name,
        path=str(path),
        exists=path.is_file(),
        declared_sha256=declared_sha if isinstance(declared_sha, str) else None,
        actual_sha256=None,
    )
    evidence.files[name] = stored
    if not stored.exists:
        stored.problems.append(f"{role} {name} is missing at {path}")
        evidence.problems.append(f"{role} {name} is missing at {path}")
        return stored
    try:
        stored.actual_sha256 = sha256_file(path)
    except OSError as exc:
        stored.problems.append(f"{role} {name} is unreadable: {exc}")
        evidence.problems.append(f"{role} {name} is unreadable: {exc}")
        return stored
    if not stored.declared_sha256:
        stored.problems.append(f"{role} {name} declares no sha256")
        evidence.problems.append(f"{role} {name} declares no sha256")
    elif stored.declared_sha256 != stored.actual_sha256:
        stored.problems.append(f"{role} {name} sha256 does not match the file on disk")
        evidence.failures.append(f"{role} {name} sha256 does not match the file on disk")
    return stored


def _load_payloads(
    bundle: dict, evidence: Evidence, *, required_ids: set[str] | None = None
) -> None:
    records = bundle.get("payload_artifacts") or {}
    if not isinstance(records, dict):
        evidence.problems.append("payload_artifacts is not an object")
        return
    for key in sorted(records):
        record = records[key]
        if not isinstance(record, dict):
            evidence.problems.append(f"payload artifact {key} is not an object")
            continue
        if "." not in key:
            evidence.problems.append(f"payload artifact key {key!r} is not '<workload>.<side>'")
            continue
        workload_id, side = key.rsplit(".", 1)
        side = _side(side)
        required = required_ids is None or workload_id in required_ids
        if record.get("stored") is not True:
            message = (
                f"normalized payload for {workload_id} ({side}) was not stored; "
                "the schema and equivalence claim cannot be re-derived"
            )
            if required:
                evidence.problems.append(message)
            else:
                evidence.warnings.append(message + " (non-required diagnostic)")
            continue
        stored = _check_declared_file(
            evidence, name=f"payload:{key}", role="payload artifact", declared=record
        )
        if stored is None or not stored.exists:
            continue
        before = len(evidence.problems)
        payload = _load_json(Path(stored.path), f"payload {key}", evidence)
        if len(evidence.problems) != before:
            continue
        try:
            declared_canonical = record.get("canonical_sha256")
            computed = canonical_sha256(payload)
            if declared_canonical:
                if declared_canonical != computed:
                    evidence.failures.append(
                        f"payload {key} canonical content hash does not match its declared hash "
                        "(reformatting is allowed, edited content is not)"
                    )
            elif record.get("sha256") != computed:
                evidence.failures.append(
                    f"payload {key} declares no canonical_sha256 and its file hash is not the "
                    "canonical content hash"
                )
        except (TypeError, ValueError) as exc:
            evidence.problems.append(f"payload {key} cannot be canonicalised: {exc}")
            continue
        evidence.payloads[(workload_id, side)] = payload


def _load_post_state_files(
    envelope: dict, key: str, evidence: Evidence
) -> dict[str, bytes]:
    """Decode the actual bytes stored for each required post-state path."""
    files = envelope.get("files")
    if not isinstance(files, dict):
        evidence.problems.append(f"post-state {key} stores no file table")
        return {}
    decoded: dict[str, bytes] = {}
    for rel, record in sorted(files.items()):
        if not isinstance(rel, str) or not isinstance(record, dict):
            evidence.problems.append(f"post-state {key} file entry {rel!r} is malformed")
            continue
        encoded = record.get("content_b64")
        if not isinstance(encoded, str):
            evidence.problems.append(f"post-state {key} file {rel!r} stores no content")
            continue
        try:
            data = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError) as exc:
            evidence.failures.append(f"post-state {key} file {rel!r} is not valid base64: {exc}")
            continue
        if record.get("bytes") is not None and record.get("bytes") != len(data):
            evidence.failures.append(
                f"post-state {key} file {rel!r} byte length does not match its declared size"
            )
            continue
        computed = sha256_bytes(data)
        if record.get("sha256") != computed:
            evidence.failures.append(
                f"post-state {key} file {rel!r} content hash does not match its stored bytes"
            )
            continue
        canonical = record.get("canonical_sha256")
        if canonical:
            try:
                parsed = json.loads(data.decode("utf-8"))
            except (UnicodeDecodeError, ValueError):
                evidence.failures.append(
                    f"post-state {key} file {rel!r} declares a canonical JSON hash but is not JSON"
                )
                continue
            if canonical != canonical_sha256(parsed):
                evidence.failures.append(
                    f"post-state {key} file {rel!r} canonical hash does not match its content"
                )
                continue
        decoded[rel] = data
    return decoded


def _load_post_states(bundle: dict, evidence: Evidence) -> None:
    records = bundle.get("post_state_artifacts") or {}
    if not isinstance(records, dict):
        evidence.problems.append("post_state_artifacts is not an object")
        return
    for key in sorted(records):
        record = records[key]
        if not isinstance(record, dict):
            evidence.problems.append(f"post-state artifact {key} is not an object")
            continue
        if "." not in key:
            evidence.problems.append(f"post-state artifact key {key!r} is not '<workload>.<side>'")
            continue
        workload_id, side = key.rsplit(".", 1)
        side = _side(side)
        if record.get("stored") is not True:
            evidence.problems.append(
                f"post-state artifact {key} was not stored; the registry's post-state contract "
                "cannot be re-derived"
            )
            continue
        stored = _check_declared_file(
            evidence, name=f"post_state:{key}", role="post-state artifact", declared=record
        )
        if stored is None or not stored.exists:
            continue
        document = _load_json(Path(stored.path), f"post-state {key}", evidence)
        if not isinstance(document, dict):
            evidence.problems.append(f"post-state {key} is not a post-state document")
            continue
        try:
            computed = canonical_sha256(document)
        except (TypeError, ValueError) as exc:
            evidence.problems.append(f"post-state {key} cannot be canonicalised: {exc}")
            continue
        declared_canonical = record.get("canonical_sha256")
        if declared_canonical:
            if declared_canonical != computed:
                evidence.failures.append(
                    f"post-state {key} canonical content hash does not match its declared hash"
                )
                continue
        elif record.get("sha256") != computed:
            evidence.failures.append(
                f"post-state {key} declares no canonical_sha256 and its file hash is not the "
                "canonical content hash"
            )
            continue
        if document.get("schema") != POST_STATE_SCHEMA:
            evidence.problems.append(
                f"post-state {key} is not a {POST_STATE_SCHEMA} envelope"
            )
            continue
        delta = document.get("delta")
        if not isinstance(delta, dict):
            evidence.problems.append(f"post-state {key} carries no delta object")
            continue
        content_roots = document.get("content_roots", [])
        if not isinstance(content_roots, list) or any(
            not isinstance(item, str) or not item for item in content_roots
        ):
            evidence.problems.append(
                f"post-state {key} content_roots is not a list of location roots"
            )
            continue
        evidence.post_states[(workload_id, side)] = delta
        evidence.post_state_envelopes[(workload_id, side)] = document
        evidence.post_state_files[(workload_id, side)] = _load_post_state_files(
            document, key, evidence
        )


def _load_fixtures(bundle: dict, evidence: Evidence) -> None:
    stored = evidence.files.get("fixtures")
    if stored is None or not stored.exists:
        return
    fixtures = _load_json(Path(stored.path), "fixtures.json", evidence)
    if not isinstance(fixtures, dict):
        evidence.problems.append("fixtures.json is not an object")
    elif fixtures != bundle.get("fixtures"):
        evidence.failures.append("fixtures.json content does not match the bundle fixture table")


def _load_samples(evidence: Evidence) -> None:
    stored = evidence.files.get("samples")
    if stored is None or not stored.exists:
        evidence.problems.append("raw samples artifact is missing; the gate cannot be recomputed")
        return
    rows: list[dict] = []
    try:
        text = Path(stored.path).read_text(encoding="utf-8")
    except OSError as exc:
        evidence.problems.append(f"raw samples artifact is unreadable: {exc}")
        return
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            item = json.loads(line)
        except ValueError as exc:
            evidence.problems.append(f"samples.jsonl line {number} is not JSON: {exc}")
            continue
        if not isinstance(item, dict):
            evidence.problems.append(f"samples.jsonl line {number} is not an object")
            continue
        workload_id = item.get("workload")
        if not isinstance(workload_id, str) or not workload_id:
            evidence.problems.append(f"samples.jsonl line {number} has no workload id")
            continue
        side = _side(str(item.get("side")))
        duration = item.get("duration_s")
        if isinstance(duration, bool) or not isinstance(duration, (int, float)):
            evidence.problems.append(
                f"samples.jsonl line {number} ({workload_id}/{side}) has no numeric duration_s"
            )
            continue
        rows.append(item)
        evidence.samples.setdefault((workload_id, side), []).append(item)
    evidence.sample_rows = rows


def _verify_manifest(bundle: dict, evidence: Evidence, *, required: bool) -> None:
    manifested = evidence.files.get("manifest")
    path = Path(manifested.path) if manifested is not None else evidence.root / "manifest.json"
    if not path.is_file():
        if required:
            evidence.problems.append(
                "manifest.json is missing; the artifact table cannot be cross-checked"
            )
        else:
            evidence.warnings.append("manifest.json is absent (provisional in-run verification)")
        return
    manifest = _load_json(path, "manifest.json", evidence)
    if not isinstance(manifest, dict):
        evidence.problems.append("manifest.json is not an object")
        return
    evidence.manifest = manifest
    if manifest.get("schema") != MANIFEST_SCHEMA:
        evidence.problems.append(f"manifest.json schema is {manifest.get('schema')!r}")
    declared_artifacts = manifest.get("artifacts")
    bundle_artifacts = bundle.get("artifacts")
    if not isinstance(declared_artifacts, dict) or not isinstance(bundle_artifacts, dict):
        evidence.problems.append("manifest.json has no artifact table to compare")
    else:
        normalised_manifest = {
            name: {"path": str((record or {}).get("path")), "sha256": (record or {}).get("sha256")}
            for name, record in declared_artifacts.items()
            if isinstance(record, dict)
        }
        normalised_bundle = {
            name: {"path": str((record or {}).get("path")), "sha256": (record or {}).get("sha256")}
            for name, record in bundle_artifacts.items()
            if isinstance(record, dict)
        }
        if normalised_manifest != normalised_bundle:
            evidence.failures.append(
                "manifest.json artifact table does not match the bundle artifact table"
            )
    bundle_record = manifest.get("bundle") or {}
    on_disk = evidence.root / "performance-evidence.json"
    if on_disk.is_file():
        if bundle_record.get("sha256") != sha256_file(on_disk):
            evidence.failures.append(
                "manifest.json does not point at the bundle file it was written with"
            )
    elif bundle_record.get("sha256") and bundle_record.get("sha256") != bundle.get("self_sha256"):
        evidence.warnings.append(
            "manifest bundle hash was compared with the in-memory self hash only"
        )
    manifest_payloads = manifest.get("payloads")
    if isinstance(manifest_payloads, dict) and isinstance(bundle.get("payload_artifacts"), dict):
        def _payload_view(table: dict) -> dict:
            return {
                name: {
                    "sha256": (record or {}).get("sha256"),
                    "canonical_sha256": (record or {}).get("canonical_sha256"),
                    "path": (record or {}).get("path"),
                    "stored": (record or {}).get("stored"),
                }
                for name, record in table.items()
                if isinstance(record, dict)
            }

        if _payload_view(manifest_payloads) != _payload_view(bundle["payload_artifacts"]):
            evidence.failures.append(
                "manifest.json payload table does not match the bundle payload table"
            )
    manifest_post = manifest.get("post_state")
    if (
        isinstance(manifest_post, dict)
        and isinstance(bundle.get("post_state_artifacts"), dict)
        and manifest_post != bundle.get("post_state_artifacts")
    ):
        evidence.failures.append(
            "manifest.json post-state table does not match the bundle post-state table"
        )


def _require_artifact_coverage(bundle: dict, evidence: Evidence) -> None:
    declared = bundle.get("artifacts")
    if not isinstance(declared, dict) or not declared:
        evidence.problems.append("bundle records no artifact hash table")
        return
    for name in REQUIRED_ARTIFACTS:
        if name not in declared:
            evidence.problems.append(f"bundle artifact table omits {name}")


def load_evidence(
    bundle: dict,
    root: Path,
    *,
    require_manifest: bool = True,
    require_artifact_coverage: bool = True,
    required_ids: set[str] | None = None,
) -> Evidence:
    """Load every stored artifact the verdict needs, verifying hash bindings.

    ``require_artifact_coverage`` is false only for the in-write provisional
    pass, before the report/summary artifacts that record the verdict exist.
    """
    if not isinstance(bundle, dict):
        raise EvidenceError("evidence bundle is not an object")
    root = Path(root)
    if not root.is_dir():
        raise EvidenceError(f"artifact root {root} is not a directory")
    evidence = Evidence(root=root)
    artifacts = bundle.get("artifacts") or {}
    if not isinstance(artifacts, dict):
        raise EvidenceError("bundle artifacts table is not an object")
    for name, record in artifacts.items():
        if not isinstance(record, dict):
            raise EvidenceError(f"artifact {name!r} is not an object")
        _check_declared_file(evidence, name=name, role="artifact", declared=record)
    if require_artifact_coverage:
        _require_artifact_coverage(bundle, evidence)
    _load_fixtures(bundle, evidence)
    _load_samples(evidence)
    _load_payloads(bundle, evidence, required_ids=required_ids)
    _load_post_states(bundle, evidence)
    _verify_manifest(bundle, evidence, required=require_manifest)
    return evidence
