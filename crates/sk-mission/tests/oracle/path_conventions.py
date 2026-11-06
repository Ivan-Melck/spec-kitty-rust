"""Actual pinned-Source receipts for path-convention, waiver, project-override and
Mission-definition resolution behaviour.

Reads `<root>/scenarios.json` (written by the Rust test), executes Source's own
functions against the fixtures under `<root>/`, and prints one JSON row per
scenario. Every authored string is transported as code points, and every ordered
mapping as an explicit pair list, so the comparison is order-sensitive and
surrogate-exact.
"""
import json, sys, warnings
from pathlib import Path

from specify_cli.acceptance.summary_core import build_warnings, evaluate_path_conventions
from specify_cli.config import path_conventions as pc
from specify_cli.core.paths import read_contracts_waiver_from_meta
from specify_cli.mission import (ArtifactsConfig, Mission, MissionConfig, PhaseConfig,
                                 WorkflowConfig, get_deliverables_path, get_mission_by_name,
                                 get_mission_for_feature)
from specify_cli.validators import paths as vp

root = Path(sys.argv[1])
repo = Path(sys.argv[2])


def U(text):
    return {"u": [ord(character) for character in text]}


def units(value):
    if isinstance(value, str):
        return U(value)
    if isinstance(value, dict):
        return {key: units(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [units(item) for item in value]
    return value


def pairs(mapping):
    return [[U(key), U(value)] for key, value in mapping.items()]


def make_mission(spec, order=0):
    directory = root / spec.get("dir", f"missions/{order}")
    directory.mkdir(parents=True, exist_ok=True)
    config = MissionConfig(
        name=spec.get("name", "M"),
        description=spec.get("description", "d"),
        version=spec.get("version", "1.0.0"),
        domain=spec.get("domain", "software"),
        workflow=WorkflowConfig(phases=[PhaseConfig(name="p", description="d")]),
        artifacts=ArtifactsConfig(required=list(spec.get("required", [])),
                                  optional=list(spec.get("optional", []))),
        paths=dict(spec.get("paths", {})),
    )
    return Mission(directory, config=config)


def dump_result(result):
    return {
        "mission_name": U(result.mission_name),
        "required_paths": pairs(result.required_paths),
        "existing": [U(item) for item in result.existing_paths],
        "missing": [U(item) for item in result.missing_paths],
        "missing_artifact_tokens": [U(item) for item in result.missing_artifact_tokens],
        "warnings": [U(item) for item in result.warnings],
        "suggestions": [U(item) for item in result.suggestions],
        "satisfied_by": pairs(result.satisfied_by),
        "remappable": [U(item) for item in result.remappable_missing_keys],
        "mission_type": None if result.mission_type is None else U(result.mission_type),
        "is_valid": result.is_valid,
        "format_errors": U(result.format_errors()),
        "format_warnings": U(result.format_warnings()),
    }


rows = []
scenarios = json.loads((root / "scenarios.json").read_text())
for order, scenario in enumerate(scenarios):
    label = scenario["label"]
    kind = scenario["kind"]
    row = {"label": label, "kind": kind}
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        try:
            if kind == "validate":
                mission = make_mission(scenario["mission"], order)
                overrides = scenario.get("overrides")
                if overrides is not None:
                    overrides = dict(overrides)
                result = vp.validate_mission_paths(
                    mission,
                    root / scenario["project_root"],
                    strict=scenario.get("strict", False),
                    path_prefix=scenario.get("path_prefix"),
                    feature_dir=None if scenario.get("feature_dir") is None
                    else root / scenario["feature_dir"],
                    path_overrides=overrides,
                    candidate_source_roots=[root / item for item in scenario.get("candidates", [])],
                    waived_artifact_tokens=frozenset(scenario.get("waived", [])),
                )
                row["result"] = dump_result(result)
            elif kind == "evaluate":
                mission = make_mission(scenario["mission"], order)
                repo_root = root / scenario["repo_root"]
                feature_dir = root / scenario["feature_dir"]
                planning = root / scenario["planning_read_dir"]
                violations, warning, tokens = evaluate_path_conventions(
                    mission,
                    repo_root,
                    feature_dir,
                    planning,
                    strict_metadata=scenario.get("strict_metadata", True),
                    candidate_source_roots=[root / item for item in scenario.get("candidates", [])],
                )
                dedup = sorted(tokens)
                missing_optional = scenario.get("optional_missing", [])
                kept = [item for item in missing_optional if vp.normalize_path_token(item) not in tokens]
                row["evaluate"] = {
                    "violations": [U(item) for item in violations],
                    "warning": None if warning is None else U(warning),
                    "dedup": [U(item) for item in dedup],
                    "build_warnings": [U(item) for item in build_warnings(
                        missing_optional=kept,
                        path_violations=violations,
                        path_convention_warning=warning,
                    )],
                }
            elif kind == "waiver":
                waiver = read_contracts_waiver_from_meta(root / scenario["meta_dir"])
                row["waiver"] = {
                    "waived": waiver.waived,
                    "rationale": None if waiver.rationale is None else U(waiver.rationale),
                    "warning": None if waiver.warning is None else U(waiver.warning),
                }
            elif kind == "config":
                overrides = pc.load_project_path_conventions(root / scenario["repo_root"])
                row["config"] = {
                    "overrides": pairs(overrides),
                }
            elif kind == "deliverables":
                value = get_deliverables_path(root / scenario["planning_read_dir"],
                                             scenario.get("slug"))
                row["deliverables"] = None if value is None else U(value)
            elif kind == "resolve":
                try:
                    if scenario.get("feature_meta"):
                        mission = get_mission_for_feature(
                            (root / scenario["project_root"]) / "kitty-specs" / "001-m")
                    else:
                        mission = get_mission_by_name(
                            scenario["name"],
                            (root / scenario["project_root"]) / ".kittify")
                except Exception as error:  # noqa: BLE001 - exact Source verdict transport
                    raise
                else:
                    row["resolve"] = {
                        "mission_dir": U(str(mission.path).replace(str(repo), "<repo>")),
                        "name": U(mission.name),
                        "domain": U(mission.domain),
                        "paths": pairs(mission.get_path_conventions()),
                        "required": [U(item) for item in mission.get_required_artifacts()],
                        "optional": [U(item) for item in mission.get_optional_artifacts()],
                    }
            else:
                raise SystemExit(f"unknown kind {kind}")
            row["warnings"] = [U(str(item.message)) for item in caught]
        except Exception as error:  # noqa: BLE001 - exact Source verdict transport
            row["error_type"] = U(type(error).__name__)
            row["error"] = U(str(error))
            row["warnings"] = [U(str(item.message)) for item in caught]
    rows.append(row)

json.dump({"rows": rows}, sys.stdout, ensure_ascii=True)
