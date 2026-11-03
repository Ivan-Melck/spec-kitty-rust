"""The frozen performance registry shared by the harness and the completion checker.

``docs/rust/inventory/performance-registry.json`` is the single source of truth
for *which* workloads the performance gate must measure. It deliberately mirrors
the hard-coded floors in ``scripts/rust/check_completion.py`` and
``scripts/rust/inventory_model.py``:

* the 20 required workload ids,
* the 17 strict gate ids (required strict workloads minus the startup pair and
  the non-strict ``cli.mission.list`` projection),
* the 20-samples-per-side / 14-strict-workloads / 3.0x / 10%-regression floor.

The harness must not invent its own ids. A workload that is not in this registry
can only be a diagnostic; a required id that is absent from a run is a blocker,
never something a filtered run can drop.

The file is self-digesting: ``digest`` is the sha256 of the canonical JSON of
itself minus that field, so editing a floor or dropping a workload is visible in
every bundle that cites the registry.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any, Iterable

REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_REGISTRY = REPO_ROOT / "docs/rust/inventory/performance-registry.json"
PLACEHOLDER_KEYS = {"repo", "fixture", "oracle", "adapters", "mission_slug"}


class RegistryError(RuntimeError):
    """The frozen registry is missing, malformed, edited, or internally inconsistent."""


def canonical_json(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def registry_digest(registry: dict) -> str:
    payload = {key: value for key, value in registry.items() if key != "digest"}
    return hashlib.sha256(canonical_json(payload).encode()).hexdigest()


class PerformanceRegistry:
    def __init__(self, path: Path, data: dict, *, file_sha256: str):
        self.path = path
        self.data = data
        self.file_sha256 = file_sha256

    # -- construction ---------------------------------------------------
    @classmethod
    def load(cls, path: Path | None = None) -> "PerformanceRegistry":
        target = Path(path) if path is not None else DEFAULT_REGISTRY
        if not target.exists():
            raise RegistryError(f"frozen performance registry is missing: {target}")
        raw = target.read_bytes()
        try:
            data = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise RegistryError(f"frozen performance registry is not valid JSON: {exc}") from None
        if not isinstance(data, dict):
            raise RegistryError("frozen performance registry must be an object")
        registry = cls(target, data, file_sha256=hashlib.sha256(raw).hexdigest())
        problems = registry.validate()
        if problems:
            raise RegistryError("frozen performance registry is invalid: " + "; ".join(problems))
        return registry

    # -- accessors ------------------------------------------------------
    @property
    def schema(self) -> str:
        return str(self.data.get("schema"))

    @property
    def declared_digest(self) -> str:
        return str(self.data.get("digest") or "")

    @property
    def policy(self) -> dict:
        return dict(self.data.get("policy") or {})

    @property
    def checker(self) -> dict:
        return dict(self.data.get("checker") or {})

    @property
    def workloads(self) -> list[dict]:
        return list(self.data.get("workloads") or [])

    @property
    def diagnostics(self) -> list[dict]:
        return list(self.data.get("diagnostics") or [])

    @property
    def required_ids(self) -> list[str]:
        return [item["id"] for item in self.workloads if item.get("required")]

    @property
    def gate_ids(self) -> list[str]:
        return [item["id"] for item in self.workloads if item.get("gate_eligible")]

    @property
    def startup_ids(self) -> list[str]:
        return [item["id"] for item in self.workloads if item.get("category") == "startup"]

    @property
    def diagnostic_ids(self) -> list[str]:
        return [item["id"] for item in self.diagnostics]

    def workload(self, workload_id: str) -> dict:
        for item in self.workloads:
            if item["id"] == workload_id:
                return item
        raise RegistryError(f"workload {workload_id!r} is not in the frozen registry")

    def has(self, workload_id: str) -> bool:
        return any(item["id"] == workload_id for item in self.workloads)

    def is_diagnostic(self, workload_id: str) -> bool:
        return any(item["id"] == workload_id for item in self.diagnostics)

    def declared_measurement(self, workload_id: str) -> str:
        return str(self.workload(workload_id).get("measurement"))

    def identity(self) -> dict:
        return {
            "path": self._relative_path(),
            "sha256": self.file_sha256,
            "digest": self.declared_digest,
            "schema": self.schema,
            "registry_version": self.data.get("registry_version"),
            "workload_ids": self.required_ids,
            "gate_workload_ids": self.gate_ids,
            "startup_workload_ids": self.startup_ids,
        }

    def _relative_path(self) -> str:
        try:
            return self.path.resolve().relative_to(REPO_ROOT).as_posix()
        except ValueError:
            return str(self.path)

    # -- validation -----------------------------------------------------
    def validate(self) -> list[str]:
        problems: list[str] = []
        if self.data.get("schema") != "skit.rust.performance-registry.v1":
            problems.append(f"unexpected schema {self.data.get('schema')!r}")
        if self.data.get("frozen") is not True:
            problems.append("registry is not frozen")
        if not self.data.get("digest"):
            problems.append("registry has no digest")
        elif self.declared_digest != "sha256:" + registry_digest(self.data):
            problems.append("registry digest does not match its contents")
        if not self.data.get("frozen_baseline", {}).get("commit"):
            problems.append("registry has no frozen baseline commit")

        checker = self.checker
        ids = [item.get("id") for item in self.workloads]
        if len(set(ids)) != len(ids):
            problems.append("duplicate workload ids")
        if len(ids) != 20:
            problems.append(f"expected the 20 frozen workloads, found {len(ids)}")
        if sorted(checker.get("required_workload_ids") or []) != sorted(ids):
            problems.append("checker.required_workload_ids does not match the workload list")
        gate_ids = [item.get("id") for item in self.workloads if item.get("gate_eligible")]
        if sorted(checker.get("gate_workload_ids") or []) != sorted(gate_ids):
            problems.append("checker.gate_workload_ids does not match gate_eligible rows")
        if sorted(checker.get("startup_workload_ids") or []) != sorted(self.startup_ids):
            problems.append("checker.startup_workload_ids does not match startup rows")
        if set(self.startup_ids) & set(gate_ids):
            problems.append("a startup workload is gate-eligible")
        if "cli.mission.list" in gate_ids:
            problems.append("cli.mission.list is a projection and must not be gate-eligible")
        if len(gate_ids) != 17:
            problems.append(f"expected 17 strict gate workloads, found {len(gate_ids)}")

        policy = self.policy
        for key, expected in (
            ("minimum_speedup", 3.0),
            ("max_per_workload_regression_fraction", 0.10),
            ("minimum_samples_per_side", 20),
            ("minimum_strict_workloads", 14),
        ):
            if policy.get(key) != expected:
                problems.append(f"policy.{key} must be {expected!r}, found {policy.get(key)!r}")
        for key in (
            "require_result_equivalence",
            "require_raw_samples",
            "require_interleaved_repetitions",
            "require_machine_and_load_context",
            "require_toolchain_identity",
            "require_fixture_identity",
            "require_portable_fixtures",
            "require_post_state_equivalence",
            "require_clean_source",
            "require_build_receipt",
            "require_recomputation",
            "require_work_excess_geomean",
        ):
            if policy.get(key) is not True:
                problems.append(f"policy.{key} must be true")

        from . import expectations as expectmod

        for item in self.workloads:
            wid = item.get("id")
            measurement = item.get("measurement")
            if measurement not in {"startup", "read", "projection", "mutation", "absent"}:
                problems.append(f"{wid}: unknown measurement {measurement!r}")
                continue
            if item.get("gate_eligible") and not item.get("strict_equivalence"):
                problems.append(f"{wid}: gate-eligible but not strict-equivalence")
            if item.get("gate_eligible") and item.get("equivalence") != "strict":
                problems.append(f"{wid}: gate-eligible but equivalence is not 'strict'")
            if measurement == "absent":
                native = item.get("native") or {}
                if native.get("state") != "absent":
                    problems.append(f"{wid}: absent workload must declare native.state=absent")
                if native.get("argv"):
                    problems.append(f"{wid}: absent workload must not declare a native argv")
                if not native.get("blocked_by"):
                    problems.append(f"{wid}: absent workload must state blocked_by")
                if item.get("oracle") is not None:
                    problems.append(f"{wid}: absent workload must not declare an oracle")
                if item.get("expectations") is not None:
                    problems.append(f"{wid}: absent workload must not declare expectations")
            else:
                native = item.get("native") or {}
                if native.get("state") != "implemented":
                    problems.append(f"{wid}: runnable workload must declare native.state=implemented")
                if not native.get("argv"):
                    problems.append(f"{wid}: runnable workload has no native argv")
                if native.get("binary") not in {"spec-kitty", "spec-kitty-rust"}:
                    problems.append(f"{wid}: unknown native binary {native.get('binary')!r}")
                if not item.get("oracle") or not item["oracle"].get("argv"):
                    problems.append(f"{wid}: runnable workload has no oracle argv")
                if not item.get("fixture") and measurement != "startup":
                    problems.append(f"{wid}: non-startup workload needs a fixture recipe")
                if measurement == "mutation":
                    post = (item.get("expectations") or {}).get("post_state")
                    if not post:
                        problems.append(f"{wid}: mutation workload needs a post_state schema")
                    elif not post.get("required_paths"):
                        problems.append(f"{wid}: mutation post_state declares no required paths")
                    elif not post.get("exclusion_ruling"):
                        problems.append(f"{wid}: mutation post_state needs an exclusion ruling")
                ok, why = expectmod.non_vacuous(item.get("expectations") or {})
                if not ok:
                    problems.append(f"{wid}: expectations are vacuous ({why})")
                for key in ("normalizer", "comparator"):
                    if not item.get(key):
                        problems.append(f"{wid}: missing {key}")
            for template in _strings(item.get("native", {}).get("argv") or []) + _strings(
                (item.get("oracle") or {}).get("argv") or []
            ):
                problem = validate_placeholder(template)
                if problem:
                    problems.append(f"{wid}: {problem}")

        required = set(ids)
        for diagnostic in self.diagnostics:
            did = diagnostic.get("id")
            if not did:
                problems.append("diagnostic entry without an id")
            elif did in required:
                problems.append(f"diagnostic {did} collides with a frozen required workload")
            if diagnostic.get("equivalence") not in {"strict", "projection", "contract"}:
                problems.append(f"diagnostic {did}: unknown equivalence")
        return problems


def _strings(values: Iterable[Any]) -> list[str]:
    return [value for value in values if isinstance(value, str)]


def validate_placeholder(text: str) -> str | None:
    """Reject unknown ``{placeholders}`` in an argv template."""
    import re

    for match in re.finditer(r"\{([^{}]+)\}", text):
        token = match.group(1)
        if token in PLACEHOLDER_KEYS:
            continue
        if token.startswith("binary:") and token.split(":", 1)[1]:
            continue
        return f"unknown argv placeholder {{{token}}}"
    return None


def resolve_argv(template: Iterable[str], values: dict[str, str]) -> list[str]:
    """Resolve an argv template with the run's concrete paths."""
    resolved: list[str] = []
    for item in template:
        text = item
        for key, value in values.items():
            text = text.replace("{" + key + "}", value)
        if text.startswith("{binary:") and text.endswith("}"):
            name = text[len("{binary:") : -1]
            resolved.append(values.get(f"binary:{name}", text))
            continue
        resolved.append(text)
    for item in resolved:
        problem = validate_placeholder(item)
        if problem:
            raise RegistryError(problem)
    return resolved
