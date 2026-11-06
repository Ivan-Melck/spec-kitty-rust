"""C6 Source-first: pinned fan-out over `.worktrees/` with full-object record.

Re-asserts the C5b facts (fresh run, fresh assertions) and records the full
observable state a Native differential can compare against: metadata before/after,
raw commit object, tree listing with modes, refs, reflogs, index entries and
`status --porcelain=v2`. Non-deterministic values (stamps, OIDs) are recorded as
actually observed, never normalized or invented.
"""
import hashlib, json, os, subprocess, sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
assert str(root).startswith("/tmp/sk-c6-oracle."), root
for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[key]).parent == root, (key, os.environ[key])
assert Path.cwd() == root / "work", Path.cwd()

def run(*args, cwd):
    return subprocess.run(args, cwd=cwd, capture_output=True, text=True, check=True).stdout.strip()

def raw(*args, cwd):
    return subprocess.run(args, cwd=cwd, capture_output=True, check=True).stdout

proj = root / "proj"
proj.mkdir()
run("git", "init", "-q", "-b", "main", cwd=proj)
run("git", "config", "user.email", "c6@example.test", cwd=proj)
run("git", "config", "user.name", "C6 Oracle", cwd=proj)
(proj / ".kittify").mkdir()
(proj / ".kittify/config.yaml").write_text(
    "agents:\n  available:\n    - claude\n  auto_commit: true\n", encoding="utf-8")
(proj / ".kittify/metadata.yaml").write_text(
    'spec_kitty:\n  version: "4.0.0rc8"\n  schema_version: 3\n'
    'environment:\n  initialized_at: "2026-01-01T00:00:00+00:00"\n', encoding="utf-8")
(proj / "README.md").write_text("base\n", encoding="utf-8")
run("git", "add", "-A", cwd=proj)
run("git", "commit", "-qm", "base", cwd=proj)

run("git", "worktree", "add", "-q", ".worktrees/lane", "-b", "feature/x", cwd=proj)
run("git", "worktree", "add", "-q", ".worktrees/mission", "-b", "kitty/mission-demo-ABCDEFGH", cwd=proj)
for name in ("lane", "mission"):
    wt = proj / ".worktrees" / name
    (wt / ".kittify").mkdir(exist_ok=True)
    (wt / ".kittify/metadata.yaml").write_text(
        'spec_kitty:\n  version: "3.2.0"\n  schema_version: 2\n'
        'environment:\n  initialized_at: "2025-01-01T00:00:00+00:00"\n', encoding="utf-8")
    run("git", "add", "-A", cwd=wt)
    run("git", "commit", "-qm", f"{name} seed", cwd=wt)

from specify_cli.upgrade.runner import MigrationRunner  # noqa: E402
from specify_cli.upgrade.autocommit import git_status_paths  # noqa: E402

def index_file(wt):
    out = run("git", "rev-parse", "--git-dir", cwd=wt)
    gitdir = (wt / out).resolve() if not Path(out).is_absolute() else Path(out)
    return gitdir / "index"

def snapshot(wt):
    meta = wt / ".kittify" / "metadata.yaml"
    idx = index_file(wt)
    return {
        "metadata": meta.read_text(encoding="utf-8") if meta.is_file() else None,
        "dirty": sorted(git_status_paths(wt) or []),
        "status_v2": run("git", "status", "--porcelain=v2", cwd=wt),
        "head": run("git", "rev-parse", "HEAD", cwd=wt),
        "branch": run("git", "branch", "--show-current", cwd=wt),
        "log": run("git", "log", "--format=%H%x09%s", "-n", "3", cwd=wt),
        "index_entries": run("git", "ls-files", "-s", cwd=wt),
        "index_sha256": hashlib.sha256(idx.read_bytes()).hexdigest() if idx.is_file() else None,
        "tree_entries": run("git", "ls-tree", "-r", "--full-tree", "HEAD", cwd=wt),
    }

def full_objects(wt):
    head = run("git", "rev-parse", "HEAD", cwd=wt)
    commit = raw("git", "cat-file", "commit", head, cwd=wt).decode("utf-8", "surrogateescape")
    return {
        "head": head,
        "cat_file_commit_raw": commit,
        "tree": run("git", "rev-parse", "HEAD^{tree}", cwd=wt),
        "parents": run("git", "rev-list", "--parents", "-n", "1", "HEAD", cwd=wt).split()[1:],
        "refs": run("git", "for-each-ref", "--format=%(refname) %(objectname)", cwd=wt),
        "reflog_head": run("git", "reflog", "--format=%H %gD %gs", "-n", "5", "HEAD", cwd=wt),
        "reflog_branch": run(
            "git", "reflog", "--format=%H %gD %gs", "-n", "5",
            run("git", "branch", "--show-current", cwd=wt), cwd=wt),
        "tree_entries": run("git", "ls-tree", "-r", "--full-tree", "HEAD", cwd=wt),
    }

before = {name: snapshot(proj / ".worktrees" / name) for name in ("lane", "mission")}
main_before = snapshot(proj)

result = MigrationRunner(proj).upgrade("4.0.0rc8", include_worktrees=True, auto_commit=True)

lane = proj / ".worktrees" / "lane"
mission = proj / ".worktrees" / "mission"
lane_meta = (lane / ".kittify/metadata.yaml").read_text(encoding="utf-8")
mission_meta = (mission / ".kittify/metadata.yaml").read_text(encoding="utf-8")

# ---- Source assertions (before any native equivalent runs) ------------------
assert result.success is True, result.errors
assert result.errors == [], result.errors
assert result.worktree_failures == [], result.worktree_failures
assert "4.0.0rc8" in lane_meta and "schema_version: 3" in lane_meta, lane_meta
assert "schema_capabilities" in lane_meta, lane_meta
assert mission_meta == before["mission"]["metadata"], mission_meta
assert git_status_paths(lane) == set(), git_status_paths(lane)
assert git_status_paths(mission) == set(), git_status_paths(mission)
lane_objects = full_objects(lane)
mission_objects = full_objects(mission)
assert len(lane_objects["parents"]) == 1, lane_objects["parents"]
assert "chore: apply spec-kitty upgrade changes (3.2.0 -> 4.0.0rc8)" in lane_objects["cat_file_commit_raw"]
for header in ("tree ", "parent ", "author ", "committer "):
    assert header in lane_objects["cat_file_commit_raw"], (header, lane_objects["cat_file_commit_raw"])
assert "feature/x" in lane_objects["refs"]
assert "kitty/mission-demo-ABCDEFGH" in mission_objects["refs"]
assert "chore: apply spec-kitty upgrade changes" not in mission_objects["cat_file_commit_raw"]
assert "chore: apply spec-kitty upgrade changes" in lane_objects["reflog_branch"]
# The bookkeeping-only bump is stamped from ONE minted fallback, not per-root `now`:
# main needed no migrations here, so main carries no `last_upgraded_at` of its own
# (observed, not assumed) and the lane's stamp must predate its own initialized_at.
import yaml  # noqa: E402
main_meta = yaml.safe_load(snapshot(proj)["metadata"])
lane_yaml = yaml.safe_load(lane_meta)
assert "last_upgraded_at" not in main_meta["spec_kitty"], main_meta["spec_kitty"]
lane_stamp = lane_yaml["spec_kitty"]["last_upgraded_at"]
lane_init = lane_yaml["spec_kitty"]["initialized_at"]
from datetime import datetime  # noqa: E402
assert datetime.fromisoformat(lane_stamp) < datetime.fromisoformat(lane_init), (lane_stamp, lane_init)

record = {
    "assertions": "all passed",
    "probe": "c6-fanout-assert",
    "python": sys.version.split()[0],
    "project_root": str(proj),
    "main_before": main_before,
    "before": before,
    "main_after": snapshot(proj),
    "after": {name: snapshot(proj / ".worktrees" / name) for name in ("lane", "mission")},
    "lane_objects": lane_objects,
    "mission_objects": mission_objects,
    "result": {
        "success": result.success,
        "warnings": result.warnings,
        "errors": result.errors,
        "worktree_failures": result.worktree_failures,
        "applied": result.migrations_applied,
        "skipped": result.migrations_skipped,
    },
}
out = Path(os.environ["HOME"]).parent / "fanout-source-record-c6.json"
out.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print("record=", out)
print(json.dumps(record, indent=2, sort_keys=True))
