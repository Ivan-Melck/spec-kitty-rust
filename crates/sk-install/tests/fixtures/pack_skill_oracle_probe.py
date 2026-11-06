"""Source-first oracle: record the pinned `resolve_project_skill_catalog` output.

Run with the repo `.venv-rust-oracle` interpreter (CPython 3.11.15, 53
distributions), an asserted temporary CWD/HOME/all XDG/TMP, and
`PYTHONPATH=<repo>/src` + `SPEC_KITTY_PACKS_ROOT=<repo>/packs`.

Writes one JSON document: the fixture inputs, the resolved skill set (name,
origin, source_ref, source_hash) and the staged tree bytes.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import sys
from pathlib import Path

import yaml

root = Path(sys.argv[1]).resolve()
assert str(root).startswith("/tmp/sk-c4-oracle."), root
for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[key]).parent == root, (key, os.environ[key])
assert Path.cwd() == root / "work", Path.cwd()

from specify_cli.skills.catalog import PackSkillCatalogError, resolve_project_skill_catalog  # noqa: E402

repo = Path(os.environ["PYTHONPATH"].split(":")[0]).resolve()


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def make_pack(pack: Path, *, namespace: str | None = "acme", skills: list[dict[str, object]]) -> None:
    write(
        pack / "pack.yaml",
        "pack_id: 01ARWG13C000000000000000AC\npack_version: 1.0.0\nparent_pack: null\nname: acme\npack_level: org\n",
    )
    charter: dict[str, object] = {"org_name": "acme-org"}
    if namespace is not None:
        charter["skill_namespace"] = namespace
    write(pack / "org-charter.yaml", yaml.safe_dump(charter))
    write(
        pack / "drg" / "fragment.yaml",
        "nodes:\n"
        '  - id: deploy-helper\n    kind: skills\n    title: "Deploy helper"\n'
        "    body_path: skills/deploy-helper.skill.yaml\n"
        '  - id: release-wrapper\n    kind: skills\n    title: "Release wrapper"\n'
        "    body_path: skills/release-wrapper.skill.yaml\n"
        '  - id: release-proc\n    kind: procedures\n    title: "Release procedure"\n'
        "    body_path: procedures/release-proc.procedure.yaml\n"
        "edges:\n"
        '  - source: "skill:deploy-helper"\n    target: "procedure:release-proc"\n    relation: requires\n'
        '  - source: "skill:release-wrapper"\n    target: "procedure:release-proc"\n    relation: requires\n',
    )
    write(
        pack / "procedures" / "release-proc.procedure.yaml",
        'schema_version: "1.0"\nid: release-proc\nname: release-proc fixture procedure\npurpose: >\n'
        "  Substance carried by a procedure.\nentry_condition: >\n  A skill requires it.\n"
        "exit_condition: >\n  Done.\nsteps:\n  - title: Act\n    description: >\n      Do the thing.\n"
        "    actor: agent\n",
    )
    for record in skills:
        skills_dir = pack / "skills"
        write(skills_dir / f"{record['id']}.skill.yaml", yaml.safe_dump(record, sort_keys=True))
        if record.get("form") == "prompt":
            write(skills_dir / str(record["body_path"]), f"Run {record['id']}: $ARGUMENTS\n")


def make_project(project: Path, pack: Path | None, *, ledger: dict[str, object], namespace: str | None = None) -> None:
    lines = []
    if pack is not None or namespace is not None:
        lines.append("charter_packs:")
    if pack is not None:
        lines += ["  org:", "    packs:", "      - name: acme", f"        local_path: {pack}"]
    if namespace is not None:
        lines += ["  project:", f"    skill_namespace: {namespace}"]
    write(project / ".kittify" / "config.yaml", "".join(f"{line}\n" for line in lines) + "agents:\n  available:\n    - claude\n")
    write(project / ".kittify" / "charter" / "activation.yaml", yaml.safe_dump(ledger, sort_keys=False))


def tree(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    if not path.exists():
        return out
    for child in sorted(path.rglob("*")):
        relative = child.relative_to(path).as_posix()
        if child.is_dir():
            out[relative + "/"] = ""
        elif child.is_symlink():
            out[relative] = "link:" + os.readlink(child)
        else:
            out[relative] = "sha256:" + hashlib.sha256(child.read_bytes()).hexdigest()
    return out


PROMPT_SKILL = {
    "schema_version": "1.0",
    "id": "deploy-helper",
    "title": "Deploy helper",
    "description": "Ships   the  release.",
    "triggers": ["ship it", 'say "go"'],
    "form": "prompt",
    "body_path": "deploy-helper.skill.md",
}
WRAPPER_SKILL = {
    "schema_version": "1.0",
    "id": "release-wrapper",
    "title": "Release wrapper",
    "description": "Runs the canonical consolidate command.",
    "form": "wrapper",
    "expands_to": {"target": "builtin:spec-kitty.consolidate", "args": "$ARGUMENTS"},
}

results: dict[str, object] = {}

# -- case 1: one prompt skill in force, staged ---------------------------------
case = root / "case1"
make_pack(case / "pack", skills=[PROMPT_SKILL, WRAPPER_SKILL])
make_project(case / "project", case / "pack", ledger={"activated_skills": ["deploy-helper"]})
catalog = resolve_project_skill_catalog(case / "project", stage=True)
results["case1"] = {
    "skills": [
        {
            "name": skill.name,
            "origin": skill.origin,
            "source_ref": skill.source_ref,
            "source_hash": skill.source_hash,
            "files": [str(path.relative_to(skill.skill_dir)) for path in skill.all_files],
        }
        for skill in catalog.discover_skills()
    ],
    "staging": tree(case / "project" / ".kittify" / "runtime" / "pack-skills"),
    "staging_text": {
        path.relative_to(case / "project").as_posix(): path.read_text(encoding="utf-8")
        for path in sorted((case / "project" / ".kittify" / "runtime" / "pack-skills").rglob("*"))
        if path.is_file()
    },
    "staging_mode": {
        path.relative_to(case / "project").as_posix(): oct(path.stat().st_mode & 0o777)
        for path in sorted((case / "project" / ".kittify" / "runtime" / "pack-skills").rglob("*"))
        if path.is_file()
    },
}

# -- case 1b: the exact hash inputs (record JSON, requires, rendered name, body)
pack_skill = [skill for skill in catalog.discover_skills() if skill.origin == "pack"][0]
record_source = case / "pack" / "skills" / "deploy-helper.skill.yaml"
from charter.offering.pack_skills.models import PackSkill  # noqa: E402

record = PackSkill.model_validate(yaml.safe_load(record_source.read_text(encoding="utf-8")))
body_text = (record_source.parent / str(record.body_path)).read_text(encoding="utf-8")
results["case1_hash_inputs"] = {
    "canonical_json": json.dumps(
        {"record": record.model_dump(mode="json"), "requires": ["procedure:release-proc"], "rendered_name": "acme-deploy-helper"},
        sort_keys=True,
    ),
    "body": body_text,
}

# -- case 2: warm re-resolution does not rewrite -------------------------------
staged = case / "project" / ".kittify" / "runtime" / "pack-skills" / "acme-deploy-helper" / "SKILL.md"
before = staged.stat().st_mtime_ns
resolve_project_skill_catalog(case / "project", stage=True)
results["case2_warm_stable"] = before == staged.stat().st_mtime_ns

# -- case 3: nothing in force retires the staged set --------------------------
make_project(case / "project", case / "pack", ledger={"activated_skills": []})
empty = resolve_project_skill_catalog(case / "project", stage=True)
results["case3"] = {
    "skills": [skill.name for skill in empty.discover_skills()],
    "staging": tree(case / "project" / ".kittify" / "runtime" / "pack-skills"),
}

# -- case 4: read-only never writes the project tree --------------------------
case4 = root / "case4"
make_pack(case4 / "pack", skills=[PROMPT_SKILL])
make_project(case4 / "project", case4 / "pack", ledger={"activated_skills": ["deploy-helper"]})
before_tree = tree(case4 / "project")
readonly = resolve_project_skill_catalog(case4 / "project", stage=False)
results["case4"] = {
    "skills": [skill.name for skill in readonly.discover_skills()],
    "project_tree_unchanged": tree(case4 / "project") == before_tree,
    "staged_outside_project": all(
        not str(path).startswith(str(case4 / "project")) for skill in readonly.discover_skills() for path in skill.all_files
    ),
}

# -- case 5: org required_skills when activated_skills is absent --------------
case5 = root / "case5"
make_pack(case5 / "pack", skills=[PROMPT_SKILL, WRAPPER_SKILL])
write(case5 / "pack" / "org-charter.yaml", yaml.safe_dump({"org_name": "acme-org", "skill_namespace": "acme", "required_skills": ["release-wrapper"]}))
make_project(case5 / "project", case5 / "pack", ledger={})
required = resolve_project_skill_catalog(case5 / "project", stage=True)
results["case5"] = {
    "pack_skills": [
        {
            "name": skill.name,
            "origin": skill.origin,
            "source_hash": skill.source_hash,
            "text": (skill.skill_dir / "SKILL.md").read_text(encoding="utf-8"),
        }
        for skill in required.discover_skills()
        if skill.origin == "pack"
    ],
    "staging": tree(case5 / "project" / ".kittify" / "runtime" / "pack-skills"),
}

# -- case 6..9: refusals leave the project tree byte-identical ----------------
def refusal(name: str, *, namespace: str | None = "acme", builtin_name: str | None = None, project_namespace: str | None = None) -> None:
    case = root / name
    make_pack(case / "pack", namespace=namespace, skills=[PROMPT_SKILL])
    make_project(case / "project", case / "pack", ledger={"activated_skills": ["deploy-helper"]}, namespace=project_namespace)
    builtin = None
    if builtin_name is not None:
        shipped_root = case / "builtin"
        write(shipped_root / builtin_name / "SKILL.md", "---\nname: x\ndescription: y\n---\n# x\n")
        from specify_cli.skills.registry import SkillRegistry

        builtin = SkillRegistry(shipped_root)
    before = tree(case / "project")
    try:
        resolve_project_skill_catalog(case / "project", builtin=builtin)
    except PackSkillCatalogError as error:
        results[name] = {
            "refused": True,
            "message_fragment": str(error).split(";")[0][:80],
            "project_unchanged": tree(case / "project") == before,
        }
    else:
        results[name] = {"refused": False}


refusal("case6_no_namespace", namespace=None)
refusal("case7_hostile_namespace", namespace="../x")
refusal("case8_builtin_collision", builtin_name="acme-deploy-helper")
refusal("case9_project_namespace", namespace=None, project_namespace="proj")

# -- case 10: the shipped doctrine skill set is untouched ---------------------
case10 = root / "case10"
make_project(case10 / "project", None, ledger={"activated_skills": []})
shipped = resolve_project_skill_catalog(case10 / "project")
results["case10"] = {
    "shipped_names": [skill.name for skill in shipped.discover_skills()],
    "project_skills_dir": (case10 / "project" / ".kittify" / "runtime" / "pack-skills").exists(),
}

print(json.dumps({"oracle": {"python": sys.version.split()[0], "distributions": len(list(__import__("importlib.metadata", fromlist=["x"]).distributions()))}, "repo": str(repo), "cases": results}, indent=2, sort_keys=True))
