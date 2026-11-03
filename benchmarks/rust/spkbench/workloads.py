"""Workload registry: the frozen completion workloads plus diagnostics.

The 20 required workload ids are **not** defined here. They come from
``docs/rust/inventory/performance-registry.json`` (see :mod:`spkbench.registry`),
which mirrors the completion checker's hard-coded floor. That removes the
original harness's most dangerous property: its own list of 26 "required" ids
shared only four entries with the checker, so a harness PASS could never satisfy
completion and a filtered run could quietly drop the real ones.

Three equivalence classes exist and they are not interchangeable:

``strict``
    Both sides compute the same contract. Every compared field is defined by
    the pinned Python reference (or maps 1:1 from its typed objects). Only
    ``strict`` workloads count toward the 3x acceptance gate.

``projection``
    Upstream exposes the same facts but not the same document shape. Only the
    shared facts are compared. Diagnostic evidence, never gate evidence.

``contract``
    Startup-class checks: exit status, a non-vacuous stdout schema and (for the
    version workload) an identical declared version. Reported separately; never
    allowed to stand in for real work.

A frozen workload whose native behaviour does not exist is ``measurement:
"absent"`` and blocks the gate. It is never paired with an invented adapter, and
two identical refusals are not equivalence.

Diagnostics are the real strict pairs that exist today (pure runtime planner,
glossary lookup, compatibility reads). They are always ``required=False`` and
never gate-eligible; they keep the measurement core honest without pretending to
be completion workloads.
"""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from .fixtures import FixtureBuilder
from .registry import PerformanceRegistry, RegistryError, resolve_argv


@dataclass
class CommandSpec:
    protocol: str  # "rust" | "py-cli" | "py-adapter"
    argv: list[str]
    floor_modules: list[str] = field(default_factory=list)
    timeout_s: float = 180.0
    template: list[str] | None = None


@dataclass
class MutationSpec:
    """Execution contract for a workload that mutates its fixture.

    Mutation workloads cannot run through the read-only executor: every
    invocation needs a fresh clone of the pristine fixture, and equivalence is
    stdout/results **plus** post-state, not stdout alone.

    ``ignored_post_state`` holds only implementation-private artifacts recorded
    separately and excluded from the equality manifest (status lock files, whose
    names differ between implementations). It is not a general nondeterminism
    escape hatch: every ignored artifact is captured per invocation, the
    exclusion is declared in the frozen registry with a written ruling, and
    every user-facing byte -- including ``status.json`` -- is compared exactly.
    """

    placeholder: str = "{FIXTURE}"
    pristine: str | None = None
    ignored_post_state: tuple[str, ...] = ()
    timing_ready: bool = False

    def rewrite(self, argv: list[str], fixture: Path) -> list[str]:
        token = str(fixture)
        rewritten = [item.replace(self.placeholder, token) for item in argv]
        if self.pristine:
            rewritten = [item.replace(self.pristine, token) for item in rewritten]
        return rewritten


@dataclass
class Workload:
    id: str
    description: str
    fixture: str | None
    klass: str  # "work" | "micro" | "startup" | "pending"
    equivalence: str  # "strict" | "projection" | "contract"
    required: bool
    rust: CommandSpec | None = None
    oracle: CommandSpec | None = None
    compare: Callable[[dict, dict], tuple[bool, str]] | None = None
    normalize: Callable[[object], object] | None = None
    expected_exit: int | None = None
    repeats: int | None = None
    notes: list[str] = field(default_factory=list)
    pending_reason: str | None = None
    mutation: MutationSpec | None = None
    expectations: dict | None = None
    params: dict = field(default_factory=dict)
    measurement: str = "read"
    registry_entry: dict | None = None
    fixture_id: str | None = None
    gate_eligible_override: bool | None = None
    source: str = "diagnostic"
    runtime_blocker: str | None = None
    fixture_path: Path | None = None

    @property
    def runnable(self) -> bool:
        if self.measurement == "absent" or self.runtime_blocker:
            return False
        if self.mutation is not None and not self.mutation.timing_ready:
            return False
        return self.equivalence != "pending" and self.rust is not None and self.oracle is not None

    @property
    def blocked(self) -> bool:
        return self.measurement == "absent"

    def counts_for_gate(self) -> bool:
        if self.gate_eligible_override is not None:
            eligible = self.gate_eligible_override
        elif self.registry_entry is not None:
            eligible = bool(self.registry_entry.get("gate_eligible"))
        else:
            eligible = False
        return (
            eligible
            and self.runnable
            and self.required
            and self.klass == "work"
            and self.equivalence == "strict"
        )


# ---------------------------------------------------------------------------
# Canonicalisation / comparison helpers
# ---------------------------------------------------------------------------


def canonical(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def _walk_drop(value: object, path: tuple[str, ...]) -> object:
    if not path:
        return value
    if isinstance(value, dict):
        key = path[0]
        if key not in value:
            return value
        cloned = dict(value)
        if len(path) == 1:
            cloned.pop(key, None)
        else:
            cloned[key] = _walk_drop(cloned[key], path[1:])
        return cloned
    return value


def drop_paths(value: object, paths: list[str]) -> object:
    for path in paths:
        value = _walk_drop(value, tuple(path.split(".")))
    return value


def replace_strings(value: object, replacements: list[tuple[str, str]]) -> object:
    if isinstance(value, dict):
        return {key: replace_strings(item, replacements) for key, item in value.items()}
    if isinstance(value, list):
        return [replace_strings(item, replacements) for item in value]
    if isinstance(value, str):
        result = value
        for needle, token in replacements:
            if needle:
                result = result.replace(needle, token)
        return result
    return value


def first_difference(left: object, right: object, path: str = "$") -> str | None:
    if type(left) is not type(right):
        return f"{path}: type {type(left).__name__} != {type(right).__name__}"
    if isinstance(left, dict):
        left_keys, right_keys = set(left), set(right)
        if left_keys != right_keys:
            missing = sorted(left_keys - right_keys)
            extra = sorted(right_keys - left_keys)
            return f"{path}: key sets differ (only-left={missing[:5]}, only-right={extra[:5]})"
        for key in sorted(left):
            diff = first_difference(left[key], right[key], f"{path}.{key}")
            if diff:
                return diff
        return None
    if isinstance(left, list):
        if len(left) != len(right):
            return f"{path}: length {len(left)} != {len(right)}"
        for index, (l_item, r_item) in enumerate(zip(left, right)):
            diff = first_difference(l_item, r_item, f"{path}[{index}]")
            if diff:
                return diff
        return None
    if left != right:
        return f"{path}: {left!r} != {right!r}"
    return None


def compare_canonical(left: dict, right: dict) -> tuple[bool, str]:
    diff = first_difference(left, right)
    return (diff is None), (diff or "")


def sha_payload(value: object) -> str:
    return hashlib.sha256(canonical(value).encode()).hexdigest()


# ---------------------------------------------------------------------------
# Payload extraction
# ---------------------------------------------------------------------------


def rust_payload(stdout: bytes) -> dict:
    """Parse the native CLI payload into a comparable document.

    Three native surfaces exist:

    * the versioned inspection envelope (``{"ok": true, "data": ...}`` or
      ``{"ok": false, "error": ...}``);
    * the ``--json`` inspection document (``{"command": ..., "data"|"error":
      ...}``), which is the documented wire shape of the ``native`` tree;
    * the compatibility routes (``spec-kitty init|specify|agent ... --json``),
      which emit the bare upstream-shaped document with no envelope.

    A bare document is treated as a successful payload. Refusals carry
    ``error``/``error_code`` and are still compared field-for-field against the
    oracle, so an envelope shape is never silently normalised away: unwrapping
    selects the documented body, it does not delete fields from it.
    """
    try:
        document = json.loads(stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return {"_unparseable": str(exc), "_raw": stdout[:500].decode("utf-8", "replace")}
    if not isinstance(document, dict):
        return {"_unparseable": "envelope is not an object"}
    if "ok" in document:
        if document.get("ok") is True:
            return {"ok": True, "data": document.get("data")}
        return {"ok": False, "error": document.get("error"), "exit_code": document.get("exit_code")}
    if "command" in document and ("data" in document or "error" in document):
        # Versioned inspection document: the body is the compared payload, and a
        # refusal is still a refusal (never a successful empty document).
        return {
            "ok": "error" not in document,
            "command": document.get("command"),
            "data": document.get("data"),
            "error": document.get("error"),
            "exit_code": document.get("exit_code"),
        }
    # Compatibility route: the bare document is the payload.
    return {"ok": True, "data": document}


def oracle_payload(stdout: bytes) -> dict:
    try:
        return json.loads(stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return {"_unparseable": str(exc), "_raw": stdout[:500].decode("utf-8", "replace")}


def _runtime_decision(payload: dict) -> dict:
    """Rust runtime.query wraps the decision; the adapter emits it directly."""
    data = _unwrap_rust_data(payload)
    if isinstance(data, dict) and "decision" in data:
        return data["decision"]
    return data


def _unwrap_rust_data(payload: dict) -> dict:
    """Return the Rust success ``data`` payload; leave adapter payloads alone."""
    if isinstance(payload, dict) and payload.get("ok") is True:
        return payload.get("data") or {}
    return payload


def _project_mission_list(payload: dict) -> dict:
    data = payload.get("data") if payload.get("ok") is True else payload
    if not isinstance(data, dict) or "missions" not in data:
        return data
    rows = [
        {
            "slug": row.get("slug"),
            "mid8": row.get("mid8"),
            "friendly_name": row.get("friendly_name"),
        }
        for row in data.get("missions", [])
    ]
    return {"count": data.get("count"), "missions": rows}


def _project_mission_graph(payload: dict) -> dict:
    data = payload.get("data") if payload.get("ok") is True else payload
    if not isinstance(data, dict):
        return data
    graph = data.get("graph") if "graph" in data else data
    if not isinstance(graph, dict) or "packages" not in graph:
        return graph
    raw_packages = graph.get("packages")
    packages: dict[str, dict] = {}
    if isinstance(raw_packages, dict):
        for wp_id, package in raw_packages.items():
            packages[wp_id] = {
                "dependencies": sorted(package.get("dependencies") or []),
                "dependents": sorted(package.get("dependents") or []),
            }
    else:
        for package in raw_packages or []:
            packages[package["id"]] = {
                "dependencies": sorted(package.get("dependencies") or []),
                "dependents": sorted(package.get("dependents") or []),
            }
    return {
        "count": graph.get("work_package_count", graph.get("count", len(packages))),
        "packages": packages,
    }


def _transition_decision(payload: dict) -> dict:
    if payload.get("ok") is False:
        error = payload.get("error") or {}
        details = error.get("details") or {}
        return {
            "from": details.get("from"),
            "to": details.get("to"),
            "force": details.get("force"),
            "admitted": details.get("admitted"),
            "refusal": details.get("refusal") or error.get("message"),
        }
    data = payload.get("data") if payload.get("ok") is True else payload
    if not isinstance(data, dict):
        return data
    return {
        "from": data.get("from"),
        "to": data.get("to"),
        "force": data.get("force"),
        "admitted": data.get("admitted"),
        "refusal": data.get("refusal"),
    }


def _scalar_projection(payload: dict, keys: list[str]) -> dict:
    data = payload.get("data") if isinstance(payload, dict) and payload.get("ok") is True else payload
    if not isinstance(data, dict):
        return {"_unexpected": data}
    return {key: data.get(key) for key in keys}


# ---------------------------------------------------------------------------
# Registry
# ---------------------------------------------------------------------------


def _sha256(path: Path) -> str:
    if not path.exists():
        return "<fixture-not-materialized>"
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _native_compat_binary(rust_binary: Path) -> Path | None:
    """Locate the compatibility CLI (``spec-kitty``) next to the inspection CLI.

    The inspection binary (``spec-kitty-rust``) has no ``agent`` routes; the
    compatibility binary (``spec-kitty``) serves both the upstream-shaped routes
    and, under ``native``, the inspection tree. ``bench.py`` historically passes
    ``target/release/spec-kitty-rust``, so compat workloads resolve the sibling
    artifact explicitly instead of assuming the caller switched binaries.

    Returning ``None`` is meaningful: the caller must not substitute the
    inspection binary (which has no ``agent`` routes), and the workload guard
    below reports the missing artifact as a runtime blocker instead of running a
    command that cannot serve the route.
    """
    candidate = rust_binary if rust_binary.name == "spec-kitty" else rust_binary.with_name("spec-kitty")
    return candidate if candidate.exists() else None



# ---------------------------------------------------------------------------
# Named normalisers / comparators
#
# The frozen registry names the transformation instead of embedding code, so a
# reviewer can see exactly which fields are dropped and which are compared. An
# unknown name is a hard error, never a silent pass-through.
# ---------------------------------------------------------------------------


def _version_projection(payload: object) -> object:
    raw = payload.get("_raw") if isinstance(payload, dict) else None
    match = re.search(r"(\d+\.\d+\.\d+(?:[.-][0-9A-Za-z.]+)?)", str(raw or ""))
    return {"version": match.group(1) if match else None}


def _raw_stdout(payload: object) -> object:
    return payload


def _compat_unwrap(payload: object) -> object:
    return _unwrap_rust_data(payload)


NORMALIZERS: dict[str, Callable[[object], object]] = {
    "unwrap_rust_data": _compat_unwrap,
    "project_mission_list": lambda value: _project_mission_list(_compat_unwrap(value)),
    "project_mission_graph": lambda value: _project_mission_graph(_compat_unwrap(value)),
    "runtime_decision": _runtime_decision,
    "transition_decision": _transition_decision,
    "version_projection": _version_projection,
    "raw_stdout": _raw_stdout,
    "identity": lambda value: value,
}

COMPARATORS: dict[str, Callable[[dict, dict], tuple[bool, str]]] = {
    "exact": compare_canonical,
    "contract": lambda left, right: (True, ""),
}


def _normalizer(name: str) -> Callable[[object], object]:
    try:
        return NORMALIZERS[name]
    except KeyError:
        raise RegistryError(f"unknown normalizer {name!r}") from None


def _comparator(name: str) -> Callable[[dict, dict], tuple[bool, str]]:
    try:
        return COMPARATORS[name]
    except KeyError:
        raise RegistryError(f"unknown comparator {name!r}") from None


# ---------------------------------------------------------------------------
# Frozen-registry workloads
# ---------------------------------------------------------------------------


def _resolve_context(
    *,
    repo_root: Path,
    builder: FixtureBuilder,
    rust_binary: Path,
    oracle_python: Path,
    adapters_dir: Path,
) -> dict[str, str]:
    compat = _native_compat_binary(rust_binary)
    values = {
        "repo": str(repo_root),
        "oracle": str(oracle_python),
        "adapters": str(adapters_dir),
        "binary:spec-kitty-rust": str(rust_binary),
    }
    # Never substitute the inspection binary for the compatibility CLI: it has
    # no ``agent`` routes, so the pair would measure the wrong program. An empty
    # argv[0] is intentional and is rejected by `_registry_workload` below.
    values["binary:spec-kitty"] = str(compat) if compat else ""
    return values


def _registry_workload(
    entry: dict,
    *,
    context: dict[str, str],
    builder: FixtureBuilder,
    rust_binary: Path,
    oracle_python: Path,
    adapters_dir: Path,
) -> Workload:
    wid = entry["id"]
    measurement = entry.get("measurement") or "read"
    fixture = entry.get("fixture")
    params = dict(entry.get("params") or {})
    context = dict(context)
    if fixture:
        context["fixture"] = str(builder.root / fixture)
    context.update({key: str(value) for key, value in params.items()})

    if measurement == "absent":
        native = entry.get("native") or {}
        return Workload(
            id=wid,
            description=f"Frozen completion workload {wid}",
            fixture=fixture,
            klass=entry.get("category") or "work",
            equivalence=str(entry.get("equivalence") or "strict"),
            required=bool(entry.get("required")),
            expected_exit=None,
            notes=list(entry.get("notes") or []),
            pending_reason=native.get("blocked_by"),
            expectations=None,
            params=params,
            measurement="absent",
            registry_entry=entry,
            fixture_id=fixture,
            source="registry",
        )

    native = entry["native"]
    oracle = entry["oracle"]
    expected_exit = int((entry.get("expectations") or {}).get("exit", 0))
    rust_argv = resolve_argv(native["argv"], context)
    oracle_argv = resolve_argv(oracle["argv"], context)
    runtime_blocker = None
    if native.get("binary") == "spec-kitty" and not (
        rust_argv and rust_argv[0] and Path(rust_argv[0]).is_file()
    ):
        runtime_blocker = (
            f"{wid}: the compatibility CLI (target/release/spec-kitty) is required but was "
            "not found next to the inspection binary. Build both executables; the inspection "
            "binary is not a substitute."
        )
    mutation = None
    if measurement == "mutation":
        post = (entry.get("expectations") or {}).get("post_state") or {}
        mutation = MutationSpec(
            pristine=context.get("fixture"),
            ignored_post_state=tuple(post.get("ignored_prefixes") or ()),
            timing_ready=True,
        )
    normalize = _normalizer(entry["normalizer"]) if entry.get("normalizer") else None
    compare = _comparator(entry["comparator"]) if entry.get("comparator") else None
    if measurement == "startup" and entry.get("normalizer") == "version_projection":
        # Compare the declared version, not the framework-specific banner text.
        compare = compare or compare_canonical
    floor_modules = list(oracle.get("floor_modules") or [])
    return Workload(
        id=wid,
        description=f"Frozen completion workload {wid} ({measurement})",
        fixture=fixture,
        klass=entry.get("category") or "work",
        equivalence=str(entry.get("equivalence") or "strict"),
        required=bool(entry.get("required")),
        rust=CommandSpec("rust", rust_argv, template=list(native["argv"])),
        oracle=CommandSpec(
            oracle.get("kind") or "py-adapter",
            oracle_argv,
            floor_modules=floor_modules,
            template=list(oracle["argv"]),
        ),
        compare=compare,
        normalize=normalize,
        expected_exit=expected_exit,
        notes=list(entry.get("notes") or []),
        mutation=mutation,
        expectations=entry.get("expectations"),
        params=params,
        measurement=measurement,
        registry_entry=entry,
        fixture_id=fixture,
        source="registry",
        runtime_blocker=runtime_blocker,
    )


# ---------------------------------------------------------------------------
# Diagnostics (never required, never gate evidence)
# ---------------------------------------------------------------------------

RUNTIME_SCENARIOS = (
    ("step", "step.json", "mission.yaml", None, None, 0, "step"),
    ("blocked", "blocked.json", "mission.yaml", None, None, 3, "blocked"),
    ("terminal", "terminal.json", "mission.yaml", None, None, 0, "terminal"),
    ("pending-decision", "pending-decision.json", "mission.yaml", None, None, 0, "decision_required"),
    ("missing-input", "missing-input.json", "mission.yaml", None, None, 0, "decision_required"),
    ("audit-blocking", "audit-blocking.json", "mission_blocking_audit.yaml", None, None, 0, "step"),
    ("audit-advisory", "audit-advisory.json", "mission_advisory_audit.yaml", None, None, 0, "step"),
    ("unschedulable", "unschedulable.json", "mission.yaml", None, None, 3, "blocked"),
    ("drift", "drift.json", "mission.yaml", "mission_live_drifted.yaml", "<sha256-of-drifted-template>", 3, "blocked"),
)

GLOSSARY_CASES = (
    ("resolved", "bench-resolved", [], 1.0, False),
    ("ambiguous", "bench-ambiguous", [], 1.0, False),
    ("unknown", "bench-absent-term", [], 1.0, False),
    ("no-active-sense", "bench-noactive", [], 1.0, False),
    ("critical-unknown", "bench-absent-term", [], 0.4, True),
    ("scoped-team", "bench-ambiguous", ["team_domain"], 1.0, False),
)

TRANSITION_CASES = (
    ("planned-claimed", "planned", "claimed", ["--actor", "bench-agent", "--dependency-ready", "true"]),
    ("claimed-in_progress", "claimed", "in_progress", ["--actor", "bench-agent", "--workspace-context", "lane-1"]),
    ("for_review-in_review", "for_review", "in_review", ["--actor", "reviewer-1", "--review-ref", "feedback://r1"]),
    ("approved-done", "approved", "done", ["--actor", "merger", "--force", "--reason", "benchmark merge"]),
    ("planned-done-refused", "planned", "done", ["--actor", "bench"]),
)


def _decision_expectations(kind: str) -> dict:
    return {
        "exit": None,
        "payload": {
            "required_keys": ["kind"],
            "nonempty_keys": ["kind"],
            "required_patterns": {"kind": f"^{re.escape(kind)}$"},
        },
    }


def _diagnostic(
    *,
    wid: str,
    description: str,
    fixture: str | None,
    klass: str,
    equivalence: str,
    rust_argv: list[str] | None,
    oracle_argv: list[str] | None,
    normalize: Callable[[object], object],
    expected_exit: int | None,
    expectations: dict,
    notes: list[str],
    floor_modules: list[str] = (),
    protocol: str = "py-adapter",
    repeats: int | None = None,
    pending_reason: str | None = None,
) -> Workload:
    return Workload(
        id=wid,
        description=description,
        fixture=fixture,
        klass=klass,
        equivalence=equivalence,
        required=False,
        rust=CommandSpec("rust", rust_argv) if rust_argv else None,
        oracle=CommandSpec(protocol, oracle_argv, floor_modules=list(floor_modules))
        if oracle_argv
        else None,
        compare=compare_canonical,
        normalize=normalize,
        expected_exit=expected_exit,
        repeats=repeats,
        notes=notes,
        expectations=expectations,
        fixture_id=fixture,
        gate_eligible_override=False,
        source="diagnostic",
        pending_reason=pending_reason,
    )


def _blocked_diagnostic(wid: str, description: str, reason: str) -> Workload:
    return Workload(
        id=wid,
        description=description,
        fixture=None,
        klass="pending",
        equivalence="pending",
        required=False,
        pending_reason=reason,
        measurement="absent",
        gate_eligible_override=False,
        source="diagnostic",
    )


def _diagnostic_workloads(
    *,
    builder: FixtureBuilder,
    rust_binary: Path,
    oracle_python: Path,
    adapters_dir: Path,
    include_micro: bool,
) -> list[Workload]:
    runtime = builder.get("runtime-large")
    project = builder.get("project-large")
    compat = builder.get("compat-project")
    compat_slug = "002-bench-mission-002"
    compat_binary = _native_compat_binary(rust_binary)
    compat_missing = compat_binary is None
    compat_bin = compat_binary or rust_binary
    path_tokens = [
        (str(runtime.path), "<FIXTURE>"),
        (str(project.path), "<FIXTURE>"),
        (str(compat.path), "<FIXTURE>"),
    ]

    def normalize(value: object) -> object:
        return replace_strings(_compat_unwrap(value), path_tokens)

    workloads: list[Workload] = []

    for name, state_name, template_name, live_name, live_hash, exit_code, kind in RUNTIME_SCENARIOS:
        state = runtime.path / "snapshots" / state_name
        template = runtime.path / template_name
        rust_argv = [
            str(rust_binary), "--json", "runtime", "query",
            "--template", str(template), "--state", str(state),
        ]
        oracle_argv = [
            str(oracle_python), str(adapters_dir / "py_runtime_query.py"),
            "--template", str(template), "--state", str(state),
        ]
        if live_name:
            actual_hash = _sha256(runtime.path / live_name)
            rust_argv.extend(["--current-template-hash", actual_hash])
            oracle_argv.extend(["--live-template", str(runtime.path / live_name)])
        workloads.append(
            _diagnostic(
                wid=f"runtime.query.{name}",
                description=f"Pure planner decision for scenario '{name}' over a 900-step template",
                fixture="runtime-large",
                klass="work",
                equivalence="strict",
                rust_argv=rust_argv,
                oracle_argv=oracle_argv,
                normalize=lambda value: normalize(_runtime_decision(value)),
                expected_exit=exit_code,
                expectations=_decision_expectations(kind),
                floor_modules=["runtime.next._internal_runtime.planner"],
                notes=[
                    "DIAGNOSTIC: a real strict pair, but runtime.query is not a frozen completion workload.",
                    "Oracle calls runtime.next._internal_runtime.planner.plan_next (pinned source).",
                ],
            )
        )

    for name, term, scopes, confidence, critical in GLOSSARY_CASES:
        rust_argv = [
            str(rust_binary), "--json", "--project-root", str(project.path),
            "glossary", "lookup", term, "--confidence", str(confidence),
        ]
        oracle_argv = [
            str(oracle_python), str(adapters_dir / "py_glossary_lookup.py"),
            "--repo", str(project.path), "--term", term, "--confidence", str(confidence),
        ]
        for scope in scopes:
            rust_argv.extend(["--scope", scope])
            oracle_argv.extend(["--scope", scope])
        if critical:
            rust_argv.append("--critical-step")
            oracle_argv.append("--critical-step")
        workloads.append(
            _diagnostic(
                wid=f"glossary.lookup.{name}",
                description=f"Resolve '{term}' across 4 scopes with 4 * 1500 + 5 seed senses",
                fixture="project-large",
                klass="work",
                equivalence="strict",
                rust_argv=rust_argv,
                oracle_argv=oracle_argv,
                normalize=lambda value: drop_paths(
                    normalize(value), ["read_only", "result.status", "status"]
                ),
                expected_exit=0,
                expectations={
                    "exit": 0,
                    "payload": {
                        "required_keys": ["requested_term", "confidence", "result", "conflict"],
                        "equals": {"requested_term": term, "confidence": confidence},
                        "nonempty_keys": ["requested_term"],
                        "row_keys": {"result.matches": ["surface", "scope", "status", "confidence"]},
                    },
                },
                floor_modules=["glossary.scope", "glossary.resolution", "glossary.conflict"],
                notes=[
                    "DIAGNOSTIC: lookup only; frozen glossary.activation requires replay/extraction/middleware.",
                    "result.status is a Rust aggregation of upstream active-sense counts and is ignored.",
                ],
            )
        )

    workloads.append(
        _diagnostic(
            wid="mission.list.projection",
            description="List 800 Missions; compare slug/mid8/friendly_name only",
            fixture="project-large",
            klass="work",
            equivalence="projection",
            rust_argv=[str(rust_binary), "--json", "--project-root", str(project.path), "mission", "list"],
            oracle_argv=[str(oracle_python), str(adapters_dir / "py_mission_list.py"), "--repo", str(project.path)],
            normalize=lambda value: _project_mission_list(normalize(value)),
            expected_exit=0,
            expectations={
                "exit": 0,
                "payload": {
                    "required_keys": ["count", "missions"],
                    "min_items": {"missions": 800},
                    "row_keys": {"missions": ["slug", "mid8", "friendly_name"]},
                },
            },
            floor_modules=["specify_cli.context.mission_resolver"],
            notes=["DIAGNOSTIC projection; not eligible for the 3x gate."],
        )
    )

    slug = "001-bench-mission-001"
    workloads.append(
        _diagnostic(
            wid="mission.graph.projection",
            description="Dependency edges and direct dependents for one 4-WP Mission",
            fixture="project-large",
            klass="work",
            equivalence="projection",
            rust_argv=[str(rust_binary), "--json", "--project-root", str(project.path), "mission", "graph", slug],
            oracle_argv=[
                str(oracle_python), str(adapters_dir / "py_mission_graph.py"),
                "--repo", str(project.path), "--slug", slug,
            ],
            normalize=lambda value: _project_mission_graph(normalize(value)),
            expected_exit=0,
            expectations={
                "exit": 0,
                "payload": {
                    "required_keys": ["count", "packages"],
                    "min_items": {"packages": 4},
                    "nonnull_keys": ["count"],
                },
            },
            floor_modules=["specify_cli.core.dependency_graph"],
            notes=[
                "DIAGNOSTIC projection: dependency facts only; topological order is not published upstream.",
            ],
        )
    )

    def normalize_compat(value: object) -> object:
        return replace_strings(_compat_unwrap(value), path_tokens)

    def project_dependents(value: object) -> dict:
        data = _compat_unwrap(value)
        if not isinstance(data, dict):
            return {"_unexpected": data}
        return {"wp_id": data.get("wp_id"), "dependents": sorted(data.get("dependents") or [])}

    def project_branch_context(value: object) -> dict:
        data = _compat_unwrap(value)
        if not isinstance(data, dict):
            return {"_unexpected": data}
        projected = drop_paths(
            data,
            ["NOW_UTC_ISO", "runtime_vars.now_utc_iso", "repo_root", "spec_kitty_version"],
        )
        return replace_strings(projected, path_tokens)

    missing_reason = (
        "The compatibility binary (target/release/spec-kitty) is absent; the inspection "
        "binary spec-kitty-rust has no `agent` routes and is not used as a fallback."
    )

    workloads.append(
        _diagnostic(
            wid="tasks.list",
            description=f"List six WPs in a mission with a coherent lane history ({compat_slug})",
            fixture="compat-project",
            klass="work",
            equivalence="strict" if not compat_missing else "pending",
            rust_argv=[str(compat_bin), "agent", "tasks", "list-tasks", "--mission", compat_slug, "--json"],
            oracle_argv=[
                str(oracle_python), str(adapters_dir / "py_tasks_list.py"),
                "--repo", str(compat.path), "--mission", compat_slug,
            ],
            normalize=normalize_compat,
            expected_exit=0,
            expectations={
                "exit": 0,
                "payload": {
                    "required_keys": ["tasks", "count"],
                    "equals": {"count": 6},
                    "min_items": {"tasks": 6},
                    "row_keys": {"tasks": ["work_package_id", "title", "lane", "path"]},
                },
            },
            floor_modules=["specify_cli.cli.commands.agent.tasks"],
            pending_reason=missing_reason if compat_missing else None,
            notes=["DIAGNOSTIC: a real strict compatibility pair, not a frozen completion workload."],
        )
    )
    workloads.append(
        _diagnostic(
            wid="tasks.list_dependents.projection",
            description="Direct dependents of WP02 in the compat fixture",
            fixture="compat-project",
            klass="work",
            equivalence="projection" if not compat_missing else "pending",
            rust_argv=[
                str(compat_bin), "agent", "tasks", "list-dependents", "WP02",
                "--mission", compat_slug, "--json",
            ],
            oracle_argv=[
                str(oracle_python), str(adapters_dir / "py_tasks_list_dependents.py"),
                "--repo", str(compat.path), "--mission", compat_slug, "--wp-id", "WP02",
            ],
            normalize=lambda value: project_dependents(value),
            expected_exit=0,
            expectations={
                "exit": 0,
                "payload": {
                    "required_keys": ["wp_id", "dependents"],
                    "equals": {"wp_id": "WP02"},
                    "min_items": {"dependents": 1},
                },
            },
            floor_modules=["specify_cli.cli.commands.agent.tasks"],
            pending_reason=missing_reason if compat_missing else None,
            notes=["DIAGNOSTIC projection of the dependency graph fact."],
        )
    )
    workloads.append(
        _diagnostic(
            wid="mission.branch_context.projection",
            description="Deterministic branch contract on the compat fixture's unborn main branch",
            fixture="compat-project",
            klass="micro",
            equivalence="projection" if not compat_missing else "pending",
            rust_argv=[str(compat_bin), "agent", "mission", "branch-context", "--json"],
            oracle_argv=[
                str(oracle_python), str(adapters_dir / "py_branch_context.py"),
                "--repo", str(compat.path),
            ],
            normalize=project_branch_context,
            expected_exit=0,
            expectations={
                "exit": 0,
                "payload": {
                    "nonempty_keys": ["target_branch"],
                    "required_keys": ["target_branch"],
                },
            },
            floor_modules=["specify_cli.cli.commands.agent.mission_branch_context"],
            repeats=3,
            pending_reason=missing_reason if compat_missing else None,
            notes=["DIAGNOSTIC micro/projection; launch-dominated and excluded."],
        )
    )

    if include_micro:
        for name, src, dst, extra in TRANSITION_CASES:
            workloads.append(
                _diagnostic(
                    wid=f"transition.check.{name}",
                    description="Pure lane FSM decision (one invocation; launch-dominated)",
                    fixture=None,
                    klass="micro",
                    equivalence="projection",
                    rust_argv=[str(rust_binary), "--json", "transition", "check", src, dst, *extra],
                    oracle_argv=[
                        str(oracle_python), str(adapters_dir / "py_transition_check.py"),
                        src, dst, *extra,
                    ],
                    normalize=lambda value: _transition_decision(value),
                    expected_exit=None,
                    expectations={
                        "exit": None,
                        "payload": {
                            "required_keys": ["from", "to", "admitted"],
                            "equals": {"from": src, "to": dst},
                        },
                    },
                    floor_modules=["specify_cli.status.wp_state"],
                    repeats=3,
                    notes=["DIAGNOSTIC micro: process launch dominates; excluded from the gate."],
                )
            )

    for wid, reason in (
        ("status.validate", "Native and upstream report different finding families (clock skew/evidence warnings vs schema/FSM edges)."),
        ("tasks.list.target-branch-guard", "Native has no detached-HEAD/target-branch guard, so the refusal contract is unimplemented."),
        ("tasks.status", "Native emits extra contract fields and derives agent/workspace kinds; result shapes differ."),
        ("lifecycle.plan", "Native skips the substantive-spec gate and planning commit."),
        ("lifecycle.init", "Native writes 14 files vs upstream's 100+ asset set."),
        ("mission.create", "Native skips charter activation and mission_number allocation."),
        ("tasks.finalize", "Different refusal contracts; requirement mapping and lanes refresh absent."),
        ("lane.implement", "Different refusal contracts; allocation, workspace persistence and auto-commit absent."),
        ("events.inspect", "Typed native projection has no upstream typed inspector."),
    ):
        workloads.append(_blocked_diagnostic(wid, f"Diagnostic gap: {wid}", reason))

    return workloads


# ---------------------------------------------------------------------------
# Public builder
# ---------------------------------------------------------------------------


def build_workloads(
    *,
    builder: FixtureBuilder,
    rust_binary: Path,
    oracle_python: Path,
    adapters_dir: Path,
    registry: PerformanceRegistry,
    repo_root: Path | None = None,
    include_micro: bool = True,
) -> list[Workload]:
    """Frozen completion workloads first, then non-gating diagnostics."""
    repo = Path(repo_root) if repo_root is not None else Path(__file__).resolve().parents[3]
    context = _resolve_context(
        repo_root=repo,
        builder=builder,
        rust_binary=rust_binary,
        oracle_python=oracle_python,
        adapters_dir=adapters_dir,
    )
    workloads: list[Workload] = []
    for entry in registry.workloads:
        workloads.append(
            _registry_workload(
                entry,
                context=context,
                builder=builder,
                rust_binary=rust_binary,
                oracle_python=oracle_python,
                adapters_dir=adapters_dir,
            )
        )
    workloads.extend(
        _diagnostic_workloads(
            builder=builder,
            rust_binary=rust_binary,
            oracle_python=oracle_python,
            adapters_dir=adapters_dir,
            include_micro=include_micro,
        )
    )
    ids = [workload.id for workload in workloads]
    if len(set(ids)) != len(ids):
        duplicates = sorted({wid for wid in ids if ids.count(wid) > 1})
        raise RegistryError(f"duplicate workload ids: {duplicates}")
    return workloads


def gate_workloads(workloads: list[Workload]) -> list[Workload]:
    return [workload for workload in workloads if workload.counts_for_gate()]


def registry_workloads(workloads: list[Workload]) -> list[Workload]:
    return [workload for workload in workloads if workload.source == "registry"]


def workload_command_summary(workload: Workload, *, fixture: str | None = None) -> dict:
    """Portable command identity recorded per workload in the evidence bundle."""
    def render(spec: CommandSpec | None) -> list[str] | None:
        if spec is None:
            return None
        if spec.template:
            return list(spec.template)
        return [item.replace(str(fixture), "{FIXTURE}") if fixture else item for item in spec.argv]

    return {"rust": render(workload.rust), "python": render(workload.oracle)}
