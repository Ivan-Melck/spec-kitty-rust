"""Workload registry: what is paired, what is only a projection, and what is
still pending.

Three equivalence classes exist and they are not interchangeable:

``strict``
    Both sides compute the same contract. Every compared field is defined by
    the pinned Python reference (or maps 1:1 from its typed objects). Only
    ``strict`` workloads count toward the 3x acceptance gate.

``projection``
    Upstream has no command with the Rust document's full shape, but it does
    expose the same underlying facts. The projection compares only those facts.
    It is diagnostic evidence, never gate evidence.

``contract``
    Startup-class checks: exit status and non-empty output only. Reported
    separately; explicitly not allowed to stand in for real work.

Anything else is ``pending`` and blocks the overall performance pass.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from .fixtures import FixtureBuilder


@dataclass
class CommandSpec:
    protocol: str  # "rust" | "py-cli" | "py-adapter"
    argv: list[str]
    floor_modules: list[str] = field(default_factory=list)
    timeout_s: float = 180.0


@dataclass
class Workload:
    id: str
    description: str
    fixture: str | None
    klass: str  # "work" | "micro" | "startup" | "pending"
    equivalence: str  # "strict" | "projection" | "contract" | "pending"
    required: bool
    rust: CommandSpec | None = None
    oracle: CommandSpec | None = None
    compare: Callable[[dict, dict], tuple[bool, str]] | None = None
    normalize: Callable[[object], object] | None = None
    expected_exit: int | None = None
    repeats: int | None = None
    notes: list[str] = field(default_factory=list)
    pending_reason: str | None = None

    @property
    def runnable(self) -> bool:
        return self.equivalence != "pending" and self.rust is not None and self.oracle is not None

    def counts_for_gate(self) -> bool:
        return self.runnable and self.required and self.klass == "work" and self.equivalence == "strict"


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
    """Parse the Rust envelope into a comparable payload.

    A success compares its ``data``; a refusal compares ``error`` (kind,
    message, details). Exit code is compared separately.
    """
    try:
        document = json.loads(stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return {"_unparseable": str(exc), "_raw": stdout[:500].decode("utf-8", "replace")}
    if not isinstance(document, dict):
        return {"_unparseable": "envelope is not an object"}
    if document.get("ok") is True:
        return {"ok": True, "data": document.get("data")}
    return {"ok": False, "error": document.get("error"), "exit_code": document.get("exit_code")}


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
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build_workloads(
    *,
    builder: FixtureBuilder,
    rust_binary: Path,
    oracle_python: Path,
    adapters_dir: Path,
    include_micro: bool = True,
) -> list[Workload]:
    workloads: list[Workload] = []
    runtime = builder.get("runtime-large")
    project = builder.get("project-large")

    path_tokens = [
        (str(runtime.path), "<FIXTURE>"),
        (str(project.path), "<FIXTURE>"),
    ]

    def normalize(value: object) -> object:
        return replace_strings(_unwrap_rust_data(value), path_tokens)

    def normalize_runtime(value: object) -> object:
        return replace_strings(_runtime_decision(value), path_tokens)

    # ---------------- runtime pure planner (strict) ----------------
    scenarios = [
        ("step", "step.json", "mission.yaml", None, None, 0),
        ("blocked", "blocked.json", "mission.yaml", None, None, 3),
        ("terminal", "terminal.json", "mission.yaml", None, None, 0),
        ("pending-decision", "pending-decision.json", "mission.yaml", None, None, 0),
        ("missing-input", "missing-input.json", "mission.yaml", None, None, 0),
        ("audit-blocking", "audit-blocking.json", "mission_blocking_audit.yaml", None, None, 0),
        ("audit-advisory", "audit-advisory.json", "mission_advisory_audit.yaml", None, None, 0),
        ("unschedulable", "unschedulable.json", "mission.yaml", None, None, 3),
        (
            "drift",
            "drift.json",
            "mission.yaml",
            "mission_live_drifted.yaml",
            _sha256(runtime.path / "mission_live_drifted.yaml")
            if (runtime.path / "mission_live_drifted.yaml").exists()
            else "<fixture-not-materialized>",
            3,
        ),
    ]
    for name, state_name, template_name, live_name, live_hash, exit_code in scenarios:
        state = runtime.path / "snapshots" / state_name
        template = runtime.path / template_name
        rust_argv = [
            str(rust_binary),
            "--json",
            "runtime",
            "query",
            "--template",
            str(template),
            "--state",
            str(state),
        ]
        oracle_argv = [
            str(oracle_python),
            str(adapters_dir / "py_runtime_query.py"),
            "--template",
            str(template),
            "--state",
            str(state),
        ]
        if live_name:
            rust_argv.extend(["--current-template-hash", live_hash])
            oracle_argv.extend(["--live-template", str(runtime.path / live_name)])
        workloads.append(
            Workload(
                id=f"runtime.query.{name}",
                description=f"Pure planner decision for scenario '{name}' over a 900-step template",
                fixture="runtime-large",
                klass="work",
                equivalence="strict",
                required=True,
                rust=CommandSpec("rust", rust_argv),
                oracle=CommandSpec(
                    "py-adapter",
                    oracle_argv,
                    floor_modules=["runtime.next._internal_runtime.planner"],
                ),
                compare=compare_canonical,
                normalize=normalize_runtime,
                expected_exit=exit_code,
                notes=[
                    "Rust plan_next over an explicit frozen template + RunSnapshot.",
                    "Oracle calls runtime.next._internal_runtime.planner.plan_next (pinned source).",
                ],
            )
        )

    # ---------------- glossary lookup (strict, derived status ignored) ----------------
    glossary_cases = [
        ("resolved", "bench-resolved", [], 1.0, False),
        ("ambiguous", "bench-ambiguous", [], 1.0, False),
        ("unknown", "bench-absent-term", [], 1.0, False),
        ("no-active-sense", "bench-noactive", [], 1.0, False),
        ("critical-unknown", "bench-absent-term", [], 0.4, True),
        ("scoped-team", "bench-ambiguous", ["team_domain"], 1.0, False),
    ]
    for name, term, scopes, confidence, critical in glossary_cases:
        rust_argv = [
            str(rust_binary),
            "--json",
            "--project-root",
            str(project.path),
            "glossary",
            "lookup",
            term,
            "--confidence",
            str(confidence),
        ]
        oracle_argv = [
            str(oracle_python),
            str(adapters_dir / "py_glossary_lookup.py"),
            "--repo",
            str(project.path),
            "--term",
            term,
            "--confidence",
            str(confidence),
        ]
        for scope in scopes:
            rust_argv.extend(["--scope", scope])
            oracle_argv.extend(["--scope", scope])
        if critical:
            rust_argv.append("--critical-step")
            oracle_argv.append("--critical-step")
        workloads.append(
            Workload(
                id=f"glossary.lookup.{name}",
                description=f"Resolve '{term}' across 4 scopes with {4 * 1500 + 5} seed senses",
                fixture="project-large",
                klass="work",
                equivalence="strict",
                required=True,
                rust=CommandSpec("rust", rust_argv),
                oracle=CommandSpec(
                    "py-adapter",
                    oracle_argv,
                    floor_modules=["glossary.scope", "glossary.resolution", "glossary.conflict"],
                ),
                compare=compare_canonical,
                normalize=lambda value, _n=normalize: drop_paths(
                    _n(value), ["read_only", "result.status", "status"]
                ),
                expected_exit=0,
                notes=[
                    "Compared fields are upstream-defined matches + conflict classification.",
                    "result.status is a Rust aggregation of upstream active-sense counts and is ignored.",
                ],
            )
        )

    # ---------------- mission listing (projection, diagnostic) ----------------
    workloads.append(
        Workload(
            id="mission.list.projection",
            description="List 800 Missions; compare slug/mid8/friendly_name only",
            fixture="project-large",
            klass="work",
            equivalence="projection",
            required=False,
            rust=CommandSpec("rust", [str(rust_binary), "--json", "--project-root", str(project.path), "mission", "list"]),
            oracle=CommandSpec(
                "py-adapter",
                [str(oracle_python), str(adapters_dir / "py_mission_list.py"), "--repo", str(project.path)],
                floor_modules=["specify_cli.context.mission_resolver"],
            ),
            compare=compare_canonical,
            normalize=lambda value: _project_mission_list(normalize(value)),
            expected_exit=0,
            notes=[
                "PROJECTION: upstream has no full inventory document; only 3 identity fields are compared.",
                "Not eligible for the 3x gate.",
            ],
        )
    )

    # ---------------- mission dependency graph (projection, diagnostic) ----------------
    slug = "001-bench-mission-001"
    workloads.append(
        Workload(
            id="mission.graph.projection",
            description="Dependency edges and direct dependents for one 4-WP Mission",
            fixture="project-large",
            klass="work",
            equivalence="projection",
            required=False,
            rust=CommandSpec(
                "rust",
                [str(rust_binary), "--json", "--project-root", str(project.path), "mission", "graph", slug],
            ),
            oracle=CommandSpec(
                "py-adapter",
                [
                    str(oracle_python),
                    str(adapters_dir / "py_mission_graph.py"),
                    "--repo",
                    str(project.path),
                    "--slug",
                    slug,
                ],
                floor_modules=["specify_cli.core.dependency_graph"],
            ),
            compare=compare_canonical,
            normalize=lambda value: _project_mission_graph(normalize(value)),
            expected_exit=0,
            notes=[
                "PROJECTION: upstream dependency_graph.build_dependency_graph + get_dependents.",
                "Topological order is not compared (different tie-breaks; no published upstream order).",
                "Not eligible for the 3x gate.",
            ],
        )
    )

    # ---------------- lane FSM (micro, launch-dominated) ----------------
    if include_micro:
        transition_cases = [
            ("planned-claimed", "planned", "claimed", ["--actor", "bench-agent", "--dependency-ready", "true"]),
            ("claimed-in_progress", "claimed", "in_progress", ["--actor", "bench-agent", "--workspace-context", "lane-1"]),
            ("for_review-in_review", "for_review", "in_review", ["--actor", "reviewer-1", "--review-ref", "feedback://r1"]),
            ("approved-done", "approved", "done", ["--actor", "merger", "--force", "--reason", "benchmark merge"]),
            ("planned-done-refused", "planned", "done", ["--actor", "bench"]),
        ]
        for name, src, dst, extra in transition_cases:
            workloads.append(
                Workload(
                    id=f"transition.check.{name}",
                    description="Pure lane FSM decision (one invocation; launch-dominated)",
                    fixture=None,
                    klass="micro",
                    equivalence="projection",
                    required=False,
                    repeats=3,
                    rust=CommandSpec(
                        "rust",
                        [str(rust_binary), "--json", "transition", "check", src, dst, *extra],
                    ),
                    oracle=CommandSpec(
                        "py-adapter",
                        [
                            str(oracle_python),
                            str(adapters_dir / "py_transition_check.py"),
                            src,
                            dst,
                            *extra,
                        ],
                        floor_modules=["specify_cli.status.wp_state"],
                    ),
                    compare=compare_canonical,
                    normalize=lambda value: _transition_decision(normalize(value)),
                    expected_exit=None,
                    notes=[
                        "MICRO: single FSM check; process launch dominates. Excluded from the gate.",
                        "Upstream authority: wp_state_for(...).check_transition(...).",
                    ],
                )
            )

    # ---------------- startup (contract, separate) ----------------
    workloads.extend(
        [
            Workload(
                id="startup.version",
                description="Process startup: version query",
                fixture=None,
                klass="startup",
                equivalence="contract",
                required=False,
                rust=CommandSpec("rust", [str(rust_binary), "--version"]),
                oracle=CommandSpec("py-cli", [str(oracle_python), "-m", "specify_cli", "--version"]),
                compare=lambda left, right: (True, ""),
                normalize=lambda value: _scalar_projection(value, ["_raw"]),
                expected_exit=0,
                notes=["Contract-only: exit status + non-empty output. Not gate evidence."],
            ),
            Workload(
                id="startup.help",
                description="Process startup: help rendering",
                fixture=None,
                klass="startup",
                equivalence="contract",
                required=False,
                rust=CommandSpec("rust", [str(rust_binary), "--help"]),
                oracle=CommandSpec("py-cli", [str(oracle_python), "-m", "specify_cli", "--help"]),
                compare=lambda left, right: (True, ""),
                normalize=lambda value: _scalar_projection(value, ["_raw"]),
                expected_exit=0,
                notes=["Contract-only: exit status + non-empty output. Not gate evidence."],
            ),
        ]
    )

    # ---------------- pending required workloads ----------------
    workloads.extend(
        [
            Workload(
                id="mission.inventory.full",
                description="Full Mission inventory document (identity, number, type, topology, meta)",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "No pinned upstream command emits the Rust inventory document. "
                    "specify_cli.context.mission_resolver.list_missions_for_selection exposes only "
                    "slug/mid8/friendly_name; comparing the full document would require reimplementing "
                    "the Rust projection in the oracle. Tracked as mission.list.projection (diagnostic)."
                ),
            ),
            Workload(
                id="mission.show",
                description="Per-mission WP summary (titles, dependencies, source paths)",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "No pinned upstream command emits the same per-mission WP summary. "
                    "agent tasks list-tasks/status are lane-and-state views; using them would "
                    "compare different work. The dependency edge subset is covered diagnostically "
                    "by mission.graph.projection."
                ),
            ),
            Workload(
                id="project.root",
                description="Project-root discovery",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Upstream locates the project root internally (locate_project_root) but exposes "
                    "no CLI contract for it. A comparison would be startup-class discovery, not a "
                    "representative workload; it must not be used as gate evidence."
                ),
            ),
            Workload(
                id="git.inspect",
                description="Read-only Git repository inspection",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Upstream has Git helpers but no read-only inspection command with an equivalent "
                    "document. ops log / safe-commit are different contracts. Needs a shared "
                    "inspection contract before timing is meaningful."
                ),
            ),
            Workload(
                id="capabilities",
                description="Rust capability/refusal matrix",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=False,
                pending_reason=(
                    "Rust-specific machine surface with no upstream counterpart. It is Rust's own "
                    "migration disclosure, not rewritten upstream behavior, so it is not an "
                    "equivalence workload."
                ),
            ),
            Workload(
                id="mission.dependency_planning.full",
                description="Mission dependency + readiness + lane planning",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Upstream planning surfaces (orchestrator-api mission-state, agent tasks status) "
                    "reconstruct lanes/readiness; Rust mission graph is explicitly read-only and does not "
                    "evaluate state. Outputs are not equivalent. Edge-only projection is diagnostic."
                ),
            ),
            Workload(
                id="status.replay",
                description="Canonical status replay / snapshot + archive parity",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Rust sk-status has no canonical reducer. events inspect states it is not a reducer. "
                    "Upstream specify_cli.status.reducer.reduce / agent status materialize are the oracle, "
                    "but no native counterpart exists yet."
                ),
            ),
            Workload(
                id="runtime.next.governed",
                description="Governed next (persistent run, cursor, workspace, prompt, gates)",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Rust runtime query is a pure planner over an explicit snapshot. Upstream next reads "
                    "and writes a run store, resolves owned workspaces, emits prompt files, and applies "
                    "gates; it is a different contract and must be implemented before it can be paired."
                ),
            ),
            Workload(
                id="events.inspect",
                description="Typed event-stream inspection",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Rust events inspect is a Rust-specific typed projection (its category partition and "
                    "torn-tail semantics have no upstream typed inspector). Python events tail is a line "
                    "stream, not an equivalent contract. Must not be benchmarked as if equivalent."
                ),
            ),
            Workload(
                id="governance.charter",
                description="Charter / pack / asset governance inspection and activation",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "No pinned upstream CLI emits the Rust charter.inspect document. Upstream charter "
                    "status/context/validate are different contracts; activation/pack acquisition are "
                    "mutating and unimplemented in Rust."
                ),
            ),
            Workload(
                id="glossary.activation",
                description="Glossary event replay, extraction, middleware, activation",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Rust sk-glossary implements seed validation + lookup only. Upstream glossary event "
                    "replay/extraction/middleware/activation mutations have no native counterpart."
                ),
            ),
            Workload(
                id="lifecycle.mutations",
                description="End-to-end init/create/implement/review/accept/consolidate/merge/recovery",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Rust CLI is read-only: no project init, mission creation, lane/worktree ownership, "
                    "event writes, acceptance, consolidation, merge, or recovery. These are the workloads "
                    "that matter most for the final gate and must be native before they are benchmarked."
                ),
            ),
        ]
    )

    return workloads


def gate_workloads(workloads: list[Workload]) -> list[Workload]:
    return [w for w in workloads if w.counts_for_gate()]
