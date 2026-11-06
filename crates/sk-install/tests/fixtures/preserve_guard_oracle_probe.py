"""Source-first: the guard archives an unowned `.kittify/` candidate into `.kittify/.backup-*`."""
import json, os, sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
assert str(root).startswith("/tmp/sk-c4-oracle.")
for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[key]).parent == root, (key, os.environ[key])

from specify_cli.asset_preservation.guard import guard_destructive_removal
from specify_cli.asset_preservation.provers import ManagedPathProver

project = root / "project"
(project / ".kittify").mkdir(parents=True)
candidate = project / ".kittify" / "retired-release-skill.md"
candidate.write_text("operator content\n", encoding="utf-8")

verdict = guard_destructive_removal(
    candidate,
    project,
    prover=ManagedPathProver(managed_relpaths={".claude/commands"}),
    is_tree=False,
    backup_parent=project / ".kittify",
    dry_run=False,
)

def tree(path):
    out = {}
    for child in sorted(path.rglob("*")):
        rel = child.relative_to(path).as_posix()
        out[rel + "/" if child.is_dir() else rel] = "" if child.is_dir() else child.read_text(encoding="utf-8")
    return out

print(json.dumps({
    "owned": verdict.owned,
    "reason": verdict.reason,
    "candidate_exists": candidate.exists(),
    "diagnostic": verdict.diagnostic,
    "backup_path_rel": (verdict.backup_path.relative_to(project).as_posix() if verdict.backup_path else None),
    "tree": tree(project / ".kittify"),
}, indent=2, sort_keys=True))
