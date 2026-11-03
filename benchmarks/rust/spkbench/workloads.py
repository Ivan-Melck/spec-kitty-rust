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
class MutationSpec:
    """Execution contract for a workload that mutates its fixture.

    Mutation workloads cannot run through the read-only executor: every
    invocation needs a fresh clone of the pristine fixture, and equivalence is
    stdout/results **plus** post-state, not stdout alone. This descriptor is
    consumed by ``spkbench.mutation``; the measurement runner needs an additive
    hook before a mutation workload can be timed (see ``ADAPTER-REPORT.md``).

    ``ignored_post_state`` holds only implementation-private artifacts that are
    recorded separately but excluded from the equality manifest (currently the
    status lock files, whose paths differ between implementations). It is not a
    general nondeterminism escape hatch.
    """

    placeholder: str = "{FIXTURE}"
    ignored_post_state: tuple[str, ...] = ()
    timing_ready: bool = False

    def rewrite(self, argv: list[str], fixture: Path) -> list[str]:
        token = str(fixture)
        return [item.replace(self.placeholder, token) for item in argv]


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
    mutation: MutationSpec | None = None

    @property
    def runnable(self) -> bool:
        if self.mutation is not None and not self.mutation.timing_ready:
            return False
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
    """Parse the native CLI payload into a comparable document.

    Two native surfaces exist:

    * the versioned inspection envelope (``{"ok": true, "data": ...}`` or
      ``{"ok": false, "error": ...}``); and
    * the compatibility routes (``spec-kitty init|specify|agent ... --json``),
      which emit the bare upstream-shaped document with no envelope.

    A bare document is treated as a successful payload. Refusals carry
    ``error``/``error_code`` and are still compared field-for-field against the
    oracle, so the envelope shape is never silently normalised away: the bare
    document itself is the compared value.
    """
    try:
        document = json.loads(stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return {"_unparseable": str(exc), "_raw": stdout[:500].decode("utf-8", "replace")}
    if not isinstance(document, dict):
        return {"_unparseable": "envelope is not an object"}
    if "ok" not in document:
        # Compatibility route: the bare document is the payload.
        return {"ok": True, "data": document}
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


def _native_compat_binary(rust_binary: Path) -> Path | None:
    """Locate the compatibility CLI (``spec-kitty``) next to the inspection CLI.

    The inspection binary (``spec-kitty-rust``) has no ``agent`` routes; the
    compatibility binary (``spec-kitty``) serves both the upstream-shaped routes
    and, under ``native``, the inspection tree. ``bench.py`` historically passes
    ``target/release/spec-kitty-rust``, so compat workloads resolve the sibling
    artifact explicitly instead of assuming the caller switched binaries.

    The harness identity currently records only the primary binary; the dual
    binary gap is reported in ``ADAPTER-REPORT.md`` until the frozen identity
    core can attest both artifacts.
    """
    candidate = rust_binary if rust_binary.name == "spec-kitty" else rust_binary.with_name("spec-kitty")
    return candidate if candidate.exists() else None


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

    # ---------------- native compatibility reads (compat-project) ----------------
    # These pair the upstream-shaped native CLI with the pinned upstream command
    # implementation (called directly by a dev adapter, not reimplemented).
    def normalize_compat(value: object) -> object:
        return replace_strings(_unwrap_rust_data(value), path_tokens)

    def project_dependents(value: object) -> dict:
        data = _unwrap_rust_data(value)
        if not isinstance(data, dict):
            return {"_unexpected": data}
        # Upstream's own `depends_on` field is produced by a scalar frontmatter
        # reader that returns the raw list text (e.g. "[]" or "- WP01"), while
        # the native CLI returns a parsed array. The dependency *graph* is the
        # shared upstream-defined fact; the raw scalar is not. It is therefore
        # excluded from this projection, and the divergence is recorded here
        # rather than normalised inside a strict comparison.
        return {
            "wp_id": data.get("wp_id"),
            "dependents": sorted(data.get("dependents") or []),
        }

    def project_branch_context(value: object) -> dict:
        data = _unwrap_rust_data(value)
        if not isinstance(data, dict):
            return {"_unexpected": data}
        # Wall-clock fields are declared nondeterministic. `spec_kitty_version`
        # is a genuine gap: upstream emits it, the native route does not, so the
        # projection excludes it and the gap is stated in the notes.
        projected = drop_paths(
            data,
            [
                "NOW_UTC_ISO",
                "runtime_vars.now_utc_iso",
                "repo_root",
                "spec_kitty_version",
            ],
        )
        return replace_strings(projected, path_tokens)

    workloads.append(
        Workload(
            id="tasks.list",
            description=f"List six WPs in a mission with a coherent lane history ({compat_slug})",
            fixture="compat-project",
            klass="work",
            equivalence="pending" if compat_missing else "strict",
            required=True,
            rust=CommandSpec(
                "rust",
                [str(compat_bin), "agent", "tasks", "list-tasks", "--mission", compat_slug, "--json"],
            ),
            oracle=CommandSpec(
                "py-adapter",
                [
                    str(oracle_python),
                    str(adapters_dir / "py_tasks_list.py"),
                    "--repo",
                    str(compat.path),
                    "--mission",
                    compat_slug,
                ],
                floor_modules=["specify_cli.cli.commands.agent.tasks"],
            ),
            compare=compare_canonical,
            normalize=normalize_compat,
            expected_exit=0,
            pending_reason=(
                "The compatibility binary (target/release/spec-kitty) is absent; the "
                "inspection binary spec-kitty-rust has no `agent` routes and is not used "
                "as a fallback. Build both executables (cargo build --release -p sk-cli) "
                "and rerun."
            ) if compat_missing else None,
            notes=[
                "Both sides emit {tasks,count} with work_package_id/title/lane/path.",
                "Oracle calls the pinned `agent tasks list-tasks` command implementation directly.",
                "Happy path only: upstream's detached-HEAD/target-branch guard is tracked separately (pending).",
            ],
        )
    )
    workloads.append(
        Workload(
            id="tasks.list_dependents.projection",
            description="Direct dependents of WP02 in the compat fixture",
            fixture="compat-project",
            klass="work",
            equivalence="pending" if compat_missing else "projection",
            required=False,
            rust=CommandSpec(
                "rust",
                [
                    str(compat_bin),
                    "agent",
                    "tasks",
                    "list-dependents",
                    "WP02",
                    "--mission",
                    compat_slug,
                    "--json",
                ],
            ),
            oracle=CommandSpec(
                "py-adapter",
                [
                    str(oracle_python),
                    str(adapters_dir / "py_tasks_list_dependents.py"),
                    "--repo",
                    str(compat.path),
                    "--mission",
                    compat_slug,
                    "--wp-id",
                    "WP02",
                ],
                floor_modules=["specify_cli.cli.commands.agent.tasks"],
            ),
            compare=compare_canonical,
            normalize=lambda value: project_dependents(value),
            expected_exit=0,
            pending_reason=(
                "The compatibility binary (target/release/spec-kitty) is absent; the "
                "inspection binary spec-kitty-rust has no `agent` routes and is not used "
                "as a fallback. Build both executables (cargo build --release -p sk-cli) "
                "and rerun."
            ) if compat_missing else None,
            notes=[
                "PROJECTION: compares wp_id + sorted dependents (the dependency graph fact).",
                "Upstream's `depends_on` is a raw scalar-frontmatter string list, not a parsed array; excluded and reported, not silently equalised.",
                "Not eligible for the 3x gate.",
            ],
        )
    )
    workloads.append(
        Workload(
            id="mission.branch_context.projection",
            description="Deterministic branch contract on the compat fixture's unborn main branch",
            fixture="compat-project",
            klass="micro",
            equivalence="pending" if compat_missing else "projection",
            required=False,
            repeats=3,
            rust=CommandSpec(
                "rust",
                [str(compat_bin), "agent", "mission", "branch-context", "--json"],
            ),
            oracle=CommandSpec(
                "py-adapter",
                [
                    str(oracle_python),
                    str(adapters_dir / "py_branch_context.py"),
                    "--repo",
                    str(compat.path),
                ],
                floor_modules=["specify_cli.cli.commands.agent.mission_branch_context"],
            ),
            compare=compare_canonical,
            normalize=project_branch_context,
            expected_exit=0,
            pending_reason=(
                "The compatibility binary (target/release/spec-kitty) is absent; the "
                "inspection binary spec-kitty-rust has no `agent` routes and is not used "
                "as a fallback. Build both executables (cargo build --release -p sk-cli) "
                "and rerun."
            ) if compat_missing else None,
            notes=[
                "PROJECTION: all branch-contract fields compare except repo_root and wall-clock timestamps.",
                "Declared nondeterministic: NOW_UTC_ISO and runtime_vars.now_utc_iso (wall clock).",
                "Gap: upstream emits spec_kitty_version; the native route does not. Excluded from the projection, recorded here.",
                "MICRO: one git/config read; launch-dominated. Not gate evidence.",
            ],
        )
    )

    # ---------------- canonical status materialization (mutation) ----------------
    workloads.append(
        Workload(
            id="status.materialize",
            description="Canonical event-log replay writes status.json for a coherent 36-event mission",
            fixture="compat-project",
            klass="pending",
            equivalence="pending",
            required=True,
            rust=CommandSpec(
                "rust",
                [str(compat_bin), "agent", "status", "materialize", "--mission", compat_slug, "--json"],
            ),
            oracle=CommandSpec(
                "py-adapter",
                [
                    str(oracle_python),
                    str(adapters_dir / "py_status_materialize.py"),
                    "--repo",
                    "{FIXTURE}",
                    "--mission",
                    compat_slug,
                ],
                floor_modules=["specify_cli.cli.commands.agent.status", "specify_cli.status"],
            ),
            compare=compare_canonical,
            normalize=normalize_compat,
            expected_exit=0,
            mutation=MutationSpec(
                ignored_post_state=(".git/spec-kitty-locks/", ".kittify/spec-kitty-locks/"),
                timing_ready=False,
            ),
            pending_reason=(
                "Replaces the old 'no native reducer' blocker: the native canonical reducer now exists, and "
                "`tools/mutation_check.py` proves stdout JSON and status.json bytes are identical on a fresh clone "
                "per invocation. Two items remain before this can enter the timed gate: (1) the lock artifact is "
                "implementation-private and differs (upstream .git/spec-kitty-locks/<slug>.status.lock, native "
                ".git/spec-kitty-locks/status-<hash>.lock), so an operator/reviewer must rule whether lock naming is "
                "part of the post-state contract; (2) the frozen runner has no fresh-clone/post-state hook, so timing "
                "would measure clone cost. Core patch is specified in ADAPTER-REPORT.md."
            ),
            notes=[
                "Mutation: fresh clone per invocation; compare results AND post-state.",
                "Oracle calls the pinned `agent status materialize` command implementation directly.",
                "status.json is byte-identical; lock artifacts are recorded separately, not treated as equal.",
                "Non-coordination path only: coordination-routed materialization is refused natively and is not compared.",
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
                id="status.validate",
                description="Canonical status validation findings",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Native `agent status validate` and upstream validate are not the same contract. On the same "
                    "chaotic 240-event fixture upstream reports 21 errors + 156 warnings (clock-skew ordering and "
                    "missing done-evidence classes) while native reports 145 errors + 0 warnings (schema + FSM-edge "
                    "class only). Native has no clock-skew or evidence warning families, so pairing them would "
                    "compare different findings even when both exit 1. No normalization is applied."
                ),
            ),
            Workload(
                id="tasks.list.target-branch-guard",
                description="Detached-HEAD / target-branch refusal for tasks reads",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Upstream `agent tasks list-tasks` refuses a detached HEAD and resolves the mission target "
                    "branch before reading. The native tasks read has no branch guard: on a detached fixture it "
                    "returns rows where upstream exits 1 with a different envelope. Happy-path rows are covered "
                    "strictly by tasks.list on an unborn main branch; the guard contract itself is unimplemented."
                ),
            ),
            Workload(
                id="tasks.status",
                description="Kanban status board (lanes, readiness, stale/workspace verdicts)",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Partial and not equivalent. On the same coherent fixture native emits extra contract "
                    "fields (auto_commit, done_count, done_percentage, progress_percentage, "
                    "progress_semantics, stale_verdicts, stale_wps, stalled_wps) and derives agent/"
                    "workspace_kind from events; upstream omits those fields and reports empty agent and "
                    "workspace_kind=unknown. Git-backed stale verdicts, stalled-review flags and config "
                    "auto_commit are declared native gaps, so neither result nor shape can be paired."
                ),
            ),
            Workload(
                id="lifecycle.plan",
                description="Setup-plan scaffold and planning gate",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Partial and behaviourally different. On the same Mission upstream returns "
                    "result=blocked with SPEC_NOT_SUBSTANTIVE_OR_UNCOMMITTED and writes nothing; native "
                    "scaffolds plan.md without the substantive-spec gate or planning commit. Lifecycle "
                    "phase events are also absent natively."
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
                    "Native now has a durable run store (runtime start/query/advance/answer/recover) and a "
                    "partial `next`, but it is still not the upstream contract: mission-board progress, WP "
                    "workspace resolution in the decision, decision-log git commits, analysis currency, "
                    "lifecycle records and retrospective hooks are missing. The native runtime routes are "
                    "extensions with no upstream counterpart, so they cannot be paired as governed next."
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
                id="lifecycle.init",
                description="Project initialization and generated asset set",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Native init is implemented but not equivalent. On empty directories with the same "
                    "`--ai codex --non-interactive` inputs, upstream writes 100+ files (full .agents/skills "
                    "tree, command copies, git hooks, charter/activation scaffolding); native writes 14 files "
                    "(config, metadata, ledger, a subset of skills). File populations and human output differ, "
                    "and org-pack acquisition is not integrated. Not eligible until the asset set and output "
                    "contract match."
                ),
            ),
            Workload(
                id="mission.create",
                description="Mission scaffold creation (agent mission create / specify)",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Partial and behaviourally different. On the same project upstream refuses with "
                    "ACTIVE_CHARTER_CONFIG_INVALID because no mission type is activated; native creates the "
                    "mission without that gate. Native also appends a ULID to the directory name and reports "
                    "mission_number allocation as absent, while upstream allocates the mission number and "
                    "commits planning artifacts. Post-state and result contracts differ."
                ),
            ),
            Workload(
                id="tasks.finalize",
                description="Finalize tasks.md dependencies into WP frontmatter",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Partial. On the same fixture both exit 1 but for different contracts: upstream validates "
                    "requirement mapping from spec/tasks.md and reports missing requirement refs; native "
                    "reports `tasks.md not found` at the mission root. Native also skips requirement-mapping "
                    "gates, planning commit, lanes.json refresh and status lifecycle events. No shared error "
                    "or post-state contract yet."
                ),
            ),
            Workload(
                id="lane.implement",
                description="Lane worktree allocation and workspace ownership",
                fixture=None,
                klass="pending",
                equivalence="pending",
                required=True,
                pending_reason=(
                    "Partial. On the same project both exit 1 for different reasons: upstream refuses first "
                    "on the protected `main` branch (auto-commit status guard); native refuses with "
                    "MISSING_LANES because no lanes.json exists. Branch/dependency preflights, workspace "
                    "context persistence and auto-commit are not integrated, so allocation cannot be "
                    "compared as one workflow."
                ),
            ),
        ]
    )

    return workloads


def gate_workloads(workloads: list[Workload]) -> list[Workload]:
    return [w for w in workloads if w.counts_for_gate()]
