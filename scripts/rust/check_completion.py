#!/usr/bin/env python3
"""Non-vacuous completion checker for the Spec Kitty Rust rewrite.

The checker refuses to call the rewrite complete unless **every** required
upstream behaviour, full workflow, contract and performance gate is evidenced.
It is deliberately hostile to greenwashing:

* It re-discovers the pinned Python CLI surface (or verifies the committed
  surface against the live source) and compares every command path, option,
  argument, alias and hidden flag with the inventory.  Deleting or skipping a
  row therefore fails, even if the JSON still parses.
* It rebuilds the structural inventory from the curated model and compares
  IDs, classification, required flags and surface contracts, so hand-editing
  coverage.json cannot erase scope.
* Every required entry must be natively implemented (Rust refs only), pass
  conformance with evidence and be independently reviewed.
* Every workflow must carry acceptance, differential, refusal, interruption,
  concurrency and user-content tests plus independent review.
* Every strict-equivalence performance workload needs raw samples, result
  equivalence, environment/toolchain/fixture identity and an aggregate
  geometric mean >= 3x with no >10% required-workload regression.  Startup is
  measured separately and can never stand in for real work.
* It refuses production Python/FFI in the Rust crates.

Development-only Python; never invoked by the Rust runtime.

Usage (oracle interpreter):

    <oracle-python> scripts/rust/check_completion.py \
        --json-report docs/rust/inventory/completion-report.json

Exit: 0 only when every gate passes; 1 otherwise; 2 for a checker/setup error.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
import sys
from dataclasses import dataclass, field
from pathlib import Path

# Development tooling must not leave bytecode in the checkout.
sys.dont_write_bytecode = True
from typing import Any, Iterable

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import build_inventory  # noqa: E402
import inventory_model  # noqa: E402

DEFAULT_INVENTORY = ROOT / "docs/rust/inventory/coverage.json"
DEFAULT_SURFACE = ROOT / "docs/rust/inventory/python-surface.json"

# Hard-coded completion floors.  These are deliberately duplicated from the
# curated model so editing that model cannot lower the acceptance bar.
REQUIRED_CONTRACT_IDS: frozenset[str] = frozenset(
    {
        "WIRE-ORCH-ENVELOPE",
        "WIRE-ORCH-ERRORS",
        "WIRE-STATUS-EVENTS",
        "WIRE-LIFECYCLE-EVENTS",
        "WIRE-DECISIONS-LEDGER",
        "WIRE-READ-SERVICE",
        "WIRE-WRITE-SERVICE",
        "WIRE-TRACKER",
        "WIRE-EVENTS-PACKAGE",
        "WIRE-GLOSSARY-EVENTS",
        "WIRE-MCP-COMPANION",
        "WIRE-JSON-ENVELOPE",
        "GOV-ACTIVATION-LEDGER",
        "GOV-DRG-CASCADE",
        "GOV-PACK-SKILLS",
        "GOV-CHARTER-LIFECYCLE",
        "GOV-PRESETS-ORG-PACKS",
        "ASSET-BUILTIN-PACK",
        "ASSET-INTERNAL-PACK",
        "ASSET-ORG-PACK",
        "ASSET-TEMPLATES",
        "ASSET-COMMANDS-SKILLS",
        "ASSET-GLOSSARY-SEEDS",
        "ASSET-MISSION-TYPES",
        "ASSET-DOCTRINE-DRG",
        "ASSET-SERVICE-PINS",
        "MIG-REGISTRY",
        "STATE-PROJECT-INIT",
        "STATE-MISSION-CREATE",
        "STATE-LANE-WORKTREE",
        "STATE-EVENT-LEDGER",
        "STATE-LANE-FSM",
        "STATE-STATUS-REDUCER",
        "STATE-RUN-STORE",
        "STATE-DECISIONS",
        "STATE-ACCEPTANCE",
        "STATE-CONSOLIDATION",
        "STATE-ROLLBACK",
        "STATE-UPGRADE-LEDGER",
        "STATE-TRACKER",
        "STATE-SERVICE-LAUNCHER",
        "STATE-SESSION-PRESENCE",
        "STATE-OPS-LEDGER",
        "EXT-EVENTS",
        "EXT-TRACKER",
        "EXT-READ-SERVICE",
        "EXT-WRITE-SERVICE",
        "EXT-CHARTER-SERVICE",
        "EXT-GIT",
        "EXT-UV-LOCK",
        "EXT-PACK-REPOS",
        "PERF-HARNESS",
    }
)
REQUIRED_WORKFLOW_IDS: frozenset[str] = frozenset(
    {
        "WF-CREATE",
        "WF-SPECIFY",
        "WF-PLAN",
        "WF-TASKS",
        "WF-IMPLEMENT",
        "WF-REVIEW",
        "WF-ACCEPT",
        "WF-CONSOLIDATE",
        "WF-MERGE",
        "WF-RECOVERY",
        "WF-HUMAN-DECISION",
        "WF-NEXT-RUNTIME",
        "WF-GOVERNANCE",
        "WF-PERSISTENT-STATE",
        "WF-EVENT-INTEGRITY",
        "WF-WORKTREE-OWNERSHIP",
    }
)
REQUIRED_PERF_WORKLOAD_IDS: frozenset[str] = frozenset(
    {
        "startup.version",
        "startup.help",
        "cli.mission.list",
        "status.replay",
        "state.event-append",
        "state.move-task",
        "state.decision.resolve",
        "runtime.next.governed",
        "governance.charter",
        "glossary.activation",
        "workflow.mission-create",
        "workflow.specify",
        "workflow.plan",
        "workflow.tasks",
        "workflow.implement",
        "workflow.review",
        "workflow.accept",
        "workflow.consolidate",
        "workflow.merge",
        "workflow.recovery",
    }
)
REQUIRED_STARTUP_WORKLOADS: frozenset[str] = frozenset({"startup.version", "startup.help"})
REQUIRED_GATE_WORKLOADS: frozenset[str] = REQUIRED_PERF_WORKLOAD_IDS - REQUIRED_STARTUP_WORKLOADS - {"cli.mission.list"}
REQUIRED_WORKFLOW_EVIDENCE: tuple[str, ...] = (
    "acceptance_tests",
    "differential_tests",
    "refusal_tests",
    "interruption_tests",
    "concurrency_tests",
    "user_content_tests",
)
ALLOWED_EXCLUSIONS: dict[str, str] = {
    "CLI:__force_multi_command_mode__": "internal multi-command-mode sentinel; not an operator surface",
}
MIN_PERF_SPEEDUP = 3.0
MAX_PERF_REGRESSION = 0.10
MIN_SAMPLES = 20
MIN_STRICT_WORKLOADS = 14


@dataclass
class Report:
    errors: list[dict[str, Any]] = field(default_factory=list)
    warnings: list[dict[str, Any]] = field(default_factory=list)
    gates: dict[str, Any] = field(default_factory=dict)
    summary: dict[str, Any] = field(default_factory=dict)
    max_detail: int = 40

    def fail(self, code: str, message: str, *, entry_id: str | None = None, gate: str = "general") -> None:
        self.errors.append({"code": code, "message": message, "entry_id": entry_id, "gate": gate})

    def warn(self, code: str, message: str, *, entry_id: str | None = None, gate: str = "general") -> None:
        self.warnings.append({"code": code, "message": message, "entry_id": entry_id, "gate": gate})

    def fail_many(
        self,
        code: str,
        message: str,
        items: Iterable[tuple[str, str]],
        *,
        gate: str,
    ) -> None:
        items = list(items)
        for entry_id, detail in items[: self.max_detail]:
            self.fail(code, f"{message}: {detail}", entry_id=entry_id, gate=gate)
        if len(items) > self.max_detail:
            self.fail(
                code,
                f"{message}: {len(items) - self.max_detail} further entries omitted (see summary)",
                gate=gate,
            )


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _ref_path(ref: str) -> Path:
    """Resolve a repo-relative ``path[:line]`` evidence reference."""
    text = ref.split(":", 1)[0] if re.search(r":\d+$", ref) else ref
    path = Path(text)
    return path if path.is_absolute() else ROOT / path


def _ref_exists(ref: str) -> bool:
    path = _ref_path(ref)
    return path.exists()


def _surface_signature(surface: dict[str, Any]) -> dict[str, dict[str, Any]]:
    expected: dict[str, dict[str, Any]] = {}
    expected["CLI:"] = _surface_contract({**surface["root"], "kind": "root"})
    for group in surface.get("groups", []):
        if not group["path"]:
            continue
        expected[group["id"]] = _surface_contract(group)
    for leaf in surface.get("leaves", []):
        expected[leaf["id"]] = _surface_contract(leaf)
    return expected


def _surface_contract(node: dict[str, Any]) -> dict[str, Any]:
    def option_signature(param: dict[str, Any]) -> dict[str, Any]:
        return {
            "name": param.get("name"),
            "flags": param.get("flags", []),
            "secondary_flags": param.get("secondary_flags", []),
            "type": param.get("type"),
            "required": param.get("required"),
            "default": param.get("default"),
            "multiple": param.get("multiple"),
            "nargs": param.get("nargs"),
            "is_flag": param.get("is_flag"),
            "hidden": param.get("hidden"),
            "alias_for": param.get("alias_for"),
            "deprecated": param.get("deprecated"),
            "injected": param.get("injected"),
        }

    def argument_signature(param: dict[str, Any]) -> dict[str, Any]:
        return {
            "name": param.get("name"),
            "type": param.get("type"),
            "required": param.get("required"),
            "default": param.get("default"),
            "multiple": param.get("multiple"),
            "nargs": param.get("nargs"),
            "hidden": param.get("hidden"),
            "deprecated": param.get("deprecated"),
            "alias_for": param.get("alias_for"),
        }

    return {
        "path": node["path"],
        "kind": node["kind"],
        "hidden": node.get("hidden", False),
        "deprecated": node.get("deprecated", False),
        "accepts_json": node.get("accepts_json", False),
        "context_settings": node.get("context_settings", {}),
        "options": sorted(
            (option_signature(option) for option in node.get("options", [])),
            key=lambda item: (item["name"] or "", item["flags"]),
        ),
        "arguments": sorted(
            (argument_signature(argument) for argument in node.get("arguments", [])),
            key=lambda item: item["name"] or "",
        ),
        "deprecated_flags": node.get("deprecated_flags", []),
    }


def _source_surface_sha(root: Path) -> str:
    root_module = root / "src/specify_cli/__init__.py"
    modules = [
        path
        for path in sorted((root / "src/specify_cli/cli/commands").rglob("*.py"))
        if "__pycache__" not in path.parts
    ]
    files = [root_module, *modules]
    digest = hashlib.sha256()
    for path in sorted(files):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(_sha256(path).encode())
        digest.update(b"\n")
    return digest.hexdigest()


def _check_surface_structure(
    report: Report,
    inventory: dict[str, Any],
    surface: dict[str, Any],
) -> None:
    gate = "surface"
    entries = {entry["id"]: entry for entry in inventory.get("entries", [])}
    if len(entries) != len(inventory.get("entries", [])):
        report.fail("DUPLICATE_ENTRY_ID", "inventory contains duplicate entry ids", gate=gate)
    expected = _surface_signature(surface)
    missing = sorted(set(expected) - set(entries))
    extra = sorted(set(entries) - set(expected) - REQUIRED_CONTRACT_IDS - {
        entry["id"]
        for entry in inventory.get("entries", [])
        if entry.get("kind") in {"migration", "workflow", "migration_contract", "performance_contract"}
        or entry.get("kind", "").endswith("_contract")
    } - {
        f"MIG:{migration['id']}" for migration in surface.get("migrations", [])
    })
    if missing:
        report.fail_many(
            "MISSING_CLI_ENTRY",
            "reachable CLI surface is absent from the inventory",
            [(item, item) for item in missing],
            gate=gate,
        )
    if extra:
        report.fail_many(
            "UNEXPECTED_CLI_ENTRY",
            "inventory contains an entry that is not reachable upstream surface",
            [(item, item) for item in extra],
            gate=gate,
        )
    for entry_id, expected_signature in expected.items():
        entry = entries.get(entry_id)
        if not entry:
            continue
        if entry.get("surface") != expected_signature:
            report.fail(
                "SURFACE_DRIFT",
                "recorded command/option/argument surface differs from the live pinned source",
                entry_id=entry_id,
                gate=gate,
            )
        if entry.get("kind") not in {"cli_root", "cli_group", "cli_leaf"}:
            report.fail("WRONG_ENTRY_KIND", "CLI surface entry has the wrong kind", entry_id=entry_id, gate=gate)

    manifest = surface.get("completion_manifest", {})
    if not manifest.get("matches_dynamic_tree"):
        report.fail("COMPLETION_MANIFEST_DRIFT", "the shipped completion manifest disagrees with the dynamic command tree", gate=gate)
    if manifest.get("leaf_count") != len(surface.get("leaves", [])):
        report.fail("COMPLETION_MANIFEST_COUNT", "completion manifest leaf count differs from discovery", gate=gate)
    if manifest.get("manifest_only") or manifest.get("surface_only"):
        report.fail(
            "COMPLETION_MANIFEST_DIFF",
            f"manifest-only={manifest.get('manifest_only')} surface-only={manifest.get('surface_only')}",
            gate=gate,
        )

    expected_counts = surface.get("counts", {})
    actual_counts = inventory.get("counts", {}).get("cli", {})
    for key in ("leaves", "groups", "hidden_leaves", "json_leaves", "options", "arguments", "migrations"):
        if expected_counts.get(key) != actual_counts.get(key):
            report.fail(
                "CLI_COUNT_DRIFT",
                f"{key}: inventory={actual_counts.get(key)} live={expected_counts.get(key)}",
                gate=gate,
            )

    # Required flags and exclusions are part of the contract.
    allowed = inventory.get("policy", {}).get("allowed_exclusions", {})
    if allowed != ALLOWED_EXCLUSIONS:
        report.fail(
            "EXCLUSION_DRIFT",
            "allowed exclusions must be exactly the frozen internal-sentinel allowlist",
            gate=gate,
        )
    for entry in inventory.get("entries", []):
        if entry["id"] in ALLOWED_EXCLUSIONS:
            if entry.get("required") is not False:
                report.fail("EXCLUDED_BUT_REQUIRED", "excluded sentinel is marked required", entry_id=entry["id"], gate=gate)
            if entry.get("exclusion", {}).get("class") != "internal_sentinel":
                report.fail("EXCLUSION_CLASS", "excluded entry has no frozen exclusion class", entry_id=entry["id"], gate=gate)
        elif entry.get("kind", "").startswith("cli_"):
            if entry.get("required") is not True:
                report.fail("UNEXCUSED_SKIP", "reachable CLI entry is marked not required without an allowlist entry", entry_id=entry["id"], gate=gate)


def _check_structural_rebuild(
    report: Report,
    inventory: dict[str, Any],
    surface: dict[str, Any],
) -> None:
    """Recompute structure from the curated model; statuses may change, scope may not."""
    gate = "structure"
    fresh = build_inventory.build(surface, ROOT)
    fresh_by_id = {entry["id"]: entry for entry in fresh["entries"]}
    entries = {entry["id"]: entry for entry in inventory.get("entries", [])}
    missing = sorted(set(fresh_by_id) - set(entries))
    extra = sorted(set(entries) - set(fresh_by_id))
    if missing:
        report.fail_many(
            "STRUCTURE_ROW_DELETED",
            "inventory row required by the curated model is missing",
            [(item, item) for item in missing],
            gate=gate,
        )
    if extra:
        report.fail_many(
            "STRUCTURE_ROW_EXTRA",
            "inventory row is not produced by the curated model",
            [(item, item) for item in extra],
            gate=gate,
        )
    structural_fields = ("required", "mutates", "mutation_trigger", "dependencies")
    for entry_id in sorted(set(fresh_by_id) & set(entries)):
        fresh_entry = fresh_by_id[entry_id]
        entry = entries[entry_id]
        for key in structural_fields:
            if entry.get(key) != fresh_entry.get(key):
                report.fail(
                    "STRUCTURE_FIELD_DRIFT",
                    f"{key} differs from the curated model (inventory={entry.get(key)!r})",
                    entry_id=entry_id,
                    gate=gate,
                )
        if entry.get("contract", {}).get("class") != fresh_entry.get("contract", {}).get("class"):
            report.fail(
                "CONTRACT_CLASS_DRIFT",
                "contract class differs from the curated model",
                entry_id=entry_id,
                gate=gate,
            )
        if entry.get("surface") != fresh_entry.get("surface"):
            report.fail("SURFACE_STRUCTURE_DRIFT", "surface contract differs from a fresh discovery", entry_id=entry_id, gate=gate)

    if set(inventory.get("policy", {}).get("required_workflow_ids", [])) != REQUIRED_WORKFLOW_IDS:
        report.fail("WORKFLOW_POLICY_DRIFT", "required workflow ids do not match the frozen completion floor", gate=gate)
    if set(inventory.get("policy", {}).get("required_perf_workload_ids", [])) != REQUIRED_PERF_WORKLOAD_IDS:
        report.fail("PERF_POLICY_DRIFT", "required performance workload ids do not match the frozen completion floor", gate=gate)
    if inventory.get("performance", {}).get("policy") != inventory_model.PERF_POLICY:
        report.fail("PERF_POLICY_MUTATED", "performance policy differs from the curated gate", gate=gate)
    if inventory.get("native_extensions") != inventory_model.NATIVE_EXTENSIONS:
        report.fail("NATIVE_EXTENSION_DRIFT", "native extension census differs from the curated model", gate=gate)
    entry_ids = set(entries)
    for extension in inventory.get("native_extensions", []):
        equivalent = extension.get("upstream_equivalent")
        if equivalent is not None and equivalent not in entry_ids:
            report.fail(
                "NATIVE_EQUIVALENT_MISSING",
                f"native extension claims parity with a missing entry: {equivalent}",
                entry_id=extension.get("id"),
                gate=gate,
            )
        projection = extension.get("projection_of")
        if projection is not None and projection not in entry_ids:
            report.fail(
                "NATIVE_PROJECTION_MISSING",
                f"native extension projects a missing entry: {projection}",
                entry_id=extension.get("id"),
                gate=gate,
            )


def _check_modules(report: Report, inventory: dict[str, Any], surface: dict[str, Any]) -> None:
    gate = "modules"
    expected = {module["module"]: module for module in surface.get("modules", [])}
    module_rows = inventory.get("modules", [])
    if len({module.get("module") for module in module_rows}) != len(module_rows):
        report.fail("MODULE_DUPLICATE_ROW", "module census contains duplicate module rows", gate=gate)
    actual = {module["module"]: module for module in module_rows}
    missing = sorted(set(expected) - set(actual))
    extra = sorted(set(actual) - set(expected))
    if missing:
        report.fail_many(
            "MODULE_ROW_MISSING",
            "Python command-module file is absent from the module census",
            [(item, item) for item in missing],
            gate=gate,
        )
    if extra:
        report.fail_many(
            "MODULE_ROW_EXTRA",
            "module census contains a file the pinned source does not",
            [(item, item) for item in extra],
            gate=gate,
        )
    for name in sorted(set(expected) & set(actual)):
        source = expected[name]
        recorded = actual[name]
        if recorded.get("sha256") != source.get("sha256"):
            report.fail("MODULE_HASH_DRIFT", f"module changed since inventory: {name}", gate=gate)
        registrations = source.get("registrations", {})
        has_registrations = bool(registrations.get("commands") or registrations.get("typers"))
        if has_registrations and recorded.get("status") not in {"registered_command_source", "dynamically_registered"}:
            report.fail("MODULE_REGISTRATION_UNACCOUNTED", f"module registers CLI commands but is classified {recorded.get('status')}", gate=gate)
        if source.get("reachable_command_source") and recorded.get("status") != "registered_command_source":
            report.fail("MODULE_STATUS_DRIFT", f"callback module has wrong status: {name}", gate=gate)
        if source.get("import_reachable") is not None and recorded.get("import_reachable") != source.get("import_reachable"):
            report.fail("MODULE_REACHABILITY_DRIFT", f"import reachability differs: {name}", gate=gate)
    top_level = sum(1 for module in actual.values() if module.get("top_level"))
    if top_level != len([name for name in expected if Path(name).parent.as_posix() == "src/specify_cli/cli/commands"]):
        report.fail("MODULE_TOP_LEVEL_COUNT", f"top-level module count={top_level}", gate=gate)
    for key, status in (
        ("modules_registered", "registered_command_source"),
        ("modules_dynamically_registered", "dynamically_registered"),
        ("modules_imported_helper", "imported_helper"),
        ("modules_unreferenced_helper", "unreferenced_helper"),
    ):
        expected_count = sum(1 for module in actual.values() if module.get("status") == status)
        if inventory.get("counts", {}).get(key) != expected_count:
            report.fail("MODULE_COUNT_DRIFT", f"{key} is stale", gate=gate)
    if inventory.get("counts", {}).get("modules_total") != len(actual):
        report.fail("MODULE_COUNT_DRIFT", "modules_total is stale", gate=gate)
    if inventory.get("counts", {}).get("modules_top_level") != top_level:
        report.fail("MODULE_COUNT_DRIFT", "modules_top_level is stale", gate=gate)


def _check_contracts(report: Report, inventory: dict[str, Any]) -> None:
    gate = "contracts"
    entries = {entry["id"]: entry for entry in inventory.get("entries", [])}
    missing = sorted(REQUIRED_CONTRACT_IDS - set(entries))
    if missing:
        report.fail_many(
            "MISSING_CONTRACT_ENTRY",
            "required contract entry is absent",
            [(item, item) for item in missing],
            gate=gate,
        )
    unresolved = sorted(
        entry_id
        for entry_id, entry in entries.items()
        if entry.get("contract", {}).get("status") == "unresolved"
    )
    if unresolved:
        report.fail_many(
            "UNRESOLVED_CONTRACT",
            "entry has no documented mutation/read contract",
            [(item, item) for item in unresolved],
            gate=gate,
        )
    if inventory.get("unresolved_entry_ids"):
        report.fail("UNRESOLVED_DECLARED", "inventory still declares unresolved entries", gate=gate)

    for entry_id, entry in entries.items():
        contract = entry.get("contract", {})
        if entry.get("mutates"):
            problems = []
            if contract.get("status") != "documented":
                problems.append("status")
            if not (contract.get("writes") or contract.get("git_effects")):
                problems.append("writes/git_effects")
            if not contract.get("authority"):
                problems.append("authority")
            if not contract.get("source_refs"):
                problems.append("source_refs")
            for field_name in ("rollback", "atomicity", "user_content", "recovery"):
                if not contract.get(field_name):
                    problems.append(field_name)
            if entry.get("mutation_trigger") and not entry.get("mutation_trigger"):
                problems.append("mutation_trigger")
            if problems:
                report.fail(
                    "MUTATION_CONTRACT_INCOMPLETE",
                    f"missing {', '.join(problems)}",
                    entry_id=entry_id,
                    gate=gate,
                )
        if entry.get("mutation_trigger") and entry.get("mutates") is not True:
            report.fail("TRIGGER_WITHOUT_MUTATION", "mutation trigger is set on a non-mutating entry", entry_id=entry_id, gate=gate)

        for ref in [*entry.get("source_refs", []), *entry.get("test_refs", [])]:
            if not _ref_exists(ref):
                report.fail("MISSING_REFERENCE", f"reference does not exist: {ref}", entry_id=entry_id, gate=gate)
        for ref in entry.get("implementation", {}).get("native_refs", []):
            if not _ref_exists(ref):
                report.fail("MISSING_NATIVE_REF", f"native reference does not exist: {ref}", entry_id=entry_id, gate=gate)
        for ref in entry.get("conformance", {}).get("refs", []):
            if not _ref_exists(ref):
                report.fail("MISSING_CONFORMANCE_REF", f"conformance reference does not exist: {ref}", entry_id=entry_id, gate=gate)
        for ref in entry.get("review", {}).get("refs", []):
            if not _ref_exists(ref):
                report.fail("MISSING_REVIEW_REF", f"review reference does not exist: {ref}", entry_id=entry_id, gate=gate)

    # Wire contract must be the pinned one.
    orchestrator = [entry for entry in entries.values() if entry["id"].startswith("CLI:orchestrator-api/")]
    if orchestrator:
        versions = {entry.get("wire", {}).get("contract_version") for entry in orchestrator}
        if len(versions) != 1 or None in versions:
            report.fail("WIRE_VERSION_SPLIT", f"orchestrator verbs disagree on contract version: {sorted(map(str, versions))}", gate=gate)
        for entry in orchestrator:
            wire = entry.get("wire") or {}
            if not wire.get("always_json"):
                report.fail("WIRE_NOT_JSON", "orchestrator verb is not marked always-JSON", entry_id=entry["id"], gate=gate)
            if not wire.get("error_codes"):
                report.fail("WIRE_NO_ERRORS", "orchestrator verb has no closed error-code vocabulary", entry_id=entry["id"], gate=gate)


def _check_migrations(report: Report, inventory: dict[str, Any], surface: dict[str, Any]) -> None:
    gate = "migrations"
    entries = {entry["id"]: entry for entry in inventory.get("entries", [])}
    live = surface.get("migrations", [])
    expected_ids = {f"MIG:{migration['id']}" for migration in live}
    present_ids = {entry_id for entry_id in entries if entry_id.startswith("MIG:")}
    missing = sorted(expected_ids - present_ids)
    extra = sorted(present_ids - expected_ids)
    if missing:
        report.fail_many(
            "MIGRATION_ROW_MISSING",
            "registered migration is absent from the inventory",
            [(item, item) for item in missing],
            gate=gate,
        )
    if extra:
        report.fail_many(
            "MIGRATION_ROW_EXTRA",
            "inventory contains a migration id the registry does not register",
            [(item, item) for item in extra],
            gate=gate,
        )
    by_id = entries
    for migration in live:
        entry = by_id.get(f"MIG:{migration['id']}")
        if not entry:
            continue
        if entry.get("target_version") != migration["target_version"]:
            report.fail("MIGRATION_VERSION_DRIFT", f"{migration['id']} target version differs", entry_id=entry["id"], gate=gate)
        if entry.get("source_refs") != [migration["source_ref"]]:
            report.fail("MIGRATION_SOURCE_DRIFT", f"{migration['id']} source reference differs", entry_id=entry["id"], gate=gate)
        if "MIG-REGISTRY" not in entry.get("dependencies", []):
            report.fail("MIGRATION_DEPENDENCY", f"{migration['id']} does not depend on the registry contract", entry_id=entry["id"], gate=gate)
    registry = entries.get("MIG-REGISTRY")
    if not registry or registry.get("required") is not True:
        report.fail("MIGRATION_REGISTRY_MISSING", "MIG-REGISTRY is not a required entry", gate=gate)


def _check_status_gates(report: Report, inventory: dict[str, Any]) -> None:
    gate = "status"
    required = [entry for entry in inventory.get("entries", []) if entry.get("required")]

    for entry in required:
        impl = entry.get("implementation", {})
        conformance = entry.get("conformance", {})
        review = entry.get("review", {})
        if impl.get("status") == "implemented":
            refs = impl.get("native_refs", [])
            if not refs:
                report.fail("IMPLEMENTATION_WITHOUT_EVIDENCE", "implemented status has no native references", entry_id=entry["id"], gate=gate)
            for ref in refs:
                path = _ref_path(ref)
                if not path.as_posix().startswith((ROOT / "crates").as_posix()):
                    report.fail("IMPLEMENTATION_NOT_NATIVE", f"implemented native ref is outside crates/: {ref}", entry_id=entry["id"], gate=gate)
                if path.suffix != ".rs":
                    report.fail("IMPLEMENTATION_NOT_RUST", f"implemented native ref is not Rust: {ref}", entry_id=entry["id"], gate=gate)
        if conformance.get("status") == "pass":
            if not conformance.get("refs"):
                report.fail("CONFORMANCE_WITHOUT_EVIDENCE", "passing conformance has no references", entry_id=entry["id"], gate=gate)
            elif not any("tests" in _ref_path(ref).parts for ref in conformance.get("refs", [])):
                report.fail(
                    "CONFORMANCE_EVIDENCE_NOT_TEST",
                    "passing conformance has no test-suite reference",
                    entry_id=entry["id"],
                    gate=gate,
                )
        if review.get("status") == "approved":
            if not review.get("refs"):
                report.fail("REVIEW_WITHOUT_EVIDENCE", "approved review has no references", entry_id=entry["id"], gate=gate)
            if review.get("independent") is not True:
                report.fail("REVIEW_NOT_INDEPENDENT", "approved review is not marked independent", entry_id=entry["id"], gate=gate)

        missing_impl = impl.get("status") != "implemented"
        missing_conformance = conformance.get("status") != "pass"
        missing_review = review.get("status") != "approved"
        if missing_impl or missing_conformance or missing_review:
            reasons = []
            if missing_impl:
                reasons.append(f"implementation={impl.get('status')}")
            if missing_conformance:
                reasons.append(f"conformance={conformance.get('status')}")
            if missing_review:
                reasons.append(f"review={review.get('status')}")
            report.fail(
                "REQUIRED_ENTRY_NOT_COMPLETE",
                "; ".join(reasons),
                entry_id=entry["id"],
                gate=gate,
            )

    required_incomplete = [
        entry["id"]
        for entry in required
        if entry.get("implementation", {}).get("status") != "implemented"
    ]
    if required_incomplete:
        report.fail(
            "UNSUPPORTED_REQUIRED_ENTRIES",
            f"{len(required_incomplete)} required entries are not natively implemented",
            gate=gate,
        )


def _check_workflows(report: Report, inventory: dict[str, Any]) -> None:
    gate = "workflows"
    workflows = inventory.get("workflows", [])
    if len({workflow.get("id") for workflow in workflows}) != len(workflows):
        report.fail("WORKFLOW_DUPLICATE_ROW", "workflow list contains duplicate ids", gate=gate)
    by_id = {workflow["id"]: workflow for workflow in workflows}
    missing = sorted(REQUIRED_WORKFLOW_IDS - set(by_id))
    if missing:
        report.fail_many(
            "MISSING_WORKFLOW",
            "required full workflow is absent",
            [(item, item) for item in missing],
            gate=gate,
        )
    extra_required = sorted(
        workflow["id"] for workflow in workflows if workflow.get("required") and workflow["id"] not in REQUIRED_WORKFLOW_IDS
    )
    if extra_required:
        report.fail_many(
            "UNDECLARED_WORKFLOW",
            "required workflow is not in the frozen completion floor",
            [(item, item) for item in extra_required],
            gate=gate,
        )
    entry_ids = {entry["id"] for entry in inventory.get("entries", [])}
    for workflow in workflows:
        workflow_id = workflow["id"]
        if workflow.get("required") is not True:
            report.fail("WORKFLOW_NOT_REQUIRED", "completion workflow is marked not required", entry_id=workflow_id, gate=gate)
        for step in workflow.get("steps", []):
            if step not in entry_ids:
                report.fail("WORKFLOW_STEP_MISSING", f"step is not an inventory entry: {step}", entry_id=workflow_id, gate=gate)
        problems = []
        for category in REQUIRED_WORKFLOW_EVIDENCE:
            evidence_refs = workflow.get(category) or []
            if not evidence_refs:
                problems.append(category)
                continue
            if not any("tests" in _ref_path(ref).parts for ref in evidence_refs):
                report.fail(
                    "WORKFLOW_EVIDENCE_NOT_TEST",
                    f"{category} has no test-suite reference",
                    entry_id=workflow_id,
                    gate=gate,
                )
            for ref in evidence_refs:
                if not _ref_exists(ref):
                    report.fail(
                        "WORKFLOW_EVIDENCE_MISSING_REF",
                        f"{category} reference does not exist: {ref}",
                        entry_id=workflow_id,
                        gate=gate,
                    )
        if problems:
            report.fail(
                "WORKFLOW_EVIDENCE_MISSING",
                f"missing evidence categories: {', '.join(problems)}",
                entry_id=workflow_id,
                gate=gate,
            )
        review = workflow.get("review", {})
        if review.get("status") != "approved":
            report.fail("WORKFLOW_NOT_REVIEWED", f"review={review.get('status')}", entry_id=workflow_id, gate=gate)
        if review.get("independent") is not True:
            report.fail("WORKFLOW_REVIEW_NOT_INDEPENDENT", "workflow review is not independent", entry_id=workflow_id, gate=gate)
        if workflow.get("status") != "pass":
            report.fail("WORKFLOW_NOT_PASSING", f"status={workflow.get('status')}", entry_id=workflow_id, gate=gate)
        for workload_id in workflow.get("performance_workloads", []):
            workload = next(
                (item for item in inventory.get("performance", {}).get("workloads", []) if item["id"] == workload_id),
                None,
            )
            if not workload or workload.get("status") != "pass":
                report.fail(
                    "WORKFLOW_PERF_NOT_PASSING",
                    f"performance workload is not passing: {workload_id}",
                    entry_id=workflow_id,
                    gate=gate,
                )


def _median(values: list[float]) -> float:
    return statistics.median(values)


def _geomean(values: list[float]) -> float:
    if not values or any(value <= 0 for value in values):
        raise ValueError("geometric mean requires positive values")
    return math.exp(sum(math.log(value) for value in values) / len(values))


def _check_performance(report: Report, inventory: dict[str, Any], root: Path) -> None:
    gate = "performance"
    performance = inventory.get("performance", {})
    policy = performance.get("policy", {})
    expected_policy = inventory_model.PERF_POLICY
    for key, expected in expected_policy.items():
        if policy.get(key) != expected:
            report.fail("PERF_POLICY_MISMATCH", f"{key} must be {expected!r}", gate=gate)
    if policy.get("minimum_speedup") != MIN_PERF_SPEEDUP:
        report.fail("PERF_THRESHOLD_LOWERED", "minimum speedup must be 3.0", gate=gate)
    if policy.get("max_per_workload_regression_fraction") != MAX_PERF_REGRESSION:
        report.fail("PERF_REGRESSION_RELAXED", "max regression must be 0.10", gate=gate)
    if policy.get("minimum_samples_per_side") != MIN_SAMPLES:
        report.fail("PERF_SAMPLES_LOWERED", "minimum samples per side must be 20", gate=gate)
    if policy.get("minimum_strict_workloads") != MIN_STRICT_WORKLOADS:
        report.fail("PERF_WORKLOAD_FLOOR_LOWERED", "minimum strict workloads must be 14", gate=gate)

    workload_rows = performance.get("workloads", [])
    if len({workload.get("id") for workload in workload_rows}) != len(workload_rows):
        report.fail("PERF_DUPLICATE_WORKLOAD", "performance workload list contains duplicate ids", gate=gate)
    workloads = {workload["id"]: workload for workload in workload_rows}
    missing = sorted(REQUIRED_PERF_WORKLOAD_IDS - set(workloads))
    if missing:
        report.fail_many(
            "PERF_WORKLOAD_MISSING",
            "required performance workload is absent",
            [(item, item) for item in missing],
            gate=gate,
        )
    extra_required = sorted(
        workload["id"]
        for workload in workloads.values()
        if workload.get("required") and workload["id"] not in REQUIRED_PERF_WORKLOAD_IDS
    )
    if extra_required:
        report.fail_many(
            "PERF_WORKLOAD_UNEXPECTED",
            "required performance workload is not in the frozen floor",
            [(item, item) for item in extra_required],
            gate=gate,
        )

    strict_speedups: list[float] = []
    strict_ids = sorted(REQUIRED_GATE_WORKLOADS)
    for workload_id in strict_ids:
        workload = workloads.get(workload_id)
        if not workload:
            continue
        if workload.get("status") != "pass":
            report.fail("PERF_WORKLOAD_NOT_PASSING", f"status={workload.get('status')}", entry_id=workload_id, gate=gate)
            continue
        if workload.get("strict_equivalence") is not True or workload.get("equivalence") != "strict":
            report.fail("PERF_NOT_STRICT", "gate workload is not strict-equivalence", entry_id=workload_id, gate=gate)
            continue
        if workload.get("result_equivalent") is not True:
            report.fail("PERF_RESULT_MISMATCH", "result equivalence is not true", entry_id=workload_id, gate=gate)
            continue
        samples = workload.get("samples") or {}
        python_samples = samples.get("python")
        rust_samples = samples.get("rust")
        if not isinstance(python_samples, list) or not isinstance(rust_samples, list):
            report.fail("PERF_SAMPLES_MISSING", "raw python/rust samples are missing", entry_id=workload_id, gate=gate)
            continue
        if len(python_samples) < MIN_SAMPLES or len(rust_samples) < MIN_SAMPLES:
            report.fail(
                "PERF_SAMPLES_TOO_FEW",
                f"python={len(python_samples)} rust={len(rust_samples)}; need >= {MIN_SAMPLES} each",
                entry_id=workload_id,
                gate=gate,
            )
            continue
        try:
            python_values = [float(value) for value in python_samples]
            rust_values = [float(value) for value in rust_samples]
        except (TypeError, ValueError):
            report.fail("PERF_SAMPLES_NOT_NUMERIC", "samples are not numeric", entry_id=workload_id, gate=gate)
            continue
        if any(value <= 0 for value in python_values + rust_values):
            report.fail("PERF_SAMPLES_NONPOSITIVE", "samples must be positive latencies", entry_id=workload_id, gate=gate)
            continue
        speedup = _median(python_values) / _median(rust_values)
        strict_speedups.append(speedup)
        recorded = workload.get("speedup")
        if not isinstance(recorded, (int, float)) or abs(float(recorded) - speedup) > max(0.05, speedup * 0.05):
            report.fail(
                "PERF_SPEEDUP_MISMATCH",
                f"recorded speedup={recorded!r} but samples give {speedup:.4f}",
                entry_id=workload_id,
                gate=gate,
            )
        if speedup < 1.0 - MAX_PERF_REGRESSION:
            report.fail(
                "PERF_WORKLOAD_REGRESSION",
                f"speedup={speedup:.4f} is more than {MAX_PERF_REGRESSION:.0%} slower than Python",
                entry_id=workload_id,
                gate=gate,
            )
        if not workload.get("evidence_refs"):
            report.fail("PERF_NO_EVIDENCE", "workload has no raw evidence reference", entry_id=workload_id, gate=gate)
        for ref in workload.get("evidence_refs", []):
            if not _ref_exists(ref):
                report.fail("PERF_EVIDENCE_MISSING", f"evidence reference does not exist: {ref}", entry_id=workload_id, gate=gate)
        for field_name in ("fixture_id", "command_argv", "environment"):
            if not workload.get(field_name):
                report.fail("PERF_WORKLOAD_METADATA", f"missing {field_name}", entry_id=workload_id, gate=gate)

    if len(strict_speedups) < MIN_STRICT_WORKLOADS:
        report.fail(
            "PERF_INSUFFICIENT_WORKLOADS",
            f"{len(strict_speedups)} strict workloads have samples; need {MIN_STRICT_WORKLOADS}",
            gate=gate,
        )
    aggregate = performance.get("aggregate", {})
    if strict_speedups:
        measured = _geomean(strict_speedups)
        reported = aggregate.get("geometric_mean_speedup")
        if not isinstance(reported, (int, float)):
            report.fail("PERF_AGGREGATE_MISSING", "aggregate geometric mean is not recorded", gate=gate)
        elif abs(float(reported) - measured) > max(0.02, measured * 0.02):
            report.fail(
                "PERF_AGGREGATE_MISMATCH",
                f"reported={reported} measured={measured:.4f}",
                gate=gate,
            )
        if measured < MIN_PERF_SPEEDUP:
            report.fail(
                "PERF_TARGET_NOT_MET",
                f"geometric mean {measured:.4f}x is below {MIN_PERF_SPEEDUP}x",
                gate=gate,
            )
        if aggregate.get("status") != "pass":
            report.fail("PERF_AGGREGATE_NOT_PASSING", f"status={aggregate.get('status')}", gate=gate)
        if aggregate.get("sample_count", 0) < MIN_STRICT_WORKLOADS * MIN_SAMPLES:
            report.fail("PERF_SAMPLE_COUNT", "aggregate sample count is below the floor", gate=gate)
    else:
        report.fail("PERF_NO_STRICT_SAMPLES", "no strict-equivalence gate workload has samples", gate=gate)

    # Startup must be measured separately and must not substitute.
    startup = performance.get("startup", {})
    if startup.get("excluded_from_gate") is not True:
        report.fail("PERF_STARTUP_IN_GATE", "startup must be excluded from the 3x gate", gate=gate)
    if set(startup.get("workload_ids", [])) != REQUIRED_STARTUP_WORKLOADS:
        report.fail("PERF_STARTUP_SET", "startup workload set is not the frozen pair", gate=gate)
    if startup.get("status") != "pass":
        report.fail("PERF_STARTUP_NOT_MEASURED", f"startup status={startup.get('status')}", gate=gate)
    if not startup.get("measurements"):
        report.fail("PERF_STARTUP_NO_SAMPLES", "startup measurements are missing", gate=gate)
    for workload_id in REQUIRED_STARTUP_WORKLOADS:
        workload = workloads.get(workload_id)
        if not workload or workload.get("status") != "pass":
            report.fail("PERF_STARTUP_WORKLOAD", f"startup workload not passing: {workload_id}", gate=gate)

    # Full evidence bundle: raw samples must be reloadable and match.
    evidence = performance.get("aggregate", {}).get("evidence")
    if not evidence:
        report.fail("PERF_EVIDENCE_BUNDLE_MISSING", "no evidence bundle path/hash is recorded", gate=gate)
    else:
        path = _ref_path(evidence.get("path", ""))
        if not path.exists():
            report.fail("PERF_EVIDENCE_BUNDLE_MISSING", f"evidence file does not exist: {path}", gate=gate)
        elif evidence.get("sha256") != _sha256(path):
            report.fail("PERF_EVIDENCE_HASH", "evidence bundle hash does not match", gate=gate)
        else:
            try:
                bundle = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                report.fail("PERF_EVIDENCE_UNREADABLE", "evidence bundle is not valid JSON", gate=gate)
            else:
                bundle_ids = {item.get("id") for item in bundle.get("workloads", [])}
                if not REQUIRED_GATE_WORKLOADS.issubset(bundle_ids):
                    report.fail("PERF_EVIDENCE_INCOMPLETE", "evidence bundle omits required gate workloads", gate=gate)
                if bundle.get("baseline", {}).get("commit") != performance.get("baseline", {}).get("commit"):
                    report.fail("PERF_EVIDENCE_BASELINE", "evidence baseline does not match the recorded baseline", gate=gate)
                if bundle.get("candidate", {}).get("commit") != performance.get("candidate", {}).get("commit"):
                    report.fail("PERF_EVIDENCE_CANDIDATE", "evidence candidate does not match the recorded candidate", gate=gate)

    candidate = performance.get("candidate", {}).get("commit")
    if not isinstance(candidate, str) or not re.fullmatch(r"[0-9a-f]{40}", candidate):
        report.fail("PERF_CANDIDATE_COMMIT", "candidate commit is not a 40-hex commit id", gate=gate)
    baseline = performance.get("baseline", {}).get("commit")
    if not isinstance(baseline, str) or not re.fullmatch(r"[0-9a-f]{40}", baseline):
        report.fail("PERF_BASELINE_COMMIT", "baseline commit is not a 40-hex commit id", gate=gate)
    if performance.get("status") != "pass":
        report.fail("PERF_STATUS_NOT_PASS", f"performance status={performance.get('status')}", gate=gate)


def _check_dependencies(report: Report, inventory: dict[str, Any], root: Path) -> None:
    gate = "dependencies"
    dependencies = inventory.get("dependencies", {})
    expected_python = build_inventory._parse_uv_lock(root)
    if dependencies.get("python") != expected_python:
        report.fail("DEPENDENCY_DRIFT", "recorded Python dependency pins differ from uv.lock", gate=gate)
    expected_oracle = build_inventory._parse_oracle_versions(expected_python)
    recorded_oracle = dependencies.get("oracle")
    if recorded_oracle != expected_oracle:
        report.fail("ORACLE_ENVIRONMENT_DRIFT", "recorded oracle package versions differ from the live interpreter", gate=gate)
    for mismatch in (recorded_oracle or {}).get("lock_mismatches", []):
        if mismatch.get("wire_contract"):
            report.fail(
                "ORACLE_WIRE_MISMATCH",
                f"{mismatch['name']}: lock={mismatch['lock_version']} installed={mismatch['installed_version']}",
                gate=gate,
            )
        else:
            report.warn(
                "ORACLE_LOCK_MISMATCH",
                f"{mismatch['name']}: lock={mismatch['lock_version']} installed={mismatch['installed_version']}",
                gate=gate,
            )
    expected_services = build_inventory._parse_pins(root)
    if dependencies.get("services") != expected_services:
        report.fail("SERVICE_PIN_DRIFT", "recorded service pins differ from pins/*.source.yaml", gate=gate)
    if dependencies.get("rust") != build_inventory._toolchain(root):
        report.fail("RUST_TOOLCHAIN_DRIFT", "recorded Rust toolchain differs from rust-toolchain.toml", gate=gate)
    versions = {row["name"]: row["version"] for row in dependencies.get("python", [])}
    if versions.get("spec-kitty-events") is None:
        report.fail("EVENTS_PIN_MISSING", "spec-kitty-events pin is absent", gate=gate)
    if versions.get("spec-kitty-tracker") is None:
        report.fail("TRACKER_PIN_MISSING", "spec-kitty-tracker pin is absent", gate=gate)
    if dependencies.get("rust", {}).get("channel") is None:
        report.fail("RUST_CHANNEL_MISSING", "Rust toolchain channel is not recorded", gate=gate)


def _check_no_python_runtime(report: Report, root: Path) -> None:
    gate = "native"
    crates = root / "crates"
    if not crates.is_dir():
        report.fail("CRATES_MISSING", "crates/ is missing", gate=gate)
        return
    python_invocation = re.compile(r'Command::new\(\s*"(?:python|python3|py|spec-kitty|spec_kitty)"')
    for path in sorted(crates.rglob("*.rs")):
        if "target" in path.parts:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        if python_invocation.search(text):
            report.fail("PYTHON_RUNTIME_INVOCATION", f"Rust production source invokes Python: {path.relative_to(root)}", gate=gate)
    for manifest in [root / "Cargo.toml", *sorted(crates.glob("*/Cargo.toml"))]:
        text = manifest.read_text(encoding="utf-8", errors="replace")
        if re.search(r"\bpyo3\b", text):
            report.fail("PYO3_DEPENDENCY", f"Python FFI dependency declared in {manifest.relative_to(root)}", gate=gate)
    lock = root / "Cargo.lock"
    if lock.exists() and re.search(r'^name = "pyo3"', lock.read_text(encoding="utf-8"), re.MULTILINE):
        report.fail("PYO3_LOCKED", "Cargo.lock contains pyo3", gate=gate)
    extensions = inventory_model.NATIVE_EXTENSIONS
    if len(extensions) != 11:
        report.fail("NATIVE_EXTENSION_CENSUS", f"expected 11 native foundation commands, found {len(extensions)}", gate=gate)
    for extension in extensions:
        for ref in extension.get("native_refs", []):
            if not _ref_exists(ref):
                report.fail("NATIVE_EXTENSION_REF", f"native extension reference does not exist: {ref}", entry_id=extension["id"], gate=gate)


def _check_coverage_accounting(report: Report, inventory: dict[str, Any]) -> None:
    gate = "accounting"
    entries = inventory.get("entries", [])
    required = [entry for entry in entries if entry.get("required")]
    implemented = [entry for entry in required if entry.get("implementation", {}).get("status") == "implemented"]
    if inventory.get("counts", {}).get("entries_total") != len(entries):
        report.fail("COUNT_TOTAL", "counts.entries_total is stale", gate=gate)
    if inventory.get("counts", {}).get("entries_required") != len(required):
        report.fail("COUNT_REQUIRED", "counts.entries_required is stale", gate=gate)
    if inventory.get("counts", {}).get("by_implementation", {}).get("implemented") != len(implemented):
        report.fail("COUNT_IMPLEMENTED", "counts.by_implementation.implemented is stale", gate=gate)
    if inventory.get("counts", {}).get("unresolved_contracts", 0) != 0:
        report.fail("UNRESOLVED_COUNT", "unresolved contracts remain", gate=gate)
    if inventory.get("counts", {}).get("mutating_entries") != sum(1 for entry in entries if entry.get("mutates")):
        report.fail("COUNT_MUTATING", "counts.mutating_entries is stale", gate=gate)
    # Every required workflow and strict workload must be represented.
    if len(inventory.get("workflows", [])) < len(REQUIRED_WORKFLOW_IDS):
        report.fail("WORKFLOW_COUNT", "workflow count is below the frozen floor", gate=gate)
    if len(inventory.get("performance", {}).get("workloads", [])) < len(REQUIRED_PERF_WORKLOAD_IDS):
        report.fail("PERF_COUNT", "performance workload count is below the frozen floor", gate=gate)


def check(
    inventory: dict[str, Any],
    surface: dict[str, Any],
    root: Path = ROOT,
    *,
    live_surface: dict[str, Any] | None = None,
    max_detail: int = 40,
) -> Report:
    report = Report(max_detail=max_detail)
    if live_surface is not None:
        if live_surface.get("baseline", {}).get("surface_sha256") != surface.get("baseline", {}).get("surface_sha256"):
            report.fail("STALE_SURFACE", "committed surface differs from live discovery", gate="surface")
        if live_surface.get("counts") != surface.get("counts"):
            report.fail("SURFACE_COUNT_MISMATCH", "committed surface counts differ from live discovery", gate="surface")
        surface = live_surface
    # Source hash check: a changed Python surface invalidates the evidence.
    try:
        source_sha = _source_surface_sha(root)
    except OSError as exc:
        report.fail("SOURCE_HASH_FAILED", f"cannot hash pinned source: {exc}", gate="surface")
        source_sha = None
    if source_sha and inventory.get("baseline", {}).get("surface_sha256") != source_sha:
        report.fail("STALE_SURFACE", "inventory source hash does not match the pinned source", gate="surface")
    if source_sha and surface.get("baseline", {}).get("surface_sha256") != source_sha:
        report.fail("STALE_SURFACE", "surface source hash does not match the pinned source", gate="surface")

    _check_surface_structure(report, inventory, surface)
    _check_structural_rebuild(report, inventory, surface)
    _check_modules(report, inventory, surface)
    _check_contracts(report, inventory)
    _check_migrations(report, inventory, surface)
    _check_status_gates(report, inventory)
    _check_workflows(report, inventory)
    _check_performance(report, inventory, root)
    _check_dependencies(report, inventory, root)
    _check_no_python_runtime(report, root)
    _check_coverage_accounting(report, inventory)

    report.gates = {
        "surface": not any(item["gate"] == "surface" for item in report.errors),
        "structure": not any(item["gate"] == "structure" for item in report.errors),
        "modules": not any(item["gate"] == "modules" for item in report.errors),
        "contracts": not any(item["gate"] == "contracts" for item in report.errors),
        "migrations": not any(item["gate"] == "migrations" for item in report.errors),
        "status": not any(item["gate"] == "status" for item in report.errors),
        "workflows": not any(item["gate"] == "workflows" for item in report.errors),
        "performance": not any(item["gate"] == "performance" for item in report.errors),
        "dependencies": not any(item["gate"] == "dependencies" for item in report.errors),
        "native": not any(item["gate"] == "native" for item in report.errors),
        "accounting": not any(item["gate"] == "accounting" for item in report.errors),
    }
    required = [entry for entry in inventory.get("entries", []) if entry.get("required")]
    report.summary = {
        "ok": not report.errors,
        "entries_total": len(inventory.get("entries", [])),
        "entries_required": len(required),
        "entries_implemented": sum(
            1 for entry in required if entry.get("implementation", {}).get("status") == "implemented"
        ),
        "entries_conformant": sum(
            1 for entry in required if entry.get("conformance", {}).get("status") == "pass"
        ),
        "entries_reviewed": sum(
            1 for entry in required if entry.get("review", {}).get("status") == "approved"
        ),
        "workflows_passing": sum(
            1 for workflow in inventory.get("workflows", []) if workflow.get("status") == "pass"
        ),
        "perf_status": inventory.get("performance", {}).get("status"),
        "error_count": len(report.errors),
        "warning_count": len(report.warnings),
        "gates": report.gates,
    }
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path, default=DEFAULT_INVENTORY)
    parser.add_argument("--surface", type=Path, default=DEFAULT_SURFACE)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--json-report", type=Path, help="write the full report as JSON")
    parser.add_argument("--max-detail", type=int, default=40)
    args = parser.parse_args(argv)
    root = args.root.resolve()
    inventory_path = args.inventory if args.inventory.is_absolute() else root / args.inventory
    surface_path = args.surface if args.surface.is_absolute() else root / args.surface
    try:
        inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
        surface = json.loads(surface_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        print(f"checker setup failed: {exc}", file=sys.stderr)
        return 2

    try:
        import discover_python_surface  # noqa: PLC0415

        live = discover_python_surface.discover(root)
    except Exception as exc:  # noqa: BLE001 - fail closed
        print(f"checker setup failed: live discovery unavailable: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2

    report = check(inventory, surface, root, live_surface=live, max_detail=args.max_detail)
    payload = {
        "schema_version": "1.0.0",
        "summary": report.summary,
        "gates": report.gates,
        "errors": report.errors,
        "warnings": report.warnings,
    }
    if args.json_report:
        path = args.json_report if args.json_report.is_absolute() else root / args.json_report
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    print("completion gate: " + ("PASS" if report.summary["ok"] else "FAIL"))
    print(
        "required entries: "
        f"{report.summary['entries_required']} | implemented {report.summary['entries_implemented']} | "
        f"conformant {report.summary['entries_conformant']} | reviewed {report.summary['entries_reviewed']}"
    )
    print(
        f"workflows passing: {report.summary['workflows_passing']}/{len(REQUIRED_WORKFLOW_IDS)} | "
        f"performance: {report.summary['perf_status']}"
    )
    print("gates: " + ", ".join(f"{name}={'ok' if ok else 'FAIL'}" for name, ok in report.gates.items()))
    if report.errors:
        print(f"blocking findings: {len(report.errors)} (first {min(len(report.errors), args.max_detail)} shown)")
        for error in report.errors[: args.max_detail]:
            suffix = f" [{error['entry_id']}]" if error.get("entry_id") else ""
            print(f"  - {error['code']}{suffix}: {error['message']}")
        if len(report.errors) > args.max_detail:
            print(f"  ... {len(report.errors) - args.max_detail} more")
    return 0 if report.summary["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
