"""Development-oracle fixture for the paired Source/Native consolidation tests.

This is NOT product code and is never imported by the Rust crates: the paired
tests run it with the pinned CPython oracle to (a) build a Source-valid mission
through the real Source CLI and (b) print a receipt of the resulting git objects,
refs, reflogs, metadata and working-tree footprints.

The recipe is the one the Source suite itself uses
(`tests/integration/test_6715_consolidate_lane_evidence.py`), inlined so the
oracle venv needs no pytest. Modes:

    seed    <topology>   build the mission through `accept`, detach the lane
                         worktrees so the directory is copyable, print
                         {"repo": ..., "slug": ...}
    fixture <strategy> <topology>
                         build the seed, print the same JSON, and exit
    receipt <repo> <slug>
                         print the full-state receipt for an existing repo
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
_CLI = [sys.executable, "-m", "specify_cli"]
_RESEARCH_FILE = "research/notes.md"


@dataclass(frozen=True)
class _Mission:
    repo: Path
    env: dict[str, str]
    slug: str
    feature_dir: Path


def _env(home: Path) -> dict[str, str]:
    env = dict(os.environ)
    env.update(
        HOME=str(home / "home"),
        XDG_CONFIG_HOME=str(home / "xc"),
        XDG_CACHE_HOME=str(home / "xcache"),
        XDG_DATA_HOME=str(home / "xd"),
        SPEC_KITTY_NO_UPGRADE_CHECK="1",
        SPEC_KITTY_WORKTREE_REMOVAL_DELAY="0",
        SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS="1",
        SPEC_KITTY_ENABLE_SAAS_SYNC="0",
    )
    for key in ("home", "xc", "xcache", "xd"):
        (home / key).mkdir(parents=True, exist_ok=True)
    return env


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, text=True).stdout.strip()


def _sk(mission, *args: str, check: bool = False):
    repo, env = (mission.repo, mission.env) if isinstance(mission, _Mission) else mission
    proc = subprocess.run([*_CLI, *args], cwd=repo, env=env, capture_output=True, text=True, timeout=400)
    if check and proc.returncode != 0:
        raise AssertionError(f"fixture step failed: spec-kitty {' '.join(args)}\nrc={proc.returncode}\nstdout:\n{proc.stdout}\nstderr:\n{proc.stderr}")
    return proc


def _commit_specs(repo: Path) -> None:
    if _git(repo, "status", "--porcelain", "--", "kitty-specs"):
        _git(repo, "add", "-A", "kitty-specs")
        _git(repo, "commit", "-qm", "status residue")


def _wp_file(wp: str, subtask: str, owned: str, *, planning: bool = False, create: bool = False) -> str:
    surface = owned.split("*")[0]
    extra = "execution_mode: planning_artifact\n" if planning else ""
    intent = f"create_intent:\n  - {owned}\n" if create else ""
    return (
        f"---\nwork_package_id: {wp}\ntitle: {wp}\ndependencies: []\nrequirement_refs: [FR-001]\n{extra}"
        f"authoritative_surface: {surface}\nowned_files:\n  - {owned}\n{intent}subtasks:\n  - {subtask}\n---\n# {wp}\n\n- [ ] {subtask} do\n"
    )


def setup_mission(tmp_path: Path, topology: str, *, planning_glob: str = "research/**") -> _Mission:
    env = _env(tmp_path)
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q", "-b", "main")
    _git(repo, "config", "user.name", "QA")
    _git(repo, "config", "user.email", "qa@example.invalid")
    _git(repo, "config", "commit.gpgsign", "false")
    _sk((repo, env), "init", "--ai", "claude", "--non-interactive", check=True)
    (repo / "src").mkdir(exist_ok=True)
    (repo / "src" / "shared.py").write_text("A = 1\n", encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "init")
    created = _sk(
        (repo, env),
        *("agent", "mission", "create", "fx", "--mission-type", "software-dev", "--topology", topology),
        *("--branch-strategy", "already-confirmed", "--target-branch", "main", "--friendly-name", "fx"),
        *("--purpose-tldr", "fx", "--purpose-context", "QA fixture.", "--json"),
        check=True,
    )
    slug = json.loads(created.stdout)["mission_slug"]
    fd = repo / "kitty-specs" / slug
    (fd / "spec.md").write_text("# Spec\n\n## Requirements\n\n- **FR-001**: things work.\n", encoding="utf-8")
    (fd / "plan.md").write_text("# Plan\n\nModules.\n", encoding="utf-8")
    (fd / "tasks").mkdir(exist_ok=True)
    sections = "".join(
        f"## Work Package {wp}: {wp}\n\n**Dependencies**: None\n\nRequirement refs: FR-001\n\n- [ ] {t} do\n\n"
        for wp, t in (("WP01", "T1"), ("WP02", "T2"), ("WP03", "T3"), ("WP04", "T4"))
    )
    (fd / "tasks.md").write_text(f"# Tasks\n\n{sections}", encoding="utf-8")
    (fd / "tasks" / "WP01-m1.md").write_text(_wp_file("WP01", "T1", "src/m1/**"), encoding="utf-8")
    (fd / "tasks" / "WP02-tools.md").write_text(_wp_file("WP02", "T2", "tools/check.sh", create=True), encoding="utf-8")
    (fd / "tasks" / "WP03-m3.md").write_text(_wp_file("WP03", "T3", "src/m3/**"), encoding="utf-8")
    (fd / "tasks" / "WP04-research.md").write_text(_wp_file("WP04", "T4", f"kitty-specs/{slug}/{planning_glob}", planning=True), encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "planning")
    mission = _Mission(repo, env, slug, fd)
    _sk(mission, "agent", "mission", "finalize-tasks", "--mission", slug, "--json", check=True)
    (tmp_path / "analysis.md").write_text("# Analysis\n\nNo blocking findings.\n", encoding="utf-8")
    _sk(mission, "agent", "mission", "record-analysis", "--mission", slug, "--input-file", str(tmp_path / "analysis.md"))
    _commit_specs(repo)
    return mission


def _workspace_for(mission: _Mission, wp: str) -> Path:
    manifest = json.loads((mission.feature_dir / "lanes.json").read_text(encoding="utf-8"))
    lane_id = next(lane["lane_id"] for lane in manifest["lanes"] if wp in lane["wp_ids"])
    worktree = mission.repo / ".worktrees" / f"{mission.slug}-{lane_id}"
    return worktree if worktree.is_dir() else mission.repo


def _commit_owned(mission: _Mission, wp: str, relpath: str) -> None:
    checkout = _workspace_for(mission, wp)
    target = checkout / relpath
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(f"# {wp}\n", encoding="utf-8")
    _git(checkout, "add", "--", relpath)
    _git(checkout, "commit", "-qm", f"feat({wp}): implement")


def drive(mission: _Mission, wp: str, subtask: str, *, implementer: str, reviewer: str, owned_path: str | None) -> None:
    slug = mission.slug
    _sk(mission, "agent", "action", "implement", wp, "--agent", implementer, "--mission", slug, check=True)
    if owned_path is not None:
        _commit_owned(mission, wp, owned_path)
    _sk(mission, "agent", "tasks", "mark-status", subtask, "--status", "done", "--mission", slug)
    _sk(mission, "agent", "tasks", "move-task", wp, "--to", "for_review", "--agent", implementer, "--note", "done", "--force", "--mission", slug, check=True)
    _sk(mission, "agent", "action", "review", wp, "--agent", reviewer, "--mission", slug, check=True)
    _sk(mission, "agent", "tasks", "move-task", wp, "--to", "approved", "--agent", reviewer, "--note", "lgtm", "--force", "--mission", slug, check=True)
    _commit_specs(mission.repo)


def lanes_of(mission: _Mission) -> dict[str, str]:
    status = _sk(mission, "agent", "tasks", "status", "--mission", mission.slug, "--json")
    try:
        return {w["id"]: w["lane"] for w in json.loads(status.stdout)["work_packages"]}
    except (ValueError, KeyError, TypeError):
        return {"<unparsed>": status.stdout[-400:]}


def accept(mission: _Mission) -> None:
    _sk(mission, "agent", "acceptance-verdict", "--mission", mission.slug, "--criterion", "FR-001", "--result", "pass", "--actor", "operator", "--evidence", "qa")
    _commit_specs(mission.repo)
    _sk(mission, "accept", "--mission", mission.slug, "--json", "--lenient")
    _commit_specs(mission.repo)


def _has(repo: Path, spec: str) -> bool:
    return subprocess.run(["git", "cat-file", "-e", spec], cwd=repo, capture_output=True).returncode == 0


def _show(repo: Path, spec: str) -> str | None:
    proc = subprocess.run(["git", "show", spec], cwd=repo, capture_output=True, text=True)
    return proc.stdout if proc.returncode == 0 else None


def receipt(mission: _Mission, label: str) -> dict:
    repo = mission.repo
    out: dict = {"label": label, "slug": mission.slug}
    out["main_tip"] = _git(repo, "rev-parse", "main")
    out["branches"] = _git(repo, "for-each-ref", "--format=%(refname)", "refs/heads").splitlines()
    out["head"] = _git(repo, "rev-parse", "HEAD")
    out["head_branch"] = _git(repo, "rev-parse", "--abbrev-ref", "HEAD")
    out["main_log"] = _git(repo, "log", "--format=%H%x09%P%x09%an%x09%ae%x09%cn%x09%ce%x09%s", "-n", "8", "main").splitlines()
    out["status"] = _git(repo, "status", "--porcelain")
    out["reflog_main"] = _git(repo, "reflog", "-n", "14", "--format=%H%x09%gD%x09%gs", "main").splitlines()
    out["meta"] = json.loads(_show(repo, "main:kitty-specs/%s/meta.json" % mission.slug) or "null")
    out["lanes_after"] = lanes_of(mission)
    out["files"] = {
        rel: _show(repo, "main:" + rel)
        for rel in (
            "tools/check.sh",
            "src/m1/real.py",
            "src/m3/real.py",
            "kitty-specs/%s/research/notes.md" % mission.slug,
        )
    }
    out["drivers"] = _git(repo, "config", "--local", "--get-regexp", r"^merge\.spec-kitty-").splitlines()
    attrs = repo / ".git/info/attributes"
    out["info_attributes"] = attrs.read_text() if attrs.exists() else None
    out["gitattributes"] = _show(repo, "main:.gitattributes")
    out["status_log"] = _show(repo, "main:kitty-specs/%s/status.events.jsonl" % mission.slug)
    return out


def seed(kind: str) -> _Mission:
    tmp = Path(tempfile.mkdtemp(prefix="sk-h2-src-"))
    mission = setup_mission(tmp, kind)
    drive(mission, "WP01", "T1", implementer="alice", reviewer="bob", owned_path="src/m1/real.py")
    drive(mission, "WP02", "T2", implementer="claude", reviewer="bob", owned_path="tools/check.sh")
    drive(mission, "WP03", "T3", implementer="claude", reviewer="bob", owned_path="src/m3/real.py")
    drive(mission, "WP04", "T4", implementer="alice", reviewer="bob", owned_path=f"kitty-specs/{mission.slug}/research/notes.md")
    accept(mission)
    # Detach the lane worktrees so the seed directory is copyable: a linked
    # worktree's gitdir is an absolute path and would point at the original
    # checkout after a copy.
    for worktree in sorted((mission.repo / ".worktrees").glob("*")) if (mission.repo / ".worktrees").is_dir() else []:
        _git(mission.repo, "worktree", "remove", "--force", str(worktree))
    _git(mission.repo, "worktree", "prune")
    return mission


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) > 1 else "seed"
    if mode == "receipt":
        repo = Path(sys.argv[2])
        slug = sys.argv[3]
        mission = _Mission(repo, {}, slug, repo / "kitty-specs" / slug)
        print(json.dumps(receipt(mission, "receipt"), indent=2))
        return 0
    strategy = mode
    topology = sys.argv[2] if len(sys.argv) > 2 else "lanes"
    extra = sys.argv[3:]
    mission = seed(topology)
    if strategy in ("seed", "fixture"):
        print(json.dumps({"tmp": str(mission.repo.parent), "repo": str(mission.repo), "slug": mission.slug}))
        return 0
    before = receipt(mission, "before")
    proc = _sk(mission, "consolidate", "--mission", mission.slug, "--strategy", strategy, *extra)
    after = receipt(mission, "after")
    print(json.dumps({"tmp": str(tmp), "strategy": strategy, "topology": topology, "rc": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr, "before": before, "after": after}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
