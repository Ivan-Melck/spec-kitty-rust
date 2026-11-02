#!/usr/bin/env python3
"""Fail closed unless the development oracle has the pinned runtime closure."""

from __future__ import annotations

import argparse
import importlib.metadata as metadata
import json
import re
import sys
import tomllib
from pathlib import Path


def normalized(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def verify(root: Path) -> dict:
    from packaging.markers import default_environment
    from packaging.requirements import Requirement

    lock = tomllib.loads((root / "uv.lock").read_text())
    project = tomllib.loads((root / "pyproject.toml").read_text())["project"]
    locked = {normalized(item["name"]): item["version"] for item in lock["package"]}
    installed: dict[str, str] = {}
    duplicates = []
    for distribution in metadata.distributions():
        name = normalized(distribution.metadata["Name"])
        version = distribution.version
        if name in installed and installed[name] != version:
            duplicates.append(name)
        installed[name] = version
    drift = [{"name": name, "installed": version, "locked": locked.get(name)} for name, version in sorted(installed.items()) if locked.get(name) != version]
    # Merely checking the overlap would certify an empty/minimal environment.
    # Walk active requirements, including propagated extras, to prove presence.
    environment = default_environment()
    roots = [Requirement(text) for text in project["dependencies"]]
    pending = [item for item in roots if item.marker is None or item.marker.evaluate(environment)]
    missing = set()
    unsatisfied = []
    seen: set[tuple[str, frozenset[str]]] = set()
    while pending:
        requirement = pending.pop()
        name = normalized(requirement.name)
        node = (name, frozenset(requirement.extras))
        if node in seen:
            continue
        seen.add(node)
        if name not in installed:
            missing.add(name)
            continue
        if requirement.specifier and not requirement.specifier.contains(installed[name], prereleases=True):
            unsatisfied.append(str(requirement))
        for text in metadata.requires(name) or []:
            dependency = Requirement(text)
            if dependency.marker is None or any(dependency.marker.evaluate({**environment, "extra": extra}) for extra in requirement.extras or {""}):
                pending.append(dependency)
    if normalized(project["name"]) not in installed:
        missing.add(normalized(project["name"]))
    return {
        "ok": sys.version_info[:2] == (3, 11) and not drift and not missing and not duplicates and not unsatisfied,
        "python": sys.version.split()[0],
        "installed_count": len(installed),
        "checked_runtime_requirements": len(seen),
        "drift": drift,
        "missing": sorted(missing),
        "unsatisfied": sorted(unsatisfied),
        "duplicate_versions": sorted(duplicates),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()
    try:
        report = verify(args.root)
    except Exception as error:
        report = {"ok": False, "error": f"{type(error).__name__}: {error}"}
    print(json.dumps(report, sort_keys=True))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
