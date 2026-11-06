#!/usr/bin/env python3
"""Discover the reachable upstream CLI surface from the pinned Python source.

Development-only tooling for the Rust rewrite inventory.  It does not run a
command: it imports the baseline Typer application with the oracle interpreter
and walks the *complete* Click/Typer command tree, including hidden commands,
groups, option aliases (`--x/--no-x`), injected compatibility flags and
deprecated spellings.

The output is deterministic JSON (sorted keys, sorted collections, no clocks)
so it can be diffed in review and hashed by the completion checker.

Run with the oracle interpreter, from anywhere:

    PYTHONPATH=<repo>/src <oracle-python> scripts/rust/discover_python_surface.py \
        --output docs/rust/inventory/python-surface.json

The script isolates HOME/XDG/cache into a temporary directory and forbids
bytecode writes: importing the app must not mutate the checkout or the user's
global `~/.kittify` state.  It resolves the built-in pack tree from the
installed oracle tool environment (or SPEC_KITTY_PACKS_ROOT if already set);
nothing is fetched from the network.

Exit codes: 0 discovery written, 2 discovery failed or invariant violated.
"""
from __future__ import annotations

import argparse
import ast
import atexit
import hashlib
import inspect
import json
import os
import re
import shutil
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

# Development tooling must not leave bytecode in the checkout.
sys.dont_write_bytecode = True
from typing import Any

COMMAND_MODULES_REL = Path("src/specify_cli/cli/commands")
ROOT_MODULE_REL = Path("src/specify_cli/__init__.py")
INSIDE_CHECKOUT_MARKER = "src/specify_cli/__init__.py"

#: Typer/Click parameter types whose repr is stable and meaningful.
_SIMPLE_TYPES = {
    "StringParamType": "string",
    "IntParamType": "integer",
    "FloatParamType": "number",
    "BoolParamType": "boolean",
    "Path": "path",
    "File": "file",
    "Choice": "choice",
    "Tuple": "tuple",
    "DateTime": "datetime",
    "UUIDParameterType": "uuid",
}


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _tree_digest(paths: list[Path], root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(_sha256_file(path).encode())
        digest.update(b"\n")
    return digest.hexdigest()


def _source_ref(value: Any, root: Path) -> str | None:
    """Return a stable ``repo-relative:line`` reference for a callable."""
    if value is None:
        return None
    target = inspect.unwrap(value) if callable(value) else value
    if not callable(target):
        return None
    try:
        filename = inspect.getsourcefile(target)
        _, lineno = inspect.getsourcelines(target)
    except (OSError, TypeError):
        code = getattr(target, "__code__", None)
        filename = getattr(code, "co_filename", None)
        lineno = getattr(code, "co_firstlineno", None)
    if not filename:
        return None
    try:
        relative = Path(filename).resolve().relative_to(root).as_posix()
    except ValueError:
        relative = str(Path(filename).resolve())
    return f"{relative}:{lineno}" if lineno else relative


def _json_safe(value: Any) -> Any:
    """Convert a Click default to a deterministic JSON-safe value."""
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, (list, tuple, set, frozenset)):
        return [_json_safe(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _json_safe(item) for key, item in sorted(value.items())}
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, os.PathLike):
        return os.fspath(value)
    # Enum-like defaults: keep the stable member name.
    member = getattr(value, "name", None)
    if isinstance(member, str):
        return f"<{type(value).__name__}.{member}>"
    return f"<{type(value).__name__}>"


def _param_type(param: Any) -> dict[str, Any]:
    click_type = getattr(param, "type", None)
    name = type(click_type).__name__ if click_type is not None else "unknown"
    result: dict[str, Any] = {
        "click_type": name,
        "json_type": _SIMPLE_TYPES.get(name, "object"),
    }
    choices = getattr(click_type, "choices", None)
    if choices is not None:
        result["choices"] = sorted(str(choice) for choice in choices)
    name_attr = getattr(click_type, "name", None)
    if isinstance(name_attr, str) and name_attr not in {"text", "string"}:
        result["name"] = name_attr
    return result


def _param_record(param: Any, root: Path, *, injected: bool = False) -> dict[str, Any]:
    flags = list(getattr(param, "opts", []) or [])
    secondary = list(getattr(param, "secondary_opts", []) or [])
    help_text = getattr(param, "help", None)
    record = {
        "name": getattr(param, "name", None),
        "flags": flags,
        "secondary_flags": secondary,
        "type": _param_type(param),
        "required": bool(getattr(param, "required", False)),
        "default": _json_safe(getattr(param, "default", None)),
        "multiple": bool(getattr(param, "multiple", False)),
        "nargs": int(getattr(param, "nargs", 1) or 1),
        "is_flag": bool(getattr(param, "is_flag", False)),
        "flag_value": _json_safe(getattr(param, "flag_value", None)),
        "count": bool(getattr(param, "count", False)),
        "hidden": bool(getattr(param, "hidden", False)),
        "help": help_text,
        "envvar": _json_safe(getattr(param, "envvar", None)),
        "expose_value": bool(getattr(param, "expose_value", True)),
        "metavar": getattr(param, "metavar", None),
        "source_ref": _source_ref(getattr(param, "callback", None), root),
        "injected": injected,
        "deprecated": _deprecated_from_help(help_text),
        "alias_for": _alias_target_from_help(help_text),
    }
    if secondary:
        record["option_aliases"] = [*flags, *secondary]
    return record


def _deprecated_from_help(help_text: str | None) -> bool:
    if not help_text:
        return False
    lowered = help_text.lower()
    return "deprecated" in lowered or "legacy alias" in lowered


def _alias_target_from_help(help_text: str | None) -> str | None:
    if not help_text:
        return None
    match = re.search(r"(?:deprecated alias for|use)\s+(--[A-Za-z0-9-]+)", help_text, re.IGNORECASE)
    return match.group(1) if match else None


def _command_record(command: Any, path: list[str], root: Path) -> dict[str, Any]:
    callback = getattr(command, "callback", None)
    params = list(command.params)
    options = []
    arguments = []
    for param in params:
        is_argument = getattr(param, "param_type_name", None) == "argument"
        # Click synthesises the mission-agnostic flag with name "mission" on a
        # class outside the command's own module; detect it structurally.
        injected = (
            not is_argument
            and list(getattr(param, "opts", []) or []) == ["--mission"]
            and getattr(param, "hidden", False)
            and not getattr(param, "required", False)
            and getattr(param, "default", None) is None
            and _source_ref(getattr(param, "callback", None), root) is None
        )
        record = _param_record(param, root, injected=injected)
        (arguments if is_argument else options).append(record)
    help_text = getattr(command, "help", None)
    return {
        "id": "CLI:" + "/".join(path),
        "path": path,
        "argv": ["spec-kitty", *path],
        "kind": "leaf" if not getattr(command, "commands", None) else "group_leaf",
        "hidden": bool(getattr(command, "hidden", False)),
        "deprecated": bool(getattr(command, "deprecated", False)),
        "help": help_text,
        "short_help": getattr(command, "short_help", None),
        "epilog": getattr(command, "epilog", None),
        "no_args_is_help": bool(getattr(command, "no_args_is_help", False)),
        "allow_extra_args": bool(getattr(command, "allow_extra_args", False)),
        "ignore_unknown_options": bool(getattr(command, "ignore_unknown_options", False)),
        "context_settings": _json_safe(dict(getattr(command, "context_settings", {}) or {})),
        "callback": {
            "qualname": getattr(callback, "__qualname__", None),
            "module": getattr(callback, "__module__", None),
            "source_ref": _source_ref(callback, root),
        },
        "options": sorted(options, key=lambda item: (item["name"] or "", item["flags"])),
        "arguments": sorted(arguments, key=lambda item: item["name"] or ""),
        "accepts_json": any("--json" in option["flags"] for option in options),
        "deprecated_flags": [
            {
                "flag": flag,
                "canonical": option["alias_for"],
                "help": option["help"],
            }
            for option in options
            for flag in option["flags"] + option["secondary_flags"]
            if option["deprecated"] or option["alias_for"]
        ],
    }


def _group_record(group: Any, path: list[str], root: Path, descendant_refs: list[str]) -> dict[str, Any]:
    callback = getattr(group, "callback", None)
    source_refs = sorted({ref for ref in descendant_refs if ref})
    own_ref = _source_ref(callback, root)
    if own_ref:
        source_refs.insert(0, own_ref)
    options = [
        _param_record(param, root)
        for param in group.params
        if getattr(param, "opts", None)
    ]
    return {
        "id": "CLI:" + "/".join(path) if path else "CLI:",
        "path": path,
        "argv": ["spec-kitty", *path],
        "kind": "group",
        "hidden": bool(getattr(group, "hidden", False)),
        "deprecated": bool(getattr(group, "deprecated", False)),
        "help": getattr(group, "help", None),
        "short_help": getattr(group, "short_help", None),
        "no_args_is_help": bool(getattr(group, "no_args_is_help", False)),
        "invoke_without_command": bool(getattr(group, "invoke_without_command", False)),
        "context_settings": _json_safe(dict(getattr(group, "context_settings", {}) or {})),
        "callback": {
            "qualname": getattr(callback, "__qualname__", None),
            "module": getattr(callback, "__module__", None),
            "source_ref": own_ref,
        },
        "options": sorted(options, key=lambda item: (item["name"] or "", item["flags"])),
        "arguments": [],
        "accepts_json": any("--json" in option["flags"] for option in options),
        "child_count": len(getattr(group, "commands", {}) or {}),
        "source_refs": source_refs,
    }


def _module_inventory(root: Path) -> list[dict[str, Any]]:
    modules_root = root / COMMAND_MODULES_REL
    return [
        {
            "module": path.relative_to(root).as_posix(),
            "public": not path.name.startswith("_"),
            "sha256": _sha256_file(path),
        }
        for path in sorted(modules_root.rglob("*.py"))
        if "__pycache__" not in path.parts
    ]


def _module_name(path: Path, root: Path) -> str:
    relative = path.relative_to(root / "src").with_suffix("")
    parts = list(relative.parts)
    if parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts)


def _resolve_import(current: str, node: ast.ImportFrom) -> list[str]:
    if node.level:
        base = current.split(".")
        base = base[: max(1, len(base) - node.level + (0 if current.endswith(".__init__") else 1))]
        prefix = ".".join(base)
        module = f"{prefix}.{node.module}" if node.module else prefix
    else:
        module = node.module or ""
    targets = [module] if module else []
    if node.module is None:
        # ``from . import accept, agent`` imports sibling modules by name.
        targets.extend(f"{module}.{alias.name}" if module else alias.name for alias in node.names)
    return targets


def _import_graph(root: Path) -> dict[str, list[str]]:
    """Map each command module to command-package modules it imports."""
    modules_root = root / COMMAND_MODULES_REL
    graph: dict[str, list[str]] = {}
    for path in sorted(modules_root.rglob("*.py")):
        if "__pycache__" in path.parts:
            continue
        name = _module_name(path, root)
        targets: list[str] = []
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"))
        except (OSError, SyntaxError):
            graph[name] = targets
            continue
        for node in ast.walk(tree):
            if isinstance(node, ast.ImportFrom):
                targets.extend(_resolve_import(name, node))
            elif isinstance(node, ast.Import):
                targets.extend(alias.name for alias in node.names)
        package_prefix = _module_name(modules_root, root)
        graph[name] = sorted(
            target
            for target in set(targets)
            if target == package_prefix or target.startswith(package_prefix + ".")
        )
    return graph


def _registration_summary(root: Path) -> dict[str, dict[str, int]]:
    """Count Typer registration calls per command module (static evidence)."""
    modules_root = root / COMMAND_MODULES_REL
    summary: dict[str, dict[str, int]] = {}
    for path in sorted(modules_root.rglob("*.py")):
        if "__pycache__" in path.parts:
            continue
        name = _module_name(path, root)
        commands = 0
        typers = 0
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"))
        except (OSError, SyntaxError):
            summary[name] = {"commands": 0, "typers": 0}
            continue
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            attr = func.attr if isinstance(func, ast.Attribute) else None
            if attr == "command":
                commands += 1
            elif attr == "add_typer":
                typers += 1
        summary[name] = {"commands": commands, "typers": typers}
    return summary


def _import_reachable(root: Path, registrars: list[str]) -> set[str]:
    graph = _import_graph(root)
    package_prefix = _module_name(root / COMMAND_MODULES_REL, root)
    reachable: set[str] = set()
    queue = list(registrars)
    while queue:
        module = queue.pop()
        if module in reachable:
            continue
        reachable.add(module)
        for target in graph.get(module, []):
            if target not in reachable:
                queue.append(target)
    # Modules below a reachable package are reachable only when imported
    # explicitly; do not infer them from the package itself.
    return {name for name in reachable if name.startswith(package_prefix)}


def _test_refs(root: Path, paths: list[list[str]]) -> dict[str, Any]:
    """Index test files by literal occurrences of each leaf name.

    Advisory only: a hit is a candidate reader, not proof the command is
    covered.  Conformance requires an explicit passing test in the inventory.
    """
    tests_root = root / "tests"
    if not tests_root.is_dir():
        return {}
    files: list[tuple[str, str]] = []
    for path in sorted(tests_root.rglob("*.py")):
        if "__pycache__" in path.parts:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        files.append((path.relative_to(root).as_posix(), text))
    index: dict[str, Any] = {}
    for path in paths:
        leaf = path[-1]
        joined = " ".join(path)
        patterns = [f'spec-kitty {joined}', f'"{" ".join(path)}"', f"'{joined}'"]
        if len(path) >= 2:
            patterns.extend([f'"{leaf}"', f"'{leaf}'"])
        matches = sorted(
            name
            for name, text in files
            if any(pattern in text for pattern in patterns)
        )
        index["/".join(path)] = {
            "method": "test_literal_index",
            "count": len(matches),
            "refs": matches[:12],
        }
    return index


@dataclass
class Discovered:
    root: Path
    app: Any


def _load_app(root: Path) -> Any:
    # Import-time isolation: HOME/XDG never point at the real user profile.
    isolated = Path(tempfile.mkdtemp(prefix="sk-surface-"))
    atexit.register(shutil.rmtree, isolated, ignore_errors=True)
    os.environ["HOME"] = str(isolated)
    os.environ["XDG_CONFIG_HOME"] = str(isolated / ".config")
    os.environ["XDG_DATA_HOME"] = str(isolated / ".local" / "share")
    os.environ["XDG_CACHE_HOME"] = str(isolated / ".cache")
    os.environ.setdefault("SPEC_KITTY_NO_UPGRADE_CHECK", "1")
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    os.environ.setdefault("NO_COLOR", "1")
    os.environ.setdefault("TERM", "dumb")
    # Resolve the installed pack tree from the oracle environment when the
    # caller has not supplied SPEC_KITTY_PACKS_ROOT.  The repo checkout pins the
    # pack source but does not vendor the tree; the oracle tool environment does.
    if not os.environ.get("SPEC_KITTY_PACKS_ROOT"):
        for entry in sys.path:
            candidate = Path(entry) / "packs"
            if (candidate / "built-in").is_dir():
                os.environ["SPEC_KITTY_PACKS_ROOT"] = str(candidate)
                break
    # Work from an empty directory so no project is discovered by accident.
    workdir = isolated / "cwd"
    workdir.mkdir()
    os.chdir(workdir)
    sys.path.insert(0, str(root / "src"))

    import specify_cli  # noqa: PLC0415
    from typer.main import get_command  # noqa: PLC0415

    return get_command(specify_cli.app)


def _completion_manifest_leaves(root: Path) -> list[list[str]]:
    path = root / "src/specify_cli/_completion_manifest.json"
    if not path.is_file():
        return []
    data = json.loads(path.read_text(encoding="utf-8"))
    leaves: list[list[str]] = []

    def walk(node: dict[str, Any], prefix: list[str]) -> None:
        commands = node.get("commands")
        if not commands:
            leaves.append(prefix)
            return
        for name, child in commands.items():
            walk(child, [*prefix, name])

    walk(data, [])
    return sorted(leaves)


def discover_migrations(root: Path) -> list[dict[str, Any]]:
    """Load every registered upgrade migration from the pinned source.

    The registry (not a filename count) is authoritative: helper modules in the
    migrations package are not migrations, and a registration is what the
    upgrade runner actually executes.
    """
    from specify_cli.upgrade.migrations import auto_discover_migrations  # noqa: PLC0415
    from specify_cli.upgrade.registry import MigrationRegistry  # noqa: PLC0415

    auto_discover_migrations()
    rows: list[dict[str, Any]] = []
    for migration in MigrationRegistry.get_all():
        cls = type(migration)
        rows.append(
            {
                "id": migration.migration_id,
                "target_version": migration.target_version,
                "description": migration.description,
                "class": cls.__name__,
                "source_ref": _source_ref(cls, root),
                "runs_on_worktrees": bool(getattr(migration, "runs_on_worktrees", False)),
                "clone_local_effect": bool(getattr(migration, "clone_local_effect", False)),
            }
        )
    return sorted(rows, key=lambda row: (row["target_version"], row["id"]))


def discover(root: Path) -> dict[str, Any]:
    if not (root / INSIDE_CHECKOUT_MARKER).is_file():
        raise SystemExit(f"not a Spec Kitty checkout: {root}")
    app = _load_app(root)
    groups: list[dict[str, Any]] = []
    leaves: list[dict[str, Any]] = []

    def walk(group: Any, path: list[str]) -> list[str]:
        commands = list((getattr(group, "commands", {}) or {}).items())
        if not path:
            groups.append(
                _group_record(
                    group,
                    [],
                    root,
                    [
                        ref
                        for child in commands
                        for ref in _walk_refs(child[1], path + [child[0]])
                    ],
                )
            )
        refs: list[str] = []
        for name, command in sorted(commands):
            child_path = [*path, name]
            child_refs = _walk_refs(command, child_path)
            refs.extend(child_refs)
            if getattr(command, "commands", None):
                groups.append(_group_record(command, child_path, root, child_refs))
                refs.extend(walk(command, child_path))
            else:
                leaves.append(_command_record(command, child_path, root))
        return refs

    def _walk_refs(command: Any, path: list[str]) -> list[str]:
        if getattr(command, "commands", None):
            refs = []
            for _, child in sorted((command.commands or {}).items()):
                refs.extend(_walk_refs(child, path))
            return refs
        callback = getattr(command, "callback", None)
        ref = _source_ref(callback, root)
        return [ref] if ref else []

    walk(app, [])

    # Shared callbacks expose deprecated aliases registered as separate names.
    by_callback: dict[str, list[str]] = {}
    for leaf in leaves:
        module = leaf["callback"]["module"] or ""
        qualname = leaf["callback"]["qualname"]
        if qualname:
            by_callback.setdefault(f"{module}:{qualname}", []).append("/".join(leaf["path"]))
    shared = {
        key: sorted(paths)
        for key, paths in by_callback.items()
        if len(paths) > 1
    }
    for leaf in leaves:
        module = leaf["callback"]["module"] or ""
        qualname = leaf["callback"]["qualname"]
        key = f"{module}:{qualname}"
        siblings = [path for path in shared.get(key, []) if path != "/".join(leaf["path"])]
        leaf["alias_of"] = siblings[0] if siblings else None
        leaf["shared_callback"] = siblings

    modules = _module_inventory(root)
    referenced_modules = set()
    for leaf in leaves:
        ref = (leaf["callback"]["source_ref"] or "").split(":", 1)[0]
        if ref:
            referenced_modules.add(ref)
    for group in groups:
        for ref in group["source_refs"]:
            referenced_modules.add(ref.split(":", 1)[0])

    def module_reachable(module: str) -> bool:
        return any(
            ref == module or ref.startswith(module.removesuffix(".py") + "/")
            for ref in referenced_modules
        )

    registrations = _registration_summary(root)
    registrar_roots = [
        "specify_cli.cli.commands",
        "specify_cli.cli.commands.init",
    ]
    import_reachable = _import_reachable(root, registrar_roots)
    for module in modules:
        name = _module_name(root / module["module"], root)
        module["reachable_command_source"] = module_reachable(module["module"])
        module["import_reachable"] = name in import_reachable
        module["registrations"] = registrations.get(name, {"commands": 0, "typers": 0})

    surface_files = [root / ROOT_MODULE_REL, *(root / COMMAND_MODULES_REL).rglob("*.py")]
    surface_files = sorted(
        path for path in surface_files if "__pycache__" not in path.parts
    )

    test_index = _test_refs(root, sorted([leaf["path"] for leaf in leaves], key=lambda p: p))
    for leaf in leaves:
        leaf["test_refs"] = test_index.get("/".join(leaf["path"]), {"count": 0, "refs": []})

    migrations = discover_migrations(root)
    manifest_leaves = _completion_manifest_leaves(root)
    surface_leaf_paths = sorted([leaf["path"] for leaf in leaves], key=lambda item: item)
    manifest_set = {tuple(path) for path in manifest_leaves}
    surface_set = {tuple(path) for path in surface_leaf_paths}

    import click  # noqa: PLC0415
    import typer  # noqa: PLC0415

    payload = {
        "schema_version": "1.0.0",
        "baseline": {
            "root_marker": INSIDE_CHECKOUT_MARKER,
            "surface_sha256": _tree_digest(surface_files, root),
            "module_count": len(modules),
        },
        "environment": {
            "sys_version": sys.version.split()[0],
            "packs_root": os.environ.get("SPEC_KITTY_PACKS_ROOT"),
            "typer_version": getattr(typer, "__version__", "unknown"),
            "click_version": getattr(click, "__version__", "unknown"),
        },
        "counts": {
            "top_level_commands": len(app.commands),
            "groups": len(groups),
            "leaves": len(leaves),
            "module_files": len(modules),
            "top_level_module_files": sum(
                1
                for module in modules
                if Path(module["module"]).parent.as_posix() == COMMAND_MODULES_REL.as_posix()
            ),
            "nested_module_files": sum(
                1
                for module in modules
                if Path(module["module"]).parent.as_posix() != COMMAND_MODULES_REL.as_posix()
            ),
            "callback_module_files": sum(1 for module in modules if module["reachable_command_source"]),
            "registration_modules": sum(1 for module in modules if any(module["registrations"].values())),
            "dynamically_registered_modules": sum(
                1
                for module in modules
                if any(module["registrations"].values()) and not module["import_reachable"]
            ),
            "helper_modules": sum(
                1
                for module in modules
                if not module["reachable_command_source"] and not any(module["registrations"].values())
            ),
            "reachable_modules": sum(1 for module in modules if module["reachable_command_source"]),
            "unreachable_modules": sum(1 for module in modules if not module["reachable_command_source"]),
            "hidden_leaves": sum(1 for leaf in leaves if leaf["hidden"]),
            "json_leaves": sum(1 for leaf in leaves if leaf["accepts_json"]),
            "options": sum(len(leaf["options"]) for leaf in leaves),
            "arguments": sum(len(leaf["arguments"]) for leaf in leaves),
            "migrations": len(migrations),
        },
        "migrations": migrations,
        "completion_manifest": {
            "path": "src/specify_cli/_completion_manifest.json",
            "leaf_count": len(manifest_leaves),
            "matches_dynamic_tree": manifest_set == surface_set,
            "manifest_only": ["/".join(path) for path in sorted(manifest_set - surface_set)],
            "surface_only": ["/".join(path) for path in sorted(surface_set - manifest_set)],
        },
        "root": _group_record(app, [], root, [ref for leaf in leaves for ref in [leaf["callback"]["source_ref"]]]),
        "groups": sorted(groups, key=lambda item: item["path"]),
        "leaves": sorted(leaves, key=lambda item: item["path"]),
        "modules": modules,
        "shared_callbacks": shared,
        "unreachable_modules": sorted(
            module["module"] for module in modules if not module["reachable_command_source"]
        ),
        "unreachable_import_modules": sorted(
            module["module"] for module in modules if not module["import_reachable"]
        ),
        "registration_module_count": sum(
            1 for module in modules if any(module["registrations"].values())
        ),
        "registration_count": {
            "commands": sum(module["registrations"]["commands"] for module in modules),
            "typers": sum(module["registrations"]["typers"] for module in modules),
        },
    }
    return payload


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="Write discovery JSON here (default: stdout)")
    parser.add_argument("--root", type=Path, default=repo_root(), help="Repository root")
    args = parser.parse_args(argv)
    output = args.output.resolve() if args.output else None
    try:
        payload = discover(args.root.resolve())
    except SystemExit as exc:
        print(f"discovery failed: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # noqa: BLE001 - fail closed with a stable message
        print(f"discovery failed: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2
    text = json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
